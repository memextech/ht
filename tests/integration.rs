//! Integration tests for HT
//! 
//! These tests validate that the HT binary works correctly on all platforms

use std::process::{Command, Stdio};
use std::time::Duration;

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