//! Shared test scaffolding for driving a real bus over WebSocket. Extracted
//! from `tests/bus.rs` so `tests/events.rs` (and anything else that wants to
//! drive the bus end-to-end) doesn't have to duplicate it. This file lives in
//! a subdirectory of `tests/`, so cargo does not treat it as its own test
//! binary — every test file that wants these helpers does `mod common;`.

#![allow(dead_code)] // not every helper is used by every test binary that includes this module.

use claude_bus::proto::{FromBus, ReplyResult, ToBus};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_tungstenite::tungstenite::Message;

pub type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// How an `InProcessAgent`'s copy of `agent::run_on` gets driven.
enum Runner {
    /// Spawned with plain `tokio::spawn` onto whatever runtime is calling
    /// `InProcessAgent::start` — the common case. This is required (not just
    /// convenient) for anything that wants `tokio::time::pause()` to reach
    /// the 10s timeout in `Handler::request`: time pausing is a per-runtime
    /// setting, and `#[tokio::test(start_paused = true)]` only pauses the
    /// test's own runtime, so the agent has to run on that same runtime for
    /// the pause to apply to it.
    Shared(tokio::task::JoinHandle<()>),
    /// Spawned onto a dedicated `Runtime` this agent owns outright. See
    /// `InProcessAgent::start_isolated` for why a test would want this
    /// instead.
    Isolated(tokio::runtime::Runtime),
}

/// In-process replacement for spawning `claude-bus agent` as a child process
/// and speaking MCP over its stdin/stdout pipes. Wires a `tokio::io::duplex()`
/// pair straight into `claude_bus::agent::run_on` in place of the process's
/// real stdio — same `Handler`, same `bridge`, same rmcp codec and JSON-RPC
/// framing, just without the process boundary. The bus side is untouched: a
/// real `start_bus*` over a real socket is still what this agent connects to.
///
/// Deliberately raw, not rmcp's own client machinery: tests read and write
/// newline-delimited JSON directly, exactly as the old subprocess-driven
/// tests did (and exactly what rmcp's own codec puts on the wire — see
/// `JsonRpcMessageCodec` — so this is the real wire format, not a shortcut).
/// That keeps every existing assertion — tool names, capability shape,
/// instructions text, notification meta keys — byte-for-byte unchanged; only
/// the transport underneath moved.
pub struct InProcessAgent {
    to_agent: tokio::io::DuplexStream,
    from_agent: BufReader<tokio::io::DuplexStream>,
    runner: Option<Runner>,
}

impl InProcessAgent {
    /// Spawns `agent::run_on` onto the caller's current Tokio runtime (must
    /// be called from inside one, e.g. a `#[tokio::test]` body), wired to a
    /// fresh pair of duplex pipes.
    pub fn start(bus_url: impl Into<String>, name: impl Into<String>) -> Self {
        let (to_agent, from_agent, agent_stdin, agent_stdout) = Self::pipes();
        let bus_url = bus_url.into();
        let name = name.into();
        let task = tokio::spawn(Self::run(
            agent_stdin,
            agent_stdout,
            bus_url,
            name,
            claude_bus::agent::bridge::Liveness::default(),
        ));
        Self {
            to_agent,
            from_agent,
            runner: Some(Runner::Shared(task)),
        }
    }

    /// Same as `start`, but on a small dedicated `Runtime` that gets torn
    /// down (not just have this one task aborted) on `Drop`.
    ///
    /// `agent::run_on` spawns the bus-reconnecting bridge as its own
    /// top-level task, independent of the MCP service task it returns a
    /// handle to. A real killed process takes every task with it for free —
    /// the OS just closes every file descriptor, bridge's WebSocket
    /// included. Aborting only the service task's `JoinHandle` does not: the
    /// bridge keeps running, and the bus connection it holds open stays up,
    /// on whatever runtime it was spawned onto. Giving this agent its own
    /// `Runtime` and calling `shutdown_background` on the whole thing is what
    /// makes "drop the harness" actually behave like "kill the process" from
    /// the bus's point of view — which
    /// `ack_advances_the_cursor_so_reconnect_reports_only_genuinely_unseen_messages`
    /// depends on to see its dropped agent go offline.
    pub fn start_isolated(bus_url: impl Into<String>, name: impl Into<String>) -> Self {
        Self::start_isolated_with_liveness(
            bus_url,
            name,
            claude_bus::agent::bridge::Liveness::default(),
        )
    }

