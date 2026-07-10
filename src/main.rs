#![windows_subsystem = "windows"]

//! `my-tray-app` – a headless system-tray daemon.
//!
//! Architecture
//! ------------
//! * A multi-threaded Tokio runtime hosts the Axum HTTP server (`127.0.0.1:3000`)
//!   and the TCP control listener (`127.0.0.1:4000`).
//! * The system tray is the only UI entry point. It is built on the main thread
//!   and the OS event loop pumps the tray's menu events, which are delivered
//!   through `tray_icon::menu::MenuEvent::receiver()`.
//! * A background worker thread owns the canonical state + the SQLite handle and
//!   drains the tray-menu receiver and the `AppCommand` receiver (from the HTTP /
//!   TCP handlers) every 100 ms. There is **no GUI window** at all – this is a
//!   true headless daemon, so there is no eframe/winit window to flash or show
//!   up in the taskbar, and tray events are handled immediately.

mod commands;
mod db;
mod http;
mod tcp;

use commands::{AppCommand, AppStatus};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::time::Duration;

/// Drives the canonical state + the SQLite handle, draining both the tray-menu
/// event receiver and the `AppCommand` receiver (HTTP/TCP handlers) every 100 ms.
/// Runs on its own thread so it is independent of the OS event loop.
fn run_worker(rx: Receiver<AppCommand>) {
    let db = match db::Db::open() {
        Ok(db) => db,
        Err(e) => {
            tracing::error!("failed to open SQLite store: {e}");
            std::process::exit(1);
        }
    };
    let mut status = AppStatus {
        running: false,
        current_task: None,
        task_count: db.count_tasks().unwrap_or(0),
    };

    tracing::info!("worker thread started; draining tray-menu + command channels");
    loop {
        // ---- Tray menu events (pumped by the OS event loop on the main thread) ----
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            match event.id.as_ref() {
                "start" => apply_start(&mut status, &db, "menu"),
                "stop" => apply_stop(&mut status),
                "quit" => {
                    tracing::info!("quit requested from tray menu");
                    // Process exits immediately; the OS removes the tray icon.
                    std::process::exit(0);
                }
                other => tracing::debug!("ignored menu event: {other}"),
            }
        }

        // ---- Commands from HTTP / TCP handlers ----
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                AppCommand::StartTask(name) => apply_start(&mut status, &db, &name),
                AppCommand::StopTask => apply_stop(&mut status),
                AppCommand::GetStatus { respond_to } => {
                    let _ = respond_to.send(status.clone());
                }
                AppCommand::ListTasks { respond_to } => {
                    let _ = respond_to.send(db.list_tasks().unwrap_or_default());
                }
            }
        }

        thread::sleep(Duration::from_millis(100));
    }
}

fn apply_start(status: &mut AppStatus, db: &db::Db, name: &str) {
    status.running = true;
    status.current_task = Some(name.to_string());
    if let Err(e) = db.insert_task(name) {
        tracing::warn!("failed to persist task '{name}': {e}");
    }
    status.task_count = db.count_tasks().unwrap_or(status.task_count);
    tracing::info!("task started: {name}");
}

fn apply_stop(status: &mut AppStatus) {
    status.running = false;
    status.current_task = None;
    tracing::info!("task stopped");
}

/// Build the tray icon + right-click menu. Returns the owned `TrayIcon`.
///
/// Must be called on the OS main/UI thread (the same thread that runs the
/// event loop), because the underlying platform menus require it.
fn build_tray_icon() -> tray_icon::TrayIcon {
    use tray_icon::menu::{Menu, MenuItem};

    let mut menu = Menu::new();
    // `None` is left untyped on purpose so the compiler infers the exact
    // accelerator type expected by `MenuItem::with_id`.
    let start_i = MenuItem::with_id("start", "Start Task", true, None);
    let stop_i = MenuItem::with_id("stop", "Stop Task", true, None);
    let quit_i = MenuItem::with_id("quit", "Quit", true, None);
    menu.append_items(&[&start_i, &stop_i, &quit_i])
        .expect("failed to build tray menu");

    let icon = load_icon();

    tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("My Tray App")
        .with_icon(icon)
        .build()
        .expect("failed to build tray icon")
}

