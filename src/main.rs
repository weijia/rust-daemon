//! `my-tray-app` – a headless system-tray daemon.
//!
//! Architecture
//! ------------
//! * A multi-threaded Tokio runtime hosts the Axum HTTP server (`127.0.0.1:3000`)
//!   and the TCP control listener (`127.0.0.1:4000`).
//! * The system tray is the only UI entry point. On Linux the GTK event loop runs
//!   on its own thread (`gtk::init()` + `gtk::main()`); on Windows/macOS the tray
//!   icon is built inside the `eframe::run_native` closure.
//! * The tray menu (and HTTP/TCP handlers) only ever send [`AppCommand`]s through
//!   a `std::sync::mpsc::Sender`. The eframe main thread drains that receiver inside
//!   `update()` and is the single owner of the canonical state + the SQLite handle.

mod commands;
mod db;
mod http;
mod tcp;

use commands::{AppCommand, AppStatus};
use eframe::egui;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

struct TrayApp {
    cmd_tx: Sender<AppCommand>,
    cmd_rx: Receiver<AppCommand>,
    status: AppStatus,
    db: db::Db,
    /// Kept alive so the icon is not dropped. On Linux the tray lives on the GTK
    /// thread instead, so this is `None` there.
    _tray: Option<tray_icon::TrayIcon>,
}

impl TrayApp {
    fn new(cmd_tx: Sender<AppCommand>, cmd_rx: Receiver<AppCommand>, tray: Option<tray_icon::TrayIcon>) -> Self {
        let db = match db::Db::open() {
            Ok(db) => db,
            Err(e) => {
                tracing::error!("failed to open SQLite store: {e}");
                panic!("SQLite init failed: {e}");
            }
        };
        let task_count = db.count_tasks().unwrap_or(0);
        Self {
            cmd_tx,
            cmd_rx,
            status: AppStatus {
                running: false,
                current_task: None,
                task_count,
            },
            db,
            _tray: tray,
        }
    }
}

impl eframe::App for TrayApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ---- Tray menu events (global receiver, cross-platform) ----
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            match event.id.as_ref() {
                "start" => {
                    let _ = self.cmd_tx.send(AppCommand::StartTask("menu".to_string()));
                }
                "stop" => {
                    let _ = self.cmd_tx.send(AppCommand::StopTask);
                }
                "quit" => {
                    tracing::info!("quit requested from tray menu");
                    std::process::exit(0);
                }
                other => tracing::debug!("ignored menu event: {other}"),
            }
        }

        // ---- Tray icon events (e.g. left click) – drained, currently unused ----
        while let Ok(_event) = tray_icon::TrayIconEvent::receiver().try_recv() {}

        // ---- Commands from HTTP / TCP handlers ----
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                AppCommand::StartTask(name) => {
                    self.status.running = true;
                    self.status.current_task = Some(name.clone());
                    if let Err(e) = self.db.insert_task(&name) {
                        tracing::warn!("failed to persist task '{name}': {e}");
                    }
                    self.status.task_count = self.db.count_tasks().unwrap_or(self.status.task_count);
                    tracing::info!("task started: {name}");
                }
                AppCommand::StopTask => {
                    self.status.running = false;
                    self.status.current_task = None;
                    tracing::info!("task stopped");
                }
                AppCommand::GetStatus { respond_to } => {
                    let _ = respond_to.send(self.status.clone());
                }
                AppCommand::ListTasks { respond_to } => {
                    let rows = self.db.list_tasks().unwrap_or_default();
                    let _ = respond_to.send(rows);
                }
            }
        }

        // There is no window to repaint, but we must keep the event loop ticking
        // so the receivers above are drained regularly.
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// Build the tray icon + right-click menu. Returns the owned `TrayIcon`.
///
/// On Linux this must be called from the GTK thread (after `gtk::init()`);
/// on Windows/macOS it is called from the eframe closure on the main thread.
fn build_tray_icon(cmd_tx: Sender<AppCommand>) -> tray_icon::TrayIcon {
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

/// Load `examples/icon.png` (RGBA) and convert it into a `tray_icon::Icon`.
/// Falls back to a generated blue square if the file cannot be found/decoded.
fn load_icon() -> tray_icon::icon::Icon {
    let candidates: Vec<PathBuf> = {
        let mut list = vec![PathBuf::from("examples/icon.png")];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let mut p = dir.to_path_buf();
                p.push("examples/icon.png");
                list.push(p);
                let mut p2 = dir.to_path_buf();
                p2.push("icon.png");
                list.push(p2);
            }
        }
        list
    };

    for path in candidates {
        if let Ok(img) = image::open(&path) {
            let rgba = img.into_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            if let Ok(icon) = tray_icon::icon::Icon::from_rgba(rgba.into_raw(), w, h) {
                tracing::info!("loaded tray icon from {}", path.display());
                return icon;
            }
        }
    }

    tracing::warn!("icon.png not found – using generated fallback icon");
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for _ in 0..(size * size) {
        rgba.extend_from_slice(&[70u8, 130u8, 220u8, 255u8]);
    }
    tray_icon::icon::Icon::from_rgba(rgba, size, size).expect("fallback icon")
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

    // ---- Tray icon ----
    #[cfg(target_os = "linux")]
    {
        // GTK must live on its own thread; winit/egui must NOT initialise GTK.
        let tx = cmd_tx.clone();
        std::thread::spawn(move || {
            gtk::init().expect("failed to initialise GTK");
            let _tray = build_tray_icon(tx);
            gtk::main();
        });
    }

    let native_options = eframe::NativeOptions {
        // No visible main window – the tray is the only UI.
        viewport: egui::ViewportBuilder::default().with_visible(false),
        ..Default::default()
    };

    let app_creator: Box<
        dyn FnOnce(&eframe::CreationContext<'_>) -> Result<Box<dyn eframe::App>, Box<dyn std::error::Error>>,
    > = {
        let cmd_tx = cmd_tx.clone();
        Box::new(move |_cc: &eframe::CreationContext<'_>| {
            // On Windows/macOS build the tray here, on the eframe thread.
            // On Linux the tray already lives on the dedicated GTK thread.
            #[cfg(not(target_os = "linux"))]
            let tray = Some(build_tray_icon(cmd_tx.clone()));
            #[cfg(target_os = "linux")]
            let tray: Option<tray_icon::TrayIcon> = None;

            Ok(Box::new(TrayApp::new(cmd_tx.clone(), cmd_rx, tray))
                as Box<dyn eframe::App>)
        })
    };

    tracing::info!("starting my-tray-app (headless tray daemon)");
    if let Err(e) = eframe::run_native("my-tray-app", native_options, app_creator) {
        tracing::error!("eframe exited with error: {e}");
        std::process::exit(1);
    }
}
