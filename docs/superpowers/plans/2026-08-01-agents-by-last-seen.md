# Agents Ordered By Last Seen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sort the agent tables by most recently seen so dormant agents fall to the bottom, and show the timestamp that explains the order.

**Architecture:** `last_seen` is already written on every registration and every online/offline transition; it has simply never been read back. Surface it on `AgentRow`, order the shared store query by it, and render it through the existing `fmt_time` helper in the shared `agent_row` renderer.

**Tech Stack:** Rust, axum, sqlx (SQLite, runtime queries not macros), chrono.

## Global Constraints

- **The `name` tiebreaker is required, not cosmetic.** `last_seen` is millisecond-granularity and several agents registering in the same millisecond is routine — especially in tests. `ORDER BY last_seen DESC` alone gives nondeterministic order on ties and produces flaky assertions. It must be `ORDER BY last_seen DESC, name`.
- **No schema change and no migration.** `last_seen` already exists as `INTEGER NOT NULL` on `agents`. This plan only reads it.
- **The web UI stays read-only.** No `POST`/`PUT`/`DELETE`, no store writes from `src/web/`. Filtering, hiding, and removing agents are all explicitly out of scope — see the spec's *What was considered and cut*.
- **`AgentInfo` and the `agents` MCP tool gain no field.** The tool's ordering changes because it shares the query; its shape does not.
- Rust formatting: `cargo +nightly fmt` (nightly specifically).
- Clippy must be clean under `cargo +1.97.1 clippy --all-targets --all-features -- -D warnings`. 1.97.1 is what CI's `@stable` resolves to and is newer than the local default, so check with it explicitly.
- Only capitalize the first letter of multi-letter acronyms (`RagService`, not `RAGService`).
- No new crate dependencies. `chrono` is already a dependency via `src/web/html.rs`.
- Baseline before Task 1: **258 tests passing**. Expected after: **262** — three new tests in `tests/store.rs`, one in `tests/web.rs`.

---

## File Structure

| File | Responsibility | Task |
| --- | --- | --- |
| `src/store/mod.rs` | `AgentRow.last_seen`, the SELECT, the ordering | 1 |
| `src/web/mod.rs` | The `last seen` cell in `agent_row`, and both table headers | 1 |
| `tests/store.rs` | Ordering is by recency, ties broken by name | 1 |
| `tests/web.rs` | The column renders on both pages | 1 |

One task: the ordering and the column that explains it are a single deliverable. Shipping the sort without the column leaves a list in an order the reader cannot account for.

---

### Task 1: Order agents by recency and show the timestamp

**Files:**
- Modify: `src/store/mod.rs`, `src/web/mod.rs`
- Test: `tests/store.rs` (append), `tests/web.rs` (append)

**Interfaces:**
- Produces: `AgentRow.last_seen: i64` (epoch milliseconds), and `Store::agents()` ordered `last_seen DESC, name`.

`AgentRow` is constructed in exactly one place — `Store::agents()` — so adding a field touches one construction site. Confirm with `grep -rn "AgentRow {" src/ tests/` before assuming.

Note for whoever releases this: adding a public field to `AgentRow` is a breaking change to the library API, so `cargo-semver-checks` will drive a minor bump (0.2.x → 0.3.0), not a patch. That is correct and expected, not a defect.

- [ ] **Step 1: Write the failing tests**

Append to `tests/store.rs`:

