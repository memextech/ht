/// Tests for PTY buffer overflow issues when sending large heredocs
/// 
/// This test suite reproduces the issue documented in:
/// - HEREDOC_INVESTIGATION_SUMMARY.md
/// - HEREDOC_ROOT_CAUSE_CONFIRMED.md
/// 
/// ## Issue Summary
/// When sending large heredocs (>~1500 characters) through the PTY interface,
/// text gets scrambled and corrupted due to PTY write buffer overflow.
/// 
/// ## Root Cause
/// The PTY master has a limited kernel buffer (typically 4096 bytes on Linux/macOS).
/// When we write data faster than the shell can read it, the buffer fills up and
/// subsequent writes either block, fail, or cause data to be lost/corrupted.
///
/// ## Reproduction
/// These tests attempt to:
/// 1. Send increasingly large heredoc commands through the PTY
/// 2. Verify that output is received in the correct order
/// 3. Demonstrate failure when size exceeds PTY buffer capacity

use ht_core::command::{Command, InputSeq};
use ht_core::pty;
use nix::pty::Winsize;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

/// Helper to create a heredoc command of a specific size
fn create_heredoc_command(content_size: usize) -> String {
    let padding = "x".repeat(content_size);
    format!(
        r#"cat <<'EOF'
{}
EOF
"#,
        padding
    )
}

/// Helper to create a complex heredoc with markdown and emojis (similar to gh pr create)
fn create_complex_heredoc(content_size: usize) -> String {
    let mut content = String::from(
        r#"🎉 Fix: UserData validation errors in Cloud Run backend

## Problem 🐛
The Cloud Run backend was experiencing validation errors.

## Solution ✨
- **Added field aliases**: All API key fields now have underscore aliases
- **Added default values**: Missing fields get sensible defaults
- **Updated validation**: More lenient validation rules

## Technical Details 📋

### Root Cause
```python
pydantic_core._pydantic_core.ValidationError: 2 validation errors
```

### Code Changes
"#,
    );

    // Pad to desired size with repeated text
    while content.len() < content_size {
        content.push_str(
            r#"
- Additional detail line to increase payload size
- More implementation details here
- Testing buffer overflow scenarios
"#,
        );
    }

    format!(
        r#"git commit -m "$(cat <<'EOF'
{}
EOF
)"
"#,
        content
    )
}

/// Test small heredoc (should work fine)
#[tokio::test]
async fn test_small_heredoc_success() {
    let command = create_heredoc_command(100);
    let result = run_command_with_pty(command).await;

    assert!(result.is_ok(), "Small heredoc should succeed");
    let output = result.unwrap();
    assert!(
        output.contains("xxxx"),
        "Output should contain the heredoc content"
    );
}

/// Test medium heredoc (~500 chars, should work)
#[tokio::test]
async fn test_medium_heredoc_success() {
    let command = create_heredoc_command(500);
    let result = run_command_with_pty(command).await;

    assert!(result.is_ok(), "Medium heredoc (500 chars) should succeed");
    let output = result.unwrap();
    assert_eq!(
        output.matches('x').count(),
        500,
        "Should receive all 500 characters"
    );
}

/// Test large heredoc (~1500 chars, at the edge of failure)
#[tokio::test]
async fn test_large_heredoc_at_limit() {
    let command = create_heredoc_command(1500);
    let result = run_command_with_pty(command).await;

    assert!(
        result.is_ok(),
        "Large heredoc (1500 chars) should still work"
    );

    let output = result.unwrap();
    let x_count = output.matches('x').count();

    // This test documents the boundary - at 1500 chars we start seeing issues
    // It might pass or fail depending on system buffer sizes
    if x_count != 1500 {
        eprintln!(
            "WARNING: Expected 1500 'x' characters but got {}. Buffer overflow may be occurring.",
            x_count
        );
    }
}

/// Test very large heredoc (~2000 chars, should fail with current implementation)
#[tokio::test]
#[should_panic(expected = "Buffer overflow")]
async fn test_very_large_heredoc_fails() {
    let command = create_heredoc_command(2000);
    let result = run_command_with_pty(command).await;

    match result {
        Ok(output) => {
            let x_count = output.matches('x').count();
            if x_count != 2000 {
                panic!(
                    "Buffer overflow: Expected 2000 chars but got {}. Data was lost or corrupted.",
                    x_count
                );
            }
        }
        Err(e) => {
            panic!("Buffer overflow: Command failed with error: {}", e);
        }
    }
}

