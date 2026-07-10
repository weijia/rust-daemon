//! Inter-thread / inter-task commands.
//!
//! The HTTP handlers (tokio tasks) and the TCP listener (tokio task) never touch
//! GUI or DB state directly. They only push an [`AppCommand`] into a
//! `std::sync::mpsc::Sender` whose receiver is drained by the background worker
//! thread (see `run_worker` in `main.rs`), which mutates the canonical state.
//! Tray-menu events are delivered separately through `tray_icon::menu::MenuEvent`
//! and handled by the same worker thread.

use serde::Serialize;
use tokio::sync::oneshot;

/// Commands understood by the main thread.
#[derive(Debug)]
pub enum AppCommand {
    /// Start a (named) task. Persisted to SQLite by the main thread.
    StartTask(String),
    /// Stop the currently running task.
    StopTask,
    /// Request a snapshot of the current status. The responder carries it back
    /// to whoever asked (the HTTP handler / TCP handler).
    GetStatus {
        respond_to: oneshot::Sender<AppStatus>,
    },
    /// Request the list of persisted tasks.
    ListTasks {
        respond_to: oneshot::Sender<Vec<TaskRow>>,
    },
}

/// Snapshot returned by `GET /status` and `STATUS` over TCP.
#[derive(Debug, Clone, Serialize)]
pub struct AppStatus {
    pub running: bool,
    pub current_task: Option<String>,
    pub task_count: usize,
}

/// A row from the `tasks` SQLite table.
#[derive(Debug, Clone, Serialize)]
pub struct TaskRow {
    pub id: i64,
    pub name: String,
    pub created_at: String,
}
