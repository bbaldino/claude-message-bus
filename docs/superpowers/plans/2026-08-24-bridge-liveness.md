# Agent Bridge Liveness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The agent bridge notices a connection that has gone silent and reconnects, instead of waiting forever on a socket the bus has already closed.

**Architecture:** One `select!` arm in `connect_once` that fires on a ticker: it declares the connection dead if no inbound frame has arrived within the idle timeout, and otherwise sends a `Ping`. The cadence is injected the same way the bus injects `Keepalive`, so tests run it in milliseconds.

**Tech Stack:** Rust, tokio, tokio-tungstenite 0.30, rmcp.

**Spec:** `docs/superpowers/specs/2026-08-24-bridge-liveness-design.md`

## Global Constraints

- Commit prefixes: `release_commits = "^(feat|fix)[(!:]"` in `release-plz.toml` cuts a release on `feat:`/`fix:`. **This is a bug fix and should ship**, so Task 1 uses `fix:`. Task 2 commits nothing.
- `cargo +nightly fmt`; clippy clean with `cargo +stable clippy --all-targets --all-features -- -D warnings`.
- Gate: `cargo +nightly fmt && cargo +stable clippy --all-targets --all-features -- -D warnings && cargo +stable test --locked`.
- **Nothing may delete from the `messages` or `events` tables.**
- Nothing under `ui/` changes in this plan. `ui/src/types/` is ts-rs output — never hand-edit it.
- Production cadence is **30s ping / 90s idle timeout**, mirroring `Keepalive`'s defaults at `src/bus/mod.rs:62-63`.
- Out of scope, from the spec: `tail`, `chat`, the console's browser socket, and any change to the bus.
- Every behavioural test must be confirmed to fail before the change exists. Watch it fail; do not assert that it would.

## Facts verified while writing this plan

Each was checked against the source, not assumed:

- **The bridge is spawned only after the MCP handshake completes.** `agent::run_on` calls `handler.serve(transport).await?` and then `tokio::spawn(bridge::run(...))` (`src/agent/mod.rs:52-64`). A test that never sends `initialize` gets no bridge at all, and would pass or hang for the wrong reason.
- **`InProcessAgent::start_isolated`** (`tests/common/mod.rs:84-99`) gives the agent its own `Runtime` torn down on `Drop`. Its doc comment records why: the bridge is a top-level task, so aborting the service task's handle leaves the bridge running and its socket open. This plan's tests reconnect in a loop, so they must use the isolated constructor.
- **`InProcessAgent::run`** (`tests/common/mod.rs:121-131`) is the single call site of `claude_bus::agent::run_on` in the harness — the one place a cadence parameter has to thread through.
- **The bus's ping payload** is `Message::Ping(Vec::new().into())` (`src/bus/mod.rs:444`); `Message` is already imported in `bridge.rs`.
- **`initialize` is private to `tests/agent_contract.rs:74-85`**, not shared in `tests/common`. The new test file needs its own copy; it is ten lines and duplicating it is cheaper than moving a helper other tests depend on.
- **`tokio-tungstenite = "0.30.0"`** is a normal dependency with default features (`Cargo.toml:30`), so `accept_async` is available to tests.

---

### Task 1: The bridge gives up on a silent connection

**Files:**
- Modify: `src/agent/bridge.rs` (the `BridgeConfig` struct at :20-26, and `connect_once` at :57-97)
- Modify: `src/agent/mod.rs:17-19` and `:54-64`
- Modify: `tests/common/mod.rs:84-99` and `:121-131`
- Test: `tests/bridge_liveness.rs` (create)

**Interfaces:**
- Produces: `claude_bus::agent::bridge::Liveness { ping_interval: Duration, idle_timeout: Duration }`, `Default` = 30s/90s
- Produces: `claude_bus::agent::run_on_with_liveness(transport, bus_url, name, liveness)`
- Produces: `common::InProcessAgent::start_isolated_with_liveness(bus_url, name, liveness)`

