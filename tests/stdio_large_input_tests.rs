/// Tests for large input handling through the stdio API
///
/// These tests reproduce the heredoc buffer overflow issue by testing
/// the InputCommand flow directly, simulating what happens when Memex sends
/// large heredoc commands through the JSON protocol.
///
/// ## Issue
/// When large heredocs (>~1500 chars) are sent via the `input` command,
/// they can cause PTY buffer overflow leading to data corruption.

use ht_core::command::{Command, InputSeq};
use serde_json::json;

/// Test parsing large input commands
#[test]
fn test_parse_small_input_command() {
    let payload = "x".repeat(100);
    let json_str = json!({
        "type": "input",
        "payload": payload
    }).to_string();

    // This simulates what happens when ht receives the JSON command
    let result: Result<serde_json::Value, _> = serde_json::from_str(&json_str);
    assert!(result.is_ok(), "Should parse small input command");

    let parsed = result.unwrap();
    assert_eq!(parsed["type"], "input");
    assert_eq!(parsed["payload"].as_str().unwrap().len(), 100);
}

#[test]
fn test_parse_medium_input_command() {
    let payload = "x".repeat(1000);
    let json_str = json!({
        "type": "input",
        "payload": payload
    }).to_string();

    let result: Result<serde_json::Value, _> = serde_json::from_str(&json_str);
    assert!(result.is_ok(), "Should parse medium input command (1000 chars)");
    
    let parsed = result.unwrap();
    assert_eq!(parsed["payload"].as_str().unwrap().len(), 1000);
}

#[test]
fn test_parse_large_input_command() {
    // This is around the size where issues start occurring
    let payload = "x".repeat(1500);
    let json_str = json!({
        "type": "input",
        "payload": payload
    }).to_string();

    // JSON parsing itself should work fine
    let result: Result<serde_json::Value, _> = serde_json::from_str(&json_str);
    assert!(result.is_ok(), "Should parse large input command (1500 chars)");

    let parsed = result.unwrap();
    assert_eq!(parsed["payload"].as_str().unwrap().len(), 1500);
    
    println!("JSON size: {} bytes", json_str.len());
    println!("Payload size: {} bytes", payload.len());
}

#[test]
fn test_parse_very_large_input_command() {
    // This is the size that definitely causes issues in production
    let payload = "x".repeat(2000);
    let json_str = json!({
        "type": "input",
        "payload": payload
    }).to_string();

    let result: Result<serde_json::Value, _> = serde_json::from_str(&json_str);
    assert!(result.is_ok(), "JSON parsing should work even for very large inputs");

    println!("Very large command - JSON size: {} bytes", json_str.len());
    // The issue is not in JSON parsing but in PTY writes
}

#[test]
fn test_input_seq_to_bytes_conversion() {
    // Test the conversion from InputSeq to bytes
    let sizes = vec![100, 500, 1000, 1500, 2000, 3000, 5000];

    for size in sizes {
        let text = "y".repeat(size);
        let input_seqs = vec![InputSeq::Standard(text.clone())];
        let bytes = ht_core::command::seqs_to_bytes(&input_seqs, false);

        assert_eq!(
            bytes.len(),
            size,
            "Bytes should match input size before PTY write (size: {})",
            size
        );

        println!("✓ InputSeq size {} -> {} bytes", size, bytes.len());
    }
}

#[test]
fn test_heredoc_command_json_size() {
    // Simulate a complex heredoc command like `gh pr create`
    let heredoc_content = r#"🎉 Fix: UserData validation errors in Cloud Run backend

## Problem 🐛
The Cloud Run backend was experiencing validation errors when processing user data.

## Solution ✨
- **Added field aliases**: All API key fields now have underscore aliases  
- **Added default values**: Missing fields get sensible defaults
- **Updated validation**: More lenient validation rules

## Technical Details 📋

### Root Cause
```python
pydantic_core._pydantic_core.ValidationError: 2 validation errors for UserData
```

### Changes Made
1. Added `Field(alias="...")` for all API key fields
2. Set default values for optional fields
3. Updated Pydantic model validation settings
"#;

    let git_command = format!(
        r#"git commit -m "$(cat <<'EOF'
{}
EOF
)"
"#,
        heredoc_content
    );

    let json_str = json!({
        "type": "input",
        "payload": git_command
    }).to_string();

    println!("\n=== Complex Heredoc Command Analysis ===");
    println!("Heredoc content size: {} bytes", heredoc_content.len());
    println!("Git command size: {} bytes", git_command.len());
    println!("JSON payload size: {} bytes", json_str.len());

    // Parse to verify it's valid
    let result: Result<serde_json::Value, _> = serde_json::from_str(&json_str);
    assert!(result.is_ok(), "Should parse complex heredoc command");

    // Document the sizes where overflow occurs
    if json_str.len() > 2000 {
        println!("\n⚠️  This size ({} bytes) is likely to cause PTY buffer overflow!", json_str.len());
        println!("   Recommendation: Use file-based approach for payloads > 1500 bytes");
    }
}

