//! Runs the real binary the way Claude Code would: arguments, JSON on stdin, and an
//! inherited environment. Asserts what the agent's session would observe.

use std::io::Write;
use std::process::{Command, Stdio};

/// `claude_pid` stands in for the `CLAUDE_PID` Claude Code sets on its hooks. `None`
/// removes it, so a suite that itself runs inside Claude Code does not leak the real
/// session's pid into the hook under test.
fn hook_with_pid(
    state_home: &std::path::Path,
    claude_pid: Option<&str>,
    args: &[&str],
    stdin: &[u8],
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_claude-bus"));
    cmd.arg("hook")
        .args(args)
        .env("XDG_STATE_HOME", state_home)
        .env_remove("CLAUDE_PID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(pid) = claude_pid {
        cmd.env("CLAUDE_PID", pid);
    }
    let mut child = cmd.spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin).ok();
    child.wait_with_output().unwrap()
}

fn hook(state_home: &std::path::Path, args: &[&str], stdin: &[u8]) -> std::process::Output {
    hook_with_pid(state_home, Some("4242"), args, stdin)
}

fn state_in(file: std::path::PathBuf) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(&file).unwrap_or_else(|e| panic!("{file:?}: {e}")))
        .unwrap()
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
    assert_eq!(
        state_in(dir.path().join("claude-bus/status/claude-4242.json"))["state"],
        "idle"
    );
}

#[test]
fn the_file_is_keyed_by_claude_pid_not_session_id() {
    // After `/clear` the hooks see a new session id while the bridge keeps the old
    // one, so the session id cannot be the key. Two different ids, one process: one
    // file.
    let dir = tempfile::tempdir().unwrap();
    hook(
        dir.path(),
        &["prompt-submit"],
        br#"{"session_id":"before"}"#,
    );
    hook(dir.path(), &["stop"], br#"{"session_id":"after-clear"}"#);
    let files: Vec<_> = std::fs::read_dir(dir.path().join("claude-bus/status"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(files, ["claude-4242.json"]);
    assert_eq!(
        state_in(dir.path().join("claude-bus/status/claude-4242.json"))["state"],
        "idle"
    );
}

#[test]
fn without_claude_pid_the_hook_uses_its_parent_pid() {
    // The hook's parent here is this test process, as Claude Code is in production.
    let dir = tempfile::tempdir().unwrap();
    let out = hook_with_pid(dir.path(), None, &["stop"], b"{}");
    assert!(out.status.success());
    let file = dir.path().join(format!(
        "claude-bus/status/claude-{}.json",
        std::process::id()
    ));
    assert_eq!(state_in(file)["state"], "idle");
}

#[test]
fn a_garbage_claude_pid_falls_back_to_the_parent_pid() {
    let dir = tempfile::tempdir().unwrap();
    let out = hook_with_pid(dir.path(), Some("../../etc"), &["stop"], b"");
    assert!(out.status.success());
    let file = dir.path().join(format!(
        "claude-bus/status/claude-{}.json",
        std::process::id()
    ));
    assert_eq!(state_in(file)["state"], "idle");
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
