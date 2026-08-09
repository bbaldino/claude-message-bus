# Hiding Rooms Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A room can be hidden from the console's rail and got back again, and a room that receives a message unhides itself.

**Architecture:** A `hidden` column on `rooms`, set through one new endpoint and cleared by `append_message_at`. The rail returns every room with the flag and the client decides what to show, following the rule the store's own comments state: the server ships data, the client writes the sentence.

**Tech Stack:** Rust, axum 0.8, sqlx/SQLite, ts-rs; React 19, TypeScript, Vite, vitest, CSS Modules.

**Spec:** `docs/superpowers/specs/2026-08-09-hiding-rooms-design.md`

## Global Constraints

- Rust commits that change behaviour use `fix:` or `feat:` **only when a release is wanted** — `release_commits = "^(feat|fix)[(!:]"` in `release-plz.toml` cuts a release on those prefixes. Use `feat:` for the finished feature and `chore:` for anything else.
- Rust is formatted with `cargo +nightly fmt`; clippy is `cargo +stable clippy --all-targets --all-features -- -D warnings`.
- Prettier config is exactly `{ "singleQuote": true, "semi": false, "printWidth": 100 }`. Run `npm run format` before committing.
- **No hex colours or `rgba()` outside `ui/src/theme.css`.** Every token needed here already exists.
- **No component may branch on the theme.**
- Never hand-edit anything under `ui/src/types/` — ts-rs generates it during `cargo test` and CI fails on drift.
- **Nothing may delete from `messages` or `events`.**
- No new npm dependencies. No `dangerouslySetInnerHTML`.
- Gates — Rust: `cargo +nightly fmt && cargo +stable clippy --all-targets --all-features -- -D warnings && cargo +stable test --locked`. UI, from `ui/`: `npm test && npm run typecheck && npm run format:check && npm run build`.
- **Every behavioural test must be confirmed to fail before the change exists.** Not asserted — run it and watch it fail.

## One deviation from the spec, decided here

The spec says the hidden count replaces the ROOMS header's right-aligned note. **It does not.** That note reads `last 60 min` and it is the caption for the volume strips in every room row — the handoff is explicit that a chart without its caption is indistinguishable from a broken one, and 2b built it for that reason.

So the toggle becomes a **footer row beneath the room rows** instead: a dimmed
`2 hidden ▾` line that expands the hidden rooms below it. The header is untouched.
This keeps the caption, avoids two labels fighting for a 320px-wide header, and
reads naturally as "and there are more".

Everything else in the spec stands.

## The blast radius to know about up front

`RailRoom` gains a required `hidden: boolean`. **Fourteen room fixtures across ten
TypeScript test files** construct that object and will all fail `npm run typecheck`
until they carry the field. That is expected and mechanical — but a sweep that
wide is where a weakened assertion hides, so Task 4 adds the field and changes
nothing else in those files.

---

## File Structure

| File | Responsibility |
|---|---|
| `src/store/mod.rs` | **Modify.** The column, `RoomRow.hidden`, `set_room_hidden`, and the unhide in `append_message_at`. |
| `src/web/api.rs` | **Modify.** `RailRoom.hidden`, and the `room_set_hidden` handler with its CSRF and 404 guards. |
| `src/web/mod.rs` | **Modify.** Register the route. |
| `tests/web.rs` | **Modify.** Endpoint tests. |
| `ui/src/data/api.ts` | **Modify.** `setRoomHidden(room, hidden)`. |
| `ui/src/data/store.ts` | **Modify.** A `setHidden` action that calls it and refreshes the rail. |
| `ui/src/transcript/RoomTabs.tsx` | **Modify.** The `hide` / `unhide` control. |
| `ui/src/transcript/RoomScreen.tsx` | **Modify.** Wire the control to the store. |
| `ui/src/rail/Rail.tsx` | **Modify.** Split visible from hidden; the footer toggle. |
| `ui/src/rail/RoomRow.tsx` | **Modify.** A dimmed variant. |
| `ui/src/rail/Rail.module.css` | **Modify.** `.hiddenToggle`, `.rowHidden`. |