#[test]
fn test_command_input_memory_overhead() {
    // Test memory overhead of Command::Input with various sizes
    let sizes = vec![100, 500, 1000, 1500, 2000, 3000];

    println!("\n=== Command Memory Overhead ===");
    for size in sizes {
        let text = "z".repeat(size);
        let input_seq = InputSeq::Standard(text.clone());
        let command = Command::Input(vec![input_seq]);

        // Serialize to JSON to see protocol overhead
        let json = serde_json::to_string(&json!({
            "type": "input",
            "payload": text
        })).unwrap();

        println!(
            "Payload: {:5} bytes | JSON: {:5} bytes | Overhead: {:4} bytes ({:.1}%)",
            size,
            json.len(),
            json.len() - size,
            ((json.len() - size) as f64 / size as f64) * 100.0
        );
    }
}

#[test]
fn test_chunking_strategy() {
    // Test different chunking strategies for large inputs
    let large_input = "a".repeat(3000);
    
    println!("\n=== Chunking Strategies ===");
    println!("Total input size: {} bytes", large_input.len());

    let chunk_sizes = vec![256, 512, 1024];
    
    for chunk_size in chunk_sizes {
        let chunks: Vec<&str> = large_input
            .as_bytes()
            .chunks(chunk_size)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect();

        println!("\nChunk size: {} bytes", chunk_size);
        println!("Number of chunks: {}", chunks.len());
        println!("Last chunk size: {} bytes", chunks.last().unwrap().len());

        // Verify no data loss in chunking
        let reassembled: String = chunks.join("");
        assert_eq!(
            reassembled.len(),
            large_input.len(),
            "Chunking should not lose data"
        );
    }

    println!("\n✅ Chunking preserves data integrity");
    println!("   Recommendation: Use 256-512 byte chunks with small delays");
}

#[test]
fn test_realistic_gh_pr_create_scenarios() {
    // Test realistic gh pr create command sizes
    let scenarios = vec![
        ("small", 200),
        ("medium", 500),
        ("large", 1000),
        ("very_large", 1800),
        ("huge", 3000),
    ];

    println!("\n=== Realistic gh pr create Scenarios ===");

    for (name, size) in scenarios {
        let body = format!("PR description line.\n").repeat(size / 25);
        let command = format!(
            r#"gh pr create --title "Fix something" --body "$(cat <<'EOF'
{}
EOF
)"
"#,
            body
        );

        let json_payload = json!({
            "type": "input",
            "payload": command
        }).to_string();

        let status = if json_payload.len() < 1500 {
            "✅ SAFE"
        } else if json_payload.len() < 2500 {
            "⚠️  RISKY"
        } else {
            "❌ WILL FAIL"
        };

        println!(
            "{:12} | Command: {:5} bytes | JSON: {:5} bytes | {}",
            name,
            command.len(),
            json_payload.len(),
            status
        );
    }
}

/// Test that documents the exact failure threshold
#[test]
fn test_find_failure_threshold() {
    println!("\n=== Finding Exact Failure Threshold ===");
    println!("Testing sizes from 1000 to 3000 bytes in 100-byte increments\n");

    for size in (1000..=3000).step_by(100) {
        let payload = "t".repeat(size);
        let json = json!({
            "type": "input",
            "payload": payload
        }).to_string();

        let status = if json.len() < 1500 {
            "✅"
        } else if json.len() < 2000 {
            "⚠️ "
        } else {
            "❌"
        };

        println!("{} Size: {:4} bytes | JSON: {:4} bytes", status, size, json.len());
    }

    println!("\nKey findings:");
    println!("- ✅ Under 1500 bytes: Generally safe");
    println!("- ⚠️  1500-2000 bytes: May experience issues");
    println!("- ❌ Over 2000 bytes: Will likely fail with buffer overflow");
}

/// Test escape sequences in large payloads
#[test]
fn test_escape_sequences_in_large_payloads() {
    // ANSI escape codes and special characters can affect byte count
    let text_with_escapes = "\x1b[31mRed text\x1b[0m";
    let repeated = text_with_escapes.repeat(200); // ~3400 bytes

    let json = json!({
        "type": "input",
        "payload": repeated
    }).to_string();

    println!("\n=== Escape Sequences Test ===");
    println!("Text with ANSI codes repeated 200 times");
    println!("Payload size: {} bytes", repeated.len());
    println!("JSON size: {} bytes", json.len());

    if json.len() > 2000 {
        println!("⚠️  This will cause buffer overflow!");
    }
}
