use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::thread;

use arboard::Clipboard;
use log::{error, info, warn};
use sha2::Digest;

use crate::error::Result;
use crate::ipc::{self, Entry, ExportFormat, Request, Response};
use crate::storage::Storage;
use crate::watcher;

// ── Shared state ──────────────────────────────────────────────────────────────

struct State {
    storage:       Arc<Mutex<Storage>>,
    /// Hash of content we just pushed to the clipboard ourselves (restore).
    /// Lets the watcher skip adding it back as a new entry.
    last_set_hash: Arc<Mutex<Option<String>>>,
    /// Clipboard instance kept alive after a restore so the X11 selection owner
    /// thread keeps serving requests.  Replaced on each new restore.
    restore_cb:    Arc<Mutex<Option<Clipboard>>>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(max_entries: usize) -> Result<()> {
    let data_dir = ipc::data_dir();
    std::fs::create_dir_all(&data_dir)?;

    let state = Arc::new(State {
        storage:       Arc::new(Mutex::new(Storage::new(&data_dir.join("history.db"), max_entries)?)),
        last_set_hash: Arc::new(Mutex::new(None)),
        restore_cb:    Arc::new(Mutex::new(None)),
    });

    // Spawn clipboard watcher thread.
    {
        let storage   = Arc::clone(&state.storage);
        let last_hash = Arc::clone(&state.last_set_hash);
        thread::Builder::new()
            .name("cbm-watcher".into())
            .spawn(move || watcher::run_watcher(storage, last_hash))?;
    }

    // Bind Unix socket (remove stale socket from a previous run if present).
    let sock = ipc::socket_path();
    if sock.exists() {
        std::fs::remove_file(&sock)?;
    }
    let listener = UnixListener::bind(&sock)?;
    info!("cbm daemon listening on {}", sock.display());

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let state = Arc::clone(&state);
                thread::Builder::new()
                    .name("cbm-conn".into())
                    .spawn(move || {
                        if let Err(e) = handle_conn(stream, state) {
                            warn!("connection: {e}");
                        }
                    })?;
            }
            Err(e) => error!("accept: {e}"),
        }
    }
    Ok(())
}

// ── Per-connection handler ────────────────────────────────────────────────────

fn handle_conn(stream: UnixStream, state: Arc<State>) -> Result<()> {
    let writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim().is_empty() {
        return Ok(());
    }

    let req: Request = serde_json::from_str(line.trim())?;
    let resp = dispatch(req, &state);

    let mut json = serde_json::to_string(&resp)?;
    json.push('\n');
    (&writer).write_all(json.as_bytes())?;
    Ok(())
}

// ── Request dispatch ──────────────────────────────────────────────────────────

fn dispatch(req: Request, state: &State) -> Response {
    match req {
        Request::List { limit, search, pinned_only } => {
            match state.storage.lock().unwrap().list(limit, search.as_deref(), pinned_only) {
                Ok(items) => Response::Entries { items },
                Err(e)    => Response::Err { message: e.to_string() },
            }
        }

        Request::Get { id } => {
            match state.storage.lock().unwrap().get(id) {
                Ok(Some(item)) => Response::Entry { item },
                Ok(None)       => Response::Err { message: format!("entry #{id} not found") },
                Err(e)         => Response::Err { message: e.to_string() },
            }
        }

        Request::Copy { id } => {
            // Look up the entry first (brief lock), then do clipboard I/O outside the lock.
            let entry = match state.storage.lock().unwrap().get(id) {
                Ok(Some(e)) => e,
                Ok(None)    => return Response::Err { message: format!("entry #{id} not found") },
                Err(e)      => return Response::Err { message: e.to_string() },
            };

            // Tell the watcher to skip the next occurrence of this hash.
            let hash = hex::encode(sha2::Sha256::digest(entry.content.as_bytes()));
            *state.last_set_hash.lock().unwrap() = Some(hash);

            match Clipboard::new() {
                Err(e) => Response::Err { message: format!("clipboard open: {e}") },
                Ok(mut cb) => {
                    if let Err(e) = cb.set_text(&entry.content) {
                        return Response::Err { message: format!("clipboard set: {e}") };
                    }
                    // Keep the Clipboard struct alive so the X11 selection-owner
                    // thread continues serving paste requests after this handler returns.
                    *state.restore_cb.lock().unwrap() = Some(cb);
                    Response::Ok
                }
            }
        }

        Request::Delete { ids } => {
            match state.storage.lock().unwrap().delete(&ids) {
                Ok(n)  => Response::Count { n },
                Err(e) => Response::Err { message: e.to_string() },
            }
        }

        Request::Pin { ids } => {
            match state.storage.lock().unwrap().set_pinned(&ids, true) {
                Ok(n)  => Response::Count { n },
                Err(e) => Response::Err { message: e.to_string() },
            }
        }

        Request::Unpin { ids } => {
            match state.storage.lock().unwrap().set_pinned(&ids, false) {
                Ok(n)  => Response::Count { n },
                Err(e) => Response::Err { message: e.to_string() },
            }
        }

        Request::Export { ids, format } => {
            // Collect entries one by one; each get() acquires/releases the lock.
            let items: Vec<Entry> = ids
                .iter()
                .filter_map(|&id| state.storage.lock().unwrap().get(id).ok().flatten())
                .collect();

            if items.is_empty() {
                return Response::Err { message: "no matching entries".into() };
            }

            let data = match format {
                ExportFormat::Json => match serde_json::to_string_pretty(&items) {
                    Ok(s)  => s,
                    Err(e) => return Response::Err { message: e.to_string() },
                },
                ExportFormat::Text => items
                    .iter()
                    .map(|e| e.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n---\n"),
            };
            Response::Exported { data }
        }

        Request::Clear { unpinned_only } => {
            match state.storage.lock().unwrap().clear(unpinned_only) {
                Ok(n)  => Response::Count { n },
                Err(e) => Response::Err { message: e.to_string() },
            }
        }

        Request::Status => {
            match state.storage.lock().unwrap().count() {
                Ok(count) => Response::Status {
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                    count,
                },
                Err(e) => Response::Err { message: e.to_string() },
            }
        }

        Request::Shutdown => {
            info!("Shutdown requested via socket");
            // TODO: clean up the socket file before exiting
            std::process::exit(0);
        }
    }
}