---

### Task 1: The store

**Files:**
- Modify: `src/store/mod.rs`
- Test: `tests/store.rs` — verified: store tests live there, not in an inline module.

**Interfaces:**
- Produces:
  ```rust
  pub struct RoomRow { pub name: String, pub mode: String, pub members: Vec<String>, pub hidden: bool }
  pub async fn set_room_hidden(&self, room: &str, hidden: bool) -> anyhow::Result<bool>
  ```
  `set_room_hidden` returns `true` if a row was updated, `false` if no such room — the caller turns `false` into a 404.

- [ ] **Step 1: Write the failing tests**

Append to `tests/store.rs`. Its helper is `temp_store()` — verified — which returns `(tempfile::TempDir, Store)`; hold the `TempDir` as `_d` so it outlives the store. There is also a `seeded()` helper that pre-creates `protocol` with two members; these tests want the bare `temp_store()`.

```rust
#[tokio::test]
async fn hiding_a_room_sets_the_flag_and_rooms_reports_it() {
    let (_d, store) = temp_store().await;
    store.ensure_room("protocol").await.unwrap();
    assert!(store.set_room_hidden("protocol", true).await.unwrap());
    let row = store.rooms().await.unwrap().into_iter().find(|r| r.name == "protocol").unwrap();
    assert!(row.hidden, "rooms() must report the flag it was just given");
}

#[tokio::test]
async fn hiding_a_room_that_does_not_exist_reports_it_rather_than_creating_one() {
    // The caller turns this false into a 404. If this created the room instead,
    // a typo would conjure a hidden room that then shows in the rail's count.
    let (_d, store) = temp_store().await;
    assert!(!store.set_room_hidden("no-such-room", true).await.unwrap());
    assert!(store.rooms().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_message_unhides_the_room() {
    // The whole point of the feature's "comes back on its own" half.
    let (_d, store) = temp_store().await;
    store.ensure_room("protocol").await.unwrap();
    store.set_room_hidden("protocol", true).await.unwrap();
    store.append_message("protocol", "caas", "hello", false, false).await.unwrap();
    let row = store.rooms().await.unwrap().into_iter().find(|r| r.name == "protocol").unwrap();
    assert!(!row.hidden, "a message must bring a hidden room back");
}

#[tokio::test]
async fn a_message_to_a_visible_room_leaves_it_visible() {
    // Guards against "fixing" the above with an unconditional UPDATE that also
    // has to be correct for the 99% case.
    let (_d, store) = temp_store().await;
    store.ensure_room("protocol").await.unwrap();
    store.append_message("protocol", "caas", "hello", false, false).await.unwrap();
    let row = store.rooms().await.unwrap().into_iter().find(|r| r.name == "protocol").unwrap();
    assert!(!row.hidden);
}
```

`append_message`'s signature is `(room, from, body, done, human)` — verified against `src/store/mod.rs:517`, and it delegates to `append_message_at`, which is why Step 6 puts the unhide there and both paths get it.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo +stable test --locked set_room_hidden hiding_a_room a_message_unhides`
Expected: FAIL to compile — no `set_room_hidden`, no `RoomRow.hidden`.

- [ ] **Step 3: Add the column**

In `Store::migrate`, beside the three existing calls:

```rust
        self.add_column_if_missing("rooms", "hidden", "INTEGER NOT NULL DEFAULT 0")
            .await?;
```

- [ ] **Step 4: Carry it on `RoomRow`**

`RoomRow` (around `src/store/mod.rs:34`) gains `pub hidden: bool`. Then `rooms()` must select and populate it — its query is currently `SELECT name, mode FROM rooms ORDER BY name`:

```rust
        let rows = sqlx::query("SELECT name, mode, hidden FROM rooms ORDER BY name")
```

and in the loop:

```rust
            out.push(RoomRow {
                name,
                mode: r.get("mode"),
                members,
                hidden: r.get::<i64, _>("hidden") != 0,
            });
```

**Check every other construction site of `RoomRow`** — `grep -rn "RoomRow {" src/` — and give each the new field. A struct literal missing a field is a compile error, so the compiler will find them; this note is so you expect them.

