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
mod windows_scrape {
    use serde_json::Value;
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::time::timeout;

    fn screen_text(vt: &avt::Vt) -> String {
        vt.view()
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn assert_shell_available(shell: &str) {
        let found = std::process::Command::new("where")
            .arg(shell)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(found, "{shell} not found on PATH");
    }

    async fn assert_shell_prompt_appears(
        shell_args: &[&str],
        prompt_marker: &str,
        deadline_secs: u64,
    ) {
        let bin = env!("CARGO_BIN_EXE_ht");

        let mut args = vec!["--subscribe", "output", "--"];
        args.extend_from_slice(shell_args);

        let mut child = tokio::process::Command::new(bin)
            .args(&args)
            .env("HT_CONSOLE_DIAG", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn ht");

        let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut stdin = child.stdin.take().unwrap();

        let mut vt = avt::Vt::builder().size(120, 40).resizable(true).build();
        let mut found = false;
        let mut raw_lines: Vec<String> = Vec::new();
        let mut seq_chunks: Vec<String> = Vec::new();
        let mut exit_reason = "deadline expired";
        let deadline = Duration::from_secs(deadline_secs);
        let _ = timeout(deadline, async {
            while let Ok(Some(line)) = stdout.next_line().await {
                raw_lines.push(line.clone());
                if let Ok(json) = serde_json::from_str::<Value>(&line) {
                    if let Some(seq) = json["data"]["seq"].as_str() {
                        seq_chunks.push(seq.to_string());
                        vt.feed_str(seq);
                    }
                }
                if screen_text(&vt).contains(prompt_marker) {
                    found = true;
                    break;
                }
            }
            if !found {
                exit_reason = "stdout closed (ht exited)";
            }
        })
        .await;

        if found {
            let exit_msg = r#"{"type":"input","payload":"exit\r\n"}"#;
            let _ = stdin.write_all(exit_msg.as_bytes()).await;
            let _ = stdin.write_all(b"\n").await;
            drop(stdin);

            let status = timeout(Duration::from_secs(5), child.wait())
                .await
                .expect("ht process timed out")
                .expect("failed to wait on ht");
            assert!(status.success(), "ht exited with: {status}");
        } else {
            let stderr = child.stderr.take();
            let _ = child.kill().await;
            let stderr_text = match stderr {
                Some(se) => {
                    let mut buf = String::new();
                    let _ = timeout(
                        Duration::from_secs(1),
                        tokio::io::AsyncReadExt::read_to_string(&mut BufReader::new(se), &mut buf),
                    )
                    .await;
                    buf
                }
                None => String::new(),
            };

            // Run-length compress consecutive identical seq chunks.
            let mut runs: Vec<(usize, usize, &str)> = Vec::new(); // (start, count, value)
            for (i, s) in seq_chunks.iter().enumerate() {
                if let Some(last) = runs.last_mut() {
                    if last.2 == s.as_str() {
                        last.1 += 1;
                        continue;
                    }
                }
                runs.push((i, 1, s.as_str()));
            }

            let format_run = |r: &(usize, usize, &str)| {
                let hex: String =
                    r.2.bytes()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                if r.1 == 1 {
                    format!("  [{}] {:?}\n       hex: {hex}", r.0, r.2)
                } else {
                    let end = r.0 + r.1 - 1;
                    format!(
                        "  [{}..{}] {:?} (\u{00d7}{})\n       hex: {hex}",
                        r.0, end, r.2, r.1
                    )
                }
            };

            // Limit to 30 display entries (first 25 + last 5).
            let seq_display = if runs.len() <= 30 {
                runs.iter().map(format_run).collect::<Vec<_>>()
            } else {
                let mut v: Vec<String> = runs[..25].iter().map(&format_run).collect();
                v.push(format!("  ... ({} runs omitted)", runs.len() - 30));
                v.extend(runs[runs.len() - 5..].iter().map(format_run));
                v
            };

            // Limit raw JSON lines: first 10 + last 5.
            let json_display = if raw_lines.len() <= 15 {
                raw_lines.join("\n")
            } else {
                let mut parts: Vec<&str> = raw_lines[..10].iter().map(|s| s.as_str()).collect();
                let omit = format!("... ({} lines omitted)", raw_lines.len() - 15);
                parts.push(&omit);
                parts.extend(raw_lines[raw_lines.len() - 5..].iter().map(|s| s.as_str()));
                parts.join("\n")
            };

            panic!(
                "Prompt '{prompt_marker}' not found within {deadline:?}\n\
                 Exit reason: {exit_reason}\n\
                 Shell args: {shell_args:?}\n\
                 Screen contents:\n{}\n\
                 Seq chunks ({} chunks, {} runs):\n{}\n\
                 Raw JSON lines ({}):\n{}\n\
                 Stderr:\n{stderr_text}",
                screen_text(&vt),
                seq_chunks.len(),
                runs.len(),
                seq_display.join("\n"),
                raw_lines.len(),
                json_display,
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shell_prompt_appears_via_scrape() {
        assert_shell_prompt_appears(
            &["cmd.exe", "/k", "prompt", "TEST_PROMPT$G$S"],
            "TEST_PROMPT>",
            15,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pwsh_prompt_appears_via_scrape() {
        assert_shell_available("pwsh.exe");
        assert_shell_prompt_appears(
            &[
                "pwsh.exe",
                "-NoLogo",
                "-NoProfile",
                "-NoExit",
                "-Command",
                "function prompt { 'TEST_PROMPT> ' }",
            ],
            "TEST_PROMPT>",
            20,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn powershell_prompt_appears_via_scrape() {
        assert_shell_available("powershell.exe");
        assert_shell_prompt_appears(
            &[
                "powershell.exe",
                "-NoLogo",
                "-NoProfile",
                "-NoExit",
                "-EncodedCommand",
                "ZgB1AG4AYwB0AGkAbwBuACAAcAByAG8AbQBwAHQAIAB7ACAAJwBUAEUAUwBUAF8AUABSAE8ATQBQAFQAPgAgACcAIAB9AA==",
            ],
            "TEST_PROMPT>",
            20,
        )
        .await;
    }
}
