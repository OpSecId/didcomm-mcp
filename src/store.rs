//! Where the agent's durable data lives: its identity (private keys), its state
//! (connections, created invitations, the v1 mediation), the inbox of received
//! messages waiting for `fetch_messages`, the message history the web UI shows, and
//! small JSON settings (the profile, peers' profiles, read markers).
//!
//! Two backends:
//! - [`Store::Files`] (the default): the identity file, the state file, and
//!   `<state>.inbox.json`, `<state>.messages.json`, `<state>.kv.json` next to it.
//! - [`Store::Postgres`] (`database_url`): three tables, created on startup. Nothing is
//!   written to disk, so no volume is needed. The identity row holds the private keys
//!   in the clear, as the identity file does: protect the database accordingly.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use didcomm_agent::Identity;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Created on startup if missing.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS didcomm_mcp_kv (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS didcomm_mcp_inbox (
    id          BIGSERIAL PRIMARY KEY,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    entry       JSONB NOT NULL
);
CREATE TABLE IF NOT EXISTS didcomm_mcp_messages (
    id        BIGSERIAL PRIMARY KEY,
    peer      TEXT NOT NULL,
    direction TEXT NOT NULL,
    at        BIGINT NOT NULL,
    entry     JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS didcomm_mcp_messages_peer ON didcomm_mcp_messages (peer, id);
";

const IDENTITY_KEY: &str = "identity";
const STATE_KEY: &str = "state";
/// The file store keeps at most this many history entries (oldest dropped first).
const FILE_HISTORY_CAP: usize = 5000;

/// One message in the history: sent (`out`) or received (`in`), with a peer key
/// (a connection id, else a DID).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry {
    pub id: i64,
    pub peer: String,
    pub direction: String,
    /// Unix seconds.
    pub at: i64,
    pub entry: Value,
}

/// A peer's latest message and counts, for the conversation list.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Conversation {
    pub peer: String,
    pub last: HistoryEntry,
    pub count: i64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct FileHistory {
    next_id: i64,
    entries: VecDeque<HistoryEntry>,
}

pub enum Store {
    Files {
        identity_path: PathBuf,
        state_path: PathBuf,
        inbox_path: PathBuf,
        history_path: PathBuf,
        kv_path: PathBuf,
        inbox: Mutex<VecDeque<Value>>,
        history: Mutex<FileHistory>,
        kv: Mutex<BTreeMap<String, String>>,
    },
    Postgres(sqlx::PgPool),
}

impl Store {
    /// Files: `identity_path`, `state_path`, and next to the state file
    /// `<stem>.inbox.json`, `<stem>.messages.json`, `<stem>.kv.json`.
    pub fn files(identity_path: &Path, state_path: &Path) -> Self {
        let inbox_path = state_path.with_extension("inbox.json");
        let history_path = state_path.with_extension("messages.json");
        let kv_path = state_path.with_extension("kv.json");
        Self::Files {
            identity_path: identity_path.to_path_buf(),
            state_path: state_path.to_path_buf(),
            inbox: Mutex::new(read_json(&inbox_path)),
            history: Mutex::new(read_json(&history_path)),
            kv: Mutex::new(read_json(&kv_path)),
            inbox_path,
            history_path,
            kv_path,
        }
    }

