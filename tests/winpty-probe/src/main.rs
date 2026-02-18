// WinPTY CI Probe
//
// Tests whether the WinPTY backend produces output in a non-interactive
// Windows session (e.g. GitHub Actions).  ConPTY is known to produce 0
// output bytes in this environment (microsoft/terminal#13914).  WinPTY
// uses a fundamentally different mechanism (hidden console + screen-scraping)
// that may bypass the limitation.
//
// Exit 0  → output received (WinPTY works)
// Exit 1  → no output / error (WinPTY has the same limitation)

#[cfg(not(windows))]
fn main() {
    eprintln!("This probe only runs on Windows.");
    std::process::exit(0);
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::ffi::OsString;
    use std::process::ExitCode;
    use std::thread;
    use std::time::{Duration, Instant};
    use winptyrs::{AgentConfig, MouseMode, PTYArgs, PTYBackend, PTY};

    println!("WinPTY CI Probe");
    println!("===============");
    println!("Testing whether WinPTY produces output in this environment...");
    println!();

    // --- Create WinPTY instance ---

    let pty_args = PTYArgs {
        cols: 80,
        rows: 24,
        mouse_mode: MouseMode::WINPTY_MOUSE_MODE_NONE,
        timeout: 10000,
        agent_config: AgentConfig::WINPTY_FLAG_COLOR_ESCAPES,
    };

    println!("[1/4] Creating WinPTY instance...");
    let mut pty = match PTY::new_with_backend(&pty_args, PTYBackend::WinPTY) {
        Ok(pty) => {
            println!("       OK (backend: {:?})", pty.get_backend());
            pty
        }
        Err(e) => {
            eprintln!("       FAILED: {:?}", e);
            return ExitCode::from(1);
        }
    };

    // --- Spawn cmd.exe ---

    println!("[2/4] Spawning cmd.exe...");
    match pty.spawn(OsString::from("cmd.exe"), None, None, None) {
        Ok(true) => println!("       OK (pid: {})", pty.get_pid()),
        Ok(false) => {
            eprintln!("       FAILED: spawn returned false");
            return ExitCode::from(1);
        }
        Err(e) => {
            eprintln!("       FAILED: {:?}", e);
            return ExitCode::from(1);
        }
    }

    // --- Read output ---

    println!("[3/4] Reading output (up to 10s)...");
    println!();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut total_bytes: usize = 0;
    let mut chunk_count: u32 = 0;
    let mut all_output = String::new();
    let mut process_exited = false;

    while Instant::now() < deadline {
        // Check if process is still alive
        match pty.is_alive() {
            Ok(false) => {
                println!("       Process exited (exit status: {:?})", pty.get_exitstatus());
                process_exited = true;
                break;
            }
            Err(e) => {
                eprintln!("       is_alive error: {:?}", e);
                break;
            }
            Ok(true) => {}
        }

        match pty.read(false) {
            Ok(output) => {
                let text = output.to_string_lossy();
                if !text.is_empty() {
                    chunk_count += 1;
                    total_bytes += text.len();
                    all_output.push_str(&text);
                    // Show chunks in CI logs for diagnostics
                    println!("       chunk {}: {} bytes", chunk_count, text.len());
                }
            }
            Err(e) => {
                let err_str = e.to_string_lossy();
                // WinPTY may return an error for "no data available" on
                // non-blocking reads — only break on real errors.
                if !err_str.is_empty() {
                    eprintln!("       read error: {:?}", err_str);
                }
                // Don't break — keep trying until deadline
            }
        }

        thread::sleep(Duration::from_millis(100));
    }

    // --- Report ---

    println!();
    println!("[4/4] Results");
    println!("       Total bytes received: {}", total_bytes);
    println!("       Total chunks: {}", chunk_count);
    println!("       Process exited early: {}", process_exited);
    if total_bytes > 0 {
        // Show first 500 chars of output for diagnostics
        let preview: String = all_output.chars().take(500).collect();
        println!("       Output preview:");
        for line in preview.lines() {
            println!("         | {}", line);
        }
    }

    // --- Cleanup ---

    let _ = pty.write(OsString::from("exit\r\n"));
    thread::sleep(Duration::from_millis(500));

    // --- Verdict ---

    println!();
    if total_bytes > 0 {
        println!("SUCCESS: WinPTY produced {} bytes of output.", total_bytes);
        println!("WinPTY works in this environment!");
        ExitCode::from(0)
    } else {
        println!("FAILURE: WinPTY produced 0 bytes of output.");
        if process_exited {
            println!("The child process exited immediately (same behavior as ConPTY).");
        } else {
            println!("The child process is alive but produced no output.");
        }
        ExitCode::from(1)
    }
}
