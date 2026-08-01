# Relayers In The Web UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show which agents hold the relayer grant, and print the configured relayer set, so a mistyped `--relayer` flag is visible on the dashboard instead of only in the startup log.

**Architecture:** The relayer set is bus configuration, not agent state. Both web handlers already hold `App`, so `agent_row` gains a flag from `app.relayers.contains(&a.name)` at render time and the pages print `app.relayers.names()`. Nothing is stored.

**Tech Stack:** Rust, axum, sqlx (SQLite, runtime queries not macros).

## Global Constraints

- **Nothing is written to the store.** Relayer status is configuration, not agent state — writing it would make the table disagree with the running config the moment the flag changed. The `agents` table gains no column and `AgentRow` gains no field.
- **The web UI stays read-only.** No `POST`/`PUT`/`DELETE`, no store writes from `src/web/`.
- **`AgentInfo` and the `agents` MCP tool gain no field.**
- **The set line is the load-bearing half, not the badge.** With badges alone, a typo'd relayer name badges nothing and the page is byte-identical to a correctly-configured bus with no relayer connected. Printing the resolved set is what makes the mistake diagnosable. A change that ships the badge without the line has missed the point.
- Rust formatting: `cargo +nightly fmt` (nightly specifically).
- Clippy clean under `cargo +1.97.1 clippy --all-targets --all-features -- -D warnings`. 1.97.1 is what CI's `@stable` resolves to and is newer than the local default, so check with it explicitly.
- Only capitalize the first letter of multi-letter acronyms (`RagService`, not `RAGService`).
- No new crate dependencies.
- Baseline before Task 1: **262 tests passing**. Expected after: **265**.

---

## File Structure

| File | Responsibility | Task |
| --- | --- | --- |
| `src/web/mod.rs` | `relayer_mark`, the `agent_row` parameter, both call sites, both set lines | 1 |
| `src/web/html.rs` | The `.relayer` badge CSS | 1 |
| `tests/web.rs` | A start helper that configures relayers, and three tests | 1 |

One task: the badge and the set line are a single deliverable, and shipping either alone leaves the misconfiguration invisible.

---

### Task 1: Show the relayer grant

**Files:**
- Modify: `src/web/mod.rs`, `src/web/html.rs`
- Test: `tests/web.rs` (append)

