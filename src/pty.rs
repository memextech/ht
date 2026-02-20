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
use std::ffi::c_void;
#[cfg(windows)]
use std::mem::{size_of, zeroed};
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
#[cfg(windows)]
use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
#[cfg(windows)]
use windows::Win32::System::Console::COORD;
#[cfg(windows)]
use windows::Win32::System::Threading::{
    CREATE_NEW_CONSOLE, CREATE_NEW_PROCESS_GROUP, CreateProcessW, PROCESS_INFORMATION,
    STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
#[cfg(windows)]
use windows::core::PWSTR;

// Scrape backend imports
#[cfg(windows)]
use windows::Win32::Foundation::GENERIC_READ;
#[cfg(windows)]
use windows::Win32::Foundation::GENERIC_WRITE;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
#[cfg(windows)]
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, CHAR_INFO, CONSOLE_MODE, CONSOLE_SCREEN_BUFFER_INFO,
    CTRL_C_EVENT, ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT,
    ENABLE_VIRTUAL_TERMINAL_PROCESSING, FreeConsole, GenerateConsoleCtrlEvent, GetConsoleMode,
    GetConsoleScreenBufferInfo, GetStdHandle, INPUT_RECORD, KEY_EVENT_RECORD, ReadConsoleOutputW,
    SMALL_RECT, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetConsoleMode,
    SetConsoleScreenBufferSize, SetConsoleWindowInfo, WriteConsoleInputW,
};
#[cfg(windows)]
use windows::Win32::System::Threading::STARTF_USESHOWWINDOW;
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW, VkKeyScanW};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
#[cfg(windows)]
use windows::core::PCWSTR;

/// A `Send`-safe wrapper for Windows `HANDLE`.
///
/// In the `windows` 0.58.0 crate `HANDLE` wraps `*mut c_void` (which is
/// `!Send`).  Windows handles are plain integer-like tokens that are safe
/// to use from any thread, so we store the value as a single `isize`
/// (which *is* `Send`) and reconstruct the original type on demand.
#[cfg(windows)]
#[derive(Clone, Copy)]
struct SendHandle(isize);

#[cfg(windows)]
unsafe impl Send for SendHandle {}

