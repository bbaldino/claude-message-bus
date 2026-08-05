# Agent Delete Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a human delete an offline agent's row — and its stranded room memberships and cursors — from the claude-bus web UI.

**Architecture:** Two new routes under `/agents/{name}/delete`: a `GET` confirmation page showing exactly what will be removed, and a `POST` that performs it inside one transaction and redirects. Liveness is read from the in-memory `Registry`, never from the persisted `agents.online` column, and is re-checked on the `POST`. Messages and events are preserved; an `agent_deleted` event is appended so the deletion itself is auditable.

**Tech Stack:** Rust, axum 0.8.9, sqlx 0.9 (SQLite), tokio 1.53, server-rendered HTML (no JS, no template engine).

**Spec:** `docs/superpowers/specs/2026-08-05-agent-delete-design.md`

## Global Constraints

- Format with **nightly** rustfmt: `cargo +nightly fmt`. CI runs `cargo +nightly fmt --check`.
- Lints are blocking: `cargo +stable clippy --all-targets --all-features -- -D warnings`.
- Tests run with `cargo +stable test --locked`.
- **Commit types matter — this repo auto-releases.** `release-plz.toml` sets `release_commits = "^(feat|fix)[(!:]"`, and the release PR auto-merges. Every `feat:` or `fix:` commit cuts a version, tags it, and publishes an image. Use **`refactor:`** or **`chore:`** for the intermediate commits in Tasks 1–5, and a single **`feat:`** on Task 6 so the whole feature ships as one release.
- Never delete from `messages` or `events`. The spec keeps both deliberately.
- Only the first letter of a multi-letter acronym is capitalised in type names.

---

### Task 1: `Registry::is_online`

The liveness authority. `Registry::online()` returns the whole sorted `Vec<String>`; the guards need a single-name check without allocating that list on every request.

**Files:**
- Modify: `src/bus/registry.rs` (add method beside `online()` at line ~169; add test in the existing `mod tests`)

**Interfaces:**
- Consumes: nothing
- Produces: `Registry::is_online(&self, name: &str) -> bool` (async)

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `src/bus/registry.rs`, beside `online_lists_effective_names_sorted`:

```rust
    #[tokio::test]
    async fn is_online_reports_attached_names_only() {
        let reg = Registry::new();
        let (tx, _rx) = channel();
        reg.attach("network-debug", "hardac", tx).await;

        assert!(reg.is_online("network-debug").await);
        // The suffixed tombstone is a different name and must not be shadowed
        // by the live bare name.
        assert!(!reg.is_online("network-debug#2").await);
        assert!(!reg.is_online("never-existed").await);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib is_online_reports_attached_names_only`
Expected: FAIL — `no method named 'is_online' found for struct 'Registry'`

- [ ] **Step 3: Write minimal implementation**

Add directly after `online()` in `src/bus/registry.rs`:

```rust
    /// Whether `name` currently holds a live connection.
    ///
    /// The authority for liveness, in preference to the persisted
    /// `agents.online` column: that column is reconciled at startup by
    /// `Store::mark_all_offline`, but between reconciliations only this map
    /// knows who is actually routable.
    pub async fn is_online(&self, name: &str) -> bool {
        self.conns.lock().await.contains_key(name)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib is_online_reports_attached_names_only`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
git add src/bus/registry.rs
git commit -m "refactor: add Registry::is_online for single-name liveness checks"
```

---

### Task 2: `Store::agent_footprint`

What the confirmation page displays. Read-only. Returns room **names** rather than a count, because the page lists the memberships at risk individually.

**Files:**
- Modify: `src/store/mod.rs` (add `AgentFootprint` beside `AgentRow` near line 24; add method after `leave_all_rooms`, line ~279)
- Test: `tests/store.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub struct AgentFootprint { pub rooms: Vec<String>, pub cursors: i64 }`
  - `Store::agent_footprint(&self, name: &str) -> anyhow::Result<AgentFootprint>`

- [ ] **Step 1: Write the failing test**

Append to `tests/store.rs`:

```rust
#[tokio::test]
async fn agent_footprint_reports_rooms_and_cursor_count() {
    let (_d, store) = temp_store().await;
    store
        .upsert_agent("network-debug#2", "hardac", "/w/nd", None, false, None)
        .await
        .unwrap();
    store.join_room("protocol", "network-debug#2").await.unwrap();
    store.join_room("ops", "network-debug#2").await.unwrap();
    store.set_cursor("protocol", "network-debug#2", 7).await.unwrap();

    let fp = store.agent_footprint("network-debug#2").await.unwrap();

    // Sorted by the underlying query, so this comparison is stable.
    assert_eq!(fp.rooms, vec!["ops".to_string(), "protocol".to_string()]);
    assert_eq!(fp.cursors, 1);
}

