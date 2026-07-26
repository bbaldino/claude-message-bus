use claude_bus::store::{MessageRow, Store};

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
        .upsert_agent("caas", "lisa", "/w/caas", Some("sess-1"))
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
        .upsert_agent("caas", "lisa", "/w/caas", Some("sess-1"))
        .await
        .unwrap();
    store
        .upsert_agent("caas", "lisa", "/w/caas", Some("sess-2"))
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
        .append_message("protocol", "caas", "first", false)
        .await
        .unwrap();
    let b = store
        .append_message("protocol", "dashboard", "second", false)
        .await
        .unwrap();
    assert!(b > a, "ids must increase: {a} then {b}");
}

#[tokio::test]
async fn history_returns_oldest_first_and_respects_limit() {
    let (_d, store) = seeded().await;
    for i in 0..5 {
        store
            .append_message("protocol", "caas", &format!("msg{i}"), false)
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
        .append_message("protocol", "caas", "hi", false)
        .await
        .unwrap();
    store.set_cursor("protocol", "dashboard", id).await.unwrap();
    assert_eq!(store.cursor("protocol", "dashboard").await.unwrap(), id);
}

#[tokio::test]
async fn unread_counts_only_messages_past_the_cursor() {
    let (_d, store) = seeded().await;
    let first = store
        .append_message("protocol", "caas", "one", false)
        .await
        .unwrap();
    store
        .append_message("protocol", "caas", "two", false)
        .await
        .unwrap();
    store
        .append_message("protocol", "caas", "three", false)
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
        .append_message("protocol", "dashboard", "self-sent", false)
        .await
        .unwrap();
    store
        .append_message("protocol", "caas", "four", false)
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
        .append_message("protocol", "caas", "one", false)
        .await
        .unwrap();
    store
        .append_message("protocol", "caas", "two", false)
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
        .append_message("protocol", "dashboard", "self-sent", false)
        .await
        .unwrap();
    store
        .append_message("protocol", "caas", "three", false)
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
        .append_message("protocol", "caas", "settled", true)
        .await
        .unwrap();
    let msgs = store.history("protocol", 10).await.unwrap();
    assert!(msgs[0].done);
}