/// Decode raw RGBA image bytes (PNG/JPEG/…) into a `tray_icon::Icon`.
/// Returns `None` if the bytes cannot be decoded.
fn icon_from_bytes(bytes: &[u8]) -> Option<tray_icon::Icon> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.into_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    tray_icon::Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

/// Build the tray icon. The real icon is **embedded in the binary** via
/// `include_bytes!`, so the released executable shows the correct icon no
/// matter where it is launched from. File-system paths are only consulted as
/// a dev convenience, and a generated square is the last-resort fallback.
fn load_icon() -> tray_icon::Icon {
    // 1) Embedded icon – always available in the compiled binary.
    match icon_from_bytes(include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/icon.png"))) {
        Some(icon) => {
            tracing::info!("loaded embedded tray icon");
            return icon;
        }
        None => tracing::warn!("embedded icon.png failed to decode – trying files"),
    }

    // 2) File paths next to the executable / CWD (dev convenience).
    let mut candidates: Vec<PathBuf> = vec![PathBuf::from("examples/icon.png")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let mut p = dir.to_path_buf();
            p.push("examples/icon.png");
            candidates.push(p);
            let mut p2 = dir.to_path_buf();
            p2.push("icon.png");
            candidates.push(p2);
        }
    }
    for path in candidates {
        if let Ok(bytes) = std::fs::read(&path) {
            if let Some(icon) = icon_from_bytes(&bytes) {
                tracing::info!("loaded tray icon from {}", path.display());
                return icon;
            }
        }
    }

    // 3) Last-resort fallback: a generated solid square.
    tracing::warn!("icon.png not found – using generated fallback icon");
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for _ in 0..(size * size) {
        rgba.extend_from_slice(&[70u8, 130u8, 220u8, 255u8]);
    }
    tray_icon::Icon::from_rgba(rgba, size, size).expect("fallback icon")
}

fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .try_init();

    // ---- Tokio runtime (multi-threaded) ----
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    let (cmd_tx, cmd_rx) = channel::<AppCommand>();

    // ---- Axum HTTP control panel ----
    let http_tx = cmd_tx.clone();
    rt.spawn(async move {
        let state = http::HttpState { cmd_tx: http_tx };
        let addr: SocketAddr = "127.0.0.1:3000".parse().expect("valid addr");
        http::run_server(state, addr).await;
    });

    // ---- TCP control listener ----
    let tcp_tx = cmd_tx.clone();
    rt.spawn(async move {
        let addr: SocketAddr = "127.0.0.1:4000".parse().expect("valid addr");
        tcp::run_listener(tcp_tx, addr).await;
    });

    // ---- Worker thread: owns state + drains channels ----
    thread::spawn(move || run_worker(cmd_rx));

    tracing::info!("starting my-tray-app (headless tray daemon)");

    // ---- Build the tray + run the OS event loop on the main thread ----
    #[cfg(target_os = "linux")]
    {
        // GTK must live on its own thread; winit/egui must NOT initialise GTK.
        gtk::init().expect("failed to initialise GTK");
        let _tray = build_tray_icon();
        gtk::main();
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _tray = build_tray_icon();
        run_os_loop();
    }
}

/// Pump the OS event loop (Windows / macOS) without ever creating a window.
/// `tray_icon` delivers menu events through this loop to `MenuEvent::receiver()`.
#[cfg(not(target_os = "linux"))]
fn run_os_loop() {
    use winit::event_loop::EventLoop;
    let event_loop = EventLoop::builder()
        .build()
        .expect("failed to build event loop");
    // No window is created – we only need the loop running so the platform can
    // dispatch tray-icon's menu messages. `run` blocks until the process exits.
    let _ = event_loop.run(|_event, _target| {});
}
