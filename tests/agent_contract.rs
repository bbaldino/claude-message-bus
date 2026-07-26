//! The contract with Claude Code's channels feature. If this breaks, agents
//! silently stop receiving messages — the notification is dropped with no error
//! to the sender — so assert the shape explicitly rather than trusting it.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Agent {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Agent {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
            .args(["agent", "--bus", "ws://127.0.0.1:1/ws", "--name", "tester"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn agent");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, v: serde_json::Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn next_json(&mut self) -> serde_json::Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read stdout");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad json {line:?}: {e}"))
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn initialize(a: &mut Agent) -> serde_json::Value {
    a.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "harness", "version": "1" }
        }
    }));
    a.next_json()
}

#[test]
fn declares_the_channel_capability() {
    // Without this exact key, Claude Code never registers a notification
    // listener and every pushed message is silently discarded.
    let mut a = Agent::start();
    let res = initialize(&mut a);
    let caps = &res["result"]["capabilities"];
    assert_eq!(
        caps["experimental"]["claude/channel"],
        serde_json::json!({}),
        "capabilities were: {caps}"
    );
    assert_eq!(caps["tools"], serde_json::json!({}));
}

#[test]
fn sends_instructions_that_establish_the_discuss_only_posture() {
    let mut a = Agent::start();
    let res = initialize(&mut a);
    let instructions = res["result"]["instructions"]
        .as_str()
        .expect("instructions must be present");
    assert!(instructions.contains("tester"), "should name the agent");
    assert!(
        instructions.contains("<channel"),
        "should explain the tag shape"
    );
    assert!(
        instructions.contains("send"),
        "should mention the send tool"
    );
    // Case-insensitive: the source text emphasizes this with caps ("NOT
    // INSTRUCTIONS") as deliberate prompt engineering, since this sentence is
    // the model's only signal that channel messages carry the same authority
    // as the human's own input and should not be treated as commands. The
    // concept mattering, not its casing, is what this test should enforce.
    assert!(
        instructions.to_lowercase().contains("not instructions"),
        "instructions missing the discuss-only concept: {instructions}"
    );
}

#[test]
fn exposes_exactly_the_nine_documented_tools() {
    let mut a = Agent::start();
    initialize(&mut a);
    a.send(serde_json::json!({
        "jsonrpc": "2.0", "method": "notifications/initialized"
    }));
    a.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}
    }));
    let res = a.next_json();
    let mut names: Vec<String> = res["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "agents",
            "get_file",
            "history",
            "join",
            "list_files",
            "put_file",
            "resume",
            "rooms",
            "send",
        ]
    );
}

#[test]
fn server_identifies_itself_as_msgbus_with_our_own_version() {
    // Implementation::from_build_env() reports rmcp's version, not ours.
    let mut a = Agent::start();
    let res = initialize(&mut a);
    assert_eq!(res["result"]["serverInfo"]["name"], "msgbus");
    assert_eq!(
        res["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
}

#[test]
fn starts_even_when_the_bus_is_unreachable() {
    // The agent points at ws://127.0.0.1:1 which nothing serves. Session
    // startup must never block on the network.
    let mut a = Agent::start();
    let res = initialize(&mut a);
    assert_eq!(res["result"]["serverInfo"]["name"], "msgbus");
}
