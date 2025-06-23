use anyhow::Result;
use std::future::Future;
use tokio::sync::mpsc;

// Platform-specific imports and implementations
#[cfg(unix)]
use crate::nbio;
#[cfg(unix)]
use nix::libc;
#[cfg(unix)]
use nix::pty;
#[cfg(unix)]
use nix::sys::signal::{self, SigHandler, Signal};
#[cfg(unix)]
use nix::sys::wait;
#[cfg(unix)]
use nix::unistd::{self, ForkResult, Pid};
#[cfg(unix)]
use std::env;
#[cfg(unix)]
use std::ffi::{CString, NulError};
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use tokio::io::unix::AsyncFd;

#[cfg(windows)]
use std::process::Stdio;
#[cfg(windows)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(windows)]
use tokio::process::Command;

// Common winsize structure that works across platforms
#[cfg(unix)]
pub use nix::pty::Winsize;

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
pub struct Winsize {
    pub ws_row: u16,
    pub ws_col: u16,
}

// Unix implementation
#[cfg(unix)]
pub fn spawn(
    command: String,
    winsize: Winsize,
    input_rx: mpsc::Receiver<Vec<u8>>,
    output_tx: mpsc::Sender<Vec<u8>>,
) -> Result<impl Future<Output = Result<()>>> {
    let result = unsafe { pty::forkpty(Some(&winsize), None) }?;

    match result.fork_result {
        ForkResult::Parent { child } => Ok(drive_child(child, result.master, input_rx, output_tx)),

        ForkResult::Child => {
            exec(command)?;
            unreachable!();
        }
    }
}

#[cfg(unix)]
async fn drive_child(
    child: Pid,
    master: OwnedFd,
    input_rx: mpsc::Receiver<Vec<u8>>,
    output_tx: mpsc::Sender<Vec<u8>>,
) -> Result<()> {
    let result = do_drive_child(master, input_rx, output_tx).await;
    eprintln!("sending HUP signal to the child process");
    unsafe { libc::kill(child.as_raw(), libc::SIGHUP) };
    eprintln!("waiting for the child process to exit");

    tokio::task::spawn_blocking(move || {
        let _ = wait::waitpid(child, None);
    })
    .await
    .unwrap();

    result
}

#[cfg(unix)]
const READ_BUF_SIZE: usize = 128 * 1024;

#[cfg(unix)]
async fn do_drive_child(
    master: OwnedFd,
    mut input_rx: mpsc::Receiver<Vec<u8>>,
    output_tx: mpsc::Sender<Vec<u8>>,
) -> Result<()> {
    let mut buf = [0u8; READ_BUF_SIZE];
    let mut input: Vec<u8> = Vec::with_capacity(READ_BUF_SIZE);
    nbio::set_non_blocking(&master.as_raw_fd())?;
    
    // FIXED: File descriptor double-close bug
    // 
    // Previously, we created both a File and an AsyncFd that owned the same FD:
    //   let mut master_file = unsafe { File::from_raw_fd(master.as_raw_fd()) };
    //   let master_fd = AsyncFd::new(master)?;
    // This caused "IO Safety violation: owned file descriptor already closed" 
    // when both objects tried to close the FD on drop.
    //
    // Solution: Use single ownership model where AsyncFd owns the FD,
    // and File is wrapped in ManuallyDrop to prevent double-close.
    
    // Create AsyncFd, which takes ownership of the OwnedFd
    let master_fd = AsyncFd::new(master)?;
    
    // Get a File handle that shares the same FD but doesn't own it
    // ManuallyDrop prevents this File from closing the FD on drop
    let raw_fd = master_fd.get_ref().as_raw_fd();
    let mut master_file = ManuallyDrop::new(unsafe { File::from_raw_fd(raw_fd) });

    loop {
        tokio::select! {
            result = input_rx.recv() => {
                match result {
                    Some(data) => {
                        input.extend_from_slice(&data);
                    }

                    None => {
                        return Ok(());
                    }
                }
            }

            result = master_fd.readable() => {
                let mut guard = result?;

                loop {
                    match nbio::read(&mut *master_file, &mut buf)? {
                        Some(0) => {
                            return Ok(());
                        }

                        Some(n) => {
                            output_tx.send(buf[0..n].to_vec()).await?;
                        }

                        None => {
                            guard.clear_ready();
                            break;
                        }
                    }
                }
            }

            result = master_fd.writable(), if !input.is_empty() => {
                let mut guard = result?;
                let mut buf: &[u8] = input.as_ref();

                loop {
                    match nbio::write(&mut *master_file, buf)? {
                        Some(0) => {
                            return Ok(());
                        }

                        Some(n) => {
                            buf = &buf[n..];

                            if buf.is_empty() {
                                break;
                            }
                        }

                        None => {
                            guard.clear_ready();
                            break;
                        }
                    }
                }

                let left = buf.len();

                if left == 0 {
                    input.clear();
                } else {
                    input.drain(..input.len() - left);
                }
            }
        }
    }
}

