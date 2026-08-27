// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! Async bridge to the `pop-launcher` IPC service.
//!
//! `pop-launcher` is a long-lived child process speaking newline-delimited JSON
//! on stdin/stdout. Keeping one instance alive for the lifetime of the daemon
//! matters: measured on a warm cache a query round-trips in 0.5–1.5 ms, but the
//! first query after spawn pays for plugin discovery and desktop-entry indexing.
//! Respawning per invocation would put that cost on the keypress that opens the
//! launcher, which is the one frame the user actually judges.
//!
//! ## Staleness
//!
//! The protocol carries no request id, so a response cannot be matched to the
//! query that caused it. The service does cancel an in-flight search when a new
//! `Search` arrives, so responses are effectively last-write-wins. Rather than
//! guess at correlation, every emitted [`Event::Update`] is stamped with the
//! query text that was most recently sent. The frontend compares that against
//! the current input and, when they differ, keeps rendering the previous results
//! instead of flashing an empty list — the behaviour Spotlight has.

use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use pop_launcher::{GpuPreference, Indice, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc};

use crate::model::Item;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to spawn the pop-launcher service")]
    Spawn(#[source] std::io::Error),
    #[error("pop-launcher stdin/stdout was not captured")]
    MissingPipe,
}

/// Something the launcher service told us.
#[derive(Debug, Clone)]
pub enum Event {
    /// A new result set. `query` is the search text that was most recently
    /// sent, not necessarily the one the user has typed by now.
    Update {
        seq: u64,
        query: String,
        items: Vec<Item>,
    },
    /// The service wants the input box replaced (tab completion).
    Fill(String),
    /// A desktop entry the frontend is responsible for launching.
    DesktopEntry {
        path: std::path::PathBuf,
        gpu_preference: GpuPreference,
        action_name: Option<String>,
    },
    /// Context menu options for a result.
    Context {
        id: Indice,
        options: Vec<pop_launcher::ContextOption>,
    },
    /// The service considers the interaction finished; dismiss the UI.
    Close,
    /// The service died. The frontend should degrade rather than hang.
    Disconnected,
}

/// Write half of the connection. Cheap to clone and share across tasks.
#[derive(Clone, Debug)]
pub struct Launcher {
    tx: mpsc::UnboundedSender<Request>,
    /// Mirrors the most recently sent search text so the reader task can stamp
    /// updates without inventing a correlation id.
    last_query: Arc<Mutex<String>>,
    seq: Arc<AtomicU64>,
}

impl Launcher {
    /// Spawn the service and start pumping both directions.
    ///
    /// Returns the write handle and the stream of events. The child is killed
    /// when the returned [`LauncherGuard`] drops.
    pub fn spawn() -> Result<(Self, mpsc::UnboundedReceiver<Event>, LauncherGuard), Error> {
        let mut child = Command::new("pop-launcher")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The service logs to stderr; forwarding it to ours keeps plugin
            // errors visible under RUST_LOG without corrupting the JSON stream.
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(Error::Spawn)?;

        let mut stdin = child.stdin.take().ok_or(Error::MissingPipe)?;
        let stdout = child.stdout.take().ok_or(Error::MissingPipe)?;

        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<Request>();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();

        let last_query = Arc::new(Mutex::new(String::new()));
        let seq = Arc::new(AtomicU64::new(0));

        // Writer: serialise requests onto the child's stdin.
        tokio::spawn(async move {
            while let Some(request) = request_rx.recv().await {
                let Ok(mut line) = serde_json::to_vec(&request) else {
                    tracing::error!(?request, "failed to serialise launcher request");
                    continue;
                };
                line.push(b'\n');
                if let Err(error) = stdin.write_all(&line).await {
                    tracing::warn!(%error, "pop-launcher stdin closed");
                    break;
                }
                if let Err(error) = stdin.flush().await {
                    tracing::warn!(%error, "failed to flush pop-launcher stdin");
                    break;
                }
            }
        });

        // Reader: decode responses and translate them into frontend events.
        let reader_query = Arc::clone(&last_query);
        let reader_seq = Arc::clone(&seq);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let response: Response = match serde_json::from_str(&line) {
                            Ok(response) => response,
                            Err(error) => {
                                tracing::warn!(%error, %line, "undecodable launcher response");
                                continue;
                            }
                        };

                        let event = match response {
                            Response::Update(results) => {
                                let query = reader_query.lock().await.clone();
                                Event::Update {
                                    seq: reader_seq.load(Ordering::Relaxed),
                                    query,
                                    items: results
                                        .into_iter()
                                        .map(Item::from_search_result)
                                        .collect(),
                                }
                            }
                            Response::Fill(text) => Event::Fill(text),
                            Response::DesktopEntry {
                                path,
                                gpu_preference,
                                action_name,
                            } => Event::DesktopEntry {
                                path,
                                gpu_preference,
                                action_name,
                            },
                            Response::Context { id, options } => Event::Context { id, options },
                            Response::Close => Event::Close,
                        };

                        if event_tx.send(event).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = event_tx.send(Event::Disconnected);
                        break;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "pop-launcher stdout read failed");
                        let _ = event_tx.send(Event::Disconnected);
                        break;
                    }
                }
            }
        });

        let launcher = Self {
            tx: request_tx,
            last_query,
            seq,
        };

        Ok((launcher, event_rx, LauncherGuard { child }))
    }

    /// Run a search. Returns the generation assigned to it.
    ///
    /// Sent unconditionally on every keystroke rather than debounced: at ~1 ms
    /// round-trip a debounce would only add latency without saving work, and
    /// the service already cancels the previous search when a new one arrives.
    pub fn search(&self, query: impl Into<String>) -> u64 {
        let query = query.into();
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;

        let last_query = Arc::clone(&self.last_query);
        let text = query.clone();
        tokio::spawn(async move {
            *last_query.lock().await = text;
        });

        self.send(Request::Search(query));
        seq
    }

    pub fn activate(&self, id: Indice) {
        self.send(Request::Activate(id));
    }

    pub fn complete(&self, id: Indice) {
        self.send(Request::Complete(id));
    }

    pub fn context(&self, id: Indice) {
        self.send(Request::Context(id));
    }

    pub fn activate_context(&self, id: Indice, context: Indice) {
        self.send(Request::ActivateContext { id, context });
    }

    /// Ask the service to close a window result (used by the switcher).
    pub fn quit(&self, id: Indice) {
        self.send(Request::Quit(id));
    }

    /// Tell the service the frontend was dismissed so it can release resources.
    /// Deliberately *not* `Exit` — we keep the process warm for the next open.
    pub fn dismissed(&self) {
        self.send(Request::Close);
    }

    pub fn interrupt(&self) {
        self.send(Request::Interrupt);
    }

    fn send(&self, request: Request) {
        if self.tx.send(request).is_err() {
            tracing::warn!("pop-launcher request channel closed");
        }
    }
}

/// Owns the child process. Dropping it terminates the service.
pub struct LauncherGuard {
    child: Child,
}

impl LauncherGuard {
    /// Ask the service to exit and reap it.
    pub async fn shutdown(mut self) {
        let _ = self.child.kill().await;
    }
}