- [ ] **Step 1: Add the cadence type and thread it through, with no behaviour change yet**

In `src/agent/bridge.rs`, add below the existing `BACKOFF_FLOOR` constant:

```rust
/// How often the bridge pings the bus, and how long it waits for any inbound
/// frame before deciding the connection is dead.
///
/// Injected the way the bus injects `Keepalive`, and for the same reason: the
/// production cadence is minutes, so a test that used it would have to sleep
/// for minutes.
///
/// The client pings rather than only listening. Relying on the bus's pings
/// alone would need no ticker at all, but it would couple this timeout to the
/// peer's configured cadence — and that cadence is configurable
/// (`Keepalive::new`). Anyone lengthening the bus's ping interval past this
/// timeout would turn every idle connection into a reconnect loop, with
/// nothing in either file to warn them.
#[derive(Clone, Copy, Debug)]
pub struct Liveness {
    pub ping_interval: Duration,
    pub idle_timeout: Duration,
}

impl Default for Liveness {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(90),
        }
    }
}
```

Add the field to `BridgeConfig`:

```rust
    pub session_id: Option<String>,
    pub liveness: Liveness,
```

In `src/agent/mod.rs`, replace `run_on` with a delegating pair. Keep `run` as it is:

```rust
pub async fn run_on<T, E, A>(transport: T, bus_url: String, name: String) -> anyhow::Result<()>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    run_on_with_liveness(transport, bus_url, name, bridge::Liveness::default()).await
}

/// Same as `run_on`, but with an injectable liveness cadence so tests don't
/// have to wait out the production 30s/90s.
pub async fn run_on_with_liveness<T, E, A>(
    transport: T,
    bus_url: String,
    name: String,
    liveness: bridge::Liveness,
) -> anyhow::Result<()>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
```

The old body of `run_on` becomes the body of `run_on_with_liveness`. In its `BridgeConfig`
literal, add the field:

```rust
        session_id: env.var("CLAUDE_CODE_SESSION_ID"),
        liveness,
    };
```

In `tests/common/mod.rs`, give `run` the parameter and add the constructor. `start` and
`start_isolated` keep their signatures and pass the default:

```rust
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
        runtime.spawn(Self::run(agent_stdin, agent_stdout, bus_url, name, liveness));
        Self {
            to_agent,
            from_agent,
            runner: Some(Runner::Isolated(runtime)),
        }
    }
```

and change `run` itself:

```rust
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
```

Both existing call sites (`start` at :61 and `start_isolated` at :93) gain a fourth
argument: `claude_bus::agent::bridge::Liveness::default()`.

- [ ] **Step 2: Verify it still compiles and the suite is unchanged**

Run: `cargo +stable test --locked`
Expected: PASS, exactly as before. Nothing has changed behaviourally yet — this step
only proves the plumbing compiles and broke nothing.

- [ ] **Step 3: Write the failing tests**

Create `tests/bridge_liveness.rs`:

```rust
//! The bridge's own liveness detection.
//!
//! The bug these exist for: the bus dropped a sleeping laptop's agent on a
//! keepalive timeout and closed its side. That FIN was lost, the bus never
//! wrote to the socket again, and the client sat in `connect_once` — which
//! wakes only on "the model wants to send" or "bytes arrived" — forever. The
//! reconnect loop it already had never got a turn. Six days offline, process
//! alive, socket still ESTAB.

mod common;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

use claude_bus::agent::bridge::Liveness;
use common::InProcessAgent;

/// `tests/agent_contract.rs` keeps its own private copy of this; the bridge is
/// spawned only after the MCP handshake completes (`src/agent/mod.rs:52-64`),
/// so every test here has to perform it or it is testing nothing at all.
async fn initialize(a: &mut InProcessAgent) -> serde_json::Value {
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

/// A bus that completes the WebSocket handshake and then goes silent, without
/// ever closing — the exact shape a sleeping laptop leaves behind. Sends one
/// `()` per accepted handshake so a test can count reconnects.
async fn silent_bus() -> (u16, mpsc::UnboundedReceiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let tx = tx.clone();
            tokio::spawn(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else { return };
                let _ = tx.send(());
                // Held, never dropped: dropping sends a close frame, which is
                // precisely the signal this test exists to withhold.
                let _held = ws;
                std::future::pending::<()>().await;
            });
        }
    });
    (port, rx)
}

/// A bus that answers. Pings every 50ms and drains whatever the client sends,
/// which is also what makes tungstenite emit the automatic pongs.
async fn chatty_bus() -> (u16, mpsc::UnboundedReceiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let tx = tx.clone();
            tokio::spawn(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else { return };
                let _ = tx.send(());
                let (mut sink, mut stream) = ws.split();
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(50)) => {
                            if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                                return;
                            }
                        }
                        msg = stream.next() => {
                            if msg.is_none() { return }
                        }
                    }
                }
            });
        }
    });
    (port, rx)
}

async fn start_agent(port: u16, name: &str) -> InProcessAgent {
    let mut a = InProcessAgent::start_isolated_with_liveness(
        format!("ws://127.0.0.1:{port}/ws"),
        name,
        Liveness {
            ping_interval: Duration::from_millis(100),
            idle_timeout: Duration::from_millis(300),
        },
    );
    initialize(&mut a).await;
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    a
}

#[tokio::test]
async fn a_silent_bus_that_never_closes_still_gets_a_reconnect() {
    let (port, mut handshakes) = silent_bus().await;
    let _agent = start_agent(port, "sleeper").await;

    timeout(Duration::from_secs(5), handshakes.recv())
        .await
        .expect("the bridge never connected at all")
        .expect("handshake channel closed");

    timeout(Duration::from_secs(5), handshakes.recv())
        .await
        .expect(
            "the bridge never reconnected: it is still waiting on a socket the bus \
             will never write to, which is the bug",
        )
        .expect("handshake channel closed");
}

#[tokio::test]
async fn a_bus_that_keeps_talking_is_not_torn_down() {
    // THE REGRESSION THAT WOULD MATTER MOST. A timer that fires on a healthy
    // connection makes every long-lived agent flap, which is worse than the
    // bug being fixed: this one is silent and rare, that one is constant.
    let (port, mut handshakes) = chatty_bus().await;
    let _agent = start_agent(port, "chatty").await;

    timeout(Duration::from_secs(5), handshakes.recv())
        .await
        .expect("the bridge never connected at all")
        .expect("handshake channel closed");

    // Five times the idle timeout, with the bus talking the whole way.
    assert!(
        timeout(Duration::from_millis(1500), handshakes.recv())
            .await
            .is_err(),
        "a connection carrying traffic was torn down and rebuilt"
    );
}
```

- [ ] **Step 4: Run to verify the first test fails**

Run: `cargo +stable test --locked --test bridge_liveness`
Expected: `a_silent_bus_that_never_closes_still_gets_a_reconnect` FAILS on the second
`timeout`, panicking with "the bridge never reconnected". The 5s bound is what makes it
fail rather than hang.

`a_bus_that_keeps_talking_is_not_torn_down` **passes** before the change — with no timer
at all, nothing can tear anything down. It is a regression guard, green at both ends. Say
so in your report; Step 6 is what proves it can fail.

- [ ] **Step 5: Add the liveness arm**

In `src/agent/bridge.rs`, `connect_once`: after the `Register` frame is sent and before
the `loop`, add the ticker and the deadline:

```rust
    // Checked on a ticker rather than by racing a timer against the read, so
    // the granularity is one interval: detection lands within
    // `idle_timeout + ping_interval`.
    let mut liveness_ticker = tokio::time::interval(cfg.liveness.ping_interval);
    liveness_ticker.tick().await; // skip the immediate first tick
    let mut last_inbound = tokio::time::Instant::now();
```