- [ ] **Step 5: The setter**

```rust
    /// Set or clear a room's hidden flag. `Ok(false)` means no such room —
    /// deliberately not an error, and deliberately not an insert: the caller
    /// turns it into a 404, and creating the row here would let a typo conjure
    /// a hidden room that then appears in the rail's count.
    pub async fn set_room_hidden(&self, room: &str, hidden: bool) -> anyhow::Result<bool> {
        let res = sqlx::query("UPDATE rooms SET hidden = ?2 WHERE name = ?1")
            .bind(room)
            .bind(hidden as i64)
            .execute(self.pool())
            .await?;
        Ok(res.rows_affected() > 0)
    }
```

- [ ] **Step 6: Unhide on a message**

In `append_message_at`, after the `INSERT INTO messages` and before returning:

```rust
        // A message brings a hidden room back. Guarded on `hidden = 1` so the
        // normal case affects zero rows — this is the send path.
        //
        // Only a message does this. Not events, not files, not presence: a room
        // whose only activity is a `room_joined` is not a conversation being
        // missed, and unhiding on every event would make the feature useless for
        // any room with members.
        sqlx::query("UPDATE rooms SET hidden = 0 WHERE name = ?1 AND hidden = 1")
            .bind(room)
            .execute(self.pool())
            .await?;
```

- [ ] **Step 7: Run to verify they pass, then commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git add src tests
git commit -F - <<'EOF'
chore: add a hidden flag to rooms

A display flag, not a delete — nothing is removed from messages or events, so the
rule that makes the audit log trustworthy holds.

set_room_hidden returns false rather than inserting when the room does not exist.
The caller turns that into a 404; creating the row would let a typo conjure a
hidden room that then appears in the rail's count.

A message clears the flag, and nothing else does. Guarded on hidden = 1 so the
normal case affects zero rows on the send path. Not events, not files, not
presence: a room whose only activity is a room_joined is not a conversation being
missed, and unhiding on every event would make the feature useless for any room
with members.
EOF
```

---

### Task 2: The endpoint

**Files:**
- Modify: `src/web/api.rs`, `src/web/mod.rs`
- Test: `tests/web.rs`

**Interfaces:**
- Consumes: `Store::set_room_hidden` from Task 1.
- Produces: `POST /api/rooms/{name}/hidden`, body `{"hidden": true|false}`. Returns 204 on success, 404 for an unknown room, 403 cross-origin, 500 on a store error.
- Produces: `RailRoom.hidden: bool`, which regenerates `ui/src/types/RailRoom.ts`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/web.rs`. **Read the file's existing helpers first** — it has a seed-then-serve pattern (a scoped `Store` in a block, dropped, and only then `start(dir.path())`) and JSON helpers reached as `common::get_json`. Follow them; do not open a second `Store` against a running bus's SQLite file.

```rust
#[tokio::test]
async fn hiding_a_room_is_reflected_in_the_rail() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.ensure_room("protocol").await.unwrap();
    }
    let (addr, _h) = start(dir.path()).await;

    let res = common::post_json(
        &addr,
        "/api/rooms/protocol/hidden",
        serde_json::json!({ "hidden": true }),
    )
    .await;
    assert_eq!(res, 204);

    let rail: serde_json::Value = common::get_json(&addr, "/api/rail").await;
    let room = rail["rooms"].as_array().unwrap().iter().find(|r| r["name"] == "protocol").unwrap();
    assert_eq!(room["hidden"], true, "the rail must ship the flag, not filter the room out");
}

#[tokio::test]
async fn the_rail_still_contains_a_hidden_room() {
    // The server ships data, the client writes the sentence. Filtering here
    // would force a second call just to learn how many were filtered.
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.ensure_room("protocol").await.unwrap();
        store.set_room_hidden("protocol", true).await.unwrap();
    }
    let (addr, _h) = start(dir.path()).await;
    let rail: serde_json::Value = common::get_json(&addr, "/api/rail").await;
    assert_eq!(rail["rooms"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn hiding_an_unknown_room_is_a_404_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _h) = start(dir.path()).await;
    let res = common::post_json(
        &addr,
        "/api/rooms/nope/hidden",
        serde_json::json!({ "hidden": true }),
    )
    .await;
    assert_eq!(res, 404);
    let rail: serde_json::Value = common::get_json(&addr, "/api/rail").await;
    assert!(rail["rooms"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_cross_origin_hide_is_refused() {
    // Same class as the delete form: a state-changing POST reachable from a
    // browser against a bus that binds 0.0.0.0 with no authentication.
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.ensure_room("protocol").await.unwrap();
    }
    let (addr, _h) = start(dir.path()).await;
    let res = common::post_json_with_origin(
        &addr,
        "/api/rooms/protocol/hidden",
        serde_json::json!({ "hidden": true }),
        "https://evil.example",
    )
    .await;
    assert_eq!(res, 403);
}
```

