//! Runs the real binary the way Claude Code would: arguments, JSON on stdin, and an
//! inherited environment. Asserts what the agent's session would observe.

use std::io::Write;
use std::process::{Command, Stdio};

fn hook(state_home: &std::path::Path, args: &[&str], stdin: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
        .arg("hook")
        .args(args)
        .env("XDG_STATE_HOME", state_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).ok();
    child.wait_with_output().unwrap()
}

#[test]
fn a_stop_hook_writes_idle_and_prints_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let out = hook(
        dir.path(),
        &["stop"],
        br#"{"session_id":"s1","hook_event_name":"Stop"}"#,
    );
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "stdout reaches the model: {:?}",
        out.stdout
    );
    let file = dir.path().join("claude-bus/status/s1.json");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(v["state"], "idle");
}

#[test]
fn bad_input_never_fails_or_prints() {
    let dir = tempfile::tempdir().unwrap();
    let big = vec![b'x'; 4 * 1024 * 1024];
    for (args, stdin) in [
        (vec!["stop"], b"".as_slice()),
        (vec!["stop"], b"not json".as_slice()),
        (vec!["stop"], br#"{"no_session":1}"#.as_slice()),
        (vec!["stop"], big.as_slice()),
        (vec!["teleport"], br#"{"session_id":"s"}"#.as_slice()),
        (vec![], br#"{"session_id":"s"}"#.as_slice()),
    ] {
        let start = std::time::Instant::now();
        let out = hook(dir.path(), &args, stdin);
        assert!(out.status.success(), "{args:?} failed");
        assert!(out.stdout.is_empty(), "{args:?} printed to stdout");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "{args:?} was slow"
        );
    }
}

#[test]
fn an_unwritable_state_dir_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let file_in_the_way = dir.path().join("claude-bus");
    std::fs::write(&file_in_the_way, "").unwrap(); // a file where the dir should be
    let out = hook(dir.path(), &["stop"], br#"{"session_id":"s1"}"#);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}
