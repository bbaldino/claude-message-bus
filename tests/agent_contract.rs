//! The contract with Claude Code's channels feature. If this breaks, agents
//! silently stop receiving messages — the notification is dropped with no error
//! to the sender — so assert the shape explicitly rather than trusting it.

mod common;

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
fn exposes_exactly_the_tools_named_in_bus_tool_names() {
    // Derived from claude_bus::agent::handler::BUS_TOOL_NAMES rather than a
    // second hardcoded list: `claude-bus init` builds its permission
    // allowlist from that same const, so if a tool is added to `list_tools`
    // without updating the const (or vice versa), this test — not a stalled
    // unattended agent exchange months later — is what catches it.
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

    let mut expected: Vec<String> = claude_bus::agent::handler::BUS_TOOL_NAMES
        .iter()
        .map(|s| s.to_string())
        .collect();
    expected.sort();

    assert_eq!(names, expected);
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
        common::wait_until_bus_ready(port).await;
        (dir, port)
    });

    let mut a = Agent::start_with_bus(port, "receiver");
    initialize(&mut a);
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    wait_until_online(&rt, port, "receiver");

    // A second agent, driven directly over the wire, sends to the first.
    rt.block_on(async {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .unwrap();
        let reg = serde_json::json!({
            "type": "register", "name": "sender", "host": "h", "cwd": "/w", "session_id": null
        });
        ws.send(Message::text(reg.to_string())).await.unwrap();
        let _ = ws.next().await; // Registered: confirms the registration landed
        let msg = serde_json::json!({
            "type": "send", "req_id": 1,
            "target": { "kind": "agent", "name": "receiver" },
            "text": "wire format proposal", "done": false
        });
        ws.send(Message::text(msg.to_string())).await.unwrap();
        let _ = ws.next().await; // Reply to Send: confirms the bus processed it
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
        common::wait_until_bus_ready(port).await;
        (dir, port)
    });

    let mut a = Agent::start_with_bus(port, "lonely");
    initialize(&mut a);
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    wait_until_online(&rt, port, "lonely");

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

#[test]
fn send_rejects_both_to_and_room() {
    // Same guard, the other bad input: giving both is just as ambiguous as
    // giving neither, and the catch-all match arm handles both — but only
    // the neither-given case had a test before this one.
    let mut a = Agent::start();
    initialize(&mut a);
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    let text = call_tool(
        &mut a,
        13,
        "send",
        serde_json::json!({ "to": "someone", "room": "somewhere", "text": "ambiguous" }),
    );
    assert!(
        text.contains("to") && text.contains("room"),
        "should explain the two options: {text}"
    );
}

#[test]
fn put_file_rejects_both_content_and_path() {
    let mut a = Agent::start();
    initialize(&mut a);
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    let text = call_tool(
        &mut a,
        14,
        "put_file",
        serde_json::json!({ "room": "r", "key": "k", "content": "abc", "path": "/tmp/x" }),
    );
    assert!(
        text.contains("content") && text.contains("path"),
        "should explain the two options: {text}"
    );
}

#[test]
fn send_to_a_room_reports_both_delivered_and_queued_members() {
    // The mixed case: `send`'s result text joins two independent clauses
    // ("delivered to …" and "queued for … (offline)") with "; " when both
    // lists are non-empty. This is the only path where a message is
    // simultaneously delivered and not — the one most likely to regress into
    // a half-truth, since dropping either clause alone still produces a
    // plausible-looking sentence.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (_dir, port, _watcher) = rt.block_on(async {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { claude_bus::bus::serve_on(listener, path).await.unwrap() });
        common::wait_until_bus_ready(port).await;

        let url = format!("ws://127.0.0.1:{port}/ws");

        // "watcher" joins the room and stays connected: it will show up as
        // delivered. The connection is kept alive by returning it — presence
        // in this bus is connection lifetime, so it must not be dropped.
        let (mut watcher, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        watcher
            .send(Message::text(
                serde_json::json!({
                    "type": "register", "name": "watcher", "host": "h", "cwd": "/w", "session_id": null
                })
                .to_string(),
            ))
            .await
            .unwrap();
        let _ = watcher.next().await; // Registered
        watcher
            .send(Message::text(
                serde_json::json!({ "type": "join", "req_id": 1, "room": "standup" }).to_string(),
            ))
            .await
            .unwrap();
        let _ = watcher.next().await; // Joined

        // "sleeper" joins the room, then disconnects: it will be offline
        // when the room send happens, so it must show up as queued.
        let (mut sleeper, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        sleeper
            .send(Message::text(
                serde_json::json!({
                    "type": "register", "name": "sleeper", "host": "h", "cwd": "/w", "session_id": null
                })
                .to_string(),
            ))
            .await
            .unwrap();
        let _ = sleeper.next().await; // Registered
        sleeper
            .send(Message::text(
                serde_json::json!({ "type": "join", "req_id": 1, "room": "standup" }).to_string(),
            ))
            .await
            .unwrap();
        // Read the Joined confirmation rather than guessing with a sleep:
        // this proves the membership landed before "sleeper" disconnects.
        let _ = sleeper.next().await; // Joined
        drop(sleeper);
        // Poll for the disconnect's teardown to actually land, rather than
        // guessing with a fixed sleep — a room `send` racing ahead of it
        // would find "sleeper" still online and never report it as queued.
        assert!(
            common::wait_until(|| async { !common::agent_is_online(port, "sleeper").await }).await,
            "sleeper never went offline after its connection was dropped"
        );

        (dir, port, watcher)
    });

    let mut a = Agent::start_with_bus(port, "reporter");
    initialize(&mut a);
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    wait_until_online(&rt, port, "reporter");

    let text = call_tool(
        &mut a,
        20,
        "send",
        serde_json::json!({ "room": "standup", "text": "daily update" }),
    );
    assert!(
        text.contains("delivered to watcher"),
        "must report the online member as delivered: {text}"
    );
    assert!(
        text.contains("queued for sleeper"),
        "must report the offline member as queued: {text}"
    );
    assert!(
        text.contains("daily update"),
        "must echo the text sent: {text}"
    );
}