**`common::post_json` and `common::post_json_with_origin` may not exist.** Check `tests/common/mod.rs`. If they don't, add them next to the existing `get_json`, returning the response status as `u16`, and say in your report that you added them.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo +stable test --locked --test web hidden`
Expected: FAIL — the route is not registered, so the POST 404s for the wrong reason. Confirm the cross-origin test fails by returning 404 rather than 403, which tells you it is failing because the route is missing, not because the guard is absent.

- [ ] **Step 3: Add the flag to the DTO**

In `src/web/api.rs`, `RailRoom` gains `pub hidden: bool`, and the construction site in the rail handler's loop gains `hidden: r.hidden,`. `r` there is the `RoomRow` from `store.rooms()`.

- [ ] **Step 4: Write the handler**

In `src/web/api.rs`, following `agent_delete`'s shape:

```rust
#[derive(serde::Deserialize)]
pub(crate) struct HiddenBody {
    hidden: bool,
}

/// Hide or unhide a room.
///
/// The same cross-origin guard the delete path carries, for the same reason: a
/// state-changing POST reachable from a browser against a bus that binds
/// `0.0.0.0` with no authentication. A request with no `Origin` is allowed —
/// those callers could already reach the port directly.
///
/// 404 for an unknown room, deliberately the opposite of
/// `GET /api/rooms/{name}/files`, which answers an unknown room with an empty
/// list. A read of "what is in this room" is answerable for a room with nothing
/// in it; a write to a room that does not exist is not a request that can be
/// honoured.
pub(crate) async fn room_set_hidden(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<HiddenBody>,
) -> StatusCode {
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        let host = headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if !crate::web::origin_matches_host(origin, host) {
            return StatusCode::FORBIDDEN;
        }
    }

    match app.store.set_room_hidden(&name, body.hidden).await {
        Ok(true) => {
            let kind = if body.hidden { "room_hidden" } else { "room_unhidden" };
            if let Err(e) = app
                .store
                .append_event(kind, None, Some(&name), serde_json::json!({}))
                .await
            {
                eprintln!("{kind} event for {name} was not recorded: {e}");
            }
            StatusCode::NO_CONTENT
        }
        Ok(false) => StatusCode::NOT_FOUND,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
```

`append_event`'s signature is `(kind: &str, agent: Option<&str>, room: Option<&str>, detail: Value)` — verified at `src/store/events.rs:38`, so the call above is correct as written. `agent` is `None` because the bus has no operator identity to attribute a hide to; that is the same gap the spec cites for rejecting per-operator scoping.

**Check `Json` is already imported** in `api.rs`, and that extractor order is legal in axum 0.8 — a body extractor must come last, which is why `Json` is the final parameter here.

- [ ] **Step 5: Register the route**

In `src/web/mod.rs`, beside the other `/api/rooms/{name}/...` routes:

```rust
        .route("/api/rooms/{name}/hidden", post(api::room_set_hidden))
```

Add `post` to the `axum::routing` import if it is not already there.

- [ ] **Step 6: Run to verify they pass, then commit**

The `cargo test` run regenerates `ui/src/types/RailRoom.ts` with the new field. Include it in the commit; **do not hand-edit it.**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git add src tests ui/src/types
git commit -F - <<'EOF'
chore: add POST /api/rooms/{name}/hidden