/// Test complex heredoc with markdown and emojis (mimics gh pr create)
#[tokio::test]
#[should_panic(expected = "Buffer overflow")]
async fn test_complex_heredoc_with_markdown_fails() {
    let command = create_complex_heredoc(1800);
    let result = run_command_with_pty(command).await;

    match result {
        Ok(output) => {
            // Check if output is scrambled or incomplete
            let has_emoji = output.contains("🎉");
            let has_markdown = output.contains("##");
            let has_code_block = output.contains("```");

            if !has_emoji || !has_markdown || !has_code_block {
                panic!(
                    "Buffer overflow: Output is incomplete or corrupted. \
                     Emoji: {}, Markdown: {}, Code: {}",
                    has_emoji, has_markdown, has_code_block
                );
            }

            // Check for scrambled text (characteristic of buffer overflow)
            if output.contains("dquote cmdsubst heredoc>") {
                panic!(
                    "Buffer overflow: Shell stuck in heredoc prompt, indicating corrupted input"
                );
            }
        }
        Err(e) => {
            panic!("Buffer overflow: Command failed with error: {}", e);
        }
    }
}

/// Test rapid fire multiple commands (tests buffer management under load)
#[tokio::test]
async fn test_rapid_multiple_commands() {
    let winsize = Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let (input_tx, input_rx) = mpsc::channel(100);
    let (output_tx, mut output_rx) = mpsc::channel(100);

    let command = "/bin/sh".to_string();
    let pty_future = pty::spawn(command, winsize, input_rx, output_tx).unwrap();

    // Spawn PTY driver
    tokio::spawn(pty_future);

    // Send multiple commands rapidly
    for i in 0..10 {
        let cmd = format!("echo 'command {}'\n", i);
        input_tx.send(cmd.into_bytes()).await.unwrap();
    }

    // Collect output with timeout
    let mut outputs = Vec::new();
    for _ in 0..10 {
        match timeout(Duration::from_secs(2), output_rx.recv()).await {
            Ok(Some(data)) => outputs.push(String::from_utf8_lossy(&data).to_string()),
            Ok(None) => break,
            Err(_) => {
                eprintln!("Timeout waiting for command output");
                break;
            }
        }
    }

    let combined = outputs.join("");

    // Verify all commands executed
    for i in 0..10 {
        let expected = format!("command {}", i);
        assert!(
            combined.contains(&expected),
            "Output should contain '{}', but got: {}",
            expected,
            combined
        );
    }
}

/// Test input command payload size limit
#[tokio::test]
async fn test_input_command_large_payload() {
    // Create an InputCommand with a very large payload
    let large_text = "x".repeat(3000);
    let input_seqs = vec![InputSeq::Standard(large_text.clone())];

    // This tests the Command::Input path directly
    let command = Command::Input(input_seqs);

    // Convert to bytes as the PTY would
    let bytes = match command {
        Command::Input(seqs) => ht_core::command::seqs_to_bytes(&seqs, false),
        _ => panic!("Expected Input command"),
    };

    assert_eq!(
        bytes.len(),
        3000,
        "Bytes should match input length before PTY write"
    );

    // Now test actual PTY write
    let result = run_command_bytes_with_pty(bytes).await;

    // This should demonstrate the buffer overflow
    match result {
        Ok(output) => {
            let x_count = output.matches('x').count();
            if x_count < 3000 {
                eprintln!(
                    "Buffer overflow detected: Sent 3000 chars, received {} chars",
                    x_count
                );
                // Don't panic here, just document the issue
            }
        }
        Err(e) => {
            eprintln!("PTY write failed: {}", e);
        }
    }
}

