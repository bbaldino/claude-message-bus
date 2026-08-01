use claude_bus::store::{MAX_BLOB_BYTES, MessageRow, Store};

async fn temp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).await.expect("open store");
    (dir, store)
}

async fn seeded() -> (tempfile::TempDir, Store) {
    let (d, store) = temp_store().await;
    store.ensure_room("protocol").await.unwrap();
    store.join_room("protocol", "caas").await.unwrap();
    store.join_room("protocol", "dashboard").await.unwrap();
    (d, store)
}

#[tokio::test]
async fn registers_an_agent_and_lists_it() {
    let (_d, store) = temp_store().await;
    store
        .upsert_agent("caas", "lisa", "/w/caas", Some("sess-1"), false, None)
        .await
        .unwrap();
    store.set_online("caas", true).await.unwrap();

    let agents = store.agents().await.unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "caas");
    assert_eq!(agents[0].host, "lisa");
    assert_eq!(agents[0].session_id.as_deref(), Some("sess-1"));
    assert!(agents[0].online);
}

#[tokio::test]
async fn reregistering_updates_rather_than_duplicates() {
    let (_d, store) = temp_store().await;
    store
        .upsert_agent("caas", "lisa", "/w/caas", Some("sess-1"), false, None)
        .await
        .unwrap();
    store
        .upsert_agent("caas", "lisa", "/w/caas", Some("sess-2"), false, None)
        .await
        .unwrap();

    let agents = store.agents().await.unwrap();
    assert_eq!(agents.len(), 1, "same name must not create a second row");
    assert_eq!(agents[0].session_id.as_deref(), Some("sess-2"));
}

#[tokio::test]
async fn membership_survives_going_offline() {
    // Membership is keyed by agent name, not session, so closing and reopening
    // a session rejoins its rooms.
    let (_d, store) = temp_store().await;
    store.ensure_room("protocol").await.unwrap();
    store.join_room("protocol", "caas").await.unwrap();
    store.set_online("caas", false).await.unwrap();

    assert_eq!(store.room_members("protocol").await.unwrap(), vec!["caas"]);
}

#[tokio::test]
async fn joining_twice_is_idempotent() {
    let (_d, store) = temp_store().await;
    store.ensure_room("protocol").await.unwrap();
    store.join_room("protocol", "caas").await.unwrap();
    store.join_room("protocol", "caas").await.unwrap();

    assert_eq!(store.room_members("protocol").await.unwrap(), vec!["caas"]);
}

#[tokio::test]
async fn rooms_come_back_with_members_and_default_mode() {
    let (_d, store) = temp_store().await;
    store.ensure_room("protocol").await.unwrap();
    store.join_room("protocol", "caas").await.unwrap();
    store.join_room("protocol", "dashboard").await.unwrap();

    let rooms = store.rooms().await.unwrap();
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].name, "protocol");
    assert_eq!(rooms[0].mode, "discuss");
    assert_eq!(rooms[0].members, vec!["caas", "dashboard"]);
}

#[tokio::test]
async fn state_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.ensure_room("protocol").await.unwrap();
        store.join_room("protocol", "caas").await.unwrap();
    }
    let store = Store::open(dir.path()).await.unwrap();
    assert_eq!(store.room_members("protocol").await.unwrap(), vec!["caas"]);
}

#[tokio::test]
async fn message_ids_increase_monotonically() {
    let (_d, store) = seeded().await;
    let a = store
        .append_message("protocol", "caas", "first", false, false)
        .await
        .unwrap();
    let b = store
        .append_message("protocol", "dashboard", "second", false, false)
        .await
        .unwrap();
    assert!(b > a, "ids must increase: {a} then {b}");
}

#[tokio::test]
async fn history_returns_oldest_first_and_respects_limit() {
    let (_d, store) = seeded().await;
    for i in 0..5 {
        store
            .append_message("protocol", "caas", &format!("msg{i}"), false, false)
            .await
            .unwrap();
    }
    let all: Vec<MessageRow> = store.history("protocol", 100).await.unwrap();
    assert_eq!(all.len(), 5);
    assert_eq!(all[0].body, "msg0", "oldest first");
    assert_eq!(all[4].body, "msg4");

    let recent = store.history("protocol", 2).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(
        recent[0].body, "msg3",
        "limit takes the most recent, oldest-first"
    );
    assert_eq!(recent[1].body, "msg4");
}