Carries the same Origin-vs-Host check the delete form uses — a state-changing POST
reachable from a browser against a bus that binds 0.0.0.0 with no authentication.
A request with no Origin is allowed, for the reason stated there: those callers
could already reach the port directly.

404 for an unknown room, deliberately the opposite of the files endpoint, which
answers an unknown room with an empty list. A read of "what is in this room" is
answerable for a room with nothing in it; a write to a room that does not exist is
not.

The rail ships the flag rather than filtering hidden rooms out. The client needs
the whole list to render the count, so filtering here would force a second call to
learn how many were filtered.
EOF
```

---

### Task 3: The hide control

**Files:**
- Modify: `ui/src/data/api.ts`, `ui/src/data/store.ts`, `ui/src/transcript/RoomTabs.tsx`, `ui/src/transcript/RoomScreen.tsx`, `ui/src/transcript/Files.module.css`
- Test: `ui/src/transcript/Transcript.test.tsx`

**Interfaces:**
- Consumes: `POST /api/rooms/{name}/hidden` from Task 2; `RailRoom.hidden`.
- Produces: `store.setHidden(room: string, hidden: boolean): Promise<void>`.

- [ ] **Step 1: Write the failing tests**

Append to `ui/src/transcript/Transcript.test.tsx`. **`RailRoom` fixtures need every field** — `name`, `members`, `lastActivity`, `buckets`, `flag`, and now `hidden`. Check `ui/src/types/RailRoom.ts` and match it.

```tsx
test('the tab bar offers hide for a visible room and unhide for a hidden one', async () => {
  renderWithStore(<RoomScreen />, {
    room: 'protocol',
    roomLoad: 'ready',
    rail: {
      rooms: [
        { name: 'protocol', members: [], lastActivity: null, buckets: [], flag: null, hidden: false },
      ],
      agents: [],
    },
    messages: [],
  })
  expect(await screen.findByRole('button', { name: 'hide' })).toBeDefined()
  expect(screen.queryByRole('button', { name: 'unhide' })).toBeNull()
})

test('clicking hide asks the store to hide this room', async () => {
  renderWithStore(<RoomScreen />, {
    room: 'protocol',
    roomLoad: 'ready',
    rail: {
      rooms: [
        { name: 'protocol', members: [], lastActivity: null, buckets: [], flag: null, hidden: false },
      ],
      agents: [],
    },
    messages: [],
  })
  fireEvent.click(await screen.findByRole('button', { name: 'hide' }))
  expect(storeActions.setHidden).toHaveBeenCalledWith('protocol', true)
})
```

Add `setHidden: vi.fn()` to `storeActions` in `ui/src/testing/fakeStore.tsx`.

- [ ] **Step 2: Run to verify they fail**

Run from `ui/`: `npm test -- --run Transcript`
Expected: FAIL — no `hide` button.

- [ ] **Step 3: The API call**

In `ui/src/data/api.ts`, beside the existing helpers:

```ts
export const setRoomHidden = (room: string, hidden: boolean) =>
  fetch(`/api/rooms/${encodeURIComponent(room)}/hidden`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ hidden }),
  }).then((r) => {
    if (!r.ok) throw new Error(`hide failed: ${r.status}`)
  })
```

The `encodeURIComponent(room)`-in-a-template-literal form is verified — it is what `fetchMessages` and `fetchAgent` already do, and it matters here because the console reaches rooms like `dm:caas|network-debug#2`, which is exactly the room this feature exists for.

**One deliberate divergence from precedent:** the only existing mutating call, the agent DELETE, is a bare `fetch` inline in `DeleteModal.tsx` rather than a helper in `api.ts`. This one goes in `api.ts` with the other calls, because the store needs it as an injectable dep for tests and an inline `fetch` in a component cannot be one.

- [ ] **Step 4: The store action**

In `ui/src/data/store.ts`, add `setRoomHidden` to the `deps` type and wire the real one in `ui/src/useStore.ts`. Then:

```ts
    async setHidden(room: string, hidden: boolean) {
      // Refresh rather than patch the local rail: the bus is the source of
      // truth for the flag, and a message may have unhidden the room since the
      // last poll. `refreshRail` already exists for exactly this — the delete
      // flow calls it so the rail reflects a change it just made rather than
      // waiting out the 25s interval.
      await deps.setRoomHidden(room, hidden)
      await refreshRail()
    },
```

- [ ] **Step 5: The control**

`RoomTabs` gains two props and a right-aligned button:

```tsx
export function RoomTabs({
  view,
  onView,
  count,
  hidden,
  onHidden,
}: {
  view: 'transcript' | 'files'
  onView: (v: 'transcript' | 'files') => void
  count: number | null
  hidden: boolean
  onHidden: (hidden: boolean) => void
}) {
```

and, after the two existing tab buttons:

```tsx
      <button className={styles.hideToggle} onClick={() => onHidden(!hidden)}>
        {hidden ? 'unhide' : 'hide'}
      </button>
```

In `Files.module.css`, `.hideToggle` is mono 400 11px `var(--text-dim)` with `margin-left: auto` to push it right, no border, and `var(--text-secondary)` on hover.

In `RoomScreen`, pass them:

```tsx
<RoomTabs
  view={view}
  onView={setView}
  count={filesFailed ? null : (files?.length ?? null)}
  hidden={railRoom?.hidden ?? false}
  onHidden={(h) => void store.setHidden(room, h)}
/>
```

`railRoom` already exists in that component at `RoomScreen.tsx:91` — `const railRoom = rail?.rooms.find((r) => r.name === room)`, used for the header — verified, so use it rather than adding a second lookup.

Note it is `undefined` for a room the rail has not loaded yet, which is why the prop is `railRoom?.hidden ?? false`: before the rail arrives the control reads `hide`, which is the honest default for a room whose flag is unknown.

- [ ] **Step 6: Run the gate and commit**

```bash
cd ui && npm run format && npm test && npm run typecheck && npm run format:check && npm run build
cd .. && git add ui/src
git commit -F - <<'EOF'
chore: add the hide control to the room screen

One control, one boolean — it reads unhide when the room is already hidden. It
lives on the room's own screen, mirroring agent delete, which is also reached by
going to the thing you want to act on.

setHidden refreshes the rail rather than patching it locally: the bus is the
source of truth, and a message may have unhidden the room since the last poll.
EOF
```

---

### Task 4: The rail's hidden section

**Files:**
- Modify: `ui/src/rail/Rail.tsx`, `ui/src/rail/RoomRow.tsx`, `ui/src/rail/Rail.module.css`
- Modify: every test file whose room fixtures now need `hidden` (fourteen fixtures across ten files)
- Test: `ui/src/rail/Rail.test.tsx`

**Interfaces:**
- Consumes: `RailRoom.hidden`.

- [ ] **Step 1: Write the failing tests**

Append to `ui/src/rail/Rail.test.tsx`, matching the file's existing fixture and render helpers:

```tsx
test('a hidden room is out of the list, and the footer says how many', () => {
  renderWithStore(<Rail />, {
    rail: {
      rooms: [
        { name: 'visible', members: [], lastActivity: null, buckets: [], flag: null, hidden: false },
        { name: 'tidied', members: [], lastActivity: null, buckets: [], flag: null, hidden: true },
      ],
      agents: [],
    },
  })
  expect(screen.getByText('visible')).toBeDefined()
  expect(screen.queryByText('tidied')).toBeNull()
  expect(screen.getByText(/1 hidden/)).toBeDefined()
})

test('expanding the footer reveals them', () => {
  renderWithStore(<Rail />, {
    rail: {
      rooms: [
        { name: 'tidied', members: [], lastActivity: null, buckets: [], flag: null, hidden: true },
      ],
      agents: [],
    },
  })
  fireEvent.click(screen.getByText(/1 hidden/))
  expect(screen.getByText('tidied')).toBeDefined()
})

test('with nothing hidden there is no affordance at all', () => {
  // The console does not advertise a state that does not exist.
  renderWithStore(<Rail />, {
    rail: {
      rooms: [
        { name: 'visible', members: [], lastActivity: null, buckets: [], flag: null, hidden: false },
      ],
      agents: [],
    },
  })
  expect(screen.queryByText(/hidden/)).toBeNull()
})

test('the volume strip caption survives', () => {
  // `last 60 min` captions the strips in every row. The spec originally put the
  // hidden count in its place; it is a footer instead precisely so this stays.
  renderWithStore(<Rail />, {
    rail: {
      rooms: [
        { name: 'tidied', members: [], lastActivity: null, buckets: [], flag: null, hidden: true },
      ],
      agents: [],
    },
  })
  expect(screen.getByText('last 60 min')).toBeDefined()
})
```