**Interfaces:**
- Consumes: `Relayers::contains(&str) -> bool` and `Relayers::names() -> Vec<&str>` (already public, already used by `serve`'s startup log), and `App.relayers`.
- Produces: `agent_row(a: &AgentRow, online: bool, is_relayer: bool) -> String`.

- [ ] **Step 1: Write the failing tests**

`tests/web.rs`'s existing `start()` helper calls `claude_bus::bus::serve_on`, which passes an empty `Relayers`. Testing a configured set needs a second helper. `serve_on_full` is already `pub` with six parameters, so no new production API is required — add this beside `start()`:

```rust
/// Like `start`, but with a configured relayer set. `serve_on` hardcodes an empty
/// one, and relayer rendering is exactly what needs a non-empty set to test.
async fn start_with_relayers(dir: &std::path::Path, names: &[&str]) -> u16 {
    let path = dir.to_path_buf();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let relayers =
        claude_bus::bus::Relayers::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>());
    tokio::spawn(async move {
        claude_bus::bus::serve_on_full(
            listener,
            path,
            claude_bus::bus::delivery::Guards::default(),
            claude_bus::bus::Keepalive::default(),
            claude_bus::bus::registry::Registry::new(),
            relayers,
        )
        .await
        .unwrap()
    });
    common::wait_until_bus_ready(port).await;
    port
}
```

Then append these three tests:

```rust
#[tokio::test]
async fn a_configured_relayer_is_marked_on_both_agent_pages() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("hub", "hardac", "/w", None, false, Some("0.3.0"))
            .await
            .unwrap();
        store
            .upsert_agent("caas", "hardac", "/w", None, false, Some("0.3.0"))
            .await
            .unwrap();
    }
    let port = start_with_relayers(dir.path(), &["hub"]).await;

    for path in ["/", "/agents"] {
        let body = get(port, path).await;
        assert!(
            body.contains("relayers: hub"),
            "{path} must state the configured set: {body}"
        );
        assert_eq!(
            body.matches("class=\"relayer\"").count(),
            1,
            "exactly the configured agent should be badged on {path}: {body}"
        );
    }
}

#[tokio::test]
async fn a_bus_with_no_relayers_says_so_rather_than_staying_silent() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "hardac", "/w", None, false, Some("0.3.0"))
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/agents").await;
    assert!(
        body.contains("relayers: (none)"),
        "an unconfigured bus must say so, not omit the line: {body}"
    );
    assert!(
        !body.contains("class=\"relayer\""),
        "nothing should be badged: {body}"
    );
}

#[tokio::test]
async fn a_relayer_configured_under_a_name_no_agent_uses_is_still_visible() {
    // The failure this feature exists for. A mistyped `--relayer hubb` badges nothing,
    // so with the badge alone the page would be identical to a correctly configured bus
    // whose relayer simply is not connected. The set line is what distinguishes them.
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("hub", "hardac", "/w", None, false, Some("0.3.0"))
            .await
            .unwrap();
    }
    let port = start_with_relayers(dir.path(), &["hubb"]).await;

    let body = get(port, "/agents").await;
    assert!(
        body.contains("relayers: hubb"),
        "the configured name must appear even with no matching agent: {body}"
    );
    assert!(
        !body.contains("class=\"relayer\""),
        "and nothing should be badged, which is the tell: {body}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test web a_configured_relayer_is_marked`
Expected: FAIL — `agent_row` takes 2 arguments, and no page renders `relayers:` or `class="relayer"`.

- [ ] **Step 3: Add the badge helper**

In `src/web/mod.rs`, beside the existing `human_mark` (around line 160):

```rust
/// The badge marking an agent the bus is configured to accept relayed authority from.
///
/// Read from `App.relayers` at render time rather than from the agent's row: the grant is
/// bus configuration, not agent state, and a stored copy would disagree with the running
/// config the moment the flag changed.
fn relayer_mark(is_relayer: bool) -> &'static str {
    if is_relayer {
        " <span class=\"relayer\">relayer</span>"
    } else {
        ""
    }
}
```

- [ ] **Step 4: Widen `agent_row` and both call sites**

Change the signature and add the badge after the existing `human` one:

```rust
fn agent_row(a: &AgentRow, online: bool, is_relayer: bool) -> String {
    format!(
        "<tr><td><a href=\"/agents/{p}\">{n}</a>{mark}{relay}</td><td>{h}</td><td>{v}</td>\
         <td class=\"when\">{w}</td><td class=\"{c}\">{s}</td></tr>",
        p = encode_path_segment(&a.name),
        n = esc(&a.name),
        mark = human_mark(a.is_human),
        relay = relayer_mark(is_relayer),
        h = esc(&a.host),
        v = version_cell(a.version.as_deref()),
        w = esc(&fmt_time(a.last_seen)),
        c = if online { "" } else { "off" },
        s = if online { "online" } else { "offline" },
    )
}
```

Both call sites — around line 231 in `overview()` and around line 420 in `agents()` — become:

```rust
        b.push_str(&agent_row(a, online, app.relayers.contains(&a.name)));
```

Both handlers already bind `app` from `State(app): State<App>`, so nothing new needs threading.

- [ ] **Step 5: Print the configured set**

Add a helper beside `relayer_mark`:

```rust
/// The configured relayer set, for the note beneath each agent table.
///
/// Printed even when empty. A mistyped flag yields a set that badges nothing, which
/// without this line is indistinguishable from a correct configuration whose relayer
/// happens to be disconnected — so the line is what makes the mistake diagnosable.
fn relayer_note(relayers: &Relayers) -> String {
    let names = relayers.names();
    if names.is_empty() {
        "relayers: (none)".to_string()
    } else {
        format!("relayers: {}", esc(&names.join(", ")))
    }
}
```

Add `use crate::bus::Relayers;` to the imports at the top of `src/web/mod.rs` if `Relayers` is not already in scope.

Then extend both existing note lines to carry it. Around line 234 in `overview()`:

```rust
    b.push_str(&format!(
        "</table><p class=\"note\">this bus is running {} — {}</p>",
        esc(env!("CARGO_PKG_VERSION")),
        relayer_note(&app.relayers),
    ));
```

and around line 424 in `agents()`, which has no `</table>` prefix:

```rust
    b.push_str(&format!(
        "<p class=\"note\">this bus is running {} — {}</p>",
        esc(env!("CARGO_PKG_VERSION")),
        relayer_note(&app.relayers),
    ));
```

Check both. A line updated on one page and not the other is the recurring mistake in this file.

- [ ] **Step 6: Add the badge CSS**

In `src/web/html.rs`, beside the existing `.stale` rule at roughly line 117:

```
.relayer{font-size:.8rem;color:#1a7f5a;border:1px solid #b8e0cd;border-radius:.6rem;padding:0 .35rem;margin-left:.4rem}\
```

Green distinguishes it from `.human` (blue) and `.stale` (amber) while keeping the same shape as both.

- [ ] **Step 7: Run the tests**

Run: `cargo test`
Expected: PASS, count up by 3 from 262.

- [ ] **Step 8: Format, lint, and commit**

```bash
cargo +nightly fmt
cargo +1.97.1 clippy --all-targets --all-features -- -D warnings
git add src/web/mod.rs src/web/html.rs tests/web.rs
git commit -m "feat: show which agents hold the relayer grant"
```

---

## Self-Review

**Spec coverage.** Each spec section against the task:

| Spec section | Covered by |
| --- | --- |
| §1 badge on the agent row, new CSS class | Steps 3, 4, 6 |
| §1 value read from `App.relayers`, not the row | Step 4 (`app.relayers.contains`) |
| §1 nothing stored | no step touches the store or `AgentRow` |
| §2 set printed on both pages | Step 5 |
| §2 `relayers: (none)` when empty | Step 5, tested |
| §2 second consumer of `Relayers::names()` | Step 5 |
| Rejected: column, stored status, `AgentInfo` field, third visual state | no step adds any |
| Out of scope: write paths | no step adds one |

No spec requirement is unimplemented.

**Placeholder scan.** No TBD/TODO. Every step carries actual code. Four facts were checked against the source rather than assumed: `Relayers::contains` and `Relayers::names` are both already `pub` (added for `serve`'s startup log); `App.relayers` is `pub(crate)` and `src/web` is in the same crate; `agent_row` has exactly two call sites, at roughly lines 231 and 420; and `tests/web.rs`'s `start()` goes through `serve_on`, which hardcodes an empty `Relayers` — hence the separate helper in Step 1, which uses the already-`pub` `serve_on_full` rather than adding production API for a test.

**Type consistency.** `agent_row(a: &AgentRow, online: bool, is_relayer: bool) -> String` is defined in Step 4 and called with three arguments at both sites in the same step. `relayer_mark(bool) -> &'static str` matches `human_mark`'s shape. `relayer_note(&Relayers) -> String` takes a reference, and `app.relayers` is a field access on an owned `App`, so `&app.relayers` is what the call sites pass.

**One judgment worth flagging.** `relayer_note` escapes the joined names even though they come from the bus's own command line rather than the wire. That is deliberate: cheap, consistent with every other cell in the file, and it means a future change that sources the set from anywhere else does not silently become an injection.