#[tokio::test]
async fn cursor_starts_at_zero_and_advances() {
    let (_d, store) = seeded().await;
    assert_eq!(store.cursor("protocol", "dashboard").await.unwrap(), 0);
    let id = store
        .append_message("protocol", "caas", "hi", false, false)
        .await
        .unwrap();
    store.set_cursor("protocol", "dashboard", id).await.unwrap();
    assert_eq!(store.cursor("protocol", "dashboard").await.unwrap(), id);
}

// A stale or out-of-order ack (or a history call that only saw an older
// window of messages) must never resurrect already-read messages as unread
// by dragging the cursor backwards.
#[tokio::test]
async fn set_cursor_never_moves_backwards() {
    let (_d, store) = seeded().await;
    let first = store
        .append_message("protocol", "caas", "one", false, false)
        .await
        .unwrap();
    let second = store
        .append_message("protocol", "caas", "two", false, false)
        .await
        .unwrap();

    store
        .set_cursor("protocol", "dashboard", second)
        .await
        .unwrap();
    store
        .set_cursor("protocol", "dashboard", first)
        .await
        .unwrap();

    assert_eq!(
        store.cursor("protocol", "dashboard").await.unwrap(),
        second,
        "a lower id must not move the cursor backwards"
    );
}

#[tokio::test]
async fn unread_counts_only_messages_past_the_cursor() {
    let (_d, store) = seeded().await;
    let first = store
        .append_message("protocol", "caas", "one", false, false)
        .await
        .unwrap();
    store
        .append_message("protocol", "caas", "two", false, false)
        .await
        .unwrap();
    store
        .append_message("protocol", "caas", "three", false, false)
        .await
        .unwrap();

    assert_eq!(
        store.unread_count("protocol", "dashboard").await.unwrap(),
        3
    );
    store
        .set_cursor("protocol", "dashboard", first)
        .await
        .unwrap();
    assert_eq!(
        store.unread_count("protocol", "dashboard").await.unwrap(),
        2
    );

    // dashboard's own message must not inflate its own unread count — only
    // caas's later message should count.
    store
        .append_message("protocol", "dashboard", "self-sent", false, false)
        .await
        .unwrap();
    store
        .append_message("protocol", "caas", "four", false, false)
        .await
        .unwrap();
    assert_eq!(
        store.unread_count("protocol", "dashboard").await.unwrap(),
        3,
        "dashboard's own message must be excluded from its own unread count"
    );
}

#[tokio::test]
async fn undelivered_returns_exactly_the_messages_past_the_cursor() {
    let (_d, store) = seeded().await;
    let first = store
        .append_message("protocol", "caas", "one", false, false)
        .await
        .unwrap();
    store
        .append_message("protocol", "caas", "two", false, false)
        .await
        .unwrap();
    store
        .set_cursor("protocol", "dashboard", first)
        .await
        .unwrap();

    let pending = store.undelivered("protocol", "dashboard").await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].body, "two");

    // dashboard's own message must not be delivered back to itself, even
    // though it is past dashboard's cursor.
    store
        .append_message("protocol", "dashboard", "self-sent", false, false)
        .await
        .unwrap();
    store
        .append_message("protocol", "caas", "three", false, false)
        .await
        .unwrap();
    let pending = store.undelivered("protocol", "dashboard").await.unwrap();
    assert_eq!(
        pending.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(),
        vec!["two", "three"],
        "dashboard's own message must be excluded from what is delivered to it"
    );
}

#[tokio::test]
async fn done_flag_round_trips() {
    let (_d, store) = seeded().await;
    store
        .append_message("protocol", "caas", "settled", true, false)
        .await
        .unwrap();
    let msgs = store.history("protocol", 10).await.unwrap();
    assert!(msgs[0].done);
}

