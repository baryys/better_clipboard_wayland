mod daemon;
mod error;
mod ipc;
mod storage;
mod watcher;

use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};

use error::{Error, Result};
use ipc::{Entry, ExportFormat, Request, Response};

// ── CLI definition ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name    = "cbm",
    version,
    about   = "Clipboard history daemon + CLI for Linux (X11 / Wayland)",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the background daemon (run in foreground; use systemd or `&`)
    Daemon {
        #[arg(long, default_value = "1000",
              help = "Maximum history entries; oldest unpinned entry is pruned on overflow")]
        max_entries: usize,
    },

    /// List clipboard history in stable display order
    ///
    /// Order: pinned entries first (most-recently-pinned at top), then unpinned
    /// entries newest-first.  Listing never mutates positions.
    List {
        #[arg(short, long, default_value = "50")]
        limit: usize,
        #[arg(short, long, help = "Case-insensitive substring filter")]
        search: Option<String>,
        #[arg(long)]
        pinned_only: bool,
    },

    /// Print the full content of one entry
    Get { id: i64 },

    /// Restore an entry to the system clipboard
    Copy { id: i64 },

    /// Delete one or more entries (batch; IDs separated by spaces)
    Delete {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<i64>,
    },

    /// Pin one or more entries (pinned entries survive pruning and appear first)
    Pin {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<i64>,
    },

    /// Unpin one or more entries
    Unpin {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<i64>,
    },

    /// Export one or more entries to stdout
    Export {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<i64>,
        #[arg(long, value_enum, default_value = "text")]
        format: ExportFormat,
    },

    /// Remove history entries
    Clear {
        #[arg(long, help = "Keep pinned entries; only remove unpinned ones")]
        unpinned_only: bool,
    },

    /// Show daemon version and history size
    Status,

    /// Ask the daemon to shut down
    Stop,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn"),
    )
    .init();

    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("cbm: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::Daemon { max_entries } => daemon::run(max_entries),
        other => {
            let req  = cmd_to_request(other);
            let resp = ipc::send_request(&req)?;
            render(resp)
        }
    }
}

// ── Command → Request mapping ─────────────────────────────────────────────────

fn cmd_to_request(cmd: Cmd) -> Request {
    match cmd {
        Cmd::List   { limit, search, pinned_only } => Request::List { limit, search, pinned_only },
        Cmd::Get    { id }                         => Request::Get    { id },
        Cmd::Copy   { id }                         => Request::Copy   { id },
        Cmd::Delete { ids }                        => Request::Delete { ids },
        Cmd::Pin    { ids }                        => Request::Pin    { ids },
        Cmd::Unpin  { ids }                        => Request::Unpin  { ids },
        Cmd::Export { ids, format }                => Request::Export { ids, format },
        Cmd::Clear  { unpinned_only }              => Request::Clear  { unpinned_only },
        Cmd::Status                                => Request::Status,
        Cmd::Stop                                  => Request::Shutdown,
        Cmd::Daemon { .. }                         => unreachable!(),
    }
}

// ── Response rendering ────────────────────────────────────────────────────────

fn render(resp: Response) -> Result<()> {
    match resp {
        Response::Entries { items }          => render_list(&items),
        Response::Entry   { item }           => print!("{}", item.content),
        Response::Exported { data }          => println!("{data}"),
        Response::Count   { n }              => println!("{n} item(s) affected"),
        Response::Status  { version, count } =>
            println!("cbm v{version}  —  {count} entries in history"),
        Response::Ok                         => {}
        Response::Err     { message }        => return Err(Error::Daemon(message)),
    }
    Ok(())
}

fn render_list(items: &[Entry]) {
    if items.is_empty() {
        println!("(no entries)");
        return;
    }
    println!("{:<6}  {:<3}  {:<10}  {}", "ID", "PIN", "AGE", "CONTENT");
    println!("{}", "─".repeat(72));
    for e in items {
        let pin     = if e.pinned { "★" } else { " " };
        let age     = fmt_age(e.created_at);
        let preview = truncate_content(&e.content, 48);
        println!("#{:<5}  {:<3}  {:<10}  {}", e.id, pin, age, preview);
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────────

fn fmt_age(unix_secs: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let age = (now - unix_secs).max(0);
    match age {
        a if a < 60    => format!("{a}s ago"),
        a if a < 3_600 => format!("{}m ago", a / 60),
        a if a < 86_400 => format!("{}h ago", a / 3_600),
        a              => format!("{}d ago", a / 86_400),
    }
}

/// Collapse whitespace runs to a single space, then truncate to `max` chars.
fn truncate_content(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max.saturating_sub(1)).collect::<String>())
    }
}
