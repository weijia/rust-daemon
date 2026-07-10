//! A tiny line-based TCP control protocol on `127.0.0.1:4000`.
//!
//! Commands (one per line):
//! * `START <name>` – start a named task
//! * `STOP`         – stop the current task
//! * `STATUS`       – ask the main thread and echo a status line
//! * `QUIT`         – close this connection
//!
//! Like the HTTP layer, this only forwards [`AppCommand`]s through the shared
//! `mpsc::Sender`.

use crate::commands::AppCommand;
use std::net::SocketAddr;
use std::sync::mpsc::Sender;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn run_listener(cmd_tx: Sender<AppCommand>, addr: SocketAddr) {
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to bind TCP listener on {addr}: {e}");
            return;
        }
    };
    tracing::info!("TCP control listener on {addr}");

    loop {
        let (socket, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let tx = cmd_tx.clone();
        tokio::spawn(async move {
            tracing::debug!("tcp client connected: {peer}");
            handle(socket, tx).await;
        });
    }
}

async fn handle(mut socket: tokio::net::TcpStream, cmd_tx: Sender<AppCommand>) {
    let (read_half, mut write_half) = socket.split();
    let mut lines = BufReader::new(read_half).lines();

    let _ = write_half
        .write_all(b"my-tray-app tcp control. commands: START <name> | STOP | STATUS | QUIT\n")
        .await;

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let response = if let Some(rest) = line.strip_prefix("START ") {
            let name = rest.trim().to_string();
            let _ = cmd_tx.send(AppCommand::StartTask(name));
            "started".to_string()
        } else if line == "STOP" {
            let _ = cmd_tx.send(AppCommand::StopTask);
            "stopped".to_string()
        } else if line == "STATUS" {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if cmd_tx.send(AppCommand::GetStatus { respond_to: tx }).is_ok() {
                match rx.await {
                    Ok(s) => format!(
                        "running={} current={:?} tasks={}",
                        s.running, s.current_task, s.task_count
                    ),
                    Err(_) => "unavailable".to_string(),
                }
            } else {
                "unavailable".to_string()
            }
        } else if line == "QUIT" {
            let _ = write_half.write_all(b"bye\n").await;
            break;
        } else {
            "unknown command".to_string()
        };

        if write_half
            .write_all(format!("{response}\n").as_bytes())
            .await
            .is_err()
        {
            break;
        }
    }
}
