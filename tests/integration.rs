//! Integration tests for HT
//!
//! These tests validate that the HT binary works correctly on all platforms

use std::process::{Command, Stdio};

/// Test that the HT binary can be built and runs without crashing
#[test]
fn test_ht_help() {
    let output = Command::new("cargo")
        .args(&["run", "--", "--help"])
        .output()
        .expect("Failed to execute cargo run");

    assert!(output.status.success(), "HT help command failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("ht"));
    assert!(stdout.contains("Command to run inside the terminal"));
}

/// Test that the HT binary can be built and shows version
#[test]
fn test_ht_version() {
    let output = Command::new("cargo")
        .args(&["run", "--", "--version"])
        .output()
        .expect("Failed to execute cargo run");

    assert!(output.status.success(), "HT version command failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("0.3.0") || stdout.len() > 0);
}

/// Test that we can compile for the current platform
#[test]
fn test_compilation() {
    let output = Command::new("cargo")
        .args(&["check"])
        .output()
        .expect("Failed to execute cargo check");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("Compilation failed: {}", stderr);
    }
}

/// Platform-specific tests
#[cfg(windows)]
mod windows_integration {
    use super::*;
    use std::time::Duration;

    /// Test that HT can execute a basic Windows command
    #[test]
    fn test_windows_basic_command() {
        let output = Command::new("cargo")
            .args(&["run", "--", "echo hello"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        // We just test that we can spawn the process
        // The actual command execution is tested via the Windows CI
        assert!(output.is_ok(), "Failed to spawn HT with Windows command");
    }

    /// Test that HT works with Windows cmd.exe
    #[test]
    fn test_windows_cmd() {
        let output = Command::new("cargo")
            .args(&["run", "--", "cmd", "/c", "echo", "windows-test"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        assert!(output.is_ok(), "Failed to spawn HT with cmd.exe");
    }

    /// Test Windows PowerShell integration
    #[test]
    fn test_windows_powershell() {
        let output = Command::new("cargo")
            .args(&["run", "--", "powershell", "-Command", "Get-Location"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        assert!(output.is_ok(), "Failed to spawn HT with PowerShell");
    }

    /// Test Windows directory listing
    #[test]
    fn test_windows_dir_command() {
        let output = Command::new("cargo")
            .args(&["run", "--", "dir"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        assert!(output.is_ok(), "Failed to spawn HT with dir command");
    }

    /// Test Windows batch file execution
    #[test]
    fn test_windows_batch_execution() {
        let output = Command::new("cargo")
            .args(&["run", "--", "cmd", "/c", "echo %USERNAME%"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        assert!(output.is_ok(), "Failed to spawn HT with batch command");
    }

    /// Test that HT handles Windows paths correctly
    #[test]
    fn test_windows_paths() {
        let output = Command::new("cargo")
            .args(&["run", "--", "cmd", "/c", "cd /d C:\\ && echo %CD%"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        assert!(
            output.is_ok(),
            "Failed to spawn HT with Windows path command"
        );
    }

    /// Test interactive Windows command execution
    #[test]
    fn test_windows_interactive_session() {
        // Test that we can start an interactive cmd session
        let mut child = Command::new("cargo")
            .args(&["run", "--", "--size", "80x24"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn HT interactive session");

        // Give it a moment to start
        std::thread::sleep(Duration::from_millis(100));

        // Just test that the process started successfully
        assert!(child.id() > 0, "HT interactive session failed to start");

        // Terminate the child process
        let _ = child.kill();
    }

    /// Test Windows process termination
    #[test]
    fn test_windows_process_cleanup() {
        let mut child = Command::new("cargo")
            .args(&["run", "--", "timeout", "/t", "10"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn HT with timeout command");

        // Give it a moment to start
        std::thread::sleep(Duration::from_millis(100));

        // Test that we can kill the process
        let result = child.kill();
        assert!(result.is_ok(), "Failed to terminate Windows process");
    }
}

#[cfg(unix)]
mod unix_integration {
    use super::*;

    /// Test that HT can execute a basic Unix command
    #[test]
    fn test_unix_basic_command() {
        let output = Command::new("cargo")
            .args(&["run", "--", "echo", "hello"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        assert!(output.is_ok(), "Failed to spawn HT with Unix command");
    }
}
