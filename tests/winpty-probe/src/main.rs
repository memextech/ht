// WinPTY-style CI Probe
//
// Tests whether a hidden-console + screen-scraping approach (the technique
// WinPTY uses) produces output in a non-interactive Windows session such as
// GitHub Actions.  ConPTY is known to produce 0 output bytes in this
// environment (microsoft/terminal#13914).
//
// The probe uses raw Windows console APIs — no external DLLs required.
//
// Exit 0  → console output received (WinPTY-style approach works)
// Exit 1  → no output / error (same limitation as ConPTY)

#[cfg(not(windows))]
fn main() {
    eprintln!("This probe only runs on Windows.");
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::process::ExitCode;
    match run() {
        Ok(true) => {
            println!();
            println!("SUCCESS: hidden-console screen-scraping works!");
            ExitCode::from(0)
        }
        Ok(false) => {
            println!();
            println!("FAILURE: no console output received.");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!();
            eprintln!("ERROR: {:?}", e);
            ExitCode::from(1)
        }
    }
}

#[cfg(windows)]
fn run() -> Result<bool, Box<dyn std::error::Error>> {
    use std::mem::zeroed;
    use std::thread;
    use std::time::Duration;

    use windows::Win32::Foundation::*;
    use windows::Win32::System::Console::*;
    use windows::Win32::System::Threading::*;

    println!("WinPTY-style CI Probe");
    println!("=====================");
    println!("Testing hidden-console + screen-scraping in this environment...");
    println!();

    // --- Step 1: Spawn cmd.exe with its own (hidden) console ---

    println!("[1/5] Spawning cmd.exe with CREATE_NEW_CONSOLE...");

    let mut si: STARTUPINFOW = unsafe { zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { zeroed() };

    // "cmd.exe /k prompt PROBE$G$S" — /k keeps cmd alive, prompt sets a marker
    let mut cmd_line: Vec<u16> = "cmd.exe /k prompt PROBE$G$S\0"
        .encode_utf16()
        .collect();

    unsafe {
        CreateProcessW(
            None,
            PWSTR(cmd_line.as_mut_ptr()),
            None,
            None,
            false,
            CREATE_NEW_CONSOLE,
            None,
            None,
            &si,
            &mut pi,
        )?;
    }

    let child_pid = pi.dwProcessId;
    println!("       OK (pid: {})", child_pid);

    // --- Step 2: Wait for the shell to initialize ---

    println!("[2/5] Waiting 3s for shell to initialize...");
    thread::sleep(Duration::from_secs(3));

    // Check if child is still alive
    let wait_result = unsafe { WaitForSingleObject(pi.hProcess, 0) };
    if wait_result == WAIT_OBJECT_0 {
        println!("       Child process already exited!");
        unsafe {
            let _ = CloseHandle(pi.hProcess);
            let _ = CloseHandle(pi.hThread);
        }
        return Ok(false);
    }
    println!("       Child still running.");

    // --- Step 3: Attach to child's console ---

    println!("[3/5] Attaching to child's console...");

    // Detach from our own console (if any — may fail in non-interactive session)
    let _ = unsafe { FreeConsole() };

    unsafe { AttachConsole(child_pid)? };
    println!("       OK — attached to child console.");

    // --- Step 4: Read the console screen buffer ---

    println!("[4/5] Reading console screen buffer...");

    let console_out = unsafe { GetStdHandle(STD_OUTPUT_HANDLE)? };

    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { zeroed() };
    unsafe { GetConsoleScreenBufferInfo(console_out, &mut info)? };

    let cols = info.dwSize.X as u32;
    let rows = info.dwSize.Y as u32;
    println!("       Buffer size: {}x{}", cols, rows);
    println!(
        "       Cursor position: ({}, {})",
        info.dwCursorPosition.X, info.dwCursorPosition.Y
    );

    // Read up to the first 25 rows of the screen buffer
    let read_rows = rows.min(25);
    let mut total_nonspace = 0usize;
    let mut screen_text = String::new();

    for row in 0..read_rows {
        let mut buf = vec![0u16; cols as usize];
        let mut chars_read: u32 = 0;
        let coord = COORD {
            X: 0,
            Y: row as i16,
        };
        unsafe {
            ReadConsoleOutputCharacterW(
                console_out,
                &mut buf,
                coord,
                &mut chars_read,
            )?;
        }
        let line = String::from_utf16_lossy(&buf[..chars_read as usize]);
        let trimmed = line.trim_end();
        if !trimmed.is_empty() {
            total_nonspace += trimmed.len();
            screen_text.push_str(trimmed);
            screen_text.push('\n');
        }
    }

    println!("       Non-space characters read: {}", total_nonspace);
    if !screen_text.is_empty() {
        println!("       Screen contents:");
        for line in screen_text.lines() {
            println!("         | {}", line);
        }
    }

    // --- Step 5: Clean up ---

    println!("[5/5] Cleaning up...");

    let _ = unsafe { FreeConsole() };
    unsafe {
        let _ = TerminateProcess(pi.hProcess, 0);
        WaitForSingleObject(pi.hProcess, 5000);
        let _ = CloseHandle(pi.hProcess);
        let _ = CloseHandle(pi.hThread);
    }

    Ok(total_nonspace > 0)
}
