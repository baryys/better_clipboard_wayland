# cbm — Clipboard Manager

Minimal clipboard history daemon + CLI for Linux.  
Supports **X11** and **Wayland** via [arboard](https://crates.io/crates/arboard).

## Features

- Persistent SQLite history (survives reboots)
- Stable display order — reading or selecting an entry never reorders the list
- Pinned entries survive history pruning and always appear first
- Batch `delete`, `pin`, `unpin`, `export` by space-separated IDs
- Case-insensitive search

## Build

```sh
cargo build --release
# Binary is at target/release/cbm
cp target/release/cbm ~/.local/bin/
```

> **Note**: `rusqlite` with `features = ["bundled"]` compiles SQLite statically,
> so no system `libsqlite3-dev` is needed.

## Quick start (manual)

```sh
# Terminal 1 — run daemon in foreground
cbm daemon

# Terminal 2 — use it
cbm list
cbm list -s "github"          # search
cbm copy 42                   # restore entry #42 to clipboard
cbm pin 1 3 7                 # pin three entries
cbm delete 5 6                # batch delete
cbm export 1 3 --format json  # export to stdout
cbm status
cbm stop
```

## systemd user service (recommended)

```sh
mkdir -p ~/.config/systemd/user
cp systemd/cbm.service ~/.config/systemd/user/

systemctl --user daemon-reload
systemctl --user enable --now cbm

# Verify
systemctl --user status cbm
journalctl --user -u cbm -f
```

## File locations

| Path | Purpose |
|---|---|
| `$XDG_RUNTIME_DIR/cbm.sock` | Unix socket (daemon ↔ CLI) |
| `$XDG_DATA_HOME/cbm/history.db` | SQLite history database |

## List output

```
ID      PIN  AGE         CONTENT
────────────────────────────────────────────────────────────────────────
#42     ★    5m ago      Hello world — pinned item
#40     ★    3h ago      Another pinned snippet
#99          12s ago     Most recent unpinned entry
#98          4m ago      Previous entry
```

**★** = pinned.  IDs are stable monotonic integers.  The order never changes due
to reads or selections — only new entries, deletions, and pin/unpin operations
can shift positions.

## Troubleshooting

### Wayland

arboard uses `wlr-data-control` (sway, KDE, wlroots-based compositors) or
`ext-data-control` (GNOME 44+). If the daemon starts but `cbm list` is always
empty, confirm:

```sh
echo $WAYLAND_DISPLAY     # must be set (e.g. wayland-0)
```

If using a non-systemd session, export `WAYLAND_DISPLAY` in the service
`Environment=` line or via `~/.config/environment.d/wayland.conf`.

### X11

`DISPLAY` must be set in the daemon's environment:

```sh
echo $DISPLAY             # must be set (e.g. :0)
```

For systemd services that start before the display server is ready, add:

```ini
[Service]
Environment=DISPLAY=:0
```

### "cannot reach daemon" error

The daemon is not running or the socket path is wrong:

```sh
systemctl --user start cbm
ls "$XDG_RUNTIME_DIR/cbm.sock"
```

### Debug logging

```sh
RUST_LOG=debug cbm daemon
```

## Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `RUST_LOG` | `warn` | Log level (`error`, `warn`, `info`, `debug`, `trace`) |
| `XDG_RUNTIME_DIR` | `/run/user/$(id -u)` | Socket location |
| `XDG_DATA_HOME` | `~/.local/share` | Database location |
| `WAYLAND_DISPLAY` | *(set by compositor)* | Required on Wayland |
| `DISPLAY` | *(set by X server)* | Required on X11 |

## Implementation plan

See [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) for the phased roadmap.
