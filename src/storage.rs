use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::ipc::Entry;

pub struct Storage {
    conn:        Connection,
    max_entries: usize,
}

impl Storage {
    pub fn new(db_path: &Path, max_entries: usize) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous  = NORMAL;

             CREATE TABLE IF NOT EXISTS entries (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 content    TEXT    NOT NULL,
                 hash       TEXT    NOT NULL UNIQUE,   -- SHA-256 hex for O(1) dedup
                 pinned     INTEGER NOT NULL DEFAULT 0,
                 pinned_at  INTEGER,                   -- unix secs; NULL when unpinned
                 created_at INTEGER NOT NULL            -- write-once
             );

             CREATE INDEX IF NOT EXISTS idx_hash ON entries(hash);",
        )?;
        Ok(Self { conn, max_entries })
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    pub fn hash(content: &str) -> String {
        hex::encode(Sha256::digest(content.as_bytes()))
    }

    fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
        Ok(Entry {
            id:         r.get(0)?,
            content:    r.get(1)?,
            pinned:     r.get::<_, i64>(2)? != 0,
            created_at: r.get(3)?,
        })
    }

    // ── Write ops ─────────────────────────────────────────────────────────────

    /// Insert new content; returns `None` if the content already exists (exact dedup).
    /// On overflow, prunes the oldest unpinned entry.
    pub fn insert(&mut self, content: &str) -> Result<Option<Entry>> {
        let hash = Self::hash(content);
        let already_exists = self
            .conn
            .query_row("SELECT 1 FROM entries WHERE hash=?1", params![hash], |_| Ok(()))
            .is_ok();

        if already_exists {
            return Ok(None);
        }

        let now = Self::now();
        self.conn.execute(
            "INSERT INTO entries (content, hash, created_at) VALUES (?1, ?2, ?3)",
            params![content, hash, now],
        )?;
        let id = self.conn.last_insert_rowid();

        // Prune oldest unpinned entries when the history exceeds max_entries.
        // OFFSET ?1 keeps the newest max_entries rows; everything beyond is deleted.
        self.conn.execute(
            "DELETE FROM entries
             WHERE id IN (
                 SELECT id FROM entries
                 WHERE  pinned = 0
                 ORDER  BY id DESC
                 LIMIT  -1 OFFSET ?1
             )",
            params![self.max_entries as i64],
        )?;

        Ok(Some(Entry { id, content: content.to_owned(), pinned: false, created_at: now }))
    }

    pub fn delete(&mut self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() { return Ok(0); }
        let tx = self.conn.transaction()?;
        let mut total = 0usize;
        for &id in ids {
            total += tx.execute("DELETE FROM entries WHERE id=?1", params![id])?;
        }
        tx.commit()?;
        Ok(total)
    }

    /// Set or clear the pinned flag for a batch of entries.
    /// `pinned_at` is recorded when pinning so we can sort pinned items stably
    /// by recency-of-pin rather than insertion order.
    pub fn set_pinned(&mut self, ids: &[i64], pinned: bool) -> Result<usize> {
        if ids.is_empty() { return Ok(0); }
        let pinned_at: Option<i64> = if pinned { Some(Self::now()) } else { None };
        let tx = self.conn.transaction()?;
        let mut total = 0usize;
        for &id in ids {
            total += tx.execute(
                "UPDATE entries SET pinned=?1, pinned_at=?2 WHERE id=?3",
                params![pinned as i64, pinned_at, id],
            )?;
        }
        tx.commit()?;
        Ok(total)
    }

    pub fn clear(&mut self, unpinned_only: bool) -> Result<usize> {
        let n = if unpinned_only {
            self.conn.execute("DELETE FROM entries WHERE pinned=0", [])?
        } else {
            self.conn.execute("DELETE FROM entries", [])?
        };
        Ok(n)
    }

    // ── Read ops ──────────────────────────────────────────────────────────────

    /// Return entries in stable display order:
    ///   1. Pinned group (sorted by pinned_at DESC — most-recently-pinned first)
    ///   2. Unpinned group (sorted by id DESC — newest insertion first)
    ///
    /// Neither read itself nor any selection operation ever writes to the DB, so
    /// calling this function cannot perturb the order of subsequent calls.
    pub fn list(
        &self,
        limit: usize,
        search: Option<&str>,
        pinned_only: bool,
    ) -> Result<Vec<Entry>> {
        let pat = search.map(|s| format!("%{s}%"));
        let mut stmt = self.conn.prepare(
            "SELECT id, content, pinned, created_at
             FROM   entries
             WHERE  (?1 = 0 OR pinned = 1)
               AND  (?2 IS NULL OR content LIKE ?2)
             ORDER  BY pinned DESC,
                       CASE WHEN pinned = 1 THEN pinned_at END DESC,
                       id DESC
             LIMIT  ?3",
        )?;
        let rows = stmt.query_map(
            params![pinned_only as i64, pat, limit as i64],
            Self::row_to_entry,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get(&self, id: i64) -> Result<Option<Entry>> {
        match self.conn.query_row(
            "SELECT id, content, pinned, created_at FROM entries WHERE id=?1",
            params![id],
            Self::row_to_entry,
        ) {
            Ok(e) => Ok(Some(e)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}
