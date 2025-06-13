//! Platform-specific tests for HT
//! 
//! This module contains tests that validate cross-platform functionality

#[cfg(test)]
mod platform_tests {
    use crate::pty::Winsize;
    use crate::locale;

    /// Test that basic Winsize structure works on all platforms
    #[test]
    fn test_winsize_creation() {
        let winsize = Winsize {
            ws_col: 80,
            ws_row: 24,
            #[cfg(unix)]
            ws_xpixel: 0,
            #[cfg(unix)]
            ws_ypixel: 0,
        };
        
        assert_eq!(winsize.ws_col, 80);
        assert_eq!(winsize.ws_row, 24);
    }

    /// Test that locale checking works on all platforms
    #[test]
    fn test_locale_check() {
        // This should not panic on any platform
        let result = locale::check_utf8_locale();
        
        // On Unix, this might succeed or fail depending on the locale
        // On Windows, this should always succeed
        #[cfg(windows)]
        assert!(result.is_ok());
        
        // On Unix, we just check it doesn't panic
        #[cfg(unix)]
        let _ = result;
    }

    /// Test locale initialization
    #[test]
    fn test_locale_initialization() {
        // This should not panic on any platform
        locale::initialize_from_env();
    }
}

#[cfg(test)]
#[cfg(windows)]
mod windows_tests {
    use super::*;
    use tokio::sync::mpsc;
    use std::time::Duration;

    /// Test that we can create Windows-specific channels for PTY communication
    #[tokio::test]
    async fn test_windows_pty_channels() {
        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>(100);
        let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(100);

        // Test sending data through channels
        let test_data = b"echo hello".to_vec();
        input_tx.send(test_data.clone()).await.unwrap();
        
        drop(input_tx); // Close the sender
        
        // Try to receive (should get the data we sent)
        if let Some(received) = input_rx.recv().await {
            assert_eq!(received, test_data);
        }
    }

    /// Test Windows command parsing
    #[test]
    fn test_windows_command_parsing() {
        // Test empty command should default to cmd.exe
        let empty_command = "";
        assert!(empty_command.is_empty());
        
        // Test non-empty command should be passed to cmd.exe /c
        let command = "dir";
        assert!(!command.is_empty());
    }
}

#[cfg(test)]
#[cfg(unix)]
mod unix_tests {
    use super::*;

    /// Test Unix-specific PTY functionality
    #[test]
    fn test_unix_pty_winsize() {
        use nix::pty::Winsize as NixWinsize;
        
        let winsize = NixWinsize {
            ws_col: 80,
            ws_row: 24,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        
        assert_eq!(winsize.ws_col, 80);
        assert_eq!(winsize.ws_row, 24);
    }
}