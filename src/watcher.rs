use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use log::{debug, error, info, warn};

use crate::storage::Storage;

const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Dispatch to the right backend: wl-paste subprocess on Wayland, arboard on X11.
///
/// arboard can initialize its Wayland connection but silently fails to receive
/// clipboard change notifications on many compositors (including Niri).  The
/// wl-paste subprocess approach is the compositor-agnostic solution; it works
/// anywhere wl-clipboard is installed.
pub fn run_watcher(
    storage:       Arc<Mutex<Storage>>,
    last_set_hash: Arc<Mutex<Option<String>>>,
) {
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        run_wayland(storage, last_set_hash);
    } else {
        run_x11(storage, last_set_hash);
    }
}

// ── Wayland backend (wl-paste subprocess) ────────────────────────────────────

fn run_wayland(storage: Arc<Mutex<Storage>>, last_set_hash: Arc<Mutex<Option<String>>>) {
    // Confirm wl-paste is present before entering the loop.
    match Command::new("wl-paste").arg("--version").output() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            error!(
                "watcher: wl-paste not found — install wl-clipboard:\n  \
                 Arch:         pacman -S wl-clipboard\n  \
                 Debian/Ubuntu: apt install wl-clipboard\n  \
                 Fedora:        dnf install wl-clipboard"
            );
            return;
        }
        Err(e) => { error!("watcher: wl-paste probe failed: {e}"); return; }
        Ok(out) => {
            if let Ok(v) = String::from_utf8(out.stdout) {
                info!("watcher: using {}", v.lines().next().unwrap_or("wl-paste"));
            }
        }
    }

    let mut prev_hash = String::new();

    loop {
        thread::sleep(POLL_INTERVAL);

        let text = match wayland_read_text() {
            Some(t) => t,
            None    => { debug!("watcher: clipboard empty or non-text"); continue; }
        };

        if text.trim().is_empty() {
            continue;
        }

        process_text(text, &mut prev_hash, &storage, &last_set_hash);
    }
}

/// Call `wl-paste` for each MIME type we care about; return first success.
fn wayland_read_text() -> Option<String> {
    for mime in &["text/plain;charset=utf-8", "text/plain"] {
        if let Ok(out) = Command::new("wl-paste")
            .args(["--no-newline", "--type", mime])
            .output()
        {
            if out.status.success() {
                if let Ok(text) = String::from_utf8(out.stdout) {
                    return Some(text);
                }
            }
        }
    }
    None
}

// ── X11 backend (arboard) ─────────────────────────────────────────────────────

fn run_x11(storage: Arc<Mutex<Storage>>, last_set_hash: Arc<Mutex<Option<String>>>) {
    let mut clipboard = match Clipboard::new() {
        Ok(c)  => { info!("watcher: arboard X11 backend initialized"); c }
        Err(e) => { error!("watcher: cannot open clipboard: {e}"); return; }
    };

    let mut prev_hash  = String::new();
    let mut fail_count: u64 = 0;

    loop {
        thread::sleep(POLL_INTERVAL);

        let text = match clipboard.get_text() {
            Ok(t) => {
                if fail_count > 0 {
                    info!("watcher: clipboard recovered after {fail_count} failures");
                    fail_count = 0;
                }
                t
            }
            Err(e) => {
                fail_count += 1;
                if fail_count == 1 || fail_count % 60 == 0 {
                    warn!("watcher: clipboard read failed ({fail_count}): {e}");
                } else {
                    debug!("watcher: get_text: {e}");
                }
                continue;
            }
        };

        if text.trim().is_empty() {
            continue;
        }

        process_text(text, &mut prev_hash, &storage, &last_set_hash);
    }
}

// ── Shared logic ──────────────────────────────────────────────────────────────

fn process_text(
    text:          String,
    prev_hash:     &mut String,
    storage:       &Arc<Mutex<Storage>>,
    last_set_hash: &Arc<Mutex<Option<String>>>,
) {
    let hash = Storage::hash(&text);

    if hash == *prev_hash {
        return;
    }

    // Skip content the daemon just pushed back via `cbm copy`.
    {
        let mut guard = last_set_hash.lock().unwrap();
        if guard.as_deref() == Some(hash.as_str()) {
            *prev_hash = hash;
            *guard = None;
            return;
        }
    }

    *prev_hash = hash;

    match storage.lock().unwrap().insert(&text) {
        Ok(Some(e)) => info!("watcher: captured entry #{} ({} bytes)", e.id, text.len()),
        Ok(None)    => debug!("watcher: duplicate, skipped"),
        Err(e)      => warn!("watcher: insert failed: {e}"),
    }
}
