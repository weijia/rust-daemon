//! Axum HTTP control panel bound to `127.0.0.1:3000`.
//!
//! Handlers never mutate shared state directly; they forward an [`AppCommand`]
//! through the `mpsc::Sender` in [`HttpState`] and (for read endpoints) await a
//! `oneshot` reply produced by the main thread.

use crate::commands::{AppCommand, AppStatus, TaskRow};
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use std::net::SocketAddr;
use std::sync::mpsc::Sender;
use tokio::sync::oneshot;

#[derive(Clone)]
pub struct HttpState {
    pub cmd_tx: Sender<AppCommand>,
}

pub fn router(state: HttpState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/task", post(start_task))
        .route("/stop", post(stop_task))
        .route("/status", get(status))
        .route("/tasks", get(list_tasks))
        .with_state(state)
}

/// Bind and serve forever. Intended to be `tokio::spawn`-ed.
pub async fn run_server(state: HttpState, addr: SocketAddr) {
    tracing::info!("HTTP control panel listening on http://{addr}");
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to bind HTTP server on {addr}: {e}");
            return;
        }
    };
    if let Err(e) = axum::serve(listener, router(state)).await {
        tracing::error!("HTTP server stopped: {e}");
    }
}

async fn health() -> &'static str {
    "ok"
}

#[derive(serde::Deserialize)]
struct TaskReq {
    name: String,
}

async fn start_task(State(state): State<HttpState>, Json(req): Json<TaskReq>) -> &'static str {
    let _ = state.cmd_tx.send(AppCommand::StartTask(req.name));
    "accepted"
}

async fn stop_task(State(state): State<HttpState>) -> &'static str {
    let _ = state.cmd_tx.send(AppCommand::StopTask);
    "accepted"
}

async fn status(State(state): State<HttpState>) -> Json<AppStatus> {
    let (tx, rx) = oneshot::channel();
    if state.cmd_tx.send(AppCommand::GetStatus { respond_to: tx }).is_ok() {
        match rx.await {
            Ok(s) => return Json(s),
            Err(_) => tracing::warn!("status request dropped (main thread gone)"),
        }
    }
    Json(AppStatus {
        running: false,
        current_task: None,
        task_count: 0,
    })
}

async fn list_tasks(State(state): State<HttpState>) -> Json<Vec<TaskRow>> {
    let (tx, rx) = oneshot::channel();
    if state.cmd_tx.send(AppCommand::ListTasks { respond_to: tx }).is_ok() {
        if let Ok(rows) = rx.await {
            return Json(rows);
        }
    }
    Json(Vec::new())
}