- [ ] **Step 2: Run to verify they fail**

Run from `ui/`: `npm test -- --run Rail`
Expected: FAIL — hidden rooms render in the list and there is no footer.

- [ ] **Step 3: Split visible from hidden**

In `Rail.tsx`, after the existing filter and sort:

```tsx
  const [showHidden, setShowHidden] = useState(false)
  const visibleRooms = rooms.filter((r) => !r.hidden)
  const hiddenRooms = rooms.filter((r) => r.hidden)
```

Render `visibleRooms` in the existing `railRows` block. Then, after it:

```tsx
      {hiddenRooms.length > 0 && (
        <>
          <button className={styles.hiddenToggle} onClick={() => setShowHidden(!showHidden)}>
            {hiddenRooms.length} hidden {showHidden ? '▴' : '▾'}
          </button>
          {showHidden && (
            <div className={styles.railRows}>
              {hiddenRooms.map((r) => (
                <RoomRow key={r.name} room={r} dimmed />
              ))}
            </div>
          )}
        </>
      )}
```

`showHidden` is component state and deliberately not persisted — it is a momentary "let me look", not a preference like the theme or the dock.

- [ ] **Step 4: The dimmed variant**

`RoomRow` gains an optional `dimmed?: boolean` prop and adds a class when set:

```tsx
export function RoomRow({ room, dimmed = false }: { room: RailRoom; dimmed?: boolean }) {
```

with the row's className gaining `${dimmed ? styles.rowHidden : ''}`.

In `Rail.module.css`: `.hiddenToggle` is mono 400 11px `var(--text-dimmest)`, full width, left-aligned, `padding: 6px 9px`, no border, transparent background, `var(--text-dim)` on hover. `.rowHidden` sets `opacity: 0.55`.

`opacity` rather than a colour token because the row carries several colours at once — name, badge, volume strip — and dimming them individually would need a parallel token for each.

- [ ] **Step 5: Fix the fixtures**

`npm run typecheck` now fails in every test file that builds a room object. Add `hidden: false` to each. **Change nothing else in those files.** A sweep this wide is where a weakened assertion hides; the reviewer will be checking for exactly that.

- [ ] **Step 6: Run the gate and commit**

```bash
cd ui && npm run format && npm test && npm run typecheck && npm run format:check && npm run build
cd .. && git add ui/src
git commit -F - <<'EOF'
chore: put hidden rooms behind a footer toggle in the rail

A footer row rather than the header the spec called for: the header's right slot
reads `last 60 min`, which captions the volume strip in every room row, and the
handoff is explicit that a chart without its caption is indistinguishable from a
broken one. Two labels would also fight for a 320px header.

No affordance at all when nothing is hidden — the console does not advertise a
state that does not exist. The expansion is component state and not persisted: a
momentary "let me look", not a preference like the theme or the dock.

The fourteen room fixtures across ten test files gained `hidden: false` and
nothing else.
EOF
```

---

### Task 5: Verify against a real bus

**Files:** none — verification only, no commit.

- [ ] **Step 1: Run both gates**

```bash
cd ui && npm run typecheck && npm run format:check && npm test && npm run build
cd .. && cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
```

- [ ] **Step 2: Build and seed**

```bash
cd ui && npm run build && cd ..
cargo build
rm -rf /tmp/claude-bus-hide
./target/debug/claude-bus serve --port 7810 --data /tmp/claude-bus-hide &
```

