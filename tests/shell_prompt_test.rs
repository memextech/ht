// Integration test: spawn a shell via PTY, parse output through avt::Vt, and
// assert that a shell prompt appears on the virtual terminal screen.

#[cfg(unix)]
mod unix {
    use ht_core::pty::{self, Winsize};
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    fn screen_text(vt: &avt::Vt) -> String {
        vt.view()
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn shell_prompt_appears_on_screen() {
        let winsize = Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>(100);
        let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(100);
        let (_resize_tx, resize_rx) = mpsc::channel::<(u16, u16)>(1);

        let mut vt = avt::Vt::builder().size(80, 24).resizable(true).build();

        let command = "env PS1='TEST_PROMPT> ' bash --norc --noprofile".to_string();
        let pty_future = pty::spawn(command, winsize, input_rx, output_tx, resize_rx, None)
            .expect("failed to spawn PTY");
        let pty_handle = tokio::spawn(pty_future);

        let prompt_marker = "TEST_PROMPT> ";
        let mut found = false;
        let mut raw_chunks: Vec<String> = Vec::new();
        let mut exit_reason = "deadline expired";
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

        while tokio::time::Instant::now() < deadline {
            match timeout(Duration::from_millis(100), output_rx.recv()).await {
                Ok(Some(data)) => {
                    let text = String::from_utf8_lossy(&data);
                    raw_chunks.push(text.to_string());
                    vt.feed_str(&text);

                    if screen_text(&vt).contains(prompt_marker) {
                        found = true;
                        break;
                    }
                }
                Ok(None) => {
                    exit_reason = "channel closed (PTY exited)";
                    break;
                }
                Err(_) => continue,
            }
        }

        let screen = screen_text(&vt);
        assert!(
            found,
            "Prompt '{}' not found.\nExit reason: {}\nScreen contents:\n{}\nRaw output chunks ({}):\n{}",
            prompt_marker,
            exit_reason,
            screen,
            raw_chunks.len(),
            raw_chunks.join("---\n"),
        );

        let _ = input_tx.send(b"exit\n".to_vec()).await;
        drop(input_tx);
        match timeout(Duration::from_secs(2), pty_handle).await {
            Ok(Ok(result)) => result.expect("PTY task returned an error"),
            Ok(Err(join_err)) => panic!("PTY task panicked: {join_err}"),
            Err(_) => panic!("timed out waiting for PTY task to finish"),
        }
    }
}

#[cfg(windows)]
mod windows {
    use ht_core::pty::{self, Winsize};
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    fn screen_text(vt: &avt::Vt) -> String {
        vt.view()
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shell_prompt_appears_on_screen() {
        let winsize = Winsize {
            ws_row: 24,
            ws_col: 80,
        };

        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>(100);
        let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(100);
        let (_resize_tx, resize_rx) = mpsc::channel::<(u16, u16)>(1);

        let mut vt = avt::Vt::builder().size(80, 24).resizable(true).build();

        // base64(utf16le("function prompt { 'TEST_PROMPT> ' }"))
        // Regenerate: python3 -c "import base64; print(base64.b64encode(\"function prompt { 'TEST_PROMPT> ' }\".encode('utf-16-le')).decode())"
        const PROMPT_SCRIPT_B64: &str = "ZgB1AG4AYwB0AGkAbwBuACAAcAByAG8AbQBwAHQAIAB7ACAAJwBUAEUAUwBUAF8AUABSAE8ATQBQAFQAPgAgACcAIAB9AA==";
        let command = format!(
            "powershell.exe -NoProfile -NoLogo -NoExit -EncodedCommand {PROMPT_SCRIPT_B64}"
        );
        let pty_future = pty::spawn(command, winsize, input_rx, output_tx, resize_rx, None)
            .expect("failed to spawn PTY");
        let pty_handle = tokio::spawn(pty_future);

        let prompt_marker = "TEST_PROMPT> ";
        let mut found = false;
        let mut raw_chunks: Vec<String> = Vec::new();
        let mut exit_reason = "deadline expired";
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);

        while tokio::time::Instant::now() < deadline {
            match timeout(Duration::from_millis(100), output_rx.recv()).await {
                Ok(Some(data)) => {
                    let text = String::from_utf8_lossy(&data);
                    raw_chunks.push(text.to_string());
                    vt.feed_str(&text);

                    if screen_text(&vt).contains(prompt_marker) {
                        found = true;
                        break;
                    }
                }
                Ok(None) => {
                    exit_reason = "channel closed (PTY exited)";
                    break;
                }
                Err(_) => continue,
            }
        }

        let screen = screen_text(&vt);
        assert!(
            found,
            "Prompt '{}' not found.\nExit reason: {}\nScreen contents:\n{}\nRaw output chunks ({}):\n{}",
            prompt_marker,
            exit_reason,
            screen,
            raw_chunks.len(),
            raw_chunks.join("---\n"),
        );

        let _ = input_tx.send(b"exit\r\n".to_vec()).await;
        drop(input_tx);
        match timeout(Duration::from_secs(2), pty_handle).await {
            Ok(Ok(result)) => result.expect("PTY task returned an error"),
            Ok(Err(join_err)) => panic!("PTY task panicked: {join_err}"),
            Err(_) => panic!("timed out waiting for PTY task to finish"),
        }
    }
}
