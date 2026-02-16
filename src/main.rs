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
    let pty = start_pty(cli.command, &cli.size, input_rx, output_tx, resize_rx)?;
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

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq)]
enum CommandKind {
    Direct,       // executable — launch directly
    ShellSyntax,  // metacharacters — inject raw into cmd.exe
    ShellBuiltin, // cmd.exe internal command — escape args, inject into cmd.exe
}

/// Returns `true` if `s` contains a `%NAME%` environment-variable token
/// (one or more alphanumeric, underscore, or parenthesis characters between
/// two `%` signs — e.g. `%USERPROFILE%`, `%ProgramFiles(x86)%`).
/// Single `%` (format strings like `%s`), `%%` (escaped percent), and
/// URL encodings like `%20` (digits only, no closing `%`) are not matched.
#[cfg(any(windows, test))]
fn contains_env_var(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let start = i + 1;
            if start < bytes.len()
                && (bytes[start].is_ascii_alphanumeric()
                    || matches!(bytes[start], b'_' | b'(' | b')'))
            {
                let mut j = start + 1;
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric() || matches!(bytes[j], b'_' | b'(' | b')'))
                {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'%' {
                    return true;
                }
                i = j;
            } else {
                i = start;
            }
        } else {
            i += 1;
        }
    }
    false
}

#[cfg(any(windows, test))]
fn classify_command(args: &[String]) -> CommandKind {
    let first = match args.first() {
        Some(s) => s.to_ascii_lowercase(),
        None => return CommandKind::Direct, // empty → default shell
    };

    // Single-string command line: e.g. ht "echo hello" arrives as
    // args = ["echo hello"]. When the first whitespace-delimited token
    // has no backslash it is a command name, not a file path, so the
    // user intended the whole string as a shell command line.  Strings
    // whose first token contains '\' are paths with spaces
    // (e.g. "C:\Program Files\foo.exe") and stay in the normal flow.
    if args.len() == 1 {
        if let Some(cmd_token) = first.split(' ').next() {
            if cmd_token != first && !cmd_token.contains('\\') && !cmd_token.contains('/') {
                return CommandKind::ShellSyntax;
            }
        }
    }

    // Pipe/redirect/chaining metacharacters in the first argument indicate the
    // user passed a shell command string (e.g. ht "dir | findstr foo").
    // These in subsequent arguments are literal program arguments
    // (e.g. ht -- python -c "print('<tag>')") and must not trigger shell mode.
    if first.contains(['|', '>', '<', '&', '^']) {
        return CommandKind::ShellSyntax;
    }

    // cmd.exe internal commands (case-insensitive) — checked before the %
    // scan so builtins keep their argument escaping via ShellBuiltin.
    const BUILTINS: &[&str] = &[
        "assoc", "break", "call", "cd", "chdir", "cls", "color", "copy", "date", "del", "dir",
        "echo", "endlocal", "erase", "exit", "for", "ftype", "goto", "if", "md", "mkdir", "mklink",
        "move", "path", "pause", "popd", "prompt", "pushd", "rd", "rem", "ren", "rename", "rmdir",
        "set", "setlocal", "shift", "start", "time", "title", "type", "ver", "verify", "vol",
    ];
    if BUILTINS.contains(&first.as_str()) {
        return CommandKind::ShellBuiltin;
    }

    // %VAR% environment-variable tokens require cmd.exe for expansion and can
    // appear in any argument position (e.g. ht notepad %USERPROFILE%\foo.txt),
    // so scan the entire argument list. Uses ShellBuiltin (not ShellSyntax) to
    // preserve argument escaping — cmd.exe expands %VAR% inside double quotes.
    if args.iter().any(|a| contains_env_var(a)) {
        return CommandKind::ShellBuiltin;
    }

    CommandKind::Direct
}