```rust
#[tokio::test]
async fn agents_are_ordered_by_most_recently_seen() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();

    // Registered oldest-first; the listing must come back newest-first.
    for name in ["oldest", "middle", "newest"] {
        store
            .upsert_agent(name, "hardac", "/w", None, false, None)
            .await
            .unwrap();
        // upsert_agent stamps last_seen with now_ms(), so distinct registrations
        // need distinct milliseconds to have a defined order at all.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let names: Vec<String> = store
        .agents()
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, vec!["newest", "middle", "oldest"]);
}

#[tokio::test]
async fn agents_seen_at_the_same_moment_fall_back_to_name_order() {
    // last_seen is millisecond-granularity, so simultaneous registrations are
    // routine. Without the name tiebreaker their order is whatever SQLite feels
    // like, which makes any assertion on this listing flaky.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();

    let mut opened = Vec::new();
    for name in ["charlie", "alpha", "bravo"] {
        opened.push(store.upsert_agent(name, "hardac", "/w", None, false, None));
    }
    for f in opened {
        f.await.unwrap();
    }

    let rows = store.agents().await.unwrap();
    // Any that share a timestamp must be alphabetical among themselves.
    for pair in rows.windows(2) {
        if pair[0].last_seen == pair[1].last_seen {
            assert!(
                pair[0].name < pair[1].name,
                "ties must break alphabetically: {:?} then {:?}",
                pair[0].name,
                pair[1].name
            );
        }
    }
    assert_eq!(rows.len(), 3);
}

#[tokio::test]
async fn last_seen_advances_when_an_agent_re_registers() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("caas", "hardac", "/w", None, false, None)
        .await
        .unwrap();
    let first = store.agents().await.unwrap()[0].last_seen;

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    store
        .upsert_agent("caas", "hardac", "/w", None, false, None)
        .await
        .unwrap();
    let second = store.agents().await.unwrap()[0].last_seen;

    assert!(second > first, "re-registering must move the agent to the top");
}
```

Append to `tests/web.rs`:

```rust
#[tokio::test]
async fn both_agent_tables_show_when_each_agent_was_last_seen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "hardac", "/w", None, false, None)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    for path in ["/", "/agents"] {
        let body = get(port, path).await;
        assert!(
            body.contains("<th>last seen"),
            "{path} must have a last seen column: {body}"
        );
        // fmt_time renders a same-day timestamp as HH:MM:SS.mmm, so a rendered
        // cell contains a colon between digits. Asserting on the header alone
        // would pass with an empty column.
        assert!(
            regex_lite_has_time(&body),
            "{path} must render an actual timestamp, not an empty cell: {body}"
        );
    }
}

/// True if the body contains something shaped like `HH:MM:SS`. Deliberately
/// crude — the point is that the cell is populated, not that the format is
/// exact, which `fmt_time` already owns.
fn regex_lite_has_time(body: &str) -> bool {
    let b = body.as_bytes();
    b.windows(8).any(|w| {
        w[0].is_ascii_digit()
            && w[1].is_ascii_digit()
            && w[2] == b':'
            && w[3].is_ascii_digit()
            && w[4].is_ascii_digit()
            && w[5] == b':'
            && w[6].is_ascii_digit()
            && w[7].is_ascii_digit()
    })
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test store agents_are_ordered_by_most_recently_seen`
Expected: FAIL — `AgentRow` has no field `last_seen`, and the listing is alphabetical (`middle`, `newest`, `oldest`).

- [ ] **Step 3: Add the field and change the ordering**

In `src/store/mod.rs`, add to `AgentRow` as the last field:

```rust
    /// Epoch milliseconds of the last registration or online/offline transition.
    /// Written since the beginning; this is the first thing to read it back.
    pub last_seen: i64,
```

Then change `Store::agents()` — both the column list and the `ORDER BY`:

```rust
    pub async fn agents(&self) -> anyhow::Result<Vec<AgentRow>> {
        let rows = sqlx::query(
            "SELECT name, host, cwd, session_id, online, is_human, version, last_seen
             FROM agents ORDER BY last_seen DESC, name",
        )
        .fetch_all(&self.pool)
        .await?;
```

and add `last_seen: r.get("last_seen"),` as the last field of the `AgentRow` construction in that same function.

The `, name` tiebreaker is required — see Global Constraints.

- [ ] **Step 4: Render the column**

In `src/web/mod.rs`, `agent_row` is the single renderer both tables share. Add a cell for the timestamp, between version and state:

