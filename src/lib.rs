pub mod api;
pub mod cli;
pub mod command;
pub mod locale;
pub mod nbio;
pub mod pty;
pub mod session;

pub use cli::Size;

#[cfg(test)]
mod tests;
pub use command::{Command, InputSeq};
pub use session::{Client, Event, Session, Subscription};

use anyhow::Result;
use std::collections::HashMap;
use std::str::FromStr;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// High-level interface for managing HT sessions
pub struct HtLibrary {
    sessions: HashMap<Uuid, HtSession>,
}

/// Internal command type for library interface
#[derive(Debug)]
pub enum LibraryCommand {
    Input(Vec<InputSeq>),
    Snapshot(oneshot::Sender<String>),
    Resize(usize, usize),
}

/// Represents a single HT session
pub struct HtSession {
    pub id: Uuid,
    pub command_tx: mpsc::Sender<LibraryCommand>,
    pub pty_handle: tokio::task::JoinHandle<Result<()>>,
    pub session_handle: tokio::task::JoinHandle<()>,
}

/// Configuration for creating a new HT session
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub command: Vec<String>,
    pub size: (usize, usize), // (cols, rows)
    pub enable_web_server: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            command: vec!["bash".to_string()],
            size: (120, 40),
            enable_web_server: false,
        }
    }
}

impl HtLibrary {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Create a new HT session
    pub async fn create_session(&mut self, config: SessionConfig) -> Result<Uuid> {
        let session_id = Uuid::new_v4();

        // Create channels for communication
        let (input_tx, input_rx) = mpsc::channel(1024);
        let (output_tx, mut output_rx) = mpsc::channel(1024);
        let (command_tx, mut command_rx) = mpsc::channel(1024);

        // Start PTY
        let command_str = config.command.join(" ");
        let cols = config.size.0;
        let rows = config.size.1;

        let pty_handle = tokio::spawn(async move {
            let size = cli::Size::from_str(&format!("{}x{}", cols, rows)).unwrap();
            pty::spawn(command_str, *size, input_rx, output_tx)
                .unwrap()
                .await
        });

        // Start session event loop
        let input_tx_clone = input_tx.clone();

        let session_handle = tokio::spawn(async move {
            let mut session = Session::new(config.size.0, config.size.1);

            loop {
                tokio::select! {
                    // Handle output from PTY
                    output = output_rx.recv() => {
                        match output {
                            Some(data) => {
                                session.output(String::from_utf8_lossy(&data).to_string());
                            }
                            None => break,
                        }
                    }

                    // Handle library commands
                    lib_command = command_rx.recv() => {
                        match lib_command {
                            Some(LibraryCommand::Input(seqs)) => {
                                let data = command::seqs_to_bytes(&seqs, session.cursor_key_app_mode());
                                let _ = input_tx_clone.send(data).await;
                            }
                            Some(LibraryCommand::Snapshot(response_tx)) => {
                                // Get the current terminal text content
                                let text = session.get_text();
                                let _ = response_tx.send(text);
                            }
                            Some(LibraryCommand::Resize(cols, rows)) => {
                                session.resize(cols, rows);
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        // Store session
        let ht_session = HtSession {
            id: session_id,
            command_tx,
            pty_handle,
            session_handle,
        };

        self.sessions.insert(session_id, ht_session);
        Ok(session_id)
    }

    /// Send input to a session
    pub async fn send_input(&self, session_id: Uuid, input: Vec<InputSeq>) -> Result<()> {
        if let Some(session) = self.sessions.get(&session_id) {
            session
                .command_tx
                .send(LibraryCommand::Input(input))
                .await?;
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    /// Take a snapshot of a session
    pub async fn take_snapshot(&self, session_id: Uuid) -> Result<String> {
        if let Some(session) = self.sessions.get(&session_id) {
            let (response_tx, response_rx) = oneshot::channel();
            session
                .command_tx
                .send(LibraryCommand::Snapshot(response_tx))
                .await?;

            // Wait for the snapshot result
            let snapshot = response_rx
                .await
                .map_err(|_| anyhow::anyhow!("Failed to receive snapshot response"))?;

            Ok(snapshot)
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    /// Resize a session
    pub async fn resize_session(&self, session_id: Uuid, cols: usize, rows: usize) -> Result<()> {
        if let Some(session) = self.sessions.get(&session_id) {
            session
                .command_tx
                .send(LibraryCommand::Resize(cols, rows))
                .await?;
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    /// Close a session
    pub async fn close_session(&mut self, session_id: Uuid) -> Result<()> {
        if let Some(session) = self.sessions.remove(&session_id) {
            // Close the command channel
            drop(session.command_tx);

            // Abort the handles
            session.pty_handle.abort();
            session.session_handle.abort();

            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", session_id)
        }
    }

    /// List all active sessions
    pub fn list_sessions(&self) -> Vec<Uuid> {
        self.sessions.keys().copied().collect()
    }

    /// Get session information
    pub fn get_session(&self, session_id: Uuid) -> Option<&HtSession> {
        self.sessions.get(&session_id)
    }
}

impl Default for HtLibrary {
    fn default() -> Self {
        Self::new()
    }
}

// Re-export commonly used types
pub use anyhow::Error;