#[tokio::test]
async fn agent_footprint_of_an_unknown_agent_is_empty() {
    let (_d, store) = temp_store().await;
    let fp = store.agent_footprint("nobody").await.unwrap();
    assert!(fp.rooms.is_empty());
    assert_eq!(fp.cursors, 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test store agent_footprint`
Expected: FAIL — `no method named 'agent_footprint'`

- [ ] **Step 3: Write minimal implementation**

Add near `AgentRow` in `src/store/mod.rs`:

```rust
/// What deleting an agent would remove, for display before it happens.
///
/// Rooms are names rather than a count because the confirmation page lists
/// them individually — a count would not tell anyone what they were losing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFootprint {
    pub rooms: Vec<String>,
    pub cursors: i64,
}
```

Add after `leave_all_rooms`:

```rust
    /// The rooms and cursors `forget_agent` would delete. Mutates nothing.
    pub async fn agent_footprint(&self, name: &str) -> anyhow::Result<AgentFootprint> {
        let rooms: Vec<String> =
            sqlx::query("SELECT room FROM room_members WHERE agent_name = ?1 ORDER BY room")
                .bind(name)
                .fetch_all(&self.pool)
                .await?
                .iter()
                .map(|r| r.get("room"))
                .collect();
        let cursors: i64 = sqlx::query("SELECT COUNT(*) AS n FROM cursors WHERE agent_name = ?1")
            .bind(name)
            .fetch_one(&self.pool)
            .await?
            .get("n");
        Ok(AgentFootprint { rooms, cursors })
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test store agent_footprint`
Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
git add src/store/mod.rs tests/store.rs
git commit -m "refactor: add Store::agent_footprint"
```

---

### Task 3: `Store::forget_agent`

The mutation, in one transaction. The store opens no transactions anywhere today; this is the first. It is warranted because a partial failure that drops the `agents` row while leaving memberships behind strands them with no route back — the row is what makes an agent visible in the UI and therefore deletable at all.

**Files:**
- Modify: `src/store/mod.rs` (add `ForgetCounts` beside `AgentFootprint`; add method after `agent_footprint`)
- Test: `tests/store.rs`

**Interfaces:**
- Consumes: `AgentFootprint` (Task 2) — same module, no coupling
- Produces:
  - `pub struct ForgetCounts { pub agents: u64, pub memberships: u64, pub cursors: u64 }`
  - `Store::forget_agent(&self, name: &str) -> anyhow::Result<ForgetCounts>`

- [ ] **Step 1: Write the failing test**

Append to `tests/store.rs`:

```rust
#[tokio::test]
async fn forget_agent_removes_row_memberships_and_cursors_but_keeps_history() {
    let (_d, store) = temp_store().await;
    store
        .upsert_agent("network-debug#2", "hardac", "/w/nd", None, false, None)
        .await
        .unwrap();
    store.join_room("protocol", "network-debug#2").await.unwrap();
    store.join_room("protocol", "caas").await.unwrap();
    store.set_cursor("protocol", "network-debug#2", 3).await.unwrap();
    store
        .append_message("protocol", "network-debug#2", "hello", false, false)
        .await
        .unwrap();
    store
        .append_event("agent_registered", Some("network-debug#2"), None, serde_json::json!({}))
        .await
        .unwrap();

    let counts = store.forget_agent("network-debug#2").await.unwrap();

    assert_eq!(counts.agents, 1);
    assert_eq!(counts.memberships, 1);
    assert_eq!(counts.cursors, 1);

    // Gone from the three tables it owns.
    assert!(!store.agents().await.unwrap().iter().any(|a| a.name == "network-debug#2"));
    assert_eq!(store.room_members("protocol").await.unwrap(), vec!["caas".to_string()]);
    assert_eq!(store.cursor("protocol", "network-debug#2").await.unwrap(), 0);

    // History and audit trail survive — this is the whole reason they are excluded.
    let msgs = store.history("protocol", 10).await.unwrap();
    assert_eq!(msgs.len(), 1, "the message must survive the delete");
    assert_eq!(msgs[0].from_agent, "network-debug#2");
    assert_eq!(
        store.events_for_agent("network-debug#2", 10).await.unwrap().len(),
        1,
        "the audit trail must survive the delete"
    );
}

#[tokio::test]
async fn forget_agent_on_an_unknown_name_removes_nothing_and_does_not_error() {
    let (_d, store) = temp_store().await;
    let counts = store.forget_agent("never-existed").await.unwrap();
    assert_eq!(counts.agents, 0);
    assert_eq!(counts.memberships, 0);
    assert_eq!(counts.cursors, 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test store forget_agent`
Expected: FAIL — `no method named 'forget_agent'`

- [ ] **Step 3: Write minimal implementation**

Add beside `AgentFootprint` in `src/store/mod.rs`:

```rust
/// Rows actually removed by `forget_agent`, for the audit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgetCounts {
    pub agents: u64,
    pub memberships: u64,
    pub cursors: u64,
}
```

Add after `agent_footprint`:

```rust
    /// Delete an agent's own rows: its `agents` entry, its room memberships,
    /// and its cursors. Messages and events are deliberately untouched — the
    /// transcript stays readable and the audit trail outlives the agent.
    ///
    /// Transactional because a partial failure is worse than none: losing the
    /// `agents` row while leaving memberships behind strands them, since the
    /// row is what makes an agent reachable in the UI and therefore deletable.
    pub async fn forget_agent(&self, name: &str) -> anyhow::Result<ForgetCounts> {
        let mut tx = self.pool.begin().await?;
        let memberships = sqlx::query("DELETE FROM room_members WHERE agent_name = ?1")
            .bind(name)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let cursors = sqlx::query("DELETE FROM cursors WHERE agent_name = ?1")
            .bind(name)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let agents = sqlx::query("DELETE FROM agents WHERE name = ?1")
            .bind(name)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(ForgetCounts {
            agents,
            memberships,
            cursors,
        })
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test store forget_agent`
Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
git add src/store/mod.rs tests/store.rs
git commit -m "refactor: add Store::forget_agent"
```

---

### Task 4: The confirmation page (`GET /agents/{name}/delete`)

**Files:**
- Modify: `src/web/mod.rs` (add `delete_agent_confirm` handler; register route in `routes()` at line ~235)
- Test: `tests/web.rs`

**Interfaces:**
- Consumes: `Store::agent_footprint` (Task 2), `Registry::is_online` (Task 1)
- Produces: route `GET /agents/{name}/delete`. No new types — existence is checked by filtering `store.agents()` inline.

- [ ] **Step 1: Write the failing test**

Append to `tests/web.rs`:

```rust
#[tokio::test]
async fn the_confirm_page_lists_what_will_be_removed() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("network-debug#2", "hardac", "/w/nd", None, false, None)
            .await
            .unwrap();
        store.join_room("protocol", "network-debug#2").await.unwrap();
        store.set_cursor("protocol", "network-debug#2", 4).await.unwrap();
    }
    let port = start(dir.path()).await;

    // `#` must be percent-encoded or the path silently truncates at the fragment.
    let body = get(port, "/agents/network-debug%232/delete").await;

    assert!(body.contains("network-debug#2"), "name must appear: {body}");
    assert!(body.contains("protocol"), "the membership at risk must be listed");
    assert!(body.contains("1 cursor"), "the cursor count must appear");
    assert!(
        body.contains("messages and events are kept"),
        "the page must say what survives"
    );
    assert!(body.contains("<form"), "an offline agent must get a real button");
}

