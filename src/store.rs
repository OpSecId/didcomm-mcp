//! Where the agent's durable data lives: its identity (private keys), its state
//! (connections, created invitations, the v1 mediation) and the inbox of messages
//! delivered straight to its own endpoint.
//!
//! Two backends:
//! - [`Store::Files`] (the default): the identity file, the state file, and an inbox
//!   file next to the state file.
//! - [`Store::Postgres`] (`database_url`): two tables, created on startup. Nothing is
//!   written to disk, so no volume is needed. The identity row holds the private keys
//!   in the clear, as the identity file does: protect the database accordingly.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use didcomm_agent::Identity;
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
";

const IDENTITY_KEY: &str = "identity";
const STATE_KEY: &str = "state";

pub enum Store {
    Files {
        identity_path: PathBuf,
        state_path: PathBuf,
        inbox_path: PathBuf,
        inbox: Mutex<VecDeque<Value>>,
    },
    Postgres(sqlx::PgPool),
}

impl Store {
    /// Files: `identity_path`, `state_path`, and the inbox at `<state_path>.inbox.json`
    /// (e.g. `connections.inbox.json`).
    pub fn files(identity_path: &Path, state_path: &Path) -> Self {
        let inbox_path = state_path.with_extension("inbox.json");
        let inbox = match read_file(&inbox_path) {
            Some(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("ignoring {}: {e}", inbox_path.display());
                VecDeque::new()
            }),
            None => VecDeque::new(),
        };
        Self::Files {
            identity_path: identity_path.to_path_buf(),
            state_path: state_path.to_path_buf(),
            inbox_path,
            inbox: Mutex::new(inbox),
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
            Self::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO didcomm_mcp_kv (key, value) VALUES ($1, $2) \
                     ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
                )
                .bind(STATE_KEY)
                .bind(json)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    /// Queue a message delivered to this agent's endpoint, for `fetch_messages`.
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
}

async fn kv_get(pool: &sqlx::PgPool, key: &str) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT value FROM didcomm_mcp_kv WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await?)
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
