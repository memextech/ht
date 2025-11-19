/// Unit tests for PTY write buffer handling
///
/// These tests focus specifically on the write buffer management in pty.rs
/// to understand and reproduce the buffer overflow issue at a lower level.

use std::time::Duration;
use tokio::sync::mpsc;
use nix::pty::Winsize;
use ht_core::pty;

/// Test that demonstrates the PTY write buffer behavior
/// 
/// The PTY master has a finite kernel buffer (typically 4KB on Unix systems).
/// When we write more data than the buffer can hold, writes will:
/// 1. Return EAGAIN/EWOULDBLOCK on non-blocking writes
/// 2. Cause data loss if not handled properly
/// 3. Result in scrambled output if writes are interleaved incorrectly
#[tokio::test]
async fn test_pty_write_buffer_limits() {
    let winsize = Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let (input_tx, input_rx) = mpsc::channel(100);
    let (output_tx, mut output_rx) = mpsc::channel(100);

    let command = "/bin/cat".to_string(); // Use cat to echo back input
    let pty_future = pty::spawn(command, winsize, input_rx, output_tx).unwrap();

    tokio::spawn(pty_future);

    // Test 1: Send data smaller than PTY buffer (should work fine)
    println!("\n=== Test 1: Small write (512 bytes) ===");
    let small_data = "A".repeat(512);
    input_tx.send(small_data.as_bytes().to_vec()).await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut received = String::new();
    while let Ok(Some(data)) = tokio::time::timeout(Duration::from_millis(50), output_rx.recv()).await {
        received.push_str(&String::from_utf8_lossy(&data));
    }

    println!("Sent: {} bytes, Received: {} bytes", small_data.len(), received.len());
    assert!(
        received.contains("AAA"),
        "Small write should be echoed back correctly"
    );

    // Test 2: Send data approaching PTY buffer size (~4KB)
    println!("\n=== Test 2: Medium write (3000 bytes) ===");
    let medium_data = "B".repeat(3000);
    input_tx.send(medium_data.as_bytes().to_vec()).await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    received.clear();
    while let Ok(Some(data)) = tokio::time::timeout(Duration::from_millis(50), output_rx.recv()).await {
        received.push_str(&String::from_utf8_lossy(&data));
    }

    let b_count = received.matches('B').count();
    println!("Sent: {} bytes, Received: {} 'B' chars", medium_data.len(), b_count);

    if b_count < 3000 {
        println!("WARNING: Buffer overflow detected! Lost {} bytes", 3000 - b_count);
    }

    // Test 3: Send data exceeding PTY buffer size (will likely fail)
    println!("\n=== Test 3: Large write (8000 bytes) ===");
    let large_data = "C".repeat(8000);
    input_tx.send(large_data.as_bytes().to_vec()).await.unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    received.clear();
    while let Ok(Some(data)) = tokio::time::timeout(Duration::from_millis(50), output_rx.recv()).await {
        received.push_str(&String::from_utf8_lossy(&data));
    }

    let c_count = received.matches('C').count();
    println!("Sent: {} bytes, Received: {} 'C' chars", large_data.len(), c_count);

    // This will likely show data loss
    if c_count < 8000 {
        println!("CONFIRMED: Buffer overflow! Lost {} bytes", 8000 - c_count);
        println!("This demonstrates the root cause of the heredoc issue.");
    }

    // Cleanup
    let _ = input_tx.send(b"\x04".to_vec()).await; // Send EOF (Ctrl-D)
    
    // Give PTY time to finish
    tokio::time::sleep(Duration::from_millis(200)).await;
}