#[tokio::test]
async fn the_confirm_page_refuses_an_online_agent() {
    let (_d, port, path) = common::start_bus_with_dir().await;
    let _ws = common::connect(port, "caas").await;
    assert!(common::agent_is_online(port, "caas").await);
    let _ = path;

    let body = get(port, "/agents/caas/delete").await;

    assert!(body.contains("online"), "the refusal reason must be shown: {body}");
    assert!(
        !body.contains("<form"),
        "there must be no button that is known to fail"
    );
}

#[tokio::test]
async fn the_confirm_page_of_an_unknown_agent_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;

    let body = get(port, "/agents/nobody/delete").await;

    assert!(body.contains("no agent named nobody"), "got: {body}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test web confirm_page`
Expected: FAIL — the routes 404, so the assertions on page content fail

- [ ] **Step 3: Write minimal implementation**

In `src/web/mod.rs`, register the route inside `routes()`:

```rust
        .route("/agents/{name}/delete", get(delete_agent_confirm))
```

Add the handler beside `agent`:

```rust
/// Confirmation page for deleting an agent. Renders the blast radius before
/// anything is removed, and renders no button at all when the agent is online —
/// a button known to fail is worse than none.
async fn delete_agent_confirm(State(app): State<App>, Path(name): Path<String>) -> Html<String> {
    let known = app
        .store
        .agents()
        .await
        .unwrap_or_default()
        .into_iter()
        .any(|a| a.name == name);
    if !known {
        return Html(page(
            "delete agent",
            &format!("<h1>delete agent</h1><p>no agent named {}</p>", esc(&name)),
        ));
    }

    if app.registry.is_online(&name).await {
        return Html(page(
            "delete agent",
            &format!(
                "<h1>delete {n}</h1><p>{n} is online. Only offline agents can be deleted — \
                 deleting a connected agent would drop the room memberships it is still \
                 receiving messages through.</p><p><a href=\"/agents/{p}\">back</a></p>",
                n = esc(&name),
                p = encode_path_segment(&name),
            ),
        ));
    }

    let fp = app
        .store
        .agent_footprint(&name)
        .await
        .unwrap_or(crate::store::AgentFootprint {
            rooms: Vec::new(),
            cursors: 0,
        });

    let mut b = format!("<h1>delete {}</h1>", esc(&name));
    b.push_str("<h2>this will remove</h2><ul>");
    b.push_str(&format!("<li>the agent row for {}</li>", esc(&name)));
    for r in &fp.rooms {
        b.push_str(&format!("<li>membership of room {}</li>", esc(r)));
    }
    b.push_str(&format!(
        "<li>{n} cursor{s}</li></ul>",
        n = fp.cursors,
        s = if fp.cursors == 1 { "" } else { "s" },
    ));
    b.push_str("<p class=\"note\">messages and events are kept: room transcripts stay \
                readable and the audit trail outlives the agent.</p>");
    b.push_str(&format!(
        "<form method=\"post\" action=\"/agents/{p}/delete\">\
         <button type=\"submit\">delete {n}</button></form>\
         <p><a href=\"/agents/{p}\">cancel</a></p>",
        p = encode_path_segment(&name),
        n = esc(&name),
    ));
    Html(page("delete agent", &b))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test web confirm_page`
Expected: PASS (3 tests)

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
git add src/web/mod.rs tests/web.rs
git commit -m "refactor: add the agent delete confirmation page"
```

---

### Task 5: Perform the delete (`POST /agents/{name}/delete`)

**Files:**
- Modify: `src/web/mod.rs` (add `delete_agent_perform`; add `post` to the axum routing import; add a `summarize` arm for `agent_deleted`)
- Test: `tests/web.rs` (add a `post` helper alongside `get`)

**Interfaces:**
- Consumes: `Store::forget_agent` (Task 3), `Registry::is_online` (Task 1)
- Produces: route `POST /agents/{name}/delete`, returning `303` to `/agents` on success

- [ ] **Step 1: Write the failing test**

Add the `post` helper to `tests/web.rs`, directly after `get`:

```rust
/// Minimal HTTP/1.1 POST with an empty body, matching `get`'s no-dependency style.
/// Returns the whole raw response so a test can assert on the status line as well
/// as the body.
async fn post(port: u16, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}
```

Then append the tests:

```rust
#[tokio::test]
async fn posting_the_delete_removes_the_agent_and_redirects() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("network-debug#2", "hardac", "/w/nd", None, false, None)
            .await
            .unwrap();
        store.join_room("protocol", "network-debug#2").await.unwrap();
    }
    let port = start(dir.path()).await;

    let res = post(port, "/agents/network-debug%232/delete").await;
    assert!(res.contains("303"), "must redirect after a POST: {res}");

    let agents = get(port, "/agents").await;
    assert!(
        !agents.contains("network-debug#2"),
        "the deleted agent must be gone from the list: {agents}"
    );

    let room = get(port, "/rooms/protocol").await;
    assert!(
        !room.contains("network-debug#2"),
        "the stranded membership must be gone too: {room}"
    );
}