#[tokio::test]
async fn file_round_trips() {
    let (_d, store) = seeded().await;
    let stored = store
        .put_file(
            "protocol",
            "schema.json",
            b"{\"a\":1}",
            Some("application/json"),
            "caas",
        )
        .await
        .unwrap();
    assert_eq!(stored.size, 7);
    assert_eq!(stored.updated_by, "caas");

    let (meta, bytes) = store
        .get_file("protocol", "schema.json")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"{\"a\":1}");
    assert_eq!(meta.content_type.as_deref(), Some("application/json"));
    assert_eq!(meta.sha256, stored.sha256);
}

#[tokio::test]
async fn writing_the_same_key_overwrites() {
    let (_d, store) = seeded().await;
    store
        .put_file("protocol", "notes.md", b"first", None, "caas")
        .await
        .unwrap();
    store
        .put_file("protocol", "notes.md", b"second", None, "dashboard")
        .await
        .unwrap();

    let files = store.list_files("protocol").await.unwrap();
    assert_eq!(files.len(), 1, "overwrite by key, no versioning");
    let (_m, bytes) = store
        .get_file("protocol", "notes.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"second");
}

#[tokio::test]
async fn missing_file_is_none_not_an_error() {
    let (_d, store) = seeded().await;
    assert!(
        store
            .get_file("protocol", "nope.txt")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn identical_content_shares_one_blob() {
    // Content addressing: two keys with the same bytes must not store twice.
    let (_d, store) = seeded().await;
    let a = store
        .put_file("protocol", "a.txt", b"same", None, "caas")
        .await
        .unwrap();
    let b = store
        .put_file("protocol", "b.txt", b"same", None, "caas")
        .await
        .unwrap();
    assert_eq!(a.sha256, b.sha256);
}

#[tokio::test]
async fn oversized_blob_is_rejected_with_a_clear_message() {
    let (_d, store) = seeded().await;
    let huge = vec![0u8; MAX_BLOB_BYTES + 1];
    let err = store
        .put_file("protocol", "huge.bin", &huge, None, "caas")
        .await
        .expect_err("must reject");
    let msg = err.to_string();

    // Derive the expected wording from MAX_BLOB_BYTES itself, not a literal,
    // so this test breaks if the message and the constant ever disagree.
    let limit_mb = MAX_BLOB_BYTES as f64 / (1024.0 * 1024.0);
    let expected = format!("the limit is {limit_mb:.0} MB");
    assert!(
        msg.contains(&expected),
        "error should state the limit derived from MAX_BLOB_BYTES ({expected:?}), got: {msg:?}"
    );
}

#[tokio::test]
async fn files_are_scoped_to_their_room() {
    let (_d, store) = seeded().await;
    store.ensure_room("other").await.unwrap();
    store
        .put_file("protocol", "k.txt", b"x", None, "caas")
        .await
        .unwrap();
    assert!(store.get_file("other", "k.txt").await.unwrap().is_none());
    assert_eq!(store.list_files("other").await.unwrap().len(), 0);
}

#[tokio::test]
async fn mark_all_offline_clears_ghosts_left_by_a_bus_that_died() {
    // A bus process killed mid-connection never runs its per-connection teardown, so
    // `set_online(false)` never fires and the row keeps claiming the agent is online.
    // Nothing else reconciles it, so without this the agent list shows ghosts forever.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("ghost", "hardac", "/w/g", None, false, None)
        .await
        .unwrap();
    store.set_online("ghost", true).await.unwrap();
    assert!(
        store
            .agents()
            .await
            .unwrap()
            .iter()
            .any(|a| a.name == "ghost" && a.online),
        "precondition: the agent is recorded online"
    );

    store.mark_all_offline().await.unwrap();

    assert!(
        store.agents().await.unwrap().iter().all(|a| !a.online),
        "a freshly started bus has no live connections, so nothing may claim online"
    );
}

#[tokio::test]
async fn a_fresh_database_has_the_is_human_column() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("bbaldino", "hardac", "/w", None, true, None)
        .await
        .unwrap();
    let agents = store.agents().await.unwrap();
    let me = agents.iter().find(|a| a.name == "bbaldino").unwrap();
    assert!(me.is_human, "the flag must round-trip through the store");
}

#[tokio::test]
async fn an_agent_defaults_to_not_human() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("caas", "hardac", "/w", None, false, None)
        .await
        .unwrap();
    let agents = store.agents().await.unwrap();
    assert!(!agents.iter().find(|a| a.name == "caas").unwrap().is_human);
}

#[tokio::test]
async fn the_migration_adds_the_column_to_a_database_that_predates_it() {
    // The real case: the deployed bus's volume already holds an `agents` table
    // without this column. Simulate it by creating the old shape by hand, then
    // opening a Store over it the way a freshly deployed binary would.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bus.db");
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rwc", db.display()))
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE agents (
               name TEXT PRIMARY KEY, host TEXT NOT NULL, cwd TEXT NOT NULL,
               session_id TEXT, connected_at INTEGER NOT NULL,
               last_seen INTEGER NOT NULL, online INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agents (name, host, cwd, connected_at, last_seen, online)
             VALUES ('caas', 'hardac', '/w', 1, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    let store = Store::open(dir.path()).await.unwrap();

    let agents = store.agents().await.unwrap();
    let caas = agents.iter().find(|a| a.name == "caas").unwrap();
    assert!(!caas.is_human, "a pre-existing row defaults to not human");
    assert_eq!(
        caas.host, "hardac",
        "existing data must survive the migration"
    );
}

#[tokio::test]
async fn the_migration_is_idempotent() {
    // Opening the same database twice must not fail on a duplicate column.
    let dir = tempfile::tempdir().unwrap();
    let first = Store::open(dir.path()).await.unwrap();
    first
        .upsert_agent("caas", "hardac", "/w", None, false, None)
        .await
        .unwrap();
    drop(first);

    let second = Store::open(dir.path()).await.unwrap();
    assert_eq!(second.agents().await.unwrap().len(), 1);
}

#[test]
fn a_register_payload_without_the_human_field_still_deserializes() {
    // This is exactly the payload an already-running agent binary sends. Claude Code
    // does not respawn stdio MCP servers mid-session, so those binaries cannot be
    // updated without restarting their sessions — if this ever fails, shipping the
    // change silently disconnects every live agent.
    let old = r#"{"type":"register","name":"caas","host":"hardac","cwd":"/w","session_id":null}"#;
    let parsed: claude_bus::proto::ToBus = serde_json::from_str(old).unwrap();
    match parsed {
        claude_bus::proto::ToBus::Register { name, human, .. } => {
            assert_eq!(name, "caas");
            assert!(!human, "an absent field must mean not human");
        }
        other => panic!("expected Register, got {other:?}"),
    }
}

#[test]
fn a_register_payload_with_human_true_round_trips() {
    let new = r#"{"type":"register","name":"bbaldino","host":"hardac","cwd":"/w","session_id":null,"human":true}"#;
    let parsed: claude_bus::proto::ToBus = serde_json::from_str(new).unwrap();
    match parsed {
        claude_bus::proto::ToBus::Register { human, .. } => assert!(human),
        other => panic!("expected Register, got {other:?}"),
    }
}

#[tokio::test]
async fn a_room_exists_once_created_even_with_no_members() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store.join_room("solo", "bbaldino").await.unwrap();
    store.leave_all_rooms("bbaldino").await.unwrap();

    assert!(store.room_members("solo").await.unwrap().is_empty());
    assert!(
        store.room_exists("solo").await.unwrap(),
        "an empty room is still a room"
    );
}

#[tokio::test]
async fn a_room_that_was_never_created_does_not_exist() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    assert!(!store.room_exists("nonesuch").await.unwrap());
}

#[tokio::test]
async fn an_agents_version_round_trips_through_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("caas", "hardac", "/w", None, false, Some("0.1.2"))
        .await
        .unwrap();
    let agents = store.agents().await.unwrap();
    let caas = agents.iter().find(|a| a.name == "caas").unwrap();
    assert_eq!(caas.version.as_deref(), Some("0.1.2"));
}