#[cfg(windows)]
impl SendHandle {
    fn from_handle(h: HANDLE) -> Self {
        Self(h.0 as isize)
    }
    fn to_handle(self) -> HANDLE {
        HANDLE(self.0 as *mut c_void)
    }
    fn is_null(self) -> bool {
        self.0 == 0
    }
}

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
    // TODO: implement resize on Unix by consuming resize_rx and calling TIOCSWINSZ on the master fd
    _resize_rx: mpsc::Receiver<(u16, u16)>,
    initial_input: Option<Vec<u8>>,
) -> Result<impl Future<Output = Result<()>>> {
    let result = unsafe { pty::forkpty(Some(&winsize), None) }?;

    match result.fork_result {
        ForkResult::Parent { child } => Ok(drive_child(
            child,
            result.master,
            input_rx,
            output_tx,
            initial_input,
        )),

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
    initial_input: Option<Vec<u8>>,
) -> Result<()> {
    let result = do_drive_child(master, input_rx, output_tx, initial_input).await;
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
    initial_input: Option<Vec<u8>>,
) -> Result<()> {
    let mut buf = [0u8; READ_BUF_SIZE];
    let mut input: Vec<u8> = initial_input.unwrap_or_default();
    nbio::set_non_blocking(&master.as_raw_fd())?;
    let master_fd = AsyncFd::new(master)?;
    let raw_fd = master_fd.get_ref().as_raw_fd();
    // ManuallyDrop: AsyncFd owns the FD; this File borrows it for read/write without closing on drop.
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

/// Escapes a single argument for a Windows command line following msvcrt conventions.
/// This replicates the logic from std::sys::windows::args::append_arg.
#[cfg(any(windows, test))]
#[allow(dead_code)]
pub(crate) fn escape_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }
    let needs_quoting = arg.contains([' ', '\t', '"']);
    if !needs_quoting {
        return arg.to_string();
    }
    let mut escaped = String::from('"');
    let mut backslashes: usize = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // Double the backslashes before a quote, then escape the quote
                for _ in 0..backslashes * 2 {
                    escaped.push('\\');
                }
                backslashes = 0;
                escaped.push('\\');
                escaped.push('"');
            }
            _ => {
                // Emit accumulated backslashes as-is (they don't precede a quote)
                for _ in 0..backslashes {
                    escaped.push('\\');
                }
                backslashes = 0;
                escaped.push(c);
            }
        }
    }
    // Double trailing backslashes before the closing quote
    for _ in 0..backslashes * 2 {
        escaped.push('\\');
    }
    escaped.push('"');
    escaped
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq)]
#[allow(dead_code)]
pub(crate) enum CommandKind {
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
            if start < bytes.len() && (bytes[start].is_ascii_alphanumeric() || bytes[start] == b'_')
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
#[allow(dead_code)]
pub(crate) fn classify_command(args: &[String]) -> CommandKind {
    let original_first = match args.first() {
        Some(s) => s,
        None => return CommandKind::Direct, // empty → default shell
    };
    let first = original_first.to_ascii_lowercase();

    // Single-string command line: e.g. ht "echo hello" arrives as
    // args = ["echo hello"]. When the first whitespace-delimited token
    // has no backslash it is a command name, not a file path, so the
    // user intended the whole string as a shell command line.  Strings
    // whose first token contains '\' are paths with spaces
    // (e.g. "C:\Program Files\foo.exe") and stay in the normal flow.
    //
    // Trade-off: this means ht "cmd.exe /k ..." or ht "notepad.exe file.txt"
    // routes through cmd.exe (creating a nested shell layer) rather than
    // launching the executable directly. Distinguishing "exe name + args"
    // from "shell command line" would require PATH lookups or extension
    // checks, so we accept the extra shell layer for correctness.
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
    if original_first.contains(['|', '>', '<', '&', '^']) {
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

// ── Screen-scraping PTY ─────────────────────────────────────────────

#[cfg(windows)]
static SCRAPE_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
#[derive(Clone, PartialEq)]
struct Cell {
    ch: char,
    width: u8,
    attr: u16,
}

/// Opens a fresh handle to the currently active console screen buffer.
///
/// `CONOUT$` always resolves to whichever buffer is active *at open time*,
/// so re-opening it each poll iteration tracks `SetConsoleActiveScreenBuffer`
/// switches (used by PowerShell 5.x among others).
#[cfg(windows)]
fn open_conout() -> Option<OwnedHandle> {
    let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
    let h = unsafe {
        CreateFileW(
            PCWSTR(name.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        )
    }
    .ok()?;

    // Propagate VT processing to the (possibly new) active buffer so the
    // child's ANSI sequences are interpreted rather than displayed literally.
    unsafe {
        let mut mode = CONSOLE_MODE(0);
        if GetConsoleMode(h, &mut mode).is_ok() {
            let _ = SetConsoleMode(
                h,
                mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING | ENABLE_PROCESSED_OUTPUT,
            );
        }
    }

    Some(unsafe { OwnedHandle::from_raw_handle(h.0 as *mut _) })
}

/// Tracks the desired console size so newly activated screen buffers can be resized.
#[cfg(windows)]
struct ConsoleSize {
    cols: std::sync::atomic::AtomicU16,
    rows: std::sync::atomic::AtomicU16,
}

#[cfg(windows)]
impl ConsoleSize {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: std::sync::atomic::AtomicU16::new(cols),
            rows: std::sync::atomic::AtomicU16::new(rows),
        }
    }

    fn get(&self) -> (u16, u16) {
        (
            self.cols.load(std::sync::atomic::Ordering::Relaxed),
            self.rows.load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    fn set(&self, cols: u16, rows: u16) {
        self.cols.store(cols, std::sync::atomic::Ordering::Relaxed);
        self.rows.store(rows, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(windows)]
fn clamp_console_dim(value: u16) -> i16 {
    (value.max(1).min(i16::MAX as u16)) as i16
}

#[cfg(windows)]
fn apply_console_size(handle: HANDLE, cols: u16, rows: u16, top: i16) {
    unsafe {
        let buf_width = clamp_console_dim(cols);
        let mut buf_height: i16 = i16::MAX;
        let small = SMALL_RECT {
            Left: 0,
            Top: 0,
            Right: 0,
            Bottom: 0,
        };
        let _ = SetConsoleWindowInfo(handle, true, &small);

        if SetConsoleScreenBufferSize(
            handle,
            COORD {
                X: buf_width,
                Y: buf_height,
            },
        )
        .is_err()
        {
            buf_height = clamp_console_dim(rows);
            let _ = SetConsoleScreenBufferSize(
                handle,
                COORD {
                    X: buf_width,
                    Y: buf_height,
                },
            );
        }

        let viewport_height = clamp_console_dim(rows);
        let viewport = SMALL_RECT {
            Left: 0,
            Top: top,
            Right: buf_width - 1,
            Bottom: top + viewport_height - 1,
        };
        let _ = SetConsoleWindowInfo(handle, true, &viewport);
    }
}

#[cfg(windows)]
fn ensure_console_size(handle: HANDLE, size: &ConsoleSize) -> Option<CONSOLE_SCREEN_BUFFER_INFO> {
    unsafe {
        let mut csbi: CONSOLE_SCREEN_BUFFER_INFO = zeroed();
        if GetConsoleScreenBufferInfo(handle, &mut csbi).is_err() {
            return None;
        }

        let (desired_cols, desired_rows) = size.get();
        let desired_cols_i16 = clamp_console_dim(desired_cols);
        let desired_rows_i16 = clamp_console_dim(desired_rows);
        let current_cols = csbi.dwSize.X;
        let current_rows = csbi.srWindow.Bottom - csbi.srWindow.Top + 1;

        if current_cols != desired_cols_i16 || current_rows != desired_rows_i16 {
            apply_console_size(handle, desired_cols, desired_rows, csbi.srWindow.Top);
            if GetConsoleScreenBufferInfo(handle, &mut csbi).is_err() {
                return None;
            }
        }

        Some(csbi)
    }
}

/// Converts a Windows console attribute word to an ANSI SGR escape sequence.
#[cfg(windows)]
fn attr_to_sgr(attr: u16) -> String {
    // Windows BGR bit ordering for foreground (bits 0-3) and background (bits 4-7)
    const WIN_FG_BLUE: u16 = 0x0001;
    const WIN_FG_GREEN: u16 = 0x0002;
    const WIN_FG_RED: u16 = 0x0004;
    const WIN_FG_INTENSITY: u16 = 0x0008;
    const WIN_BG_BLUE: u16 = 0x0010;
    const WIN_BG_GREEN: u16 = 0x0020;
    const WIN_BG_RED: u16 = 0x0040;
    const WIN_BG_INTENSITY: u16 = 0x0080;
    const COMMON_LVB_REVERSE_VIDEO: u16 = 0x4000;
    const COMMON_LVB_UNDERSCORE: u16 = 0x8000;

    // Map Windows BGR 3-bit to ANSI color index (0-7)
    fn win_to_ansi(blue: bool, green: bool, red: bool) -> u8 {
        // ANSI: 0=black 1=red 2=green 3=yellow 4=blue 5=magenta 6=cyan 7=white
        let mut idx = 0u8;
        if red {
            idx |= 1;
        }
        if green {
            idx |= 2;
        }
        if blue {
            idx |= 4;
        }
        idx
    }

    let fg_idx = win_to_ansi(
        attr & WIN_FG_BLUE != 0,
        attr & WIN_FG_GREEN != 0,
        attr & WIN_FG_RED != 0,
    );
    let fg_bright = attr & WIN_FG_INTENSITY != 0;
    let bg_idx = win_to_ansi(
        attr & WIN_BG_BLUE != 0,
        attr & WIN_BG_GREEN != 0,
        attr & WIN_BG_RED != 0,
    );
    let bg_bright = attr & WIN_BG_INTENSITY != 0;

    let fg_code = if fg_bright {
        90 + fg_idx as u32
    } else {
        30 + fg_idx as u32
    };
    let bg_code = if bg_bright {
        100 + bg_idx as u32
    } else {
        40 + bg_idx as u32
    };

    let mut sgr = format!("\x1b[0;{};{}", fg_code, bg_code);
    if attr & COMMON_LVB_REVERSE_VIDEO != 0 {
        sgr.push_str(";7");
    }
    if attr & COMMON_LVB_UNDERSCORE != 0 {
        sgr.push_str(";4");
    }
    sgr.push('m');
    sgr
}

/// Decodes a row of CHAR_INFO into a Vec<Cell>.
#[cfg(windows)]
fn decode_char_info_row(row: &[CHAR_INFO]) -> Vec<Cell> {
    const LVB_LEADING_BYTE: u16 = 0x0100;
    const LVB_TRAILING_BYTE: u16 = 0x0200;
    // Mask to remove width-hint LVB bits but preserve colors and style flags
    const ATTR_MASK: u16 = 0xCCFF;

    let mut cells = Vec::with_capacity(row.len());
    let mut i = 0;
    while i < row.len() {
        let ci = &row[i];
        let attrs = ci.Attributes;

        if attrs & LVB_TRAILING_BYTE != 0 {
            // Padding cell for a wide character, skip
            i += 1;
            continue;
        }

        let raw_char = unsafe { ci.Char.UnicodeChar };
        let masked_attr = attrs & ATTR_MASK;

        if attrs & LVB_LEADING_BYTE != 0 {
            // Wide character — might be a surrogate pair
            let ch = if (0xD800..=0xDBFF).contains(&raw_char) && i + 1 < row.len() {
                let next_raw = unsafe { row[i + 1].Char.UnicodeChar };
                // Decode UTF-16 surrogate pair
                char::decode_utf16([raw_char, next_raw])
                    .next()
                    .and_then(|r| r.ok())
                    .unwrap_or('\u{FFFD}')
            } else {
                char::from_u32(raw_char as u32).unwrap_or('\u{FFFD}')
            };
            cells.push(Cell {
                ch,
                width: 2,
                attr: masked_attr,
            });
        } else {
            // Normal single-width character — check for surrogate pair
            let ch = if (0xD800..=0xDBFF).contains(&raw_char) && i + 1 < row.len() {
                let next_raw = unsafe { row[i + 1].Char.UnicodeChar };
                if (0xDC00..=0xDFFF).contains(&next_raw) {
                    i += 1; // consume the low surrogate
                    char::decode_utf16([raw_char, next_raw])
                        .next()
                        .and_then(|r| r.ok())
                        .unwrap_or('\u{FFFD}')
                } else {
                    char::from_u32(raw_char as u32).unwrap_or('\u{FFFD}')
                }
            } else {
                char::from_u32(raw_char as u32).unwrap_or('\u{FFFD}')
            };
            cells.push(Cell {
                ch,
                width: 1,
                attr: masked_attr,
            });
        }
        i += 1;
    }
    cells
}

/// Diffs previous and current viewport buffers and emits ANSI escape sequences.
/// Cursor positions are 1-based (ANSI convention).
#[cfg(windows)]
fn diff_and_emit(
    prev: &[Vec<Cell>],
    curr: &[Vec<Cell>],
    cursor_row: u16,
    cursor_col: u16,
    cols: u16,
) -> String {
    let mut out = String::new();
    let mut last_attr: Option<u16> = None;

    for (row_idx, row) in curr.iter().enumerate() {
        let changed = if row_idx >= prev.len() {
            true
        } else {
            prev[row_idx] != *row
        };

        if !changed {
            continue;
        }

        // Move cursor to start of this row (1-based)
        out.push_str(&format!("\x1b[{};1H", row_idx + 1));

        for cell in row {
            // Emit SGR if attribute changed
            if last_attr != Some(cell.attr) {
                out.push_str(&attr_to_sgr(cell.attr));
                last_attr = Some(cell.attr);
            }
            out.push(cell.ch);
        }

        // Pad with spaces if row is shorter than viewport width
        let row_visual_width: u16 = row.iter().map(|c| c.width as u16).sum();
        if row_visual_width < cols {
            // Erase to end of line to clear stale content
            out.push_str("\x1b[K");
        }
    }

    // Position cursor
    out.push_str(&format!("\x1b[{};{}H", cursor_row, cursor_col));
    out
}

// ── Input parser ────────────────────────────────────────────────────────

/// Returns the expected byte length of a UTF-8 sequence given its start byte,
/// or 0 if the byte is not a valid UTF-8 start byte (continuation or invalid).
#[cfg(windows)]
fn utf8_seq_len(b: u8) -> usize {
    match b {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 0,
    }
}

#[cfg(windows)]
enum InputAction {
    KeyEvent(KEY_EVENT_RECORD),
    GenerateCtrlC,
}

// Virtual key codes
#[cfg(windows)]
const VK_BACK: u16 = 0x08;
#[cfg(windows)]
const VK_TAB: u16 = 0x09;
#[cfg(windows)]
const VK_RETURN: u16 = 0x0D;
#[cfg(windows)]
const VK_ESCAPE: u16 = 0x1B;
#[cfg(windows)]
const VK_PRIOR: u16 = 0x21; // Page Up
#[cfg(windows)]
const VK_NEXT: u16 = 0x22; // Page Down
#[cfg(windows)]
const VK_END: u16 = 0x23;
#[cfg(windows)]
const VK_HOME: u16 = 0x24;
#[cfg(windows)]
const VK_LEFT: u16 = 0x25;
#[cfg(windows)]
const VK_UP: u16 = 0x26;
#[cfg(windows)]
const VK_RIGHT: u16 = 0x27;
#[cfg(windows)]
const VK_DOWN: u16 = 0x28;
#[cfg(windows)]
const VK_INSERT: u16 = 0x2D;
#[cfg(windows)]
const VK_DELETE: u16 = 0x2E;
#[cfg(windows)]
const VK_F1: u16 = 0x70;
#[cfg(windows)]
const VK_F2: u16 = 0x71;
#[cfg(windows)]
const VK_F3: u16 = 0x72;
#[cfg(windows)]
const VK_F4: u16 = 0x73;
#[cfg(windows)]
const VK_F5: u16 = 0x74;
#[cfg(windows)]
const VK_F6: u16 = 0x75;
#[cfg(windows)]
const VK_F7: u16 = 0x76;
#[cfg(windows)]
const VK_F8: u16 = 0x77;
#[cfg(windows)]
const VK_F9: u16 = 0x78;
#[cfg(windows)]
const VK_F10: u16 = 0x79;
#[cfg(windows)]
const VK_F11: u16 = 0x7A;
#[cfg(windows)]
const VK_F12: u16 = 0x7B;
#[cfg(windows)]
const VK_PACKET: u16 = 0xE7;

// Modifier flags for dwControlKeyState
#[cfg(windows)]
const LEFT_ALT_PRESSED: u32 = 0x0002;
#[cfg(windows)]
const LEFT_CTRL_PRESSED: u32 = 0x0008;
#[cfg(windows)]
const SHIFT_PRESSED: u32 = 0x0010;

#[cfg(windows)]
struct InputParser {
    pending: Vec<u8>,
}

#[cfg(windows)]
impl InputParser {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    fn flush_pending(&mut self) -> Vec<InputAction> {
        let pending = std::mem::take(&mut self.pending);
        let mut actions = Vec::new();
        let mut i = 0;
        while i < pending.len() {
            let b = pending[i];
            if b == 0x1b {
                actions.push(self.make_key_action(0x1b_u16, 0x1b, 0));
                i += 1;
            } else if b >= 0x80 {
                let seq_len = utf8_seq_len(b);
                if seq_len > 0 && i + seq_len <= pending.len() {
                    if let Ok(s) = std::str::from_utf8(&pending[i..i + seq_len]) {
                        for ch in s.chars() {
                            actions.extend(self.char_to_actions(ch));
                        }
                        i += seq_len;
                        continue;
                    }
                }
                // Invalid or incomplete — send byte as VK_PACKET
                actions.push(self.make_vk_packet_action(b as u16));
                i += 1;
            } else {
                actions.extend(self.byte_to_actions(b));
                i += 1;
            }
        }
        actions
    }

    fn parse(&mut self, data: &[u8], conin: HANDLE) -> Vec<InputAction> {
        let mut input = Vec::new();
        input.append(&mut self.pending);
        input.extend_from_slice(data);

        let mut actions = Vec::new();
        let mut i = 0;

        while i < input.len() {
            let b = input[i];

            if b == 0x1b {
                // ESC — check for sequence continuation
                if i + 1 >= input.len() {
                    // ESC at end of input — buffer it
                    self.pending = input[i..].to_vec();
                    break;
                }

                let next = input[i + 1];

                if next == b'[' {
                    // CSI sequence
                    match self.parse_csi(&input[i..]) {
                        ParseResult::Complete(acts, consumed) => {
                            actions.extend(acts);
                            i += consumed;
                        }
                        ParseResult::Incomplete => {
                            self.pending = input[i..].to_vec();
                            break;
                        }
                        ParseResult::Unrecognized(consumed) => {
                            // Passthrough: ESC as VK_ESCAPE, then remaining bytes
                            actions.push(self.make_key_action(0x1b_u16, 0x1b, 0));
                            for &pb in &input[i + 1..i + consumed] {
                                actions.extend(self.byte_to_actions(pb));
                            }
                            i += consumed;
                        }
                    }
                } else if next == b'O' {
                    // SS3 sequence
                    if i + 2 >= input.len() {
                        self.pending = input[i..].to_vec();
                        break;
                    }
                    let final_byte = input[i + 2];
                    match final_byte {
                        b'A' => {
                            actions.push(self.make_key_action(VK_UP, 0, 0));
                            i += 3;
                        }
                        b'B' => {
                            actions.push(self.make_key_action(VK_DOWN, 0, 0));
                            i += 3;
                        }
                        b'C' => {
                            actions.push(self.make_key_action(VK_RIGHT, 0, 0));
                            i += 3;
                        }
                        b'D' => {
                            actions.push(self.make_key_action(VK_LEFT, 0, 0));
                            i += 3;
                        }
                        b'H' => {
                            actions.push(self.make_key_action(VK_HOME, 0, 0));
                            i += 3;
                        }
                        b'F' => {
                            actions.push(self.make_key_action(VK_END, 0, 0));
                            i += 3;
                        }
                        b'P' => {
                            actions.push(self.make_key_action(VK_F1, 0, 0));
                            i += 3;
                        }
                        b'Q' => {
                            actions.push(self.make_key_action(VK_F2, 0, 0));
                            i += 3;
                        }
                        b'R' => {
                            actions.push(self.make_key_action(VK_F3, 0, 0));
                            i += 3;
                        }
                        b'S' => {
                            actions.push(self.make_key_action(VK_F4, 0, 0));
                            i += 3;
                        }
                        _ => {
                            // Unknown SS3 — passthrough
                            actions.push(self.make_key_action(VK_ESCAPE, 0x1b, 0));
                            actions.extend(self.byte_to_actions(b'O'));
                            actions.extend(self.byte_to_actions(final_byte));
                            i += 3;
                        }
                    }
                } else if (0x20..=0x7E).contains(&next) {
                    // Alt+printable character
                    let ch = next as char;
                    let (vk, mut mods) = self.vkscan_char(ch);
                    mods |= LEFT_ALT_PRESSED;
                    actions.push(self.make_key_action(vk, next as u16, mods));
                    i += 2;
                } else {
                    // ESC followed by non-printable — send ESC then the byte
                    actions.push(self.make_key_action(VK_ESCAPE, 0x1b, 0));
                    i += 1;
                }
            } else if b == 0x03 {
                // Ctrl+C — check console mode
                let processed = self.is_processed_input(conin);
                if processed {
                    actions.push(InputAction::GenerateCtrlC);
                } else {
                    // Raw mode — send as key event
                    actions.push(self.make_key_action(b'C' as u16, 0x03, LEFT_CTRL_PRESSED));
                }
                i += 1;
            } else if b >= 0x80 {
                // UTF-8 multi-byte sequence
                let seq_len = utf8_seq_len(b);
                if seq_len == 0 {
                    // Invalid start byte — send as VK_PACKET
                    actions.push(self.make_vk_packet_action(b as u16));
                    i += 1;
                } else if i + seq_len > input.len() {
                    // Incomplete UTF-8 sequence at end of input — buffer it
                    self.pending = input[i..].to_vec();
                    break;
                } else {
                    match std::str::from_utf8(&input[i..i + seq_len]) {
                        Ok(s) => {
                            for ch in s.chars() {
                                actions.extend(self.char_to_actions(ch));
                            }
                            i += seq_len;
                        }
                        Err(_) => {
                            // Invalid UTF-8 — send first byte as VK_PACKET, advance one byte
                            actions.push(self.make_vk_packet_action(b as u16));
                            i += 1;
                        }
                    }
                }
            } else {
                actions.extend(self.byte_to_actions(b));
                i += 1;
            }
        }

        actions
    }

    fn is_processed_input(&self, conin: HANDLE) -> bool {
        let mut mode = CONSOLE_MODE(0);
        let result = unsafe { GetConsoleMode(conin, &mut mode) };
        if result.is_ok() {
            mode & ENABLE_PROCESSED_INPUT != CONSOLE_MODE(0)
        } else {
            true // default assumption: processed input
        }
    }

    fn byte_to_actions(&self, b: u8) -> Vec<InputAction> {
        match b {
            0x0D => vec![self.make_key_action(VK_RETURN, b as u16, 0)],
            0x0A => {
                // Ctrl+J (NOT VK_RETURN)
                vec![self.make_key_action(b'J' as u16, 0x0A, LEFT_CTRL_PRESSED)]
            }
            0x09 => vec![self.make_key_action(VK_TAB, b as u16, 0)],
            0x08 | 0x7F => vec![self.make_key_action(VK_BACK, 0x08, 0)],
            0x1B => vec![self.make_key_action(VK_ESCAPE, 0x1B, 0)],
            0x1A => {
                // Ctrl+Z
                vec![self.make_key_action(b'Z' as u16, 0x1A, LEFT_CTRL_PRESSED)]
            }
            0x01..=0x02 | 0x04..=0x08 | 0x0B..=0x0C | 0x0E..=0x19 => {
                // Other Ctrl+letter combinations
                let letter = b'A' + (b - 1);
                vec![self.make_key_action(letter as u16, b as u16, LEFT_CTRL_PRESSED)]
            }
            0x20..=0x7E => {
                // Printable ASCII
                self.char_to_actions(b as char)
            }
            _ => {
                // Lone high byte (invalid UTF-8) — send as VK_PACKET
                vec![self.make_vk_packet_action(b as u16)]
            }
        }
    }

    fn char_to_actions(&self, ch: char) -> Vec<InputAction> {
        let (vk, mods) = self.vkscan_char(ch);
        if vk == VK_PACKET {
            self.vk_packet_for_char(ch)
        } else {
            vec![self.make_key_action(vk, ch as u16, mods)]
        }
    }

    /// Uses VkKeyScanW to get VK code and modifier state for a character.
    /// Returns (vk_code, dwControlKeyState). Returns (VK_PACKET, 0) on failure.
    fn vkscan_char(&self, ch: char) -> (u16, u32) {
        let result = unsafe { VkKeyScanW(ch as u16) };
        if result as u16 == 0xFFFF {
            return (VK_PACKET, 0);
        }
        let vk = (result & 0xFF) as u16;
        let shift_state = ((result >> 8) & 0xFF) as u8;
        let mut mods = 0u32;
        if shift_state & 1 != 0 {
            mods |= SHIFT_PRESSED;
        }
        if shift_state & 2 != 0 {
            mods |= LEFT_CTRL_PRESSED;
        }
        if shift_state & 4 != 0 {
            mods |= LEFT_ALT_PRESSED;
        }
        (vk, mods)
    }

    fn vk_packet_for_char(&self, ch: char) -> Vec<InputAction> {
        let mut buf = [0u16; 2];
        let encoded = ch.encode_utf16(&mut buf);
        encoded
            .iter()
            .map(|&u| self.make_vk_packet_action(u))
            .collect()
    }

    fn make_vk_packet_action(&self, unicode_char: u16) -> InputAction {
        let mut ke: KEY_EVENT_RECORD = unsafe { zeroed() };
        ke.wVirtualKeyCode = VK_PACKET;
        ke.wVirtualScanCode = 0;
        ke.uChar.UnicodeChar = unicode_char;
        ke.dwControlKeyState = 0;
        ke.wRepeatCount = 1;
        InputAction::KeyEvent(ke)
    }

    fn make_key_action(&self, vk: u16, unicode_char: u16, mods: u32) -> InputAction {
        let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
        let mut ke: KEY_EVENT_RECORD = unsafe { zeroed() };
        ke.wVirtualKeyCode = vk;
        ke.wVirtualScanCode = scan;
        ke.uChar.UnicodeChar = unicode_char;
        ke.dwControlKeyState = mods;
        ke.wRepeatCount = 1;
        InputAction::KeyEvent(ke)
    }

    /// Parses modifier parameter value to dwControlKeyState flags.
    fn modifier_to_flags(modifier: u8) -> u32 {
        match modifier {
            2 => SHIFT_PRESSED,
            3 => LEFT_ALT_PRESSED,
            4 => LEFT_ALT_PRESSED | SHIFT_PRESSED,
            5 => LEFT_CTRL_PRESSED,
            6 => LEFT_CTRL_PRESSED | SHIFT_PRESSED,
            7 => LEFT_CTRL_PRESSED | LEFT_ALT_PRESSED,
            8 => LEFT_CTRL_PRESSED | LEFT_ALT_PRESSED | SHIFT_PRESSED,
            _ => 0,
        }
    }

    fn parse_csi(&self, input: &[u8]) -> ParseResult {
        // input starts with \x1b[
        debug_assert!(input.len() >= 2 && input[0] == 0x1b && input[1] == b'[');

        // Find the final byte (0x40-0x7E)
        let mut param_end = 2;
        while param_end < input.len() {
            let b = input[param_end];
            if (0x40..=0x7E).contains(&b) {
                break;
            }
            if !(b.is_ascii_digit() || b == b';') {
                // Invalid CSI parameter character
                return ParseResult::Unrecognized(param_end + 1);
            }
            param_end += 1;
        }

        if param_end >= input.len() {
            return ParseResult::Incomplete;
        }

        let final_byte = input[param_end];
        let params_str = std::str::from_utf8(&input[2..param_end]).unwrap_or("");
        let consumed = param_end + 1;

        // Parse parameters
        let params: Vec<&str> = if params_str.is_empty() {
            vec![]
        } else {
            params_str.split(';').collect()
        };

        // Handle arrow keys: CSI A/B/C/D or CSI 1;mod A/B/C/D
        match final_byte {
            b'A' | b'B' | b'C' | b'D' => {
                let vk = match final_byte {
                    b'A' => VK_UP,
                    b'B' => VK_DOWN,
                    b'C' => VK_RIGHT,
                    b'D' => VK_LEFT,
                    _ => unreachable!(),
                };
                let mods = self.extract_modifier(&params);
                ParseResult::Complete(vec![self.make_key_action(vk, 0, mods)], consumed)
            }
            b'H' => {
                let mods = self.extract_modifier(&params);
                ParseResult::Complete(vec![self.make_key_action(VK_HOME, 0, mods)], consumed)
            }
            b'F' => {
                let mods = self.extract_modifier(&params);
                ParseResult::Complete(vec![self.make_key_action(VK_END, 0, mods)], consumed)
            }
            b'P' | b'Q' | b'R' | b'S' => {
                // F1-F4 (CSI form with modifier: CSI 1;mod P/Q/R/S)
                let vk = match final_byte {
                    b'P' => VK_F1,
                    b'Q' => VK_F2,
                    b'R' => VK_F3,
                    b'S' => VK_F4,
                    _ => unreachable!(),
                };
                let mods = self.extract_modifier(&params);
                ParseResult::Complete(vec![self.make_key_action(vk, 0, mods)], consumed)
            }
            b'~' => {
                // Tilde sequences: CSI num ~ or CSI num;mod ~
                let num: u16 = params.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                let mods = if params.len() >= 2 {
                    params[1]
                        .parse::<u8>()
                        .ok()
                        .map(Self::modifier_to_flags)
                        .unwrap_or(0)
                } else {
                    0
                };
                let vk = match num {
                    2 => Some(VK_INSERT),
                    3 => Some(VK_DELETE),
                    5 => Some(VK_PRIOR),
                    6 => Some(VK_NEXT),
                    15 => Some(VK_F5),
                    17 => Some(VK_F6),
                    18 => Some(VK_F7),
                    19 => Some(VK_F8),
                    20 => Some(VK_F9),
                    21 => Some(VK_F10),
                    23 => Some(VK_F11),
                    24 => Some(VK_F12),
                    _ => None,
                };
                if let Some(vk) = vk {
                    ParseResult::Complete(vec![self.make_key_action(vk, 0, mods)], consumed)
                } else {
                    ParseResult::Unrecognized(consumed)
                }
            }
            _ => ParseResult::Unrecognized(consumed),
        }
    }

    fn extract_modifier(&self, params: &[&str]) -> u32 {
        if params.len() >= 2 {
            params[1]
                .parse::<u8>()
                .ok()
                .map(Self::modifier_to_flags)
                .unwrap_or(0)
        } else {
            0
        }
    }
}

#[cfg(windows)]
enum ParseResult {
    Complete(Vec<InputAction>, usize), // actions, bytes consumed
    Incomplete,
    Unrecognized(usize), // bytes consumed
}

/// Expands a KEY_EVENT_RECORD into key-down + key-up INPUT_RECORD pair.
#[cfg(windows)]
fn expand_key_event(ke: &KEY_EVENT_RECORD) -> [INPUT_RECORD; 2] {
    let mut down: INPUT_RECORD = unsafe { zeroed() };
    down.EventType = 0x0001; // KEY_EVENT

    let mut key_down = *ke;
    key_down.bKeyDown = windows::Win32::Foundation::BOOL(1);
    down.Event.KeyEvent = key_down;

    let mut up = down;
    let mut key_up = *ke;
    key_up.bKeyDown = windows::Win32::Foundation::BOOL(0);
    up.Event.KeyEvent = key_up;

    [down, up]
}

// ── ScrapePty struct ────────────────────────────────────────────────────

#[cfg(windows)]
struct ScrapePty {
    child_pid: u32,
    proc_handle: Option<OwnedHandle>,
    thread_handle: Option<OwnedHandle>,
    conout: SendHandle,
    conin: SendHandle,
    needs_cleanup: bool,
    size: std::sync::Arc<ConsoleSize>,
}

#[cfg(windows)]
unsafe impl Send for ScrapePty {}

#[cfg(windows)]
impl Drop for ScrapePty {
    fn drop(&mut self) {
        if self.needs_cleanup {
            unsafe {
                if !self.conout.is_null() {
                    let _ = windows::Win32::Foundation::CloseHandle(self.conout.to_handle());
                }
                if !self.conin.is_null() {
                    let _ = windows::Win32::Foundation::CloseHandle(self.conin.to_handle());
                }
                if let Some(ref h) = self.proc_handle {
                    let _ = TerminateProcess(HANDLE(h.as_raw_handle()), 1);
                }
                let _ = SetConsoleCtrlHandler(None, false);
                let _ = FreeConsole();
                let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            }
            SCRAPE_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
        }
    }
}

/// RAII guard for console attachment during ScrapePty::new().
#[cfg(windows)]
struct ConsoleGuard {
    detached: bool,
    ctrl_handler_set: bool,
}

#[cfg(windows)]
impl ConsoleGuard {
    fn new() -> Self {
        Self {
            detached: false,
            ctrl_handler_set: false,
        }
    }
    fn mark_detached(&mut self) {
        self.detached = true;
    }
    fn mark_ctrl_handler_set(&mut self) {
        self.ctrl_handler_set = true;
    }
    fn disarm(self) {
        std::mem::forget(self);
    }
}

#[cfg(windows)]
impl Drop for ConsoleGuard {
    fn drop(&mut self) {
        unsafe {
            if self.ctrl_handler_set {
                let _ = SetConsoleCtrlHandler(None, false);
            }
            if self.detached {
                let _ = FreeConsole();
                let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
        SCRAPE_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(windows)]
impl ScrapePty {
    fn new(winsize: Winsize, command: &str) -> Result<Self> {
        use std::sync::atomic::Ordering;

        // 1. Acquire single-instance lock
        if SCRAPE_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            anyhow::bail!("only one ht instance can be active at a time");
        }

        let mut guard = ConsoleGuard::new();

        // 2. Verify stdio is pipe-backed (not console)
        unsafe {
            for &std_handle in &[STD_INPUT_HANDLE, STD_OUTPUT_HANDLE] {
                let h = GetStdHandle(std_handle)?;
                if h != INVALID_HANDLE_VALUE && !h.is_invalid() {
                    let mut mode = CONSOLE_MODE(0);
                    if GetConsoleMode(h, &mut mode).is_ok() {
                        anyhow::bail!(
                            "ht requires redirected stdio (pipes or files).\n\
                             It cannot be used from an interactive console.\n\
                             Typical usage: orchestrator spawns `ht ...` with piped stdin/stdout."
                        );
                    }
                }
            }
        }

        // 3. FreeConsole — detach from current console
        let _ = unsafe { FreeConsole() };
        guard.mark_detached();

        // 4. CreateProcessW with CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP
        assert!(
            !command.is_empty(),
            "command should not be empty; caller provides a default"
        );
        let mut cmd_wide: Vec<u16> = command
            .encode_utf16()
            .chain(std::iter::once(0u16))
            .collect();
        let cmd_pwstr = PWSTR(cmd_wide.as_mut_ptr());

        let mut si: STARTUPINFOW = unsafe { zeroed() };
        si.cb = size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESHOWWINDOW;
        si.wShowWindow = SW_HIDE.0 as u16;

        let mut proc_info: PROCESS_INFORMATION = unsafe { zeroed() };

        unsafe {
            CreateProcessW(
                None,
                cmd_pwstr,
                None,
                None,
                false,
                CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP,
                None,
                None,
                &si,
                &mut proc_info,
            )
        }?;

        let child_pid = proc_info.dwProcessId;
        let proc_handle =
            Some(unsafe { OwnedHandle::from_raw_handle(proc_info.hProcess.0 as *mut _) });
        let thread_handle =
            Some(unsafe { OwnedHandle::from_raw_handle(proc_info.hThread.0 as *mut _) });

        // 5. Polling attach loop — retry AttachConsole every 50ms for up to 5s
        let attach_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut attached = false;
        while std::time::Instant::now() < attach_deadline {
            // Check if child already exited
            let wait_result = unsafe { WaitForSingleObject(proc_info.hProcess, 0) };
            if wait_result == WAIT_OBJECT_0 {
                anyhow::bail!("child process exited before console was ready");
            }
            if unsafe { AttachConsole(child_pid) }.is_ok() {
                attached = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if !attached {
            anyhow::bail!("failed to attach to child console after 5 seconds");
        }

        // 6. SetConsoleCtrlHandler — ignore Ctrl+C/Break in HT
        unsafe { SetConsoleCtrlHandler(None, true) }?;
        guard.mark_ctrl_handler_set();

        // 7. Open CONOUT$ and CONIN$
        let conout_name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        let conout = unsafe {
            CreateFileW(
                PCWSTR(conout_name.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                Default::default(),
                None,
            )
        }?;
        let conout_handle = SendHandle::from_handle(conout);

        let conin_name: Vec<u16> = "CONIN$\0".encode_utf16().collect();
        let conin = unsafe {
            CreateFileW(
                PCWSTR(conin_name.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                Default::default(),
                None,
            )
        }?;
        let conin_handle = SendHandle::from_handle(conin);

        // 8. Enable VT processing
        unsafe {
            let mut mode = CONSOLE_MODE(0);
            if GetConsoleMode(conout, &mut mode).is_ok() {
                let _ = SetConsoleMode(
                    conout,
                    mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING | ENABLE_PROCESSED_OUTPUT,
                );
            }
            let mut in_mode = CONSOLE_MODE(0);
            if GetConsoleMode(conin, &mut in_mode).is_ok() {
                let _ = SetConsoleMode(conin, in_mode | ENABLE_VIRTUAL_TERMINAL_INPUT);
            }
        }

        // 9. Apply terminal size — viewport = requested size, buffer height = maximum
        apply_console_size(conout, winsize.ws_col, winsize.ws_row, 0);

        // 10. Disarm guard — ownership transfers to ScrapePty
        guard.disarm();

        let size = std::sync::Arc::new(ConsoleSize::new(winsize.ws_col, winsize.ws_row));

        Ok(ScrapePty {
            child_pid,
            proc_handle,
            thread_handle,
            conout: conout_handle,
            conin: conin_handle,
            needs_cleanup: true,
            size,
        })
    }

    async fn drive(
        mut self,
        input_rx: mpsc::Receiver<Vec<u8>>,
        output_tx: mpsc::Sender<Vec<u8>>,
        resize_rx: mpsc::Receiver<(u16, u16)>,
        initial_input: Option<Vec<u8>>,
    ) -> Result<()> {
        let conout = self.conout;
        let conin = self.conin;
        let child_pid = self.child_pid;
        let console_size = self.size.clone();

        // Take ownership of process/thread handles out of self.
        // self.proc_handle becomes None, so Drop won't try to TerminateProcess.
        // proc_owned keeps the OS handle alive; proc_send is the Send-safe copy
        // for spawned tasks.  The handle is closed exactly once via drop(proc_owned).
        let proc_owned = self
            .proc_handle
            .take()
            .ok_or_else(|| anyhow::anyhow!("proc_handle missing"))?;
        let proc_send = SendHandle::from_handle(HANDLE(proc_owned.as_raw_handle()));
        let thread_owned = self.thread_handle.take();

        // Shared flag for signaling poll thread to stop
        let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag_poll = stop_flag.clone();

        // Screen poll thread (spawn_blocking)
        let output_tx_poll = output_tx.clone();
        let size_for_poll = console_size.clone();
        let mut poll_handle = tokio::task::spawn_blocking(move || {
            let mut prev_viewport: Vec<Vec<Cell>> = Vec::new();
            let mut prev_sr_window_top: i16 = 0;
            let mut first_poll = true;

            while !stop_flag_poll.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(40));

                // Re-open CONOUT$ to track active screen buffer switches.
                // Falls back to the original handle if the open fails.
                let conout_owned = open_conout();
                let conout_h = if let Some(ref h) = conout_owned {
                    HANDLE(h.as_raw_handle() as *mut _)
                } else {
                    conout.to_handle()
                };

                // Get current screen buffer info
                let mut csbi = match ensure_console_size(conout_h, &size_for_poll) {
                    Some(info) => info,
                    None => break,
                };

                let sr = csbi.srWindow;
                let viewport_width = (sr.Right - sr.Left + 1) as usize;
                let viewport_height = (sr.Bottom - sr.Top + 1) as usize;

                if viewport_width == 0 || viewport_height == 0 {
                    continue;
                }

                let mut output_data = String::new();

                // Scroll detection: recover scrolled-out rows
                if !first_poll && sr.Top > prev_sr_window_top {
                    let scroll_start = prev_sr_window_top;
                    let scroll_end = sr.Top;
                    let scroll_rows = (scroll_end - scroll_start) as usize;

                    // Read scrolled-out rows from scrollback buffer
                    let read_height = scroll_rows.min(8192);
                    let mut scroll_buf =
                        vec![unsafe { zeroed::<CHAR_INFO>() }; viewport_width * read_height];
                    let scroll_size = COORD {
                        X: viewport_width as i16,
                        Y: read_height as i16,
                    };
                    let scroll_coord = COORD { X: 0, Y: 0 };
                    let mut scroll_rect = SMALL_RECT {
                        Left: sr.Left,
                        Top: scroll_start,
                        Right: sr.Right,
                        Bottom: scroll_start + read_height as i16 - 1,
                    };

                    if unsafe {
                        ReadConsoleOutputW(
                            conout_h,
                            scroll_buf.as_mut_ptr(),
                            scroll_size,
                            scroll_coord,
                            &mut scroll_rect,
                        )
                    }
                    .is_ok()
                    {
                        for row_idx in 0..read_height {
                            let start = row_idx * viewport_width;
                            let end = start + viewport_width;
                            let row_cells = decode_char_info_row(&scroll_buf[start..end]);
                            // Emit scrolled row with full color
                            let mut last_attr: Option<u16> = None;
                            for cell in &row_cells {
                                if last_attr != Some(cell.attr) {
                                    output_data.push_str(&attr_to_sgr(cell.attr));
                                    last_attr = Some(cell.attr);
                                }
                                output_data.push(cell.ch);
                            }
                            output_data.push_str("\r\n");
                        }
                    }

                    // Force full viewport redraw after scroll
                    prev_viewport.clear();
                }

                prev_sr_window_top = sr.Top;

                // Read current viewport via ReadConsoleOutputW
                let mut buf =
                    vec![unsafe { zeroed::<CHAR_INFO>() }; viewport_width * viewport_height];
                let buf_size = COORD {
                    X: viewport_width as i16,
                    Y: viewport_height as i16,
                };
                let buf_coord = COORD { X: 0, Y: 0 };
                let mut read_region = SMALL_RECT {
                    Left: sr.Left,
                    Top: sr.Top,
                    Right: sr.Right,
                    Bottom: sr.Bottom,
                };

                if unsafe {
                    ReadConsoleOutputW(
                        conout_h,
                        buf.as_mut_ptr(),
                        buf_size,
                        buf_coord,
                        &mut read_region,
                    )
                }
                .is_err()
                {
                    break;
                }

                // Decode CHAR_INFO buffer into Cell grid
                let mut curr_viewport: Vec<Vec<Cell>> = Vec::with_capacity(viewport_height);
                for row_idx in 0..viewport_height {
                    let start = row_idx * viewport_width;
                    let end = start + viewport_width;
                    curr_viewport.push(decode_char_info_row(&buf[start..end]));
                }

                // Convert cursor to 1-based ANSI coordinates
                let cursor_row = (csbi.dwCursorPosition.Y - sr.Top + 1).max(1) as u16;
                let cursor_col = (csbi.dwCursorPosition.X - sr.Left + 1).max(1) as u16;

                // Diff and emit
                let diff = diff_and_emit(
                    &prev_viewport,
                    &curr_viewport,
                    cursor_row,
                    cursor_col,
                    viewport_width as u16,
                );

                output_data.push_str(&diff);

                if !output_data.is_empty()
                    && output_tx_poll
                        .blocking_send(output_data.into_bytes())
                        .is_err()
                {
                    break;
                }

                prev_viewport = curr_viewport;
                first_poll = false;
            }
        });

        // Input relay task (tokio::spawn — async for timeout support)
        let conin_input = conin;
        let mut input_relay = tokio::spawn(async move {
            let mut parser = InputParser::new();
            let mut input_rx = input_rx;

            // Handle initial input
            if let Some(data) = initial_input {
                let conin_h = conin_input.to_handle();
                let actions = parser.parse(&data, conin_h);
                Self::dispatch_actions(&actions, conin_h, child_pid);
            }

            loop {
                let result =
                    tokio::time::timeout(std::time::Duration::from_millis(1000), input_rx.recv())
                        .await;

                // Reconstruct HANDLE after .await (HANDLE is !Send)
                let conin_h = conin_input.to_handle();
                match result {
                    Ok(Some(data)) => {
                        let actions = parser.parse(&data, conin_h);
                        Self::dispatch_actions(&actions, conin_h, child_pid);
                    }
                    Ok(None) => {
                        // Channel closed — flush pending
                        if parser.has_pending() {
                            let actions = parser.flush_pending();
                            Self::dispatch_actions(&actions, conin_h, child_pid);
                        }
                        break;
                    }
                    Err(_timeout) => {
                        if parser.has_pending() {
                            let actions = parser.flush_pending();
                            Self::dispatch_actions(&actions, conin_h, child_pid);
                        }
                    }
                }
            }
        });

        // Resize task
        let resize_conout = conout;
        let size_for_resize = console_size.clone();
        let resize_task = tokio::spawn(async move {
            let mut resize_rx = resize_rx;
            while let Some((new_cols, new_rows)) = resize_rx.recv().await {
                // Re-open CONOUT$ to track active screen buffer switches.
                // Falls back to the original handle if the open fails.
                let conout_owned = open_conout();
                let conout_h = if let Some(ref h) = conout_owned {
                    HANDLE(h.as_raw_handle() as *mut _)
                } else {
                    resize_conout.to_handle()
                };
                let top = {
                    let mut csbi: CONSOLE_SCREEN_BUFFER_INFO = unsafe { zeroed() };
                    if unsafe { GetConsoleScreenBufferInfo(conout_h, &mut csbi) }.is_ok() {
                        csbi.srWindow.Top
                    } else {
                        0
                    }
                };
                apply_console_size(conout_h, new_cols, new_rows, top);
                size_for_resize.set(new_cols, new_rows);
            }
        });

        // Process wait thread
        let mut wait_handle = tokio::task::spawn_blocking(move || unsafe {
            WaitForSingleObject(proc_send.to_handle(), u32::MAX);
        });

        // Wait for any thread to finish
        #[derive(PartialEq)]
        enum Finished {
            Poll,
            Input,
            Wait,
        }
        let finished = tokio::select! {
            _ = &mut poll_handle => Finished::Poll,
            _ = &mut input_relay => Finished::Input,
            _ = &mut wait_handle => Finished::Wait,
        };

        // --- Cleanup sequence ---

        // 1. Signal poll thread to stop and abort resize task
        stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        resize_task.abort();

        // 2. Terminate child if still alive (use proc_send — single owner)
        {
            let proc_raw = proc_send.to_handle();
            let wait_result = unsafe { WaitForSingleObject(proc_raw, 0) };
            if wait_result != WAIT_OBJECT_0 {
                let _ = unsafe { TerminateProcess(proc_raw, 1) };
                tokio::task::spawn_blocking(move || unsafe {
                    WaitForSingleObject(proc_send.to_handle(), 5000);
                })
                .await?;
            }
        }

        // 3. Await remaining threads (all must finish before closing handles)
        if finished != Finished::Poll {
            let _ = poll_handle.await;
        }
        if finished != Finished::Input {
            input_relay.abort();
            let _ = input_relay.await;
        }
        if finished != Finished::Wait {
            let _ = wait_handle.await;
        }
        let _ = resize_task.await;

        // 4. Close handles
        unsafe {
            if !self.conout.is_null() {
                let _ = windows::Win32::Foundation::CloseHandle(self.conout.to_handle());
                self.conout = SendHandle(0);
            }
            if !self.conin.is_null() {
                let _ = windows::Win32::Foundation::CloseHandle(self.conin.to_handle());
                self.conin = SendHandle(0);
            }
        }

        // 5. Re-enable Ctrl+C
        let _ = unsafe { SetConsoleCtrlHandler(None, false) };

        // 6. Detach from child's console
        let _ = unsafe { FreeConsole() };

        // 7. Reattach to parent console
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };

        // 8. Cleanup complete
        self.needs_cleanup = false;
        SCRAPE_ACTIVE.store(false, std::sync::atomic::Ordering::Release);

        // Drop handles (proc_owned/thread_owned own the OS handles; closing happens here)
        drop(proc_owned);
        drop(thread_owned);

        Ok(())
    }

    fn dispatch_actions(actions: &[InputAction], conin: HANDLE, child_pid: u32) {
        for action in actions {
            match action {
                InputAction::KeyEvent(ke) => {
                    let records = expand_key_event(ke);
                    let mut written = 0u32;
                    let _ = unsafe { WriteConsoleInputW(conin, &records, &mut written) };
                }
                InputAction::GenerateCtrlC => {
                    let _ = unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, child_pid) };
                }
            }
        }
    }
}

#[cfg(windows)]
pub fn spawn(
    command: String,
    winsize: Winsize,
    input_rx: mpsc::Receiver<Vec<u8>>,
    output_tx: mpsc::Sender<Vec<u8>>,
    resize_rx: mpsc::Receiver<(u16, u16)>,
    initial_input: Option<Vec<u8>>,
) -> Result<impl Future<Output = Result<()>>> {
    let scrape = ScrapePty::new(winsize, &command)?;
    Ok(scrape.drive(input_rx, output_tx, resize_rx, initial_input))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── escape_arg ────────────────────────────────────────────────

    #[test]
    fn escape_empty_string() {
        assert_eq!(escape_arg(""), "\"\"");
    }

    #[test]
    fn escape_simple_arg() {
        assert_eq!(escape_arg("hello"), "hello");
    }

    #[test]
    fn escape_arg_with_space() {
        assert_eq!(escape_arg("hello world"), "\"hello world\"");
    }

    #[test]
    fn escape_arg_with_tab() {
        assert_eq!(escape_arg("hello\tworld"), "\"hello\tworld\"");
    }

    #[test]
    fn escape_arg_with_embedded_quote() {
        assert_eq!(escape_arg(r#"say "hi""#), r#""say \"hi\"""#);
    }

    #[test]
    fn escape_arg_backslash_before_quote() {
        // Input: foo\"bar  → backslash must be doubled before the quote
        assert_eq!(escape_arg("foo\\\"bar"), "\"foo\\\\\\\"bar\"");
    }

    #[test]
    fn escape_arg_trailing_backslash() {
        // Input: C:\dir\  → trailing backslash doubled before closing quote only if quoting needed
        assert_eq!(
            escape_arg(r"C:\dir with space\"),
            r#""C:\dir with space\\""#
        );
    }

    #[test]
    fn escape_arg_multiple_trailing_backslashes() {
        assert_eq!(escape_arg("a b\\\\"), "\"a b\\\\\\\\\"");
    }

    #[test]
    fn escape_windows_path_with_space() {
        // Backslashes NOT before quotes are kept as-is
        assert_eq!(
            escape_arg(r"C:\Program Files\foo.exe"),
            r#""C:\Program Files\foo.exe""#
        );
    }

    #[test]
    fn escape_no_quoting_needed() {
        assert_eq!(escape_arg(r"C:\Windows\system32"), r"C:\Windows\system32");
    }

    // ── classify_command ────────────────────────────────────────────

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

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
    fn classify_metachar_mixed_case_is_shell_syntax() {
        assert_eq!(classify_command(&args(&["A|B"])), CommandKind::ShellSyntax);
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

    #[test]
    fn no_env_var_paren_at_start() {
        assert!(!contains_env_var("%(x86)%"));
    }
}

#[cfg(all(test, windows))]
mod scrape_tests {
    use super::*;

    // ── attr_to_sgr ─────────────────────────────────────────────────

    #[test]
    fn attr_default_white_on_black() {
        // Default console: white fg (RGB=111) = 0x07, black bg = 0x00
        let sgr = attr_to_sgr(0x0007);
        assert!(sgr.contains("37"), "expected white fg (37), got: {sgr}");
        assert!(sgr.contains("40"), "expected black bg (40), got: {sgr}");
    }

    #[test]
    fn attr_red_foreground() {
        // Red fg = FOREGROUND_RED (0x04)
        let sgr = attr_to_sgr(0x0004);
        assert!(sgr.contains("31"), "expected red fg (31), got: {sgr}");
    }

    #[test]
    fn attr_blue_background() {
        // Blue bg = BACKGROUND_BLUE (0x10)
        let sgr = attr_to_sgr(0x0017); // white fg + blue bg
        assert!(sgr.contains("44"), "expected blue bg (44), got: {sgr}");
    }

    #[test]
    fn attr_bright_green_foreground() {
        // Bright green = GREEN(0x02) | INTENSITY(0x08)
        let sgr = attr_to_sgr(0x000A);
        assert!(
            sgr.contains("92"),
            "expected bright green fg (92), got: {sgr}"
        );
    }

    #[test]
    fn attr_reverse_video() {
        let sgr = attr_to_sgr(0x4007); // COMMON_LVB_REVERSE_VIDEO | white on black
        assert!(
            sgr.contains(";7"),
            "expected reverse video (;7), got: {sgr}"
        );
    }

    #[test]
    fn attr_underscore() {
        let sgr = attr_to_sgr(0x8007); // COMMON_LVB_UNDERSCORE | white on black
        assert!(sgr.contains(";4"), "expected underline (;4), got: {sgr}");
    }

    #[test]
    fn attr_combined_reverse_underline() {
        let sgr = attr_to_sgr(0xC004); // REVERSE + UNDERSCORE + red fg
        assert!(sgr.contains(";7"), "expected reverse, got: {sgr}");
        assert!(sgr.contains(";4"), "expected underline, got: {sgr}");
        assert!(sgr.contains("31"), "expected red fg, got: {sgr}");
    }

    // ── diff_and_emit ───────────────────────────────────────────────

    #[test]
    fn diff_first_frame_emits_all() {
        let row = vec![
            Cell {
                ch: 'A',
                width: 1,
                attr: 0x07,
            },
            Cell {
                ch: 'B',
                width: 1,
                attr: 0x07,
            },
        ];
        let curr = vec![row];
        let result = diff_and_emit(&[], &curr, 1, 1, 2);
        assert!(result.contains('A'), "should contain 'A': {result}");
        assert!(result.contains('B'), "should contain 'B': {result}");
        // Should have cursor positioning
        assert!(
            result.contains("\x1b[1;1H"),
            "should position cursor: {result}"
        );
    }

    #[test]
    fn diff_no_change_emits_cursor_only() {
        let row = vec![Cell {
            ch: 'X',
            width: 1,
            attr: 0x07,
        }];
        let viewport = vec![row.clone()];
        let result = diff_and_emit(&viewport, &viewport, 1, 1, 1);
        // Should only contain cursor positioning, not 'X'
        assert!(
            !result.contains('X'),
            "unchanged row should not re-emit: {result}"
        );
    }

    #[test]
    fn diff_changed_row_has_erase() {
        let old = vec![vec![Cell {
            ch: 'A',
            width: 1,
            attr: 0x07,
        }]];
        let new = vec![vec![Cell {
            ch: 'B',
            width: 1,
            attr: 0x07,
        }]];
        let result = diff_and_emit(&old, &new, 1, 1, 3);
        assert!(result.contains('B'), "should contain new char: {result}");
        assert!(
            result.contains("\x1b[K"),
            "should have erase-to-end: {result}"
        );
    }

    #[test]
    fn diff_cursor_position_1based() {
        let row = vec![Cell {
            ch: ' ',
            width: 1,
            attr: 0x07,
        }];
        let viewport = vec![row];
        let result = diff_and_emit(&viewport, &viewport, 3, 5, 1);
        assert!(
            result.contains("\x1b[3;5H"),
            "cursor should be at row 3 col 5: {result}"
        );
    }

    // ── InputParser ─────────────────────────────────────────────────

    #[test]
    fn parse_printable_ascii() {
        let parser = InputParser::new();
        let actions = parser.byte_to_actions(b'a');
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(unsafe { ke.uChar.UnicodeChar }, b'a' as u16);
                assert_eq!(ke.wRepeatCount, 1);
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn parse_enter() {
        let parser = InputParser::new();
        let actions = parser.byte_to_actions(0x0D);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.wVirtualKeyCode, 0x0D); // VK_RETURN
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn parse_ctrl_z() {
        let parser = InputParser::new();
        let actions = parser.byte_to_actions(0x1A);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.wVirtualKeyCode, b'Z' as u16);
                assert_eq!(ke.dwControlKeyState & 0x0008, 0x0008); // LEFT_CTRL_PRESSED
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn parse_arrow_keys_csi() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());
        let actions = parser.parse(b"\x1b[A", conin);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.wVirtualKeyCode, 0x26); // VK_UP
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn parse_arrow_keys_ss3() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());
        let actions = parser.parse(b"\x1bOA", conin);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.wVirtualKeyCode, 0x26); // VK_UP
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn parse_ctrl_up_modifier() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());
        let actions = parser.parse(b"\x1b[1;5A", conin); // Ctrl+Up
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.wVirtualKeyCode, 0x26); // VK_UP
                assert_eq!(ke.dwControlKeyState & 0x0008, 0x0008); // LEFT_CTRL_PRESSED
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn parse_ctrl_alt_up_modifier() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());
        let actions = parser.parse(b"\x1b[1;7A", conin); // Ctrl+Alt+Up
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.wVirtualKeyCode, 0x26); // VK_UP
                assert_eq!(ke.dwControlKeyState & 0x0008, 0x0008); // LEFT_CTRL_PRESSED
                assert_eq!(ke.dwControlKeyState & 0x0002, 0x0002); // LEFT_ALT_PRESSED
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn parse_function_keys() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());

        // F5 = \x1b[15~
        let actions = parser.parse(b"\x1b[15~", conin);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.wVirtualKeyCode, 0x74); // VK_F5
            }
            _ => panic!("expected KeyEvent for F5"),
        }