```rust
fn agent_row(a: &AgentRow, online: bool) -> String {
    format!(
        "<tr><td><a href=\"/agents/{p}\">{n}</a>{mark}</td><td>{h}</td><td>{v}</td>\
         <td class=\"when\">{w}</td><td class=\"{c}\">{s}</td></tr>",
        p = encode_path_segment(&a.name),
        n = esc(&a.name),
        mark = human_mark(a.is_human),
        h = esc(&a.host),
        v = version_cell(a.version.as_deref()),
        w = esc(&fmt_time(a.last_seen)),
        c = if online { "" } else { "off" },
        s = if online { "online" } else { "offline" },
    )
}
```

`class="when"` reuses the existing timestamp styling already applied to message and event times. `fmt_time` is already imported in this file.

Then add the header cell to **both** tables, in the same position — between version and state:

- Around line 226, in `overview()`:
  `"<h1>overview</h1><h2>agents</h2><table><tr><th>name<th>host<th>version<th>last seen<th>state</tr>"`
- Around line 414, in `agents()`:
  `"<h1>agents</h1><table><tr><th>name<th>host<th>version<th>last seen<th>state</tr>"`

A header added to one table but not the other, or in a different position from the cell, produces a subtly misaligned table that a `contains` assertion will not catch. Check both.

- [ ] **Step 5: Run the tests**

Run: `cargo test`
Expected: PASS, count up by 4 from 258 — three in `tests/store.rs`, one in `tests/web.rs`.

If `tests/bus.rs`'s `assert_eq!(agents[0].name, "caas")` assertions fail, check them rather than editing them: both assert `agents.len() == 1` immediately before, so ordering cannot affect them. A failure there means something else changed.

- [ ] **Step 6: Format, lint, and commit**

```bash
cargo +nightly fmt
cargo +1.97.1 clippy --all-targets --all-features -- -D warnings
git add src/store/mod.rs src/web/mod.rs tests/store.rs tests/web.rs
git commit -m "feat: order agents by last seen and show the timestamp"
```

---

## Self-Review

**Spec coverage.** Each spec section against the task:

| Spec section | Covered by |
| --- | --- |
| §1 `ORDER BY last_seen DESC, name` | Step 3 |
| §1 name tiebreaker is load-bearing | Step 3 + the tie test in Step 1 |
| §1 ordering applies to all consumers of `Store::agents()` | Step 3 (shared query, nothing overrides it) |
| §1 no new data recorded | Step 3 reads only; no schema change anywhere in the plan |
| §2 `AgentRow.last_seen` | Step 3 |
| §2 `last seen` column on both tables via `fmt_time` | Step 4 |
| §2 `AgentInfo`/MCP tool gain nothing | not touched by any step |
| Out of scope: filtering, removal, web writes | no step adds any |

No spec requirement is unimplemented.

**Placeholder scan.** No TBD/TODO. Every step carries actual code. Four facts were checked against the source rather than assumed: `agent_row` already exists as a single shared renderer (extracted during the version-reporting fix wave), so the cell is one edit rather than two; `fmt_time` is already imported in `src/web/mod.rs` and already used for message and event timestamps; `class="when"` already exists in the CSS; and `tests/bus.rs`'s two positional agent assertions are guarded by a `len() == 1` check immediately above them, so the ordering change cannot break them.

**Type consistency.** `last_seen` is `i64` on `AgentRow` and is read with `r.get("last_seen")` from an `INTEGER NOT NULL` column. `fmt_time(ms: i64) -> String` matches. `agent_row(a: &AgentRow, online: bool) -> String` is unchanged in signature; only its body grows a cell.

**Test count.** 258 baseline → 262. The Step 5 expectation of "up by 4" is the accurate figure; three store tests plus one web test.

**One risk.** The two ordering tests depend on real elapsed milliseconds between registrations, which is why they sleep 5ms rather than relying on execution speed. If they ever prove flaky on a loaded machine, raise the sleep rather than removing the assertion — the ordering is the entire feature.