Record inbound traffic. In the existing `inbound = stream.next()` arm, immediately after
the `let Some(msg) = inbound else { return Ok(()) };` line:

```rust
                // Any frame, not specifically a pong: it is strictly more
                // information, and a busy connection must not trip the timer
                // just because a pong queued behind a burst of messages.
                last_inbound = tokio::time::Instant::now();
```

Add the third arm to the `select!`, after the `inbound` arm:

```rust
            _ = liveness_ticker.tick() => {
                // Checked before pinging, so a connection already known to be
                // dead is not written into first.
                if last_inbound.elapsed() > cfg.liveness.idle_timeout {
                    eprintln!(
                        "[agent] no traffic from the bus in {:?}, assuming the connection is dead",
                        cfg.liveness.idle_timeout
                    );
                    anyhow::bail!("no traffic from the bus in {:?}", cfg.liveness.idle_timeout);
                }
                // The ping is also a write, so a dead socket eventually fails
                // here on its own — a second detection path that does not
                // depend on the timer above.
                sink.send(Message::Ping(Vec::new().into())).await?;
            }
```

- [ ] **Step 6: Run to verify both pass, then prove the guard can fail**

Run: `cargo +stable test --locked --test bridge_liveness`
Expected: PASS, both tests.

**Then prove `a_bus_that_keeps_talking_is_not_torn_down` can fail**: temporarily delete
the `last_inbound = tokio::time::Instant::now();` line you added to the `inbound` arm, so
the deadline never resets. Re-run and confirm that test FAILS — a healthy connection now
gets torn down every 300ms. Restore the line, re-run, confirm it passes again.

Report both observations with their output. That one line is the entire difference
between "detects death" and "flaps constantly", and nothing else in the suite covers it.

- [ ] **Step 7: Commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git add src tests
git commit -F - <<'EOF'
fix: the agent bridge detects a connection that has gone silent

The bus dropped a sleeping laptop's agent on a keepalive timeout and closed its
side. The FIN was lost to the sleeping host, the bus never wrote to that socket
again, and the client sat in connect_once — which wakes only when the model
wants to send or when bytes arrive — forever. Neither could ever happen again: a
passive agent never writes, and the only peer that would write was gone. The
reconnect loop the bridge already had never got a turn. Six days offline, with
the process alive and the socket still reading ESTAB.

The bridge now pings every 30s and treats 90s without any inbound frame as a
dead connection, mirroring the bus's own keepalive. The cadence is injected the
way Keepalive is, so tests run it in milliseconds.

The client pings rather than only listening: relying on the bus's pings would
couple this timeout to a cadence configured in another file, and lengthening it
there would make every idle connection flap.
EOF
```

---

### Task 2: Verify against a scratch bus at the production cadence

**Files:** none — verification only, no commit.

Task 1's tests run at 100ms/300ms. This task exercises the real 30s/90s, which is
the cadence that actually ships and the one no test covers.

- [ ] **Step 1: Run the gate**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
```

- [ ] **Step 2: Build and start a scratch bus**

```bash
cd ui && npm run build && cd ..
cargo build
rm -rf /tmp/claude-bus-liveness
./target/debug/claude-bus serve --port 7813 --data /tmp/claude-bus-liveness &
```

Build order is load-bearing — `rust-embed` compiles the UI bundle into the binary, and
a bus already running keeps its old copy. Use only this scratch bus: the real bus at
`claude-msg-bus.home:7777` carries live conversations and must not be touched.

- [ ] **Step 3: Attach a real agent**

The agent speaks MCP on stdin/stdout and only starts its bridge after the handshake, so
stdin must stay open. In one shell:

