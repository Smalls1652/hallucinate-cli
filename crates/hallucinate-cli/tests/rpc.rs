use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn rpc_process_handles_requests_and_notifications() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hallucinate"))
        .arg("--rpc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn rpc process");

    let stdin = child.stdin.as_mut().expect("rpc stdin");
    writeln!(stdin, r#"{{"jsonrpc":"2.0","method":"ping","id":1}}"#).unwrap();
    writeln!(stdin, "{{").unwrap();
    writeln!(stdin, "[]").unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"1.0","method":"ping","id":2}}"#).unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","method":"ping"}}"#).unwrap();
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait for rpc process");
    assert!(output.status.success());

    let responses: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 4);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["pong"], true);
    assert_eq!(responses[1]["error"]["code"], -32700);
    assert_eq!(responses[2]["error"]["code"], -32600);
    assert_eq!(responses[3]["id"], 2);
    assert_eq!(responses[3]["error"]["code"], -32600);
}