    /// Same as `start_isolated`, but with an injectable liveness cadence.
    pub fn start_isolated_with_liveness(
        bus_url: impl Into<String>,
        name: impl Into<String>,
        liveness: claude_bus::agent::bridge::Liveness,
    ) -> Self {
        let (to_agent, from_agent, agent_stdin, agent_stdout) = Self::pipes();
        let bus_url = bus_url.into();
        let name = name.into();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("build agent runtime");
        runtime.spawn(Self::run(
            agent_stdin,
            agent_stdout,
            bus_url,
            name,
            liveness,
        ));
        Self {
            to_agent,
            from_agent,
            runner: Some(Runner::Isolated(runtime)),
        }
    }

    /// One duplex pair stands in for the agent's stdin (test writes, agent
    /// reads), another for its stdout (agent writes, test reads) — mirrors
    /// the two separate pipes a real child process would have.
    #[allow(clippy::type_complexity)]
    fn pipes() -> (
        tokio::io::DuplexStream,
        BufReader<tokio::io::DuplexStream>,
        tokio::io::DuplexStream,
        tokio::io::DuplexStream,
    ) {
        let (to_agent, agent_stdin) = tokio::io::duplex(64 * 1024);
        let (agent_stdout, from_agent) = tokio::io::duplex(64 * 1024);
        (
            to_agent,
            BufReader::new(from_agent),
            agent_stdin,
            agent_stdout,
        )
    }

    async fn run(
        agent_stdin: tokio::io::DuplexStream,
        agent_stdout: tokio::io::DuplexStream,
        bus_url: String,
        name: String,
        liveness: claude_bus::agent::bridge::Liveness,
    ) {
        if let Err(e) = claude_bus::agent::run_on_with_liveness(
            (agent_stdin, agent_stdout),
            bus_url,
            name,
            liveness,
        )
        .await
        {
            eprintln!("[InProcessAgent] run_on exited with an error: {e}");
        }
    }

    pub async fn send(&mut self, v: serde_json::Value) {
        let line = format!("{v}\n");
        self.to_agent
            .write_all(line.as_bytes())
            .await
            .expect("write to agent stdin");
    }

    pub async fn next_json(&mut self) -> serde_json::Value {
        let mut line = String::new();
        self.from_agent
            .read_line(&mut line)
            .await
            .expect("read agent stdout");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad json {line:?}: {e}"))
    }

    /// Read agent stdout until a notification with the given method appears.
    pub async fn next_notification(&mut self, method: &str) -> serde_json::Value {
        for _ in 0..50 {
            let v = self.next_json().await;
            if v["method"] == method {
                return v;
            }
        }
        panic!("never saw a {method} notification");
    }
}

impl Drop for InProcessAgent {
    fn drop(&mut self) {
        match self.runner.take() {
            // Aborting just the service task is all `start` offers: the
            // bridge task (and its bus connection) lives on until whatever
            // runtime it shares with the rest of the test tears down at the
            // test's end. Fine for every test except the one that spawns via
            // `start_isolated` instead.
            Some(Runner::Shared(task)) => task.abort(),
            // `shutdown_background` (rather than a plain `drop`, which blocks
            // the current thread waiting for tasks to finish) tears the
            // runtime down immediately, taking the bridge task and its live
            // WebSocket connection with it — even mid-reconnect-backoff.
            Some(Runner::Isolated(rt)) => rt.shutdown_background(),
            None => {}
        }
    }
}

