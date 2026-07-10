# my-tray-app

A headless **system-tray daemon** written in Rust. There is no main window – the
tray icon is the only UI entry point, and it exposes a REST API plus a TCP
control socket so other programs/scripts can drive it.

## Stack

| Concern            | Crate |
|--------------------|-------|
| GUI / tray         | `eframe` + `egui` + `tray-icon` (tauri-apps) |
| Async runtime      | `tokio` (multi-threaded) |
| HTTP control panel | `axum` on `127.0.0.1:3000` (REST/JSON) |
| Inter-thread comm. | `std::sync::mpsc` (handlers → main thread) |
| Local storage      | `rusqlite` at `~/.my-tray-app/db.sqlite` |
| Extra local ability| `tokio::net::TcpListener` on `127.0.0.1:4000` |
| Icon               | `examples/icon.png` read via `image` → `tray_icon::Icon` |

> No Tauri / Dioxus / WebView is used. Pure `egui` + `tray-icon`.

## Architecture

```
 HTTP :3000 ─┐
             ├─► mpsc::Sender<AppCommand> ─► main thread (eframe update) ─► state + SQLite
 TCP  :4000 ─┘                                            ▲
                                                           │ oneshot (GetStatus / ListTasks)
 Tray menu ────────────────────────────────────────────────┘
```

The main thread is the **single owner** of state and the SQLite connection, so
there is never any cross-thread data race. On Linux the GTK event loop runs on
its own thread; on Windows/macOS the tray is built inside the `eframe` closure.

## Build & run

```bash
cargo run --release
```

* Linux needs `libgtk-3-dev` and `libappindicator3-dev` (and optionally
  `libwebkit2gtk-4.1-dev` for a future WebView).
* The tray icon is the only UI; the app keeps running until you pick **Quit**
  from the tray menu.

## HTTP API

| Method | Path      | Body              | Description |
|--------|-----------|-------------------|-------------|
| GET    | `/health` | –                 | returns `ok` |
| POST   | `/task`   | `{"name":"..."}`  | start a task (`StartTask`) |
| POST   | `/stop`   | –                 | stop the current task |
| GET    | `/status` | –                 | JSON snapshot of current status |
| GET    | `/tasks`  | –                 | JSON list of persisted tasks |

```bash
curl http://127.0.0.1:3000/health
curl -X POST http://127.0.0.1:3000/task -H 'content-type: application/json' -d '{"name":"backup"}'
curl http://127.0.0.1:3000/status
```

## TCP protocol (`127.0.0.1:4000`, one command per line)

* `START <name>` – start a task
* `STOP` – stop the current task
* `STATUS` – echo a status line
* `QUIT` – close the connection

```bash
printf 'START backup\nSTATUS\n' | nc 127.0.0.1 4000
```

## Release builds

`.github/workflows/release.yml` builds per-OS artifacts on `push` of a `v*`
tag (or manual dispatch) via `dtolnay/rust-action@stable` and uploads them to a
GitHub release with SHA-256 checksums.
