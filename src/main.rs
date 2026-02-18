mod api;
mod cli;
mod command;
mod locale;
mod nbio;
mod pty;
mod session;
use anyhow::{Context, Result};
use command::Command;
use session::Session;
use std::net::{SocketAddr, TcpListener};
use tokio::{sync::mpsc, task::JoinHandle};

#[tokio::main]
async fn main() -> Result<()> {
    locale::check_utf8_locale()?;
    let cli = cli::Cli::new();

    let (input_tx, input_rx) = mpsc::channel(1024);
    let (output_tx, output_rx) = mpsc::channel(1024);
    let (command_tx, command_rx) = mpsc::channel(1024);
    let (clients_tx, clients_rx) = mpsc::channel(1);
    let (resize_tx, resize_rx) = mpsc::channel::<(u16, u16)>(16);

    start_http_api(cli.listen, clients_tx.clone()).await?;
    let api = start_stdio_api(command_tx, clients_tx, cli.subscribe.unwrap_or_default());
    let pty = start_pty(
        cli.command,
        &cli.size,
        input_rx,
        output_tx,
        resize_rx,
        #[cfg(windows)]
        cli.backend,
    )?;
    let session = build_session(&cli.size);
    run_event_loop(
        output_rx, input_tx, command_rx, clients_rx, session, api, resize_tx,
    )
    .await?;
    pty.await?
}

fn build_session(size: &cli::Size) -> Session {
    Session::new(size.cols(), size.rows())
}

fn start_stdio_api(
    command_tx: mpsc::Sender<Command>,
    clients_tx: mpsc::Sender<session::Client>,
    sub: api::Subscription,
) -> JoinHandle<Result<()>> {
    tokio::spawn(api::stdio::start(command_tx, clients_tx, sub))
}

fn start_pty(
    command: Vec<String>,
    size: &cli::Size,
    input_rx: mpsc::Receiver<Vec<u8>>,
    output_tx: mpsc::Sender<Vec<u8>>,
    resize_rx: mpsc::Receiver<(u16, u16)>,
    #[cfg(windows)] backend: cli::Backend,
) -> Result<JoinHandle<Result<()>>> {
    let winsize = **size;

    #[cfg(unix)]
    let (command_str, initial_input) = {
        let cmd = command.join(" ");
        eprintln!("launching \"{}\" in terminal of size {}", cmd, size);
        (cmd, None)
    };

    #[cfg(windows)]
    let (command_str, initial_input) = {
        match pty::classify_command(&command) {
            pty::CommandKind::Direct => {
                let cmd = command
                    .iter()
                    .map(|s| pty::escape_arg(s))
                    .collect::<Vec<_>>()
                    .join(" ");
                eprintln!("launching \"{}\" in terminal of size {}", cmd, size);
                (cmd, None)
            }
            pty::CommandKind::ShellSyntax => {
                let user_cmd = command.join(" ");
                eprintln!(
                    "launching cmd.exe for shell command \"{}\" \
                     in terminal of size {}",
                    user_cmd, size
                );
                let inject = format!("{}\r\nexit\r\n", user_cmd);
                ("cmd.exe".to_string(), Some(inject.into_bytes()))
            }
            pty::CommandKind::ShellBuiltin => {
                let user_cmd = command
                    .iter()
                    .map(|s| pty::escape_arg(s))
                    .collect::<Vec<_>>()
                    .join(" ");
                eprintln!(
                    "launching cmd.exe for builtin \"{}\" \
                     in terminal of size {}",
                    user_cmd, size
                );
                let inject = format!("{}\r\nexit\r\n", user_cmd);
                ("cmd.exe".to_string(), Some(inject.into_bytes()))
            }
        }
    };

    #[cfg(unix)]
    {
        Ok(tokio::spawn(pty::spawn(
            command_str,
            winsize,
            input_rx,
            output_tx,
            resize_rx,
            initial_input,
        )?))
    }

    #[cfg(windows)]
    {
        if matches!(backend, cli::Backend::Scrape) {
            eprintln!("warning: using scrape backend — this is a CI fallback with limited input fidelity.");
            eprintln!("         Piped stdin data will be rewritten as keystrokes (may corrupt binary/raw input).");
            eprintln!("         Use the default ConPTY backend for interactive use or piped data workloads.");
        }
        Ok(tokio::spawn(pty::spawn_with_backend(
            command_str,
            winsize,
            input_rx,
            output_tx,
            resize_rx,
            initial_input,
            backend,
        )?))
    }
}

async fn start_http_api(
    listen_addr: Option<SocketAddr>,
    clients_tx: mpsc::Sender<session::Client>,
) -> Result<()> {
    if let Some(addr) = listen_addr {
        let listener = TcpListener::bind(addr).context("cannot start HTTP listener")?;
        tokio::spawn(api::http::start(listener, clients_tx).await?);
    }

    Ok(())
}

async fn run_event_loop(
    mut output_rx: mpsc::Receiver<Vec<u8>>,
    input_tx: mpsc::Sender<Vec<u8>>,
    mut command_rx: mpsc::Receiver<Command>,
    mut clients_rx: mpsc::Receiver<session::Client>,
    mut session: Session,
    mut api_handle: JoinHandle<Result<()>>,
    resize_tx: mpsc::Sender<(u16, u16)>,
) -> Result<()> {
    let mut serving = true;

    loop {
        tokio::select! {
            result = output_rx.recv() => {
                match result {
                    Some(data) => {
                        session.output(String::from_utf8_lossy(&data).to_string());
                    },

                    None => {
                        eprintln!("process exited, shutting down...");
                        break;
                    }
                }
            }

            command = command_rx.recv() => {
                match command {
                    Some(Command::Input(seqs)) => {
                        let data = command::seqs_to_bytes(&seqs, session.cursor_key_app_mode());
                        input_tx.send(data).await?;
                    }

                    Some(Command::Snapshot) => {
                        session.snapshot();
                    }

                    Some(Command::Resize(cols, rows)) => {
                        session.resize(cols, rows);
                        let cols_u16 = u16::try_from(cols).unwrap_or(u16::MAX);
                        let rows_u16 = u16::try_from(rows).unwrap_or(u16::MAX);
                        let _ = resize_tx.send((cols_u16, rows_u16)).await;
                    }

                    None => {
                        eprintln!("stdin closed, shutting down...");
                        break;
                    }
                }
            }

            client = clients_rx.recv(), if serving => {
                match client {
                    Some(client) => {
                        client.accept(session.subscribe());
                    }

                    None => {
                        serving = false;
                    }
                }
            }

            _ = &mut api_handle => {
                eprintln!("stdin closed, shutting down...");
                break;
            }
        }
    }

    Ok(())
}
