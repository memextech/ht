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
        let pty_future = pty::spawn(command, winsize, input_rx, output_tx, resize_rx)
            .expect("failed to spawn PTY");
        let pty_handle = tokio::spawn(pty_future);

        let prompt_marker = "TEST_PROMPT> ";
        let mut found = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

        while tokio::time::Instant::now() < deadline {
            match timeout(Duration::from_millis(100), output_rx.recv()).await {
                Ok(Some(data)) => {
                    let text = String::from_utf8_lossy(&data);
                    vt.feed_str(&text);

                    if screen_text(&vt).contains(prompt_marker) {
                        found = true;
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }

        assert!(found, "Expected prompt '{}' on screen", prompt_marker);

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

    #[tokio::test]
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
        let command =
            format!("pwsh -NoProfile -NoLogo -NoExit -EncodedCommand {PROMPT_SCRIPT_B64}");
        let pty_future = pty::spawn(command, winsize, input_rx, output_tx, resize_rx)
            .expect("failed to spawn PTY");
        let pty_handle = tokio::spawn(pty_future);

        let prompt_marker = "TEST_PROMPT> ";
        let mut found = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

        while tokio::time::Instant::now() < deadline {
            match timeout(Duration::from_millis(100), output_rx.recv()).await {
                Ok(Some(data)) => {
                    let text = String::from_utf8_lossy(&data);
                    vt.feed_str(&text);

                    if screen_text(&vt).contains(prompt_marker) {
                        found = true;
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }

        assert!(found, "Expected prompt '{}' on screen", prompt_marker);

        let _ = input_tx.send(b"exit\n".to_vec()).await;
        drop(input_tx);
        match timeout(Duration::from_secs(2), pty_handle).await {
            Ok(Ok(result)) => result.expect("PTY task returned an error"),
            Ok(Err(join_err)) => panic!("PTY task panicked: {join_err}"),
            Err(_) => panic!("timed out waiting for PTY task to finish"),
        }
    }
}
