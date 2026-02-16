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
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
#[cfg(windows)]
use windows::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
#[cfg(windows)]
use windows::Win32::System::Pipes::CreatePipe;
#[cfg(windows)]
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTUPINFOEXW, STARTUPINFOW,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};
#[cfg(windows)]
use windows::core::PWSTR;

/// A `Send`-safe wrapper for Windows handles (`HANDLE` / `HPCON`).
///
/// In the `windows` 0.58.0 crate `HANDLE` wraps `*mut c_void` (which is
/// `!Send`), while `HPCON` still wraps `isize`.  Windows handles are plain
/// integer-like tokens that are safe to use from any thread, so we store
/// the value as a single `isize` (which *is* `Send`) and reconstruct the
/// original type on demand.
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
    fn from_hpcon(h: HPCON) -> Self {
        Self(h.0)
    }
    fn to_hpcon(self) -> HPCON {
        HPCON(self.0)
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
#[cfg(windows)]
const READ_BUF_SIZE: usize = 128 * 1024;

#[cfg(windows)]
struct ConPty {
    hpc: HPCON,
    input_write: Option<OwnedHandle>,
    output_read: Option<OwnedHandle>,
    proc_handle: Option<OwnedHandle>,
    thread_handle: Option<OwnedHandle>,
    attr_list_buf: Vec<u8>,
}

#[cfg(windows)]
// Safety: The only non-Send field is HPCON (wraps isize; the windows crate
// does not impl Send for it). Windows console handles are plain integer tokens
// that are safe to use from any thread. All other fields (OwnedHandle, Vec<u8>)
// are already Send.
unsafe impl Send for ConPty {}

#[cfg(windows)]
impl Drop for ConPty {
    fn drop(&mut self) {
        // 1. Close input_write if still held (not yet taken by write thread).
        drop(self.input_write.take());

        // 2. ClosePseudoConsole — only if drive() didn't already do it.
        //    drive() zeroes hpc after calling ClosePseudoConsole in spawn_blocking.
        //    In the abort path (Drop called without drive() completing),
        //    we must call it here.
        if self.hpc.0 != 0 {
            unsafe {
                ClosePseudoConsole(self.hpc);
            }
        }

        // 3. Close output_read if still held (not yet taken by read thread).
        //    In the normal path, drive() moves output_read into the read thread.
        //    In the abort path, this closes it here.
        drop(self.output_read.take());

        // 4. proc_handle, thread_handle (OwnedHandle) dropped automatically.
        drop(self.proc_handle.take());
        drop(self.thread_handle.take());

        // 5. DeleteProcThreadAttributeList:
        if !self.attr_list_buf.is_empty() {
            unsafe {
                let attr_list =
                    LPPROC_THREAD_ATTRIBUTE_LIST(self.attr_list_buf.as_mut_ptr() as *mut c_void);
                DeleteProcThreadAttributeList(attr_list);
            }
        }
    }
}

/// Escapes a single argument for a Windows command line following msvcrt conventions.
/// This replicates the logic from std::sys::windows::args::append_arg.
#[cfg(windows)]
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

#[cfg(windows)]
impl ConPty {
    fn new(winsize: Winsize, command: &str) -> Result<Self> {
        unsafe {
            // 1. Create pipe pairs — wrap each end immediately
            let (mut input_read_raw, mut input_write_raw) = (HANDLE::default(), HANDLE::default());
            let (mut output_read_raw, mut output_write_raw) =
                (HANDLE::default(), HANDLE::default());
            CreatePipe(&mut input_read_raw, &mut input_write_raw, None, 0)?;
            let input_read = OwnedHandle::from_raw_handle(input_read_raw.0 as *mut _);
            let input_write = OwnedHandle::from_raw_handle(input_write_raw.0 as *mut _);
            CreatePipe(&mut output_read_raw, &mut output_write_raw, None, 0)?;
            let output_read = OwnedHandle::from_raw_handle(output_read_raw.0 as *mut _);
            let output_write = OwnedHandle::from_raw_handle(output_write_raw.0 as *mut _);

            // 2. Create pseudo-console
            let size = COORD {
                X: winsize.ws_col.min(i16::MAX as u16) as i16,
                Y: winsize.ws_row.min(i16::MAX as u16) as i16,
            };
            let hpc = CreatePseudoConsole(
                size,
                HANDLE(input_read.as_raw_handle()),
                HANDLE(output_write.as_raw_handle()),
                0,
            )?;

            // 3. Close pipe ends given to ConPTY (it duplicated them)
            drop(input_read);
            drop(output_write);

            // Build partial ConPty immediately so Drop covers hpc on any later failure
            let mut conpty = ConPty {
                hpc,
                input_write: Some(input_write),
                output_read: Some(output_read),
                proc_handle: None,
                thread_handle: None,
                attr_list_buf: vec![],
            };

            // 4. Initialize proc thread attribute list (two-call pattern)
            let mut attr_list_size: usize = 0;
            let _ = InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
                1,
                0,
                &mut attr_list_size,
            );
            conpty.attr_list_buf = vec![0u8; attr_list_size];
            let attr_list =
                LPPROC_THREAD_ATTRIBUTE_LIST(conpty.attr_list_buf.as_mut_ptr() as *mut c_void);
            InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_list_size)?;

            // 5. Wire hpc into the attribute list
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(&conpty.hpc as *const HPCON as *const c_void),
                size_of::<HPCON>(),
                None,
                None,
            )?;

            // 6. Build STARTUPINFOEXW referencing the attribute list
            let mut si_ex: STARTUPINFOEXW = zeroed();
            si_ex.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            si_ex.lpAttributeList = attr_list;

            // 7. Build command line + CreateProcessW
            debug_assert!(
                !command.is_empty(),
                "command should not be empty; caller provides a default"
            );
            let cmd_str = if command.is_empty() {
                "cmd.exe".to_string()
            } else {
                command.to_string()
            };
            let mut cmd_wide: Vec<u16> = cmd_str
                .encode_utf16()
                .chain(std::iter::once(0u16))
                .collect();
            let cmd_pwstr = PWSTR(cmd_wide.as_mut_ptr());

            let mut proc_info: PROCESS_INFORMATION = zeroed();

            CreateProcessW(
                None,
                cmd_pwstr,
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT,
                None,
                None,
                &si_ex.StartupInfo as *const STARTUPINFOW,
                &mut proc_info,
            )?;

            // 8. Extract process/thread handles from PROCESS_INFORMATION
            conpty.proc_handle = Some(OwnedHandle::from_raw_handle(proc_info.hProcess.0 as *mut _));
            conpty.thread_handle =
                Some(OwnedHandle::from_raw_handle(proc_info.hThread.0 as *mut _));

            Ok(conpty)
        }
    }

    async fn drive(
        mut self,
        input_rx: mpsc::Receiver<Vec<u8>>,
        output_tx: mpsc::Sender<Vec<u8>>,
        resize_rx: mpsc::Receiver<(u16, u16)>,
        initial_input: Option<Vec<u8>>,
    ) -> Result<()> {
        // Take ownership of input_write for the write thread
        let input_write = self
            .input_write
            .take()
            .ok_or_else(|| anyhow::anyhow!("input_write handle missing"))?;

        // Take ownership of output_read for the read thread
        let output_read = self
            .output_read
            .take()
            .ok_or_else(|| anyhow::anyhow!("output_read handle missing"))?;

        // Spawn write thread — takes ownership of input_write
        let mut input_rx = input_rx;
        let mut write_handle = tokio::task::spawn_blocking(move || -> Option<OwnedHandle> {
            let raw = HANDLE(input_write.as_raw_handle());

            // Inject initial input before relaying user input
            if let Some(data) = initial_input {
                let mut offset = 0;
                while offset < data.len() {
                    let mut written: u32 = 0;
                    let ok =
                        unsafe { WriteFile(raw, Some(&data[offset..]), Some(&mut written), None) };
                    if ok.is_err() || written == 0 {
                        return Some(input_write);
                    }
                    offset += written as usize;
                }
            }

            loop {
                match input_rx.blocking_recv() {
                    Some(data) => {
                        let mut offset = 0;
                        while offset < data.len() {
                            let mut written: u32 = 0;
                            let ok = unsafe {
                                WriteFile(raw, Some(&data[offset..]), Some(&mut written), None)
                            };
                            if ok.is_err() || written == 0 {
                                // WriteFile failed or pipe closed — return ownership for cleanup
                                return Some(input_write);
                            }
                            offset += written as usize;
                        }
                    }
                    None => {
                        // Channel closed (sender dropped) — drop input_write to propagate EOF
                        drop(input_write);
                        return None;
                    }
                }
            }
        });

        // Spawn read thread — takes ownership of output_read
        let mut read_handle = tokio::task::spawn_blocking(move || {
            let raw = HANDLE(output_read.as_raw_handle());
            let mut buf = vec![0u8; READ_BUF_SIZE];
            loop {
                let mut bytes_read: u32 = 0;
                let ok = unsafe { ReadFile(raw, Some(&mut buf), Some(&mut bytes_read), None) };
                if ok.is_err() || bytes_read == 0 {
                    break;
                }
                let data = buf[..bytes_read as usize].to_vec();
                if output_tx.blocking_send(data).is_err() {
                    break;
                }
            }
            drop(output_read);
        });

        // Spawn resize task
        let hpc_send = SendHandle::from_hpcon(self.hpc);
        let resize_task = tokio::spawn(resize_loop(hpc_send, resize_rx));

        // Get raw process handle for waiting.
        // Safety: self.proc_handle is not dropped until after all tasks using
        // this raw copy have been awaited (see cleanup step 4).
        let proc_send = SendHandle::from_handle(HANDLE(
            self.proc_handle
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("proc_handle missing"))?
                .as_raw_handle(),
        ));

        // Spawn a task to wait for child process exit
        let mut wait_handle = tokio::task::spawn_blocking(move || unsafe {
            WaitForSingleObject(proc_send.to_handle(), u32::MAX);
        });

        // Wait for any of the three events.
        // Use &mut references to preserve JoinHandles for cleanup.
        #[derive(PartialEq)]
        enum Finished {
            Write,
            Read,
            Wait,
        }
        let finished = tokio::select! {
            result = &mut write_handle => {
                // Write thread finished (channel closed or WriteFile failed)
                if let Ok(Some(handle)) = result {
                    drop(handle);
                }
                Finished::Write
            }
            _ = &mut read_handle => {
                // Read thread finished (EOF or pipe error)
                Finished::Read
            }
            _ = &mut wait_handle => {
                // Child process exited
                Finished::Wait
            }
        };

        // --- Cleanup sequence ---

        // 1. Abort the resize task
        resize_task.abort();

        // 2. ClosePseudoConsole in spawn_blocking (may briefly block).
        //    This breaks the output pipe (unblocking the read thread's ReadFile)
        //    and signals the child process to exit (unblocking the wait thread).
        let hpc_send = SendHandle::from_hpcon(self.hpc);
        if !hpc_send.is_null() {
            match tokio::task::spawn_blocking(move || unsafe {
                ClosePseudoConsole(hpc_send.to_hpcon());
            })
            .await
            {
                Ok(()) => {
                    // ClosePseudoConsole ran; prevent Drop from double-closing.
                    self.hpc = HPCON(0);
                }
                Err(join_err) => {
                    // Task panicked or was cancelled before ClosePseudoConsole ran.
                    // Leave self.hpc non-zero so Drop will close it.
                    return Err(join_err.into());
                }
            }
        }

        // 3. Wait for child to exit or kill it.
        //    Safety: proc_send is a copy of self.proc_handle's raw value.
        //    self.proc_handle is not dropped until step 5, after all tasks
        //    using proc_send have been awaited in step 4.
        if !proc_send.is_null() {
            tokio::task::spawn_blocking(move || unsafe {
                let proc_raw = proc_send.to_handle();
                let wait_result = WaitForSingleObject(proc_raw, 5000);
                if wait_result != WAIT_OBJECT_0 {
                    let _ = TerminateProcess(proc_raw, 1);
                    WaitForSingleObject(proc_raw, u32::MAX);
                }
            })
            .await?;
        }

        // 4. All blocking threads should now be done (ClosePseudoConsole broke
        //    the pipe and the child has exited). Await remaining JoinHandles to
        //    ensure no thread still holds a raw handle copy before we drop self.
        if finished != Finished::Write {
            let _ = write_handle.await;
        }
        if finished != Finished::Read {
            let _ = read_handle.await;
        }
        if finished != Finished::Wait {
            let _ = wait_handle.await;
        }

        // 5. Drop self — closes proc_handle/thread_handle,
        //    calls DeleteProcThreadAttributeList.
        //    output_read was already consumed by the read thread.
        drop(self);

        Ok(())
    }
}

#[cfg(windows)]
async fn resize_loop(hpc: SendHandle, mut resize_rx: mpsc::Receiver<(u16, u16)>) {
    while let Some((cols, rows)) = resize_rx.recv().await {
        let coord = COORD {
            X: cols.min(i16::MAX as u16) as i16,
            Y: rows.min(i16::MAX as u16) as i16,
        };
        unsafe {
            let _ = ResizePseudoConsole(hpc.to_hpcon(), coord);
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
    let conpty = ConPty::new(winsize, &command)?;
    Ok(conpty.drive(input_rx, output_tx, resize_rx, initial_input))
}