/// The bridge is spawned only after the MCP handshake completes
/// (`src/agent/mod.rs:52-64`), so every test driving an `InProcessAgent` has
/// to perform it first or it is testing nothing at all.
pub async fn initialize(a: &mut InProcessAgent) -> serde_json::Value {
    a.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "harness", "version": "1" }
        }
    }))
    .await;
    a.next_json().await
}

/// Common plumbing behind all the `start_bus*` variants below: bind an
/// ephemeral port, spawn the bus on it with the given `Guards`/`Keepalive`/
/// `Registry`, and hand back the temp data dir (so a test can open a second
/// `Store` against the same database), the port, and the dir's path.
async fn start_bus_full(
    guards: claude_bus::bus::delivery::Guards,
    keepalive: claude_bus::bus::Keepalive,
    registry: claude_bus::bus::registry::Registry,
    relayers: claude_bus::bus::Relayers,
) -> (tempfile::TempDir, u16, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let serve_path = path.clone();
    tokio::spawn(async move {
        claude_bus::bus::serve_on_full(listener, serve_path, guards, keepalive, registry, relayers)
            .await
            .unwrap();
    });
    wait_until_bus_ready(port).await;
    (dir, port, path)
}

/// Poll an async condition until it holds or the deadline expires. Returns
/// `false` on timeout so the caller can fail with its own descriptive
/// message.
///
/// Trap to avoid at call sites: converting `sleep; assert!(x)` into
/// `wait_until(|| x); assert!(x)` makes the assertion vacuous — once the
/// poll succeeds the assert can never fail, a broken product just times out
/// instead. Prefer `assert!(wait_until(...).await, "descriptive message")`
/// so a broken product still fails loudly. And never use this to poll a
/// *negative* claim ("X never happens") — polling until "X hasn't happened
/// yet" succeeds instantly and proves nothing; those need a real elapsed
/// wait so the forbidden thing has genuine opportunity to occur.
pub async fn wait_until<F, Fut>(f: F) -> bool
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    wait_until_timeout(std::time::Duration::from_secs(5), f).await
}

/// Minimal HTTP/1.1 GET, matching `tests/web.rs`'s no-dependency style (this
/// module cannot depend on that file, so the request-building logic is
/// duplicated rather than shared). Returns the whole raw response so
/// `get_json`/`get_status` can pull out the piece each one wants.
async fn get_raw(port: u16, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}

/// GET `path` and parse the response body as JSON.
pub async fn get_json(port: u16, path: &str) -> serde_json::Value {
    let raw = get_raw(port, path).await;
    let body = raw.rsplit("\r\n\r\n").next().unwrap();
    serde_json::from_str(body.trim()).unwrap_or_else(|e| panic!("bad json {body:?}: {e}"))
}

/// GET `path` and return just the numeric status code, for tests that only
/// care whether the route answered 404/200/etc.
pub async fn get_status(port: u16, path: &str) -> u16 {
    let raw = get_raw(port, path).await;
    status_of(&raw)
}