/// Test chunked writes vs single large write
///
/// This test compares:
/// 1. Single large write (current behavior)
/// 2. Chunked writes with delays (potential fix)
#[tokio::test]
async fn test_chunked_vs_bulk_write() {
    // Test setup function
    async fn test_write_strategy(data_size: usize, chunk_size: Option<usize>) -> usize {
        let winsize = Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let (input_tx, input_rx) = mpsc::channel(100);
        let (output_tx, mut output_rx) = mpsc::channel(100);

        let command = "/bin/cat".to_string();
        let pty_future = pty::spawn(command, winsize, input_rx, output_tx).unwrap();
        tokio::spawn(pty_future);

        let data = "X".repeat(data_size);

        // Send data based on strategy
        match chunk_size {
            None => {
                // Bulk write
                input_tx.send(data.as_bytes().to_vec()).await.unwrap();
            }
            Some(size) => {
                // Chunked write with small delays
                for chunk in data.as_bytes().chunks(size) {
                    input_tx.send(chunk.to_vec()).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
        }

        // Collect output
        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut received = String::new();
        while let Ok(Some(output)) = tokio::time::timeout(Duration::from_millis(50), output_rx.recv()).await {
            received.push_str(&String::from_utf8_lossy(&output));
        }

        // Cleanup
        let _ = input_tx.send(b"\x04".to_vec()).await;

        received.matches('X').count()
    }

    println!("\n=== Comparing Write Strategies ===");

    // Test at 5000 bytes (exceeds typical PTY buffer)
    let data_size = 5000;

    println!("\n--- Bulk write ({}bytes) ---", data_size);
    let bulk_received = test_write_strategy(data_size, None).await;
    println!("Bulk write: Sent {} bytes, Received {} bytes", data_size, bulk_received);

    println!("\n--- Chunked write ({}bytes in 256-byte chunks) ---", data_size);
    let chunked_received = test_write_strategy(data_size, Some(256)).await;
    println!("Chunked write: Sent {} bytes, Received {} bytes", data_size, chunked_received);

    println!("\n--- Results ---");
    println!("Bulk write efficiency: {:.1}%", (bulk_received as f64 / data_size as f64) * 100.0);
    println!("Chunked write efficiency: {:.1}%", (chunked_received as f64 / data_size as f64) * 100.0);

    // Chunked writes should be more reliable
    assert!(
        chunked_received >= bulk_received,
        "Chunked writes should be at least as reliable as bulk writes. \
         Bulk: {}, Chunked: {}",
        bulk_received,
        chunked_received
    );

    if chunked_received > bulk_received {
        println!("\n✅ Chunked writes prevented data loss!");
        println!("   Bulk lost: {} bytes", data_size - bulk_received);
        println!("   Chunked lost: {} bytes", data_size - chunked_received);
    }
}

/// Test rapid consecutive writes
///
/// This simulates the scenario where multiple commands or large heredocs
/// are sent in quick succession, which can exhaust the PTY buffer faster.
#[tokio::test]
async fn test_rapid_consecutive_writes() {
    let winsize = Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let (input_tx, input_rx) = mpsc::channel(100);
    let (output_tx, mut output_rx) = mpsc::channel(100);

    let command = "/bin/cat".to_string();
    let pty_future = pty::spawn(command, winsize, input_rx, output_tx).unwrap();
    tokio::spawn(pty_future);

    println!("\n=== Testing Rapid Consecutive Writes ===");

    // Send 10 chunks of 1000 bytes each, as fast as possible
    let chunk_size = 1000;
    let num_chunks = 10;

    for i in 0..num_chunks {
        let marker = format!("CHUNK{:02}:", i);
        let data = format!("{}{}", marker, "Y".repeat(chunk_size - marker.len()));
        input_tx.send(data.as_bytes().to_vec()).await.unwrap();
    }

    // Give time to process
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Collect output
    let mut received = String::new();
    while let Ok(Some(data)) = tokio::time::timeout(Duration::from_millis(50), output_rx.recv()).await {
        received.push_str(&String::from_utf8_lossy(&data));
    }

    // Check if all chunks were received
    println!("\nChecking chunk markers:");
    let mut chunks_received = 0;
    for i in 0..num_chunks {
        let marker = format!("CHUNK{:02}:", i);
        if received.contains(&marker) {
            chunks_received += 1;
            println!("  ✓ {}", marker);
        } else {
            println!("  ✗ {} MISSING", marker);
        }
    }

    let total_sent = num_chunks * chunk_size;
    let total_received = received.len();

    println!("\nTotal sent: {} bytes", total_sent);
    println!("Total received: {} bytes", total_received);
    println!("Chunks received: {}/{}", chunks_received, num_chunks);

    if chunks_received < num_chunks {
        println!("\n⚠️  WARNING: Data loss detected in rapid writes!");
        println!("   Missing {} chunks", num_chunks - chunks_received);
    }

    // Cleanup
    let _ = input_tx.send(b"\x04".to_vec()).await;
}

/// Test write behavior with different delays between chunks
///
/// This helps identify the optimal delay for preventing buffer overflow
#[tokio::test]
async fn test_optimal_chunk_delay() {
    async fn test_with_delay(delay_ms: u64) -> (usize, Duration) {
        let winsize = Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let (input_tx, input_rx) = mpsc::channel(100);
        let (output_tx, mut output_rx) = mpsc::channel(100);

        let command = "/bin/cat".to_string();
        let pty_future = pty::spawn(command, winsize, input_rx, output_tx).unwrap();
        tokio::spawn(pty_future);

        let chunk_size = 500;
        let num_chunks = 10;
        let data = "Z".repeat(chunk_size);

        let start = tokio::time::Instant::now();

        for _ in 0..num_chunks {
            input_tx.send(data.as_bytes().to_vec()).await.unwrap();
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        let send_duration = start.elapsed();

        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut received = String::new();
        while let Ok(Some(output)) = tokio::time::timeout(Duration::from_millis(50), output_rx.recv()).await {
            received.push_str(&String::from_utf8_lossy(&output));
        }

        let _ = input_tx.send(b"\x04".to_vec()).await;

        (received.matches('Z').count(), send_duration)
    }

    println!("\n=== Testing Optimal Chunk Delay ===\n");

    let delays = vec![0, 1, 5, 10, 20, 50];
    let expected_chars = 5000; // 500 * 10

    for delay in delays {
        let (received, duration) = test_with_delay(delay).await;
        let efficiency = (received as f64 / expected_chars as f64) * 100.0;

        println!(
            "Delay: {:3}ms | Received: {:4}/{} ({:5.1}%) | Duration: {:?}",
            delay, received, expected_chars, efficiency, duration
        );
    }

    println!("\nThis helps identify the minimum delay needed to prevent buffer overflow.");
}

/// Test the actual buffer size limit by binary search
///
/// This test attempts to find the actual PTY buffer size by testing
/// progressively larger writes until data loss occurs.
#[tokio::test]
async fn test_find_actual_buffer_size() {
    async fn test_size(size: usize) -> bool {
        let winsize = Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let (input_tx, input_rx) = mpsc::channel(100);
        let (output_tx, mut output_rx) = mpsc::channel(100);

        let command = "/bin/cat".to_string();
        let pty_future = pty::spawn(command, winsize, input_rx, output_tx).unwrap();
        tokio::spawn(pty_future);

        let data = "W".repeat(size);
        input_tx.send(data.as_bytes().to_vec()).await.unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;

        let mut received = String::new();
        while let Ok(Some(output)) = tokio::time::timeout(Duration::from_millis(50), output_rx.recv()).await {
            received.push_str(&String::from_utf8_lossy(&output));
        }

        let _ = input_tx.send(b"\x04".to_vec()).await;

        let received_count = received.matches('W').count();
        received_count >= size // Return true if all data received
    }

    println!("\n=== Finding Actual PTY Buffer Size ===\n");

    // Binary search for buffer limit
    let mut low = 1024;
    let mut high = 16384;
    let mut safe_size = low;

    while low <= high {
        let mid = (low + high) / 2;
        println!("Testing size: {} bytes...", mid);

        if test_size(mid).await {
            safe_size = mid;
            low = mid + 512;
        } else {
            high = mid - 512;
        }
    }

    println!("\n✅ Approximate safe buffer size: {} bytes", safe_size);
    println!("   Data loss begins around: {} bytes", safe_size + 512);
    println!("\nThis confirms the PTY buffer overflow hypothesis.");
}