#[cfg(unix)]
fn exec(command: String) -> io::Result<()> {
    let command = ["/bin/sh".to_owned(), "-c".to_owned(), command]
        .iter()
        .map(|s| CString::new(s.as_bytes()))
        .collect::<Result<Vec<CString>, NulError>>()?;

    unsafe { env::set_var("TERM", "xterm-256color") };
    unsafe { signal::signal(Signal::SIGPIPE, SigHandler::SigDfl) }?;
    unistd::execvp(&command[0], &command)?;
    unsafe { libc::_exit(1) }
}

// Windows implementation
#[cfg(windows)]
pub fn spawn(
    command: String,
    _winsize: Winsize,
    input_rx: mpsc::Receiver<Vec<u8>>,
    output_tx: mpsc::Sender<Vec<u8>>,
) -> Result<impl Future<Output = Result<()>>> {
    // Parse command for Windows cmd.exe
    let cmd_args = if command.is_empty() {
        vec!["cmd.exe".to_string()]
    } else {
        vec!["cmd.exe".to_string(), "/c".to_string(), command]
    };

    // Spawn the process using tokio::process
    let mut child = Command::new(&cmd_args[0])
        .args(&cmd_args[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn child process: {}", e))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to get child stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to get child stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to get child stderr"))?;

    Ok(drive_child_windows(
        child, stdin, stdout, stderr, input_rx, output_tx,
    ))
}

#[cfg(windows)]
async fn drive_child_windows(
    mut child: tokio::process::Child,
    mut stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    mut input_rx: mpsc::Receiver<Vec<u8>>,
    output_tx: mpsc::Sender<Vec<u8>>,
) -> Result<()> {
    let mut stdout_reader = BufReader::new(stdout);
    let mut stderr_reader = BufReader::new(stderr);
    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();

    loop {
        tokio::select! {
            // Handle input from the application
            result = input_rx.recv() => {
                match result {
                    Some(data) => {
                        if let Err(e) = stdin.write_all(&data).await {
                            eprintln!("Failed to write to child stdin: {}", e);
                            break;
                        }
                        if let Err(e) = stdin.flush().await {
                            eprintln!("Failed to flush child stdin: {}", e);
                            break;
                        }
                    }
                    None => {
                        // Input channel closed
                        break;
                    }
                }
            }

            // Handle stdout output
            result = stdout_reader.read_until(b'\n', &mut stdout_buf) => {
                match result {
                    Ok(0) => {
                        // EOF on stdout
                        break;
                    }
                    Ok(_) => {
                        if output_tx.send(stdout_buf.clone()).await.is_err() {
                            // Output channel closed
                            break;
                        }
                        stdout_buf.clear();
                    }
                    Err(e) => {
                        eprintln!("Failed to read from child stdout: {}", e);
                        break;
                    }
                }
            }

            // Handle stderr output
            result = stderr_reader.read_until(b'\n', &mut stderr_buf) => {
                match result {
                    Ok(0) => {
                        // EOF on stderr - continue since stdout might still be active
                    }
                    Ok(_) => {
                        if output_tx.send(stderr_buf.clone()).await.is_err() {
                            // Output channel closed
                            break;
                        }
                        stderr_buf.clear();
                    }
                    Err(e) => {
                        eprintln!("Failed to read from child stderr: {}", e);
                        // Continue even if stderr fails
                    }
                }
            }

            // Handle child process exit
            result = child.wait() => {
                match result {
                    Ok(status) => {
                        eprintln!("Child process exited with status: {}", status);
                        break;
                    }
                    Err(e) => {
                        eprintln!("Failed to wait for child process: {}", e);
                        break;
                    }
                }
            }
        }
    }

    // Ensure child process is terminated
    if let Err(e) = child.kill().await {
        eprintln!("Failed to kill child process: {}", e);
    }

    Ok(())
}
