//! An MCP server that lets AI agents discover, learn and use DIDComm v2 protocols with
//! any peer, through a fixed set of tools. See `PLAN.md` and `README.md`.

pub mod api;
pub mod bridge;
pub mod config;
pub mod http;
pub mod server;
pub mod short_url;
pub mod profile;
pub mod store;
pub mod web;