/// Minimal HTTP/1.1 POST with a JSON body, matching `delete_raw`'s shape.
/// `origin` is the `Origin` header to send, or `None` to send none at all —
/// the same distinction `delete_raw` draws for the DELETE path.
async fn post_json_raw(
    port: u16,
    path: &str,
    body: serde_json::Value,
    origin: Option<&str>,
) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let payload = body.to_string();
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let mut req = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Content-Length: {len}\r\nConnection: close\r\n",
        len = payload.len(),
    );
    if let Some(o) = origin {
        req.push_str(&format!("Origin: {o}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(&payload);
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}

/// POST `path` with a JSON body and no `Origin` header — like curl, which
/// state-changing routes must allow since such a caller could already reach
/// the port directly. Returns just the numeric status code.
pub async fn post_json(port: u16, path: &str, body: serde_json::Value) -> u16 {
    let raw = post_json_raw(port, path, body, None).await;
    status_of(&raw)
}

/// Same as `post_json`, but with an explicit `Origin` header — for asserting
/// a cross-origin refusal on a state-changing POST, the same distinction
/// `delete_with_origin` draws for the DELETE path. Returns just the numeric
/// status code.
pub async fn post_json_with_origin(
    port: u16,
    path: &str,
    body: serde_json::Value,
    origin: &str,
) -> u16 {
    let raw = post_json_raw(port, path, body, Some(origin)).await;
    status_of(&raw)
}

/// Pull the numeric status code off a raw HTTP response's status line.
fn status_of(raw: &str) -> u16 {
    raw.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("could not parse a status code out of: {raw:?}"))
}

/// Minimal HTTP/1.1 DELETE, matching `get_raw`'s no-dependency style. `origin`
/// is the `Origin` header to send, or `None` to send none at all — the same
/// distinction `tests/web.rs`'s `post`/`post_with_headers` draw for the HTML
/// delete's same-origin check.
async fn delete_raw(port: u16, path: &str, origin: Option<&str>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let mut req = format!(
        "DELETE {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n"
    );
    if let Some(o) = origin {
        req.push_str(&format!("Origin: {o}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}

/// DELETE `path` with an `Origin` naming the same host as the `Host` header
/// every helper here hardcodes (`localhost`) — the same-origin case a real
/// console `fetch` sends. Returns just the status code.
pub async fn delete_same_origin(port: u16, path: &str) -> u16 {
    delete_with_origin(port, path, "http://localhost").await
}

/// DELETE `path` with an arbitrary `Origin`, for asserting the cross-origin
/// refusal. Returns just the status code.
pub async fn delete_with_origin(port: u16, path: &str, origin: &str) -> u16 {
    let raw = delete_raw(port, path, Some(origin)).await;
    status_of(&raw)
}

/// DELETE `path` with no `Origin` header at all — like curl, or any
/// non-browser caller. The same-origin check must allow this: such a caller
/// could already reach the port directly, so refusing it buys nothing. See
/// `origin_matches_host`'s doc comment and `src/web/mod.rs`'s module doc.
/// Returns just the status code.
pub async fn delete_no_origin(port: u16, path: &str) -> u16 {
    let raw = delete_raw(port, path, None).await;
    status_of(&raw)
}

/// Same as `wait_until`, but with an explicit deadline instead of the 5s
/// default — used by `wait_until_bus_ready`, which wants longer.
pub async fn wait_until_timeout<F, Fut>(timeout: std::time::Duration, f: F) -> bool
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if f().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Poll `GET /` on `port` until the bus actually answers HTTP requests,
/// instead of guessing with a fixed sleep after spawning the server task.
///
/// The `TcpListener` is bound *before* the server task is spawned, so the
/// port accepts connections into the kernel backlog immediately — a bare
/// TCP connect proves nothing about whether the bus is actually serving
/// requests yet. The web UI is mounted on every `serve_on*` variant, so `/`
/// is a valid readiness signal no matter which flavor of bus was started.
pub async fn wait_until_bus_ready(port: u16) {
    let ready = wait_until_timeout(std::time::Duration::from_secs(10), || async move {
        let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await else {
            return false;
        };
        let req = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        if stream.write_all(req).await.is_err() {
            return false;
        }
        let mut buf = Vec::new();
        // A short per-attempt read timeout: a connection accepted into the
        // kernel backlog before the server's accept loop is actually
        // running would otherwise just hang here instead of letting the
        // outer poll retry with a fresh connection.
        matches!(
            tokio::time::timeout(
                std::time::Duration::from_millis(200),
                stream.read_to_end(&mut buf),
            )
            .await,
            Ok(Ok(_)) if buf.starts_with(b"HTTP/1.1 200")
        )
    })
    .await;
    assert!(
        ready,
        "bus on 127.0.0.1:{port} never answered GET / within the startup deadline"
    );
}

/// One-shot check of whether the bus currently reports `name` as online.
/// Connects as a throwaway probe agent and asks `ListAgents`. Meant to be
/// driven through `wait_until`/`wait_until_timeout` — e.g. to confirm a
/// disconnect's teardown has actually landed before an assertion that
/// depends on it, rather than guessing with a fixed sleep.
pub async fn agent_is_online(port: u16, name: &str) -> bool {
    let mut probe = connect(port, "probe").await;
    next_event(&mut probe).await; // Registered
    send(&mut probe, &ToBus::ListAgents { req_id: 0 }).await;
    matches!(
        next_event(&mut probe).await,
        FromBus::Reply { result: ReplyResult::Agents { agents }, .. }
            if agents.iter().any(|a| a.name == name && a.online)
    )
}

/// Same as `start_bus`, but also hands back the bus's data directory so a
/// test can open a second `Store` against the same database and read what
/// the bus wrote.
pub async fn start_bus_with_dir() -> (tempfile::TempDir, u16, std::path::PathBuf) {
    // Rate limit disabled: these tests send bursts deliberately. The
    // exchange cap stays at its default so the runaway test exercises it.
    let guards = claude_bus::bus::delivery::Guards::new(claude_bus::bus::delivery::DEFAULT_CAP, 0);
    start_bus_full(
        guards,
        claude_bus::bus::Keepalive::default(),
        claude_bus::bus::registry::Registry::new(),
        claude_bus::bus::Relayers::default(),
    )
    .await
}

pub async fn start_bus() -> (tempfile::TempDir, u16) {
    let (dir, port, _path) = start_bus_with_dir().await;
    (dir, port)
}

/// Same as `start_bus_with_dir`, but the caller supplies `Guards` directly —
/// for tests that need the rate limit *enabled* (`start_bus_with_dir`
/// deliberately disables it, since most callers send bursts on purpose).
pub async fn start_bus_with_guards_dir(
    guards: claude_bus::bus::delivery::Guards,
) -> (tempfile::TempDir, u16, std::path::PathBuf) {
    start_bus_full(
        guards,
        claude_bus::bus::Keepalive::default(),
        claude_bus::bus::registry::Registry::new(),
        claude_bus::bus::Relayers::default(),
    )
    .await
}

/// Same as `start_bus_with_dir`, but with a configured relayer set.
pub async fn start_bus_with_relayers_dir(
    names: impl IntoIterator<Item = String>,
) -> (tempfile::TempDir, u16, std::path::PathBuf) {
    let guards = claude_bus::bus::delivery::Guards::new(claude_bus::bus::delivery::DEFAULT_CAP, 0);
    start_bus_full(
        guards,
        claude_bus::bus::Keepalive::default(),
        claude_bus::bus::registry::Registry::new(),
        claude_bus::bus::Relayers::new(names),
    )
    .await
}

pub async fn start_bus_with_relayers(
    names: impl IntoIterator<Item = String>,
) -> (tempfile::TempDir, u16) {
    let (dir, port, _path) = start_bus_with_relayers_dir(names).await;
    (dir, port)
}

/// Same as `start_bus_with_dir`, but with an injectable keepalive cadence so
/// the "vanished peer" / keepalive-timeout tests don't have to sleep for the
/// production 30s/90s timeout.
pub async fn start_bus_with_keepalive_dir(
    ping_interval: std::time::Duration,
    pong_timeout: std::time::Duration,
) -> (tempfile::TempDir, u16, std::path::PathBuf) {
    let guards = claude_bus::bus::delivery::Guards::new(claude_bus::bus::delivery::DEFAULT_CAP, 0);
    let keepalive = claude_bus::bus::Keepalive::new(ping_interval, pong_timeout);
    start_bus_full(
        guards,
        keepalive,
        claude_bus::bus::registry::Registry::new(),
        claude_bus::bus::Relayers::default(),
    )
    .await
}

pub async fn start_bus_with_keepalive(
    ping_interval: std::time::Duration,
    pong_timeout: std::time::Duration,
) -> (tempfile::TempDir, u16) {
    let (dir, port, _path) = start_bus_with_keepalive_dir(ping_interval, pong_timeout).await;
    (dir, port)
}

/// Same as `start_bus`, but the caller supplies (and keeps a clone of) the
/// `Registry`, so a test can reach in and call `Registry::send_to` directly
/// against a connection the running bus already has live. `cap` sets the
/// exchange guard's cap directly (rate limit stays disabled), so a test that
/// needs to trip `Paused` repeatedly and cheaply can use a cap of 1 instead
/// of burning through the production default of 20 each time.
pub async fn start_bus_with_registry(
    registry: claude_bus::bus::registry::Registry,
    cap: u32,
) -> (tempfile::TempDir, u16) {
    let (dir, port, _path) = start_bus_full(
        claude_bus::bus::delivery::Guards::new(cap, 0),
        claude_bus::bus::Keepalive::default(),
        registry,
        claude_bus::bus::Relayers::default(),
    )
    .await;
    (dir, port)
}

pub async fn connect(port: u16, name: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    let reg = ToBus::Register {
        name: name.into(),
        host: "testhost".into(),
        cwd: format!("/w/{name}"),
        session_id: Some(format!("sess-{name}")),
        human: false,
        version: None,
    };
    ws.send(Message::text(serde_json::to_string(&reg).unwrap()))
        .await
        .unwrap();
    ws
}

/// Like `connect`, but registers with `human: true` — a person joining
/// rather than an agent.
pub async fn connect_human(port: u16, name: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    let reg = ToBus::Register {
        name: name.into(),
        host: "testhost".into(),
        cwd: format!("/w/{name}"),
        session_id: Some(format!("sess-{name}")),
        human: true,
        version: None,
    };
    ws.send(Message::text(serde_json::to_string(&reg).unwrap()))
        .await
        .unwrap();
    ws
}

/// Like `connect`, but with an explicit reported version — `None` stands in for an
/// agent binary predating the field.
pub async fn connect_versioned(port: u16, name: &str, version: Option<&str>) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    let reg = ToBus::Register {
        name: name.into(),
        host: "testhost".into(),
        cwd: format!("/w/{name}"),
        session_id: Some(format!("sess-{name}")),
        human: false,
        version: version.map(String::from),
    };
    ws.send(Message::text(serde_json::to_string(&reg).unwrap()))
        .await
        .unwrap();
    ws
}

/// Like `connect`, but identifies via `Observe` instead of `Register` — a
/// viewer, not a participant. See `ToBus::Observe`.
pub async fn connect_observer(port: u16, name: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    let obs = ToBus::Observe { name: name.into() };
    ws.send(Message::text(serde_json::to_string(&obs).unwrap()))
        .await
        .unwrap();
    ws
}

pub async fn next_event(ws: &mut Ws) -> FromBus {
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a bus event")
            .expect("stream ended")
            .expect("ws error");
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).expect("parse FromBus");
        }
    }
}

/// The next `Message` on this connection, skipping control frames that may be
/// interleaved ahead of it.
///
/// Deliberately separate from `next_event` rather than a change to it: several
/// tests assert on exact frame *ordering*, and making the shared helper skip
/// frames would silently weaken them. Use this only where the test's subject is
/// "a message arrives", not "this frame arrives next".
///
/// The interleaving is real and timing-dependent — the bus can deliver an
/// `Unread` summary on the same connection, and which lands first depends on
/// scheduling. Asserting on the very next frame made
/// `a_dm_reaches_a_connected_agent` fail under CPU contention with
/// "expected Message, got Unread". A sleep would only widen the window; skipping
/// non-`Message` frames is what actually expresses the assertion.
pub async fn next_message(ws: &mut Ws) -> FromBus {
    loop {
        let event = next_event(ws).await;
        if matches!(event, FromBus::Message { .. }) {
            return event;
        }
    }
}

/// Like `next_event`, but skips over the flood of `FromBus::Message` events
/// from "attacker" that the routing-queue-pressure tests push through
/// `Registry::send_to` directly. Those exist only to occupy the queue while
/// it's being filled; once the writer task gets scheduled it drains and
/// forwards them like any other routed message, so a test waiting for a
/// specific reply afterward has to look past them.
pub async fn next_non_flood_event(ws: &mut Ws) -> FromBus {
    loop {
        let ev = next_event(ws).await;
        if matches!(&ev, FromBus::Message { from, .. } if from == "attacker") {
            continue;
        }
        return ev;
    }
}

pub async fn send(ws: &mut Ws, cmd: &ToBus) {
    ws.send(Message::text(serde_json::to_string(cmd).unwrap()))
        .await
        .unwrap();
}

/// Keeps polling `ws` for `duration` without expecting any particular event.
///
/// `tokio-tungstenite` only flushes its automatic `Pong` reply the next time
/// something reads (or writes) the stream — see `WebSocket::write`'s docs.
/// A connection that is simply waiting (e.g. via a bare `sleep`) is
/// therefore indistinguishable, from the bus's point of view, from a
/// genuinely vanished peer: nobody is pumping it, so no pong goes out. Tests
/// that want a *live* connection to survive an idle period need to pump it
/// like this instead of sleeping past it.
pub async fn pump_for(ws: &mut Ws, duration: std::time::Duration) {
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let _ = tokio::time::timeout(
            remaining.min(std::time::Duration::from_millis(20)),
            ws.next(),
        )
        .await;
    }
}