Build order is load-bearing — `rust-embed` compiles the bundle into the binary, and a bus already running keeps its old copy. Seed two or three rooms with a few messages, using the scoped-`Store`-then-`start` pattern from `tests/web.rs`.

- [ ] **Step 3: Confirm each of these and report the result for each**

1. A visible room's tab bar shows `hide`; clicking it removes the room from the rail and the footer appears reading `1 hidden`.
2. Expanding the footer shows the room dimmed; clicking it opens the room; the tab bar now reads `unhide`; clicking that returns it to the list.
3. With nothing hidden, there is **no** footer affordance at all.
4. `last 60 min` is still in the ROOMS header throughout.
5. **The automatic unhide, end to end:** hide a room, then send a message into it from a second client (`claude-bus chat`, or the composer in a second tab), and confirm it returns to the rail on its own within the rail's 25s poll.
6. Hide a room whose name contains a `|` and a `#` — a DM key like `dm:caas|network-debug#2`. This is the room the feature exists for, and it is where path encoding breaks.
7. `room_hidden` and `room_unhidden` appear in the events dock for manual actions, and **no event** is written for the automatic unhide.
8. Both themes: the dimmed row and the footer are legible in light as well as dark.

- [ ] **Step 4: Commit nothing; report**

Report each check's result, including anything that looked wrong but you could not attribute.

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task |
|---|---|
| `hidden` column via `add_column_if_missing` | 1 |
| `set_room_hidden`, false rather than insert for unknown room | 1 |
| A message unhides; nothing else does | 1 |
| `POST /api/rooms/{name}/hidden`, CSRF, 404 | 2 |
| Rail ships every room with the flag | 2 |
| `room_hidden` / `room_unhidden` events; none for the automatic unhide | 2 |
| Control on the room screen, `hide` / `unhide` | 3 |
| Count and expansion, collapsed by default, absent when nothing hidden | 4 |
| Dimmed hidden rows | 4 |
| Manual pass incl. the automatic unhide and a DM-key room | 5 |
| The unhide-round-trip friction judged | 5 (check 2) |

**Placeholder scan:** no TBD/TODO, no "add error handling".

**All five "verify this" points were resolved here rather than pushed downstream** — writing "check the signature before using it" into a plan does not remove an error, it relocates it to someone with less context. Four were correct as drafted; one was wrong:

1. **Wrong:** the plan had store tests in an inline `#[cfg(test)]` module with a `test_store()` helper. They live in `tests/store.rs` and the helper is `temp_store()`. Corrected in six places.
2. `append_message(room, from, body, done, human)` — correct, and it delegates to `append_message_at`, which is why the unhide goes there.
3. `append_event(kind, Option<&str>, Option<&str>, Value)` at `events.rs:38` — correct as called.
4. `encodeURIComponent` in a template literal — matches `fetchMessages` and `fetchAgent`.
5. `railRoom` exists at `RoomScreen.tsx:91` — correct, and its `undefined` case is now handled explicitly.

Two remain as genuine judgement calls for the implementer, both flagged inline: whether `common::post_json` helpers already exist in `tests/common/mod.rs`, and axum 0.8's extractor ordering for a body extractor.

**Type consistency:** `RoomRow.hidden` (Task 1) is read by the rail handler as `r.hidden` (Task 2). `RailRoom.hidden` (Task 2) is consumed as `railRoom?.hidden` (Task 3) and `r.hidden` (Task 4). `store.setHidden(room, hidden)` has the same signature in Task 3's implementation and its tests. `setRoomHidden` is the api-layer name; `setHidden` is the store action — deliberately different, and used consistently.

**One deviation from the approved spec, flagged at the top of this plan rather than buried:** the hidden count is a footer row, not a replacement for the header's `last 60 min`. Task 4 includes a test that the caption survives.

**One risk restated:** Task 4 touches fourteen fixtures across ten files. The reviewer should confirm those files gained `hidden: false` and nothing else — a weakened assertion inside a mechanical sweep is exactly what nobody looks at.