```bash
( printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"manual","version":"1"}}}'
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  sleep 900
) | ./target/debug/claude-bus agent --bus ws://127.0.0.1:7813/ws --name probe
```

Confirm it registered: `curl -s localhost:7813/api/agents` shows `probe` online.

- [ ] **Step 4: Confirm each of these and report the result for each**

1. **A healthy idle connection is not torn down.** Leave `probe` connected and idle for
   **3 minutes** (twice the idle timeout). `curl -s "localhost:7813/api/events?limit=20"`
   must show exactly one `agent_registered` for `probe` — no repeats. Flapping here
   would be worse than the bug being fixed.
2. **A silent bus is detected.** `kill -STOP <bus pid>`, then watch the agent's stderr.
   Within ~90–120s it must print `no traffic from the bus in 90s, assuming the connection
   is dead`, followed by `[agent] reconnecting in 1s` — the 1s floor, because the
   connection had been up longer than `STABLE_CONNECTION_THRESHOLD`.
3. **It recovers.** `kill -CONT <bus pid>`. `probe` must reconnect and appear online in
   `/api/agents` again, with a second `agent_registered` event.
4. **Queued messages arrive on that reconnect.** Before the `CONT`, send `probe` a
   message from another agent so it queues; after reconnect, the agent's stderr must show
   an unread summary line.

Note anything that looked wrong but you could not attribute. One thing is expected and is
not a defect to fix here: while the bus is stopped, a reconnect attempt can sit in
`connect_async` with no timeout of its own — the kernel completes the TCP handshake from
the listen backlog but the stopped process never completes the WebSocket handshake. It
resolves on `CONT`. Report it if you see it; it is out of scope for this plan.

- [ ] **Step 5: Clean up and report**

```bash
kill %1          # the scratch bus
rm -rf /tmp/claude-bus-liveness
```

Report each check's result. Commit nothing.

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task |
|---|---|
| Ping every 30s | 1 (Step 5 arm, `Liveness::default`) |
| 90s idle deadline, any inbound frame resets it | 1 (Step 5, the `last_inbound` line) |
| Return so the existing reconnect loop runs | 1 (`anyhow::bail!` in the arm) |
| Client pings rather than only listening | 1 (`Liveness` doc comment records why) |
| The ping is also a write | 1 (comment on the `sink.send`) |
| Distinct log line for the give-up path | 1 (Step 5 `eprintln!`) |
| Test: silent-but-open bus still gets a reconnect | 1 (`silent_bus`) |
| Test: a talking connection is not torn down | 1 (`chatty_bus`, plus the Step 6 proof) |
| `STABLE_CONNECTION_THRESHOLD` gives a fast retry | 2 (check 2 asserts the 1s floor) |
| Production 30s/90s exercised for real | 2 |
| No change to bus, `tail`, `chat`, console | — nothing in either task touches them |

**Placeholder scan:** no TBD/TODO, no "add error handling", no "similar to Task N". Every
code block is the literal text to write. The two harness details that would otherwise
send an implementer digging — that the bridge only spawns after the MCP handshake, and
that `initialize` is private to `agent_contract.rs` — are stated in Facts rather than left
to be rediscovered.

**Type consistency:** `Liveness { ping_interval, idle_timeout }` is defined in Task 1
Step 1 and used with those exact field names in Step 3's `start_agent`, Step 5's arm, and
nowhere else. `run_on_with_liveness` and `start_isolated_with_liveness` each have exactly
one definition and are called with matching arities. `BridgeConfig.liveness` is added in
the same step as the struct literal that sets it.

**One risk restated:** `a_bus_that_keeps_talking_is_not_torn_down` passes before the
change for a trivial reason — there is no timer to misfire yet. Step 6 requires proving it
can fail by deleting the one line that resets the deadline. Without that proof the test is
decoration, and the failure mode it guards against (every agent on the bus reconnecting
every 90 seconds) is worse than the bug this plan fixes.