#[tokio::test]
async fn an_agent_that_reports_no_version_stores_null() {
    // This is the population the feature exists to surface: a binary that predates
    // the field. `None` must survive as `None`, not become an empty string.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("old", "hardac", "/w", None, false, None)
        .await
        .unwrap();
    let agents = store.agents().await.unwrap();
    assert_eq!(
        agents.iter().find(|a| a.name == "old").unwrap().version,
        None
    );
}

#[tokio::test]
async fn re_registering_updates_a_stale_version() {
    // A session restarted onto a new binary must stop reporting the old one.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("caas", "hardac", "/w", None, false, Some("0.1.0"))
        .await
        .unwrap();
    store
        .upsert_agent("caas", "hardac", "/w", None, false, Some("0.1.2"))
        .await
        .unwrap();
    let agents = store.agents().await.unwrap();
    assert_eq!(
        agents
            .iter()
            .find(|a| a.name == "caas")
            .unwrap()
            .version
            .as_deref(),
        Some("0.1.2")
    );
}

#[tokio::test]
async fn the_migration_adds_the_version_column_to_an_older_database() {
    // The deployed bus's volume holds an `agents` table without this column.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bus.db");
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rwc", db.display()))
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE agents (
               name TEXT PRIMARY KEY, host TEXT NOT NULL, cwd TEXT NOT NULL,
               session_id TEXT, connected_at INTEGER NOT NULL,
               last_seen INTEGER NOT NULL, online INTEGER NOT NULL DEFAULT 0,
               is_human INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agents (name, host, cwd, connected_at, last_seen, online, is_human)
             VALUES ('caas', 'hardac', '/w', 1, 1, 1, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    let store = Store::open(dir.path()).await.unwrap();

    let agents = store.agents().await.unwrap();
    let caas = agents.iter().find(|a| a.name == "caas").unwrap();
    assert_eq!(caas.version, None, "a pre-existing row reports no version");
    assert_eq!(
        caas.host, "hardac",
        "existing data must survive the migration"
    );
}

#[test]
fn a_register_payload_without_the_version_field_still_deserializes() {
    // Exactly what an already-running agent binary sends. If this ever fails,
    // shipping the change disconnects every live agent — including the stale ones
    // this feature is meant to reveal.
    let old = r#"{"type":"register","name":"caas","host":"hardac","cwd":"/w","session_id":null,"human":false}"#;
    let parsed: claude_bus::proto::ToBus = serde_json::from_str(old).unwrap();
    match parsed {
        claude_bus::proto::ToBus::Register { name, version, .. } => {
            assert_eq!(name, "caas");
            assert_eq!(
                version, None,
                "an absent field must stay absent, not default to a string"
            );
        }
        other => panic!("expected Register, got {other:?}"),
    }
}

#[tokio::test]
async fn a_message_records_whether_a_human_sent_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store.join_room("protocol", "caas").await.unwrap();
    store
        .append_message("protocol", "bbaldino", "please refactor this", false, true)
        .await
        .unwrap();
    store
        .append_message("protocol", "caas", "on it", false, false)
        .await
        .unwrap();

    let rows = store.history("protocol", 10).await.unwrap();
    let human = rows.iter().find(|m| m.from_agent == "bbaldino").unwrap();
    let agent = rows.iter().find(|m| m.from_agent == "caas").unwrap();
    assert!(human.human, "a human's message must be marked");
    assert!(!agent.human, "an agent's must not be");
}

#[tokio::test]
async fn the_migration_adds_the_message_origin_column_to_an_older_database() {
    // The deployed bus's volume already holds a `messages` table without this
    // column. Simulate that shape, then open a Store over it.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bus.db");
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rwc", db.display()))
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE messages (
               id INTEGER PRIMARY KEY AUTOINCREMENT, room TEXT NOT NULL,
               from_agent TEXT NOT NULL, body TEXT NOT NULL,
               done INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (room, from_agent, body, done, created_at)
             VALUES ('protocol', 'caas', 'from before the column existed', 0, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    let store = Store::open(dir.path()).await.unwrap();

    let rows = store.history("protocol", 10).await.unwrap();
    assert_eq!(rows.len(), 1, "existing data must survive the migration");
    assert_eq!(rows[0].body, "from before the column existed");
    assert!(!rows[0].human, "a pre-existing row defaults to not human");
}