#[tokio::test]
async fn posting_the_delete_records_an_audit_event() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("network-debug#2", "hardac", "/w/nd", None, false, None)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;
    post(port, "/agents/network-debug%232/delete").await;

    let events = get(port, "/events").await;
    assert!(events.contains("agent_deleted"), "got: {events}");
    assert!(
        events.contains("network-debug#2"),
        "the audit event must name the deleted agent, since its row is gone"
    );
}

#[tokio::test]
async fn posting_the_delete_refuses_an_agent_that_came_online_after_the_confirm_page() {
    let (_d, port, _path) = common::start_bus_with_dir().await;
    // Offline when the confirm page would have been rendered...
    {
        let ws = common::connect(port, "caas").await;
        drop(ws);
    }
    assert!(common::wait_until(|| async move { !common::agent_is_online(port, "caas").await }).await);

    // ...and online by the time the POST lands.
    let _ws = common::connect(port, "caas").await;
    assert!(common::agent_is_online(port, "caas").await);

    let res = post(port, "/agents/caas/delete").await;
    assert!(res.contains("online"), "the POST must re-check liveness: {res}");

    let agents = get(port, "/agents").await;
    assert!(agents.contains("caas"), "the live agent must survive");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test web posting_the_delete`
Expected: FAIL — `POST` is not routed, so the responses are 405

- [ ] **Step 3: Write minimal implementation**

Change the routing import at the top of `src/web/mod.rs`:

```rust
use axum::{Router, routing::{get, post}};
```

Register the route in `routes()`:

```rust
        .route("/agents/{name}/delete", get(delete_agent_confirm).post(delete_agent_perform))
```

Add the handler after `delete_agent_confirm`:

```rust
/// Perform the delete.
///
/// Re-checks liveness rather than trusting the confirmation page: an agent can
/// reconnect between the `GET` and this `POST`, and dropping a live agent's
/// memberships is exactly what the offline-only rule exists to prevent.
async fn delete_agent_perform(
    State(app): State<App>,
    Path(name): Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    if app.registry.is_online(&name).await {
        return Html(page(
            "delete agent",
            &format!(
                "<h1>delete {n}</h1><p>{n} came online while this page was open, so nothing \
                 was deleted. Only offline agents can be deleted.</p>\
                 <p><a href=\"/agents/{p}\">back</a></p>",
                n = esc(&name),
                p = encode_path_segment(&name),
            ),
        ))
        .into_response();
    }

    let host = app
        .store
        .agents()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|a| a.name == name)
        .map(|a| a.host)
        .unwrap_or_default();

    match app.store.forget_agent(&name).await {
        Ok(counts) => {
            // The only surviving record that this agent ever existed.
            let _ = app
                .store
                .append_event(
                    "agent_deleted",
                    Some(&name),
                    None,
                    serde_json::json!({
                        "name": name,
                        "host": host,
                        "memberships": counts.memberships,
                        "cursors": counts.cursors,
                    }),
                )
                .await;
            axum::response::Redirect::to("/agents").into_response()
        }
        Err(e) => Html(page(
            "delete agent",
            &format!(
                "<h1>delete {n}</h1><p>nothing was deleted: {e}</p>",
                n = esc(&name),
                e = esc(&e.to_string()),
            ),
        ))
        .into_response(),
    }
}
```

Add the `summarize` arm in `src/web/mod.rs`, beside the `"agent_registered"` arm:

```rust
        "agent_deleted" => {
            let host = text("host");
            let mut s = format!("deleted {}", text("name"));
            if !host.is_empty() {
                s.push_str(&format!(" on {host}"));
            }
            s.push_str(&format!(
                " · {} memberships, {} cursors",
                num("memberships").unwrap_or(0),
                num("cursors").unwrap_or(0),
            ));
            s
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test web posting_the_delete`
Expected: PASS (3 tests)

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
git add src/web/mod.rs tests/web.rs
git commit -m "refactor: perform the agent delete and record it"
```

---

### Task 6: Entry point and the module doc

The feature is unreachable until the detail page links to it. This task also corrects `src/web/mod.rs`'s opening doc, which currently asserts the UI performs no writes — leaving it would make it the same class of defect the spec exists to remove: a comment that reads as settled fact and is not.

**Files:**
- Modify: `src/web/mod.rs` (lines 1-3 module doc; `agent` handler at line ~460)
- Test: `tests/web.rs`

**Interfaces:**
- Consumes: route `GET /agents/{name}/delete` (Task 4)
- Produces: nothing downstream

- [ ] **Step 1: Write the failing test**

Append to `tests/web.rs`:

```rust
#[tokio::test]
async fn the_agent_page_links_to_the_delete_page_with_a_percent_encoded_href() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("network-debug#2", "hardac", "/w/nd", None, false, None)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/agents/network-debug%232").await;

    // A bare `#` here would make the browser treat everything after it as a
    // fragment and request `/agents/network-debug` instead.
    assert!(
        body.contains("href=\"/agents/network-debug%232/delete\""),
        "the delete link must be percent-encoded: {body}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test web links_to_the_delete_page`
Expected: FAIL — no delete link is rendered

- [ ] **Step 3: Write the implementation**

In the `agent` handler in `src/web/mod.rs`, change the opening line of the body from:

```rust
    let mut b = format!("<h1>{}</h1><h2>rooms</h2><ul>", esc(&name));
```

to:

```rust
    let mut b = format!(
        "<h1>{n}</h1><p><a href=\"/agents/{p}/delete\">delete this agent</a></p>\
         <h2>rooms</h2><ul>",
        n = esc(&name),
        p = encode_path_segment(&name),
    );
```

Replace the module doc at the top of `src/web/mod.rs`:

```rust
//! Web views over the bus's own data.
//!
//! Read-only with exactly one exception: deleting an offline agent. Everything
//! else performs no writes, so the UI cannot be the cause of a bug it is being
//! used to investigate.
//!
//! The exception is deliberate and deliberately narrow. The bus has no
//! authentication and binds `0.0.0.0`, so anything this can do is available to
//! anything that can reach the port — which is why the delete refuses an agent
//! that is online, touches no messages or events, and records what it removed.
//! An unauthenticated caller can clear metadata for connections that are
//! already dead, and nothing more.
//!
//! See `docs/superpowers/specs/2026-08-05-agent-delete-design.md`.
```

- [ ] **Step 4: Run the full suite**

Run: `cargo +nightly fmt --check && cargo +stable clippy --all-targets --all-features -- -D warnings && cargo +stable test --locked`
Expected: PASS — this is exactly what CI runs

- [ ] **Step 5: Commit**

```bash
git add src/web/mod.rs tests/web.rs
git commit -m "feat: delete an offline agent from the web UI

A name collision leaves a permanent tombstone row in \`agents\`, and the
stranded room membership keeps reporting that name in \`queued_for\` forever.
Adds a confirmation page and a delete restricted to offline agents, removing
the agent row, its memberships and its cursors while keeping messages and
events.

This is the first write the web UI has ever performed; the module doc no
longer claims otherwise."
```

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task |
|---|---|
| `GET`/`POST /agents/{name}/delete` | 4, 5 |
| Confirm page lists memberships, cursor count, what is kept | 4 |
| Entry point on the detail page, not the list | 6 |
| Unknown name renders a page, not a 500 | 4 |
| Online agent refused, no button rendered | 4 |
| POST re-checks liveness (TOCTOU) | 5 |
| Liveness from the registry, not `agents.online` | 1, 4, 5 |
| `Registry::is_online` | 1 |
| `Store::forget_agent` transactional, returns counts | 3 |
| `Store::agent_footprint` returns room names + cursor count | 2 |
| `agent_deleted` audit event | 5 |
| `summarize` arm for the new kind | 5 |
| Messages and events preserved | 3 (asserted), 5 |
| `#2` name survives the URL round trip | 4, 6 |
| Module doc updated | 6 |

No gaps.

**Placeholder scan:** no TBD/TODO, no "add error handling", no "similar to Task N". Every code step carries the actual code.

**Type consistency:** `AgentFootprint { rooms, cursors }` (Task 2) is consumed in Task 4 with those field names. `ForgetCounts { agents, memberships, cursors }` (Task 3) is consumed in Task 5 as `counts.memberships` / `counts.cursors`. `Registry::is_online` (Task 1) is called identically in Tasks 4 and 5. `forget_agent` returns `ForgetCounts`, not `AgentFootprint` — the two shapes are intentionally different, per the spec.