/// Poll until the bus reports `name` offline. A blind sleep after killing a
/// process is a race: if the registry hasn't yet noticed the old connection
/// close by the time the process reconnects, `Registry::attach` suffixes the
/// new connection (e.g. `receiver#2`) instead of handing back the bare name.
/// That name was never joined to any room, so it would never get an unread
/// summary — silently hanging a test that waits on one, instead of failing
/// fast.
fn wait_until_offline(rt: &tokio::runtime::Runtime, port: u16, name: &str) {
    let ok = rt.block_on(common::wait_until(|| async {
        !common::agent_is_online(port, name).await
    }));
    assert!(ok, "{name} never went offline according to the bus");
}

/// Poll until the bus reports `name` online. The MCP agent process connects
/// to the bus asynchronously in the background after
/// `notifications/initialized`; a fixed sleep can expire before that
/// handshake completes under load, letting a subsequent tool call or a
/// message sent by another connection race a bus link that isn't up yet.
fn wait_until_online(rt: &tokio::runtime::Runtime, port: u16, name: &str) {
    let ok = rt.block_on(common::wait_until(|| common::agent_is_online(port, name)));
    assert!(
        ok,
        "{name} never registered with the bus within the deadline"
    );
}

// Regression for the missing Ack producer: `bridge::dispatch` used to take an
// unused `_rx` and nothing ever sent `ToBus::Ack`, so `Store::set_cursor` was
// dead code and every reconnect's `unread_count` compared against cursor 0 —
// reporting the room's *entire* history from other agents as unread, not
// just what was genuinely missed. This drives a real agent process through
// two live messages (which the bridge must inject and then ack), kills it,
// sends three more while it's offline, then reconnects and asserts the
// unread summary is 3, not 5.
#[test]
fn ack_advances_the_cursor_so_reconnect_reports_only_genuinely_unseen_messages() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (_dir, port) = rt.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { claude_bus::bus::serve_on(listener, path).await.unwrap() });
        common::wait_until_bus_ready(port).await;
        (dir, port)
    });
    let store_path = _dir.path().to_path_buf();

    {
        // First session for "receiver": joins the room and is live for two
        // messages, which the bridge injects and (if the fix holds) acks.
        let mut receiver = Agent::start_with_bus(port, "receiver");
        initialize(&mut receiver);
        receiver
            .send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        wait_until_online(&rt, port, "receiver");
        call_tool(
            &mut receiver,
            2,
            "join",
            serde_json::json!({ "room": "protocol" }),
        );

        rt.block_on(async {
            use futures_util::{SinkExt, StreamExt};
            use tokio_tungstenite::tungstenite::Message;
            // Two distinct senders, not one sender twice: `serve_on` runs
            // with the production-default guards, whose rate limit is 2s
            // per (room, agent) — sending twice from the same agent this
            // close together would silently drop the second message.
            for (name, text) in [("senderA", "live 0"), ("senderB", "live 1")] {
                let (mut ws, _) =
                    tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
                        .await
                        .unwrap();
                ws.send(Message::text(
                    serde_json::json!({
                        "type": "register", "name": name, "host": "h", "cwd": "/w", "session_id": null
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
                let _ = ws.next().await; // Registered
                let msg = serde_json::json!({
                    "type": "send", "req_id": 1,
                    "target": { "kind": "room", "room": "protocol" },
                    "text": text, "done": false
                });
                ws.send(Message::text(msg.to_string())).await.unwrap();
                let _ = ws.next().await; // Reply to Send
            }
        });

        // Drain the two channel notifications: proof the bridge actually
        // injected them and therefore had the chance to ack.
        receiver.next_notification("notifications/claude/channel");
        receiver.next_notification("notifications/claude/channel");
        // Poll for the bridge to actually flush both Acks to the store
        // before the process is killed, rather than guessing with a fixed
        // sleep: these are the first two messages ever written to this
        // fresh database, so their ids are deterministically 1 and 2.
        let acked = rt.block_on(async {
            let store = claude_bus::store::Store::open(&store_path).await.unwrap();
            common::wait_until(|| async {
                store.cursor("protocol", "receiver").await.unwrap_or(0) >= 2
            })
            .await
        });
        assert!(
            acked,
            "the bridge never flushed both Acks to the store before the process was killed"
        );
    } // receiver's Agent is dropped here: process killed, connection closes.

    // Wait for the bus to actually notice the disconnect, rather than
    // guessing with a fixed sleep — see `wait_until_offline`.
    wait_until_offline(&rt, port, "receiver");

    // While "receiver" is genuinely offline, three more messages arrive —
    // again from three distinct agents, for the same rate-limit reason.
    rt.block_on(async {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        for (name, text) in [
            ("senderC", "away 0"),
            ("senderD", "away 1"),
            ("senderE", "away 2"),
        ] {
            let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
                .await
                .unwrap();
            ws.send(Message::text(
                serde_json::json!({
                    "type": "register", "name": name, "host": "h", "cwd": "/w", "session_id": null
                })
                .to_string(),
            ))
            .await
            .unwrap();
            let _ = ws.next().await; // Registered
            let msg = serde_json::json!({
                "type": "send", "req_id": 1,
                "target": { "kind": "room", "room": "protocol" },
                "text": text, "done": false
            });
            ws.send(Message::text(msg.to_string())).await.unwrap();
            let _ = ws.next().await; // Reply to Send
        }
    });

    // Reconnect and confirm the unread summary reflects only the 3 messages
    // sent while genuinely offline — not those 3 plus the 2 already shown.
    let mut receiver2 = Agent::start_with_bus(port, "receiver");
    initialize(&mut receiver2);
    receiver2.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    let note = receiver2.next_notification("notifications/claude/channel");
    assert_eq!(note["params"]["meta"]["kind"], "unread");
    assert_eq!(note["params"]["meta"]["rooms"], "protocol");
    let content = note["params"]["content"].as_str().unwrap_or("");
    assert!(
        content.starts_with("3 "),
        "expected exactly the 3 messages missed while offline (cursor should \
         have advanced past the 2 already shown), got: {content:?}"
    );
}

// Regression for the Paused guard arm reporting the bus as unreachable: it
// used to send only `FromBus::Paused` (no req_id) and never resolve the
// outstanding request, so `send` blocked the full 10s timeout and reported
// "the bus did not reply within 10s; it may be unreachable" — a lie about
// system health at the exact moment the runaway guard fires.
#[test]
fn send_reports_the_pause_not_a_bus_outage() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (_dir, port) = rt.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        // Cap of 1, no rate limit: the second send in the room trips Paused
        // immediately without needing to burn through the default cap of 20.
        let guards = claude_bus::bus::delivery::Guards::new(1, 0);
        tokio::spawn(async move {
            claude_bus::bus::serve_on_with(listener, path, guards)
                .await
                .unwrap()
        });
        common::wait_until_bus_ready(port).await;
        (dir, port)
    });

    let mut a = Agent::start_with_bus(port, "runaway");
    initialize(&mut a);
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    wait_until_online(&rt, port, "runaway");

    // Consume the cap of 1.
    call_tool(
        &mut a,
        30,
        "send",
        serde_json::json!({ "room": "loop", "text": "one" }),
    );

    let started = std::time::Instant::now();
    let text = call_tool(
        &mut a,
        31,
        "send",
        serde_json::json!({ "room": "loop", "text": "two" }),
    );
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "send should resolve promptly when the room is paused, not block \
         toward the 10s timeout: took {elapsed:?}"
    );
    assert!(
        text.to_lowercase().contains("paused"),
        "must name the pause: {text}"
    );
    assert!(
        text.to_lowercase().contains("resume"),
        "must point at the resume path: {text}"
    );
    assert!(
        !text.to_lowercase().contains("unreachable"),
        "must not claim the bus is unreachable: {text}"
    );
}