/// Test incremental writes vs bulk write
#[tokio::test]
async fn test_incremental_writes_vs_bulk() {
    let large_text = "y".repeat(2000);

    // Test 1: Bulk write (current behavior)
    let bulk_result = run_command_with_pty(format!("cat <<'EOF'\n{}\nEOF\n", large_text)).await;

    // Test 2: Incremental writes (chunked)
    let chunk_result = run_command_chunked_with_pty(large_text.clone(), 100).await;

    // Compare results
    match (bulk_result, chunk_result) {
        (Ok(bulk_output), Ok(chunk_output)) => {
            let bulk_count = bulk_output.matches('y').count();
            let chunk_count = chunk_output.matches('y').count();

            println!("Bulk write received: {} chars", bulk_count);
            println!("Chunked write received: {} chars", chunk_count);

            // Chunked writes should be more reliable
            assert!(
                chunk_count >= bulk_count,
                "Chunked writes should be at least as reliable as bulk writes"
            );
        }
        _ => {
            eprintln!("One or both write methods failed");
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Run a command through the PTY and collect output
async fn run_command_with_pty(command: String) -> Result<String, String> {
    let winsize = Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let (input_tx, input_rx) = mpsc::channel(100);
    let (output_tx, mut output_rx) = mpsc::channel(100);

    let shell_command = "/bin/sh".to_string();
    let pty_future = pty::spawn(shell_command, winsize, input_rx, output_tx)
        .map_err(|e| format!("Failed to spawn PTY: {}", e))?;

    // Spawn PTY driver  
    let pty_handle = tokio::spawn(pty_future);

    // Send command
    input_tx
        .send(command.into_bytes())
        .await
        .map_err(|e| format!("Failed to send command: {}", e))?;

    // Send exit command
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = input_tx.send(b"exit\n".to_vec()).await;

    // Collect output with timeout
    let mut output = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(100), output_rx.recv()).await {
            Ok(Some(data)) => {
                output.push_str(&String::from_utf8_lossy(&data));
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    // Drop sender to close input channel
    drop(input_tx);

    // Wait for PTY to finish (with timeout)
    let _ = timeout(Duration::from_secs(2), pty_handle).await;

    Ok(output)
}

/// Run command bytes directly through PTY
async fn run_command_bytes_with_pty(bytes: Vec<u8>) -> Result<String, String> {
    let winsize = Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let (input_tx, input_rx) = mpsc::channel(100);
    let (output_tx, mut output_rx) = mpsc::channel(100);

    let command = "/bin/sh".to_string();
    let pty_future = pty::spawn(command, winsize, input_rx, output_tx)
        .map_err(|e| format!("Failed to spawn PTY: {}", e))?;

    let pty_handle = tokio::spawn(pty_future);

    // Send bytes
    input_tx
        .send(bytes)
        .await
        .map_err(|e| format!("Failed to send bytes: {}", e))?;

    // Add newline and exit
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = input_tx.send(b"\nexit\n".to_vec()).await;

    // Collect output
    let mut output = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(100), output_rx.recv()).await {
            Ok(Some(data)) => {
                output.push_str(&String::from_utf8_lossy(&data));
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    drop(input_tx);
    let _ = timeout(Duration::from_secs(2), pty_handle).await;

    Ok(output)
}

/// Run command with chunked writes (simulates a potential fix)
async fn run_command_chunked_with_pty(text: String, chunk_size: usize) -> Result<String, String> {
    let winsize = Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let (input_tx, input_rx) = mpsc::channel(100);
    let (output_tx, mut output_rx) = mpsc::channel(100);

    let command = "/bin/sh".to_string();
    let pty_future = pty::spawn(command, winsize, input_rx, output_tx)
        .map_err(|e| format!("Failed to spawn PTY: {}", e))?;

    let pty_handle = tokio::spawn(pty_future);

    // Send command start
    input_tx
        .send(b"cat <<'EOF'\n".to_vec())
        .await
        .map_err(|e| format!("Failed to send command start: {}", e))?;

    // Send text in chunks with small delays
    for chunk in text.as_bytes().chunks(chunk_size) {
        input_tx
            .send(chunk.to_vec())
            .await
            .map_err(|e| format!("Failed to send chunk: {}", e))?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Send command end
    input_tx
        .send(b"\nEOF\n".to_vec())
        .await
        .map_err(|e| format!("Failed to send EOF: {}", e))?;

    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = input_tx.send(b"exit\n".to_vec()).await;

    // Collect output
    let mut output = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(100), output_rx.recv()).await {
            Ok(Some(data)) => {
                output.push_str(&String::from_utf8_lossy(&data));
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    drop(input_tx);
    let _ = timeout(Duration::from_secs(2), pty_handle).await;

    Ok(output)
}
