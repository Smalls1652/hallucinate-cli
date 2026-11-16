use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

#[test]
fn streamed_prompt_flushes_before_completion_without_creating_a_session() {
    let directory =
        std::env::temp_dir().join(format!("hallucinate-stream-process-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);

    std::fs::create_dir_all(directory.join(".hallucinate")).unwrap();
    std::fs::write(
        directory.join(".hallucinate/settings.toml"),
        "[providers.openai]\n",
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let (continue_tx, continue_rx) = mpsc::channel();

    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();

        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut content_length = 0;

        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();

            if line == "\r\n" {
                break;
            }

            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().unwrap();
                }
            }
        }

        let mut request_body = vec![0; content_length];
        reader.read_exact(&mut request_body).unwrap();
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap();

        let first =
            "data: {\"choices\":[{\"delta\":{\"content\":\"first\"},\"finish_reason\":null}]}\n\n";
        write!(socket, "{:x}\r\n{}\r\n", first.len(), first).unwrap();
        socket.flush().unwrap();

        continue_rx.recv_timeout(Duration::from_secs(10)).unwrap();

        let last = "data: {\"choices\":[{\"delta\":{\"content\":\"second\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        write!(socket, "{:x}\r\n{}\r\n0\r\n\r\n", last.len(), last).unwrap();
    });

    let mut child = Command::new(env!("CARGO_BIN_EXE_hallucinate"))
        .current_dir(&directory)
        .env("HOME", &directory)
        .env("XDG_CONFIG_HOME", directory.join("config"))
        .env("XDG_DATA_HOME", directory.join("data"))
        .env("XDG_CACHE_HOME", directory.join("cache"))
        .args([
            "prompt",
            "--stream",
            "--no-skills",
            "--model",
            "test",
            "--base-url",
            &format!("http://{address}"),
            "--api-key",
            "test",
            "hello",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdout = child.stdout.take().unwrap();

    let (output_tx, output_rx) = mpsc::channel();

    let reader = std::thread::spawn(move || {
        let mut first = [0; 5];
        stdout.read_exact(&mut first).unwrap();
        output_tx.send(first).unwrap();

        let mut remainder = Vec::new();
        stdout.read_to_end(&mut remainder).unwrap();
        remainder
    });

    let first = output_rx.recv_timeout(Duration::from_secs(10));
    continue_tx.send(()).unwrap();
    assert_eq!(&first.unwrap(), b"first");

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(reader.join().unwrap(), b"second");

    server.join().unwrap();

    fn assert_no_session_files(path: &std::path::Path) {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries {
                let path = entry.unwrap().path();

                if path.is_dir() {
                    assert_no_session_files(&path);
                } else {
                    assert_ne!(
                        path.extension().and_then(|value| value.to_str()),
                        Some("jsonl")
                    );
                }
            }
        }
    }

    assert_no_session_files(&directory);
    std::fs::remove_dir_all(directory).unwrap();
}
