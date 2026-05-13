use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

// ── Wire types ────────────────────────────────────────────────────────────────

/// A single clipboard history entry.  The `id` is immutable after insertion and
/// serves as the stable, user-visible handle for every CLI operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id:         i64,
    pub content:    String,
    pub pinned:     bool,
    pub created_at: i64,   // unix seconds, write-once
}

/// Every command the CLI sends to the daemon.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Request {
    List   { limit: usize, search: Option<String>, pinned_only: bool },
    Get    { id: i64 },
    Copy   { id: i64 },
    Delete { ids: Vec<i64> },
    Pin    { ids: Vec<i64> },
    Unpin  { ids: Vec<i64> },
    Export { ids: Vec<i64>, format: ExportFormat },
    Clear  { unpinned_only: bool },
    Status,
    Shutdown,
}

/// Output format for `export`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Json,
    Text,
}

/// Every response the daemon can send back.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result")]
pub enum Response {
    Entries  { items: Vec<Entry> },
    Entry    { item: Entry },
    Exported { data: String },
    Count    { n: usize },
    Status   { version: String, count: usize },
    Ok,
    Err      { message: String },
}

// ── Path helpers ──────────────────────────────────────────────────────────────

pub fn socket_path() -> PathBuf {
    // $XDG_RUNTIME_DIR is a tmpfs managed by systemd-logind; it vanishes on
    // logout, so the stale-socket problem is automatically avoided between sessions.
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::runtime_dir()
                .or_else(|| dirs::home_dir().map(|h| h.join(".local/run")))
                .unwrap_or_else(|| PathBuf::from("/tmp"))
        })
        .join("cbm.sock")
}

pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
        })
        .join("cbm")
}

// ── Client-side IPC ───────────────────────────────────────────────────────────

/// Connect to the running daemon, send one request, return the response.
pub fn send_request(req: &Request) -> Result<Response> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|e| {
        Error::Daemon(format!(
            "cannot reach daemon at {}: {}  (is `cbm daemon` running?)",
            path.display(), e
        ))
    })?;

    let json = serde_json::to_string(req)?;
    stream.write_all(json.as_bytes())?;
    stream.write_all(b"\n")?;
    // Signal EOF so the daemon's BufRead::read_line returns.
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}