pub fn flood_message() -> FromBus {
    FromBus::Message {
        id: 0,
        room: "flood".into(),
        from: "attacker".into(),
        text: "x".into(),
        done: false,
        human: false,
    }
}

/// Keeps a connection's routing queue saturated for as long as `active`
/// stays `true`, by calling `Registry::send_to` directly (the same path
/// other connections' room/DM fan-out goes through) in a tight loop.
///
/// A one-shot fill isn't enough to prove control traffic is immune to
/// routing pressure: the writer task drains the queue as fast as it can
/// forward messages onto the socket, so by the time a test's own
/// request/reply round trip actually reaches the point of contention, a
/// single burst has usually already drained away. Continuously refilling —
/// racing the writer's drain rate — is what reproduces sustained pressure,
/// which is the scenario the review's finding actually describes (a
/// connection that is a member of *several concurrently busy rooms*, not
/// one that received one burst and then went quiet).
///
/// `yield_now` between sends is required, not cosmetic: `tokio::sync::Mutex`
/// resolves synchronously when uncontended, so a bare `while active.load()
/// { send_to(...).await }` never actually yields to the scheduler and starves
/// every other task on a single-threaded runtime — including the writer task
/// this is racing against, and the test's own main task.
pub async fn flood_continuously(
    registry: claude_bus::bus::registry::Registry,
    name: String,
    active: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    while active.load(Ordering::Relaxed) {
        // Re-saturate the queue in one uninterrupted burst (no `.await`
        // yield point inside this inner loop — `try_send` and an
        // uncontended `tokio::sync::Mutex` both resolve synchronously) so
        // that whenever another task — the writer draining it, or this
        // connection's own read loop trying to enqueue a reply — actually
        // gets to run, it is as likely as possible to see the queue at
        // capacity, not mid-drain.
        while registry.send_to(&name, flood_message()).await {}
        tokio::task::yield_now().await;
    }
}
