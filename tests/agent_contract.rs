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

    fn start_with_bus(port: u16, name: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
            .args([
                "agent",
                "--bus",
                &format!("ws://127.0.0.1:{port}/ws"),
                "--name",
                name,
            ])
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

    /// Read stdout until a notification with the given method appears.
    fn next_notification(&mut self, method: &str) -> serde_json::Value {
        for _ in 0..50 {
            let v = self.next_json();
            if v["method"] == method {
                return v;
            }
        }
        panic!("never saw a {method} notification");
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

// Full loop with a real bus: a message sent by another agent must surface on
// this agent's stdout as a notifications/claude/channel with the meta keys the
// channel contract requires.
#[test]
fn injects_bus_messages_as_channel_notifications() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (_dir, port) = rt.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { claude_bus::bus::serve_on(listener, path).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        (dir, port)
    });

    let mut a = Agent::start_with_bus(port, "receiver");
    initialize(&mut a);
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    std::thread::sleep(std::time::Duration::from_millis(800));

    // A second agent, driven directly over the wire, sends to the first.
    rt.block_on(async {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .unwrap();
        let reg = serde_json::json!({
            "type": "register", "name": "sender", "host": "h", "cwd": "/w", "session_id": null
        });
        ws.send(Message::text(reg.to_string())).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let msg = serde_json::json!({
            "type": "send", "req_id": 1,
            "target": { "kind": "agent", "name": "receiver" },
            "text": "wire format proposal", "done": false
        });
        ws.send(Message::text(msg.to_string())).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    });

    let note = a.next_notification("notifications/claude/channel");
    assert_eq!(note["params"]["content"], "wire format proposal");
    assert_eq!(note["params"]["meta"]["from"], "sender");
    assert_eq!(note["params"]["meta"]["room"], "dm:receiver|sender");
    // This is the first message ever inserted into a fresh per-test sqlite
    // database (messages.id is INTEGER PRIMARY KEY AUTOINCREMENT), so its id
    // is deterministically 1. Pinning the value, not just the type, is
    // required: `.is_string()` alone would pass for any string, including a
    // wrong one.
    assert_eq!(
        note["params"]["meta"]["msg_id"], "1",
        "msg_id must be the string \"1\", not a bare number or the wrong value"
    );
    assert_eq!(
        note["params"]["meta"]["done"], "false",
        "done must be the string \"false\", not a bare bool"
    );
}

fn call_tool(a: &mut Agent, id: u64, name: &str, args: serde_json::Value) -> String {
    a.send(serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": name, "arguments": args }
    }));
    for _ in 0..50 {
        let v = a.next_json();
        if v["id"] == id {
            return v["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or("")
                .to_string();
        }
    }
    panic!("no tool result for id {id}");
}

#[test]
fn send_reports_queued_when_the_recipient_is_offline() {
    // The POC 3 correction, asserted at the tool boundary the model actually sees.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (_dir, port) = rt.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { claude_bus::bus::serve_on(listener, path).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        (dir, port)
    });

    let mut a = Agent::start_with_bus(port, "lonely");
    initialize(&mut a);
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    std::thread::sleep(std::time::Duration::from_millis(800));

    let text = call_tool(
        &mut a,
        10,
        "send",
        serde_json::json!({ "to": "nobody", "text": "hello?" }),
    );
    assert!(
        text.contains("queued"),
        "must say queued, not claim delivery: {text}"
    );
    assert!(text.contains("hello?"), "must echo the text sent: {text}");
}

#[test]
fn tools_fail_clearly_when_the_bus_is_unreachable() {
    let mut a = Agent::start(); // points at ws://127.0.0.1:1
    initialize(&mut a);
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    let text = call_tool(&mut a, 11, "agents", serde_json::json!({}));
    assert!(
        text.to_lowercase().contains("bus"),
        "error should mention the bus: {text}"
    );
}

#[test]
fn send_requires_exactly_one_destination() {
    let mut a = Agent::start();
    initialize(&mut a);
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    let text = call_tool(&mut a, 12, "send", serde_json::json!({ "text": "orphan" }));
    assert!(
        text.contains("to") && text.contains("room"),
        "should explain the two options: {text}"
    );
}