        // F12 = \x1b[24~
        let actions = parser.parse(b"\x1b[24~", conin);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.wVirtualKeyCode, 0x7B); // VK_F12
            }
            _ => panic!("expected KeyEvent for F12"),
        }
    }

    #[test]
    fn parse_standalone_escape() {
        let parser = InputParser::new();
        let actions = parser.byte_to_actions(0x1B);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(unsafe { ke.uChar.UnicodeChar }, 0x1B);
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn parse_alt_letter() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());
        let actions = parser.parse(b"\x1bf", conin); // Alt+f
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.dwControlKeyState & 0x0002, 0x0002); // LEFT_ALT_PRESSED
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn expand_key_event_produces_down_up() {
        let mut ke: KEY_EVENT_RECORD = unsafe { zeroed() };
        ke.wVirtualKeyCode = 0x41; // 'A'
        ke.wRepeatCount = 1;
        ke.uChar.UnicodeChar = b'a' as u16;
        let records = expand_key_event(&ke);
        assert_eq!(records.len(), 2);
        // First is key-down
        unsafe {
            assert_eq!(records[0].Event.KeyEvent.bKeyDown.0, 1);
            assert_eq!(records[1].Event.KeyEvent.bKeyDown.0, 0);
        }
    }

    #[test]
    fn parse_cross_chunk_buffering() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());

        // First chunk: just ESC
        let actions1 = parser.parse(b"\x1b", conin);
        assert!(actions1.is_empty(), "ESC alone should be buffered");
        assert!(parser.has_pending());

        // Second chunk: completes the sequence
        let actions2 = parser.parse(b"[A", conin);
        assert_eq!(actions2.len(), 1);
        match &actions2[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.wVirtualKeyCode, 0x26); // VK_UP
            }
            _ => panic!("expected KeyEvent"),
        }
        assert!(!parser.has_pending());
    }

    #[test]
    fn flush_pending_emits_esc() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());

        // Buffer an ESC
        let _ = parser.parse(b"\x1b", conin);
        assert!(parser.has_pending());

        let actions = parser.flush_pending();
        assert!(!actions.is_empty());
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(unsafe { ke.uChar.UnicodeChar }, 0x1B);
            }
            _ => panic!("expected ESC KeyEvent"),
        }
        assert!(!parser.has_pending());
    }

    // ── UTF-8 decoding in InputParser ────────────────────────────────

    #[test]
    fn parse_utf8_two_byte_char() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());
        // é = U+00E9 = UTF-8 0xC3 0xA9
        let actions = parser.parse(&[0xC3, 0xA9], conin);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.wVirtualKeyCode, 0xE7); // VK_PACKET
                assert_eq!(unsafe { ke.uChar.UnicodeChar }, 0x00E9); // é
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn parse_utf8_three_byte_char() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());
        // € = U+20AC = UTF-8 0xE2 0x82 0xAC
        let actions = parser.parse(&[0xE2, 0x82, 0xAC], conin);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(ke.wVirtualKeyCode, 0xE7); // VK_PACKET
                assert_eq!(unsafe { ke.uChar.UnicodeChar }, 0x20AC); // €
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn parse_utf8_four_byte_char_produces_surrogate_pair() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());
        // 🎉 = U+1F389 = UTF-8 0xF0 0x9F 0x8E 0x89
        let actions = parser.parse(&[0xF0, 0x9F, 0x8E, 0x89], conin);
        // U+1F389 > U+FFFF, so vk_packet_for_char produces 2 VK_PACKET events (surrogate pair)
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn parse_utf8_split_across_chunks() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());
        // é = 0xC3 0xA9, split across two parse() calls
        let actions1 = parser.parse(&[0xC3], conin);
        assert!(actions1.is_empty(), "incomplete UTF-8 should be buffered");
        assert!(parser.has_pending());

        let actions2 = parser.parse(&[0xA9], conin);
        assert_eq!(actions2.len(), 1);
        match &actions2[0] {
            InputAction::KeyEvent(ke) => {
                assert_eq!(unsafe { ke.uChar.UnicodeChar }, 0x00E9);
            }
            _ => panic!("expected KeyEvent"),
        }
    }

    #[test]
    fn parse_mixed_ascii_and_utf8() {
        let mut parser = InputParser::new();
        let conin = HANDLE(std::ptr::null_mut());
        // "aé" = 0x61 0xC3 0xA9
        let actions = parser.parse(&[0x61, 0xC3, 0xA9], conin);
        assert_eq!(actions.len(), 2); // 'a' + 'é'
    }

    // ── decode_char_info_row ────────────────────────────────────────

    #[test]
    fn decode_simple_ascii_row() {
        let mut ci: CHAR_INFO = unsafe { zeroed() };
        ci.Char.UnicodeChar = b'H' as u16;
        ci.Attributes = 0x07;

        let mut ci2: CHAR_INFO = unsafe { zeroed() };
        ci2.Char.UnicodeChar = b'i' as u16;
        ci2.Attributes = 0x07;

        let cells = decode_char_info_row(&[ci, ci2]);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].ch, 'H');
        assert_eq!(cells[0].width, 1);
        assert_eq!(cells[1].ch, 'i');
    }
}
