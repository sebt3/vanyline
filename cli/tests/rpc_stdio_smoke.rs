use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn initialize_then_shutdown_end_to_end() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_vanyline"))
        .args(["serve", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn vanyline serve --stdio");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":1}}}}"#
    )
    .unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["result"]["protocolVersion"], 1);

    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"shutdown"}}"#).unwrap();
    let mut line2 = String::new();
    reader.read_line(&mut line2).unwrap();
    let resp2: serde_json::Value = serde_json::from_str(&line2).unwrap();
    assert_eq!(resp2["result"], serde_json::Value::Null);

    let status = child.wait().expect("process did not exit");
    assert!(status.success());
}