    /// Connect to Postgres and create the tables if they don't exist.
    pub async fn postgres(url: &str) -> anyhow::Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(15))
            .connect(url)
            .await
            .context("connecting to the database")?;
        sqlx::raw_sql(SCHEMA).execute(&pool).await.context("creating the database tables")?;
        Ok(Self::Postgres(pool))
    }

    /// `"files"` or `"postgres"`, for `get_identity`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Files { .. } => "files",
            Self::Postgres(_) => "postgres",
        }
    }

    /// Whether the store answers: always for files; a round trip for Postgres.
    pub async fn ping(&self) -> anyhow::Result<()> {
        match self {
            Self::Files { .. } => Ok(()),
            Self::Postgres(pool) => {
                sqlx::query("SELECT 1").execute(pool).await?;
                Ok(())
            }
        }
    }

    /// The stored identity, or a new one (stored) if there's none yet.
    pub async fn load_or_generate_identity(&self) -> anyhow::Result<Identity> {
        match self {
            Self::Files { identity_path, .. } => Identity::load_or_generate(identity_path)
                .with_context(|| format!("identity file {}", identity_path.display())),
            Self::Postgres(pool) => {
                if let Some(json) = kv_get(pool, IDENTITY_KEY).await? {
                    return Identity::from_json(&json).context("the identity stored in the database");
                }
                let generated = Identity::generate()?;
                sqlx::query("INSERT INTO didcomm_mcp_kv (key, value) VALUES ($1, $2) ON CONFLICT (key) DO NOTHING")
                    .bind(IDENTITY_KEY)
                    .bind(generated.to_json()?)
                    .execute(pool)
                    .await
                    .context("storing the identity")?;
                // Another instance starting at the same time may have stored its own
                // first: everyone uses whichever made it.
                let json = kv_get(pool, IDENTITY_KEY).await?.context("the identity row is missing")?;
                Identity::from_json(&json).context("the identity stored in the database")
            }
        }
    }

    /// The saved state (JSON), if any. A file that can't be read is logged and
    /// skipped, as before; a database that can't be read is an error, so a session
    /// never starts empty and then overwrites the real state.
    pub async fn load_state(&self) -> anyhow::Result<Option<String>> {
        match self {
            Self::Files { state_path, .. } => Ok(read_file(state_path)),
            Self::Postgres(pool) => kv_get(pool, STATE_KEY).await,
        }
    }

    /// Save the state (JSON).
    pub async fn save_state(&self, json: String) -> anyhow::Result<()> {
        match self {
            Self::Files { state_path, .. } => write_atomically(state_path, json.as_bytes()),
            Self::Postgres(pool) => kv_put(pool, STATE_KEY, &json).await,
        }
    }

    /// A JSON setting (`None` if unset).
    pub async fn get_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> anyhow::Result<Option<T>> {
        let text = match self {
            Self::Files { kv, .. } => kv.lock().expect("kv lock poisoned").get(key).cloned(),
            Self::Postgres(pool) => kv_get(pool, key).await?,
        };
        text.map(|t| serde_json::from_str(&t).with_context(|| format!("setting {key}"))).transpose()
    }

    /// Store a JSON setting.
    pub async fn put_json<T: Serialize>(&self, key: &str, value: &T) -> anyhow::Result<()> {
        let text = serde_json::to_string(value)?;
        match self {
            Self::Files { kv, kv_path, .. } => {
                let mut kv = kv.lock().expect("kv lock poisoned");
                kv.insert(key.to_string(), text);
                write_atomically(kv_path, &serde_json::to_vec_pretty(&*kv)?)
            }
            Self::Postgres(pool) => kv_put(pool, key, &text).await,
        }
    }

    /// Queue a received message for `fetch_messages`.
    pub async fn push_inbox(&self, entry: Value) -> anyhow::Result<()> {
        match self {
            Self::Files { inbox, inbox_path, .. } => {
                let mut inbox = inbox.lock().expect("inbox lock poisoned");
                inbox.push_back(entry);
                write_atomically(inbox_path, &serde_json::to_vec(&*inbox)?)
            }
            Self::Postgres(pool) => {
                sqlx::query("INSERT INTO didcomm_mcp_inbox (entry) VALUES ($1)")
                    .bind(sqlx::types::Json(entry))
                    .execute(pool)
                    .await?;
                Ok(())
            }
        }
    }

    /// Take (remove and return) up to `limit` of the oldest queued messages.
    pub async fn take_inbox(&self, limit: usize) -> anyhow::Result<Vec<Value>> {
        match self {
            Self::Files { inbox, inbox_path, .. } => {
                let mut inbox = inbox.lock().expect("inbox lock poisoned");
                let n = limit.min(inbox.len());
                let taken: Vec<Value> = inbox.drain(..n).collect();
                if !taken.is_empty() {
                    write_atomically(inbox_path, &serde_json::to_vec(&*inbox)?)?;
                }
                Ok(taken)
            }
            Self::Postgres(pool) => {
                // SKIP LOCKED: two concurrent fetches never get the same message.
                let mut rows: Vec<(i64, sqlx::types::Json<Value>)> = sqlx::query_as(
                    "DELETE FROM didcomm_mcp_inbox WHERE id IN ( \
                         SELECT id FROM didcomm_mcp_inbox ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED \
                     ) RETURNING id, entry",
                )
                .bind(i64::try_from(limit).unwrap_or(i64::MAX))
                .fetch_all(pool)
                .await?;
                rows.sort_by_key(|(id, _)| *id);
                Ok(rows.into_iter().map(|(_, entry)| entry.0).collect())
            }
        }
    }

    /// How many messages wait in the inbox.
    pub async fn inbox_len(&self) -> anyhow::Result<i64> {
        match self {
            Self::Files { inbox, .. } => Ok(inbox.lock().expect("inbox lock poisoned").len() as i64),
            Self::Postgres(pool) => Ok(sqlx::query_scalar("SELECT count(*) FROM didcomm_mcp_inbox").fetch_one(pool).await?),
        }
    }

    /// Append to the message history; returns the entry's id.
    pub async fn record(&self, peer: &str, direction: &str, at: i64, entry: Value) -> anyhow::Result<i64> {
        match self {
            Self::Files { history, history_path, .. } => {
                let mut history = history.lock().expect("history lock poisoned");
                history.next_id += 1;
                let id = history.next_id;
                history.entries.push_back(HistoryEntry { id, peer: peer.into(), direction: direction.into(), at, entry });
                while history.entries.len() > FILE_HISTORY_CAP {
                    history.entries.pop_front();
                }
                write_atomically(history_path, &serde_json::to_vec(&*history)?)?;
                Ok(id)
            }
            Self::Postgres(pool) => Ok(sqlx::query_scalar(
                "INSERT INTO didcomm_mcp_messages (peer, direction, at, entry) VALUES ($1, $2, $3, $4) RETURNING id",
            )
            .bind(peer)
            .bind(direction)
            .bind(at)
            .bind(sqlx::types::Json(entry))
            .fetch_one(pool)
            .await?),
        }
    }

    /// Remove a history entry (a message that couldn't be sent after all).
    pub async fn forget(&self, id: i64) -> anyhow::Result<()> {
        match self {
            Self::Files { history, history_path, .. } => {
                let mut history = history.lock().expect("history lock poisoned");
                history.entries.retain(|e| e.id != id);
                write_atomically(history_path, &serde_json::to_vec(&*history)?)
            }
            Self::Postgres(pool) => {
                sqlx::query("DELETE FROM didcomm_mcp_messages WHERE id = $1").bind(id).execute(pool).await?;
                Ok(())
            }
        }
    }

    /// A peer's messages, oldest first: the latest `limit`, or the `limit` before id
    /// `before`, or everything after id `after`.
    pub async fn history(&self, peer: &str, before: Option<i64>, after: Option<i64>, limit: i64) -> anyhow::Result<Vec<HistoryEntry>> {
        match self {
            Self::Files { history, .. } => {
                let history = history.lock().expect("history lock poisoned");
                let matching: Vec<&HistoryEntry> = history
                    .entries
                    .iter()
                    .filter(|e| e.peer == peer && before.is_none_or(|b| e.id < b) && after.is_none_or(|a| e.id > a))
                    .collect();
                let skip = if after.is_some() { 0 } else { matching.len().saturating_sub(limit.max(0) as usize) };
                Ok(matching.into_iter().skip(skip).cloned().collect())
            }
            Self::Postgres(pool) => {
                let rows: Vec<(i64, String, String, i64, sqlx::types::Json<Value>)> = if let Some(after) = after {
                    sqlx::query_as("SELECT id, peer, direction, at, entry FROM didcomm_mcp_messages WHERE peer = $1 AND id > $2 ORDER BY id")
                        .bind(peer)
                        .bind(after)
                        .fetch_all(pool)
                        .await?
                } else {
                    sqlx::query_as(
                        "SELECT * FROM (SELECT id, peer, direction, at, entry FROM didcomm_mcp_messages \
                         WHERE peer = $1 AND id < $2 ORDER BY id DESC LIMIT $3) t ORDER BY id",
                    )
                    .bind(peer)
                    .bind(before.unwrap_or(i64::MAX))
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                };
                Ok(rows.into_iter().map(|(id, peer, direction, at, entry)| HistoryEntry { id, peer, direction, at, entry: entry.0 }).collect())
            }
        }
    }

    /// Every peer with history: its latest message and message count, newest first.
    pub async fn conversations(&self) -> anyhow::Result<Vec<Conversation>> {
        match self {
            Self::Files { history, .. } => {
                let history = history.lock().expect("history lock poisoned");
                let mut by_peer: BTreeMap<&str, Conversation> = BTreeMap::new();
                for e in &history.entries {
                    let c = by_peer.entry(&e.peer).or_insert_with(|| Conversation { peer: e.peer.clone(), last: e.clone(), count: 0 });
                    c.count += 1;
                    c.last = e.clone();
                }
                let mut list: Vec<Conversation> = by_peer.into_values().collect();
                list.sort_by_key(|c| std::cmp::Reverse(c.last.id));
                Ok(list)
            }
            Self::Postgres(pool) => {
                let rows: Vec<(i64, String, String, i64, sqlx::types::Json<Value>, i64)> = sqlx::query_as(
                    "SELECT m.id, m.peer, m.direction, m.at, m.entry, c.n FROM didcomm_mcp_messages m \
                     JOIN (SELECT peer, max(id) AS last, count(*) AS n FROM didcomm_mcp_messages GROUP BY peer) c \
                       ON m.id = c.last ORDER BY m.id DESC",
                )
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|(id, peer, direction, at, entry, count)| Conversation {
                        peer: peer.clone(),
                        last: HistoryEntry { id, peer, direction, at, entry: entry.0 },
                        count,
                    })
                    .collect())
            }
        }
    }

    /// How many received messages a peer has after id `after`.
    pub async fn count_in_after(&self, peer: &str, after: i64) -> anyhow::Result<i64> {
        match self {
            Self::Files { history, .. } => Ok(history
                .lock()
                .expect("history lock poisoned")
                .entries
                .iter()
                .filter(|e| e.peer == peer && e.direction == "in" && e.id > after)
                .count() as i64),
            Self::Postgres(pool) => Ok(sqlx::query_scalar(
                "SELECT count(*) FROM didcomm_mcp_messages WHERE peer = $1 AND direction = 'in' AND id > $2",
            )
            .bind(peer)
            .bind(after)
            .fetch_one(pool)
            .await?),
        }
    }
}

async fn kv_get(pool: &sqlx::PgPool, key: &str) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT value FROM didcomm_mcp_kv WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await?)
}

async fn kv_put(pool: &sqlx::PgPool, key: &str, value: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO didcomm_mcp_kv (key, value) VALUES ($1, $2) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// A JSON file's contents, or the default if it's missing or unreadable (logged).
fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    match read_file(path) {
        Some(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!("ignoring {}: {e}", path.display());
            T::default()
        }),
        None => T::default(),
    }
}

/// A file's contents; `None` if it doesn't exist (or can't be read, which is logged).
fn read_file(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!("can't read {}: {e}", path.display());
            None
        }
    }
}

/// Write through a sibling temporary file renamed over `path`.
fn write_atomically(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