fn start_pty(
    command: Vec<String>,
    size: &cli::Size,
    input_rx: mpsc::Receiver<Vec<u8>>,
    output_tx: mpsc::Sender<Vec<u8>>,
    resize_rx: mpsc::Receiver<(u16, u16)>,
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
        match classify_command(&command) {
            CommandKind::Direct => {
                let cmd = command
                    .iter()
                    .map(pty::escape_arg)
                    .collect::<Vec<_>>()
                    .join(" ");
                eprintln!("launching \"{}\" in terminal of size {}", cmd, size);
                (cmd, None)
            }
            CommandKind::ShellSyntax => {
                let user_cmd = command.join(" ");
                eprintln!(
                    "launching cmd.exe for shell command \"{}\" \
                     in terminal of size {}",
                    user_cmd, size
                );
                let inject = format!("{}\r\nexit\r\n", user_cmd);
                ("cmd.exe".to_string(), Some(inject.into_bytes()))
            }
            CommandKind::ShellBuiltin => {
                let user_cmd = command
                    .iter()
                    .map(pty::escape_arg)
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

    Ok(tokio::spawn(pty::spawn(
        command_str,
        winsize,
        input_rx,
        output_tx,
        resize_rx,
        initial_input,
    )?))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    // ── classify_command ────────────────────────────────────────────

    #[test]
    fn classify_empty_args_is_direct() {
        assert_eq!(classify_command(&args(&[])), CommandKind::Direct);
    }

    #[test]
    fn classify_single_executable_is_direct() {
        assert_eq!(classify_command(&args(&["notepad"])), CommandKind::Direct);
    }

    #[test]
    fn classify_multi_arg_executable_is_direct() {
        assert_eq!(
            classify_command(&args(&["python", "-c", "print()"])),
            CommandKind::Direct
        );
    }

    #[test]
    fn classify_single_string_command_is_shell_syntax() {
        assert_eq!(
            classify_command(&args(&["echo hello"])),
            CommandKind::ShellSyntax
        );
    }

    #[test]
    fn classify_single_string_with_switches_is_shell_syntax() {
        assert_eq!(
            classify_command(&args(&["dir /w"])),
            CommandKind::ShellSyntax
        );
    }

    #[test]
    fn classify_single_string_path_with_spaces_is_direct() {
        assert_eq!(
            classify_command(&args(&["C:\\Program Files\\foo.exe"])),
            CommandKind::Direct
        );
    }

    #[test]
    fn classify_single_string_relative_path_with_spaces_is_direct() {
        assert_eq!(
            classify_command(&args(&[".\\my app\\foo.exe"])),
            CommandKind::Direct
        );
    }

    #[test]
    fn classify_single_string_forward_slash_path_is_direct() {
        assert_eq!(
            classify_command(&args(&["C:/Program Files/foo.exe"])),
            CommandKind::Direct
        );
    }

    #[test]
    fn classify_metachar_pipe_is_shell_syntax() {
        assert_eq!(
            classify_command(&args(&["dir | findstr foo"])),
            CommandKind::ShellSyntax
        );
    }

    #[test]
    fn classify_metachar_in_first_multi_arg_is_shell_syntax() {
        assert_eq!(classify_command(&args(&["a|b"])), CommandKind::ShellSyntax);
    }

    #[test]
    fn classify_metachar_in_later_arg_only_is_direct() {
        assert_eq!(
            classify_command(&args(&["python", "-c", "a>b"])),
            CommandKind::Direct
        );
    }

    #[test]
    fn classify_builtin_is_shell_builtin() {
        assert_eq!(
            classify_command(&args(&["echo"])),
            CommandKind::ShellBuiltin
        );
    }

    #[test]
    fn classify_builtin_case_insensitive() {
        assert_eq!(
            classify_command(&args(&["ECHO"])),
            CommandKind::ShellBuiltin
        );
    }

    #[test]
    fn classify_builtin_with_args_is_shell_builtin() {
        assert_eq!(
            classify_command(&args(&["echo", "hello"])),
            CommandKind::ShellBuiltin
        );
    }

    #[test]
    fn classify_env_var_in_first_arg_is_shell_builtin() {
        assert_eq!(
            classify_command(&args(&["%USERPROFILE%\\foo.txt"])),
            CommandKind::ShellBuiltin
        );
    }

    #[test]
    fn classify_env_var_in_later_arg_is_shell_builtin() {
        assert_eq!(
            classify_command(&args(&["notepad", "%USERPROFILE%\\foo.txt"])),
            CommandKind::ShellBuiltin
        );
    }

    #[test]
    fn classify_literal_percent_not_env_var_is_direct() {
        assert_eq!(
            classify_command(&args(&["python", "-c", "print('%s')"])),
            CommandKind::Direct
        );
    }

    // ── contains_env_var ────────────────────────────────────────────

    #[test]
    fn env_var_standard() {
        assert!(contains_env_var("%USERPROFILE%"));
    }

    #[test]
    fn env_var_with_parentheses() {
        assert!(contains_env_var("%ProgramFiles(x86)%"));
    }

    #[test]
    fn env_var_short_name() {
        assert!(contains_env_var("%PATH%"));
    }

    #[test]
    fn env_var_embedded_in_path() {
        assert!(contains_env_var("C:\\%USERPROFILE%\\docs"));
    }

    #[test]
    fn no_env_var_format_string() {
        assert!(!contains_env_var("hello %s"));
    }

    #[test]
    fn no_env_var_escaped_percent() {
        assert!(!contains_env_var("100%%"));
    }

    #[test]
    fn no_env_var_url_encoding() {
        assert!(!contains_env_var("hello%20world"));
    }

    #[test]
    fn env_var_after_escaped_percent() {
        assert!(contains_env_var("%%PATH%%"));
    }

    #[test]
    fn no_env_var_empty_string() {
        assert!(!contains_env_var(""));
    }
}
