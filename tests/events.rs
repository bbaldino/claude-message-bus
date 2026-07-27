use claude_bus::store::Store;
use serde_json::json;

async fn temp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).await.expect("open store");
    (dir, store)
}

#[tokio::test]
async fn appends_and_reads_back_an_event() {
    let (_d, store) = temp_store().await;
    store
        .append_event(
            "message_sent",
            Some("caas"),
            Some("protocol"),
            json!({"msg_id": 7, "delivered_to": ["dashboard"]}),
        )
        .await
        .unwrap();

    let evs = store.events(10).await.unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].kind, "message_sent");
    assert_eq!(evs[0].agent.as_deref(), Some("caas"));
    assert_eq!(evs[0].room.as_deref(), Some("protocol"));
    assert_eq!(evs[0].detail["msg_id"], 7);
    assert_eq!(evs[0].detail["delivered_to"][0], "dashboard");
}

#[tokio::test]
async fn nullable_agent_and_room_round_trip_as_none() {
    let (_d, store) = temp_store().await;
    store
        .append_event("bus_started", None, None, json!({}))
        .await
        .unwrap();

    let evs = store.events(10).await.unwrap();
    assert!(evs[0].agent.is_none());
    assert!(evs[0].room.is_none());
}

#[tokio::test]
async fn events_returns_most_recent_first() {
    let (_d, store) = temp_store().await;
    for i in 0..3 {
        store
            .append_event("ack", Some("caas"), Some("r"), json!({"n": i}))
            .await
            .unwrap();
    }
    let evs = store.events(10).await.unwrap();
    assert_eq!(evs[0].detail["n"], 2, "newest first");
    assert_eq!(evs[2].detail["n"], 0);
}

#[tokio::test]
async fn events_for_room_returns_oldest_first_for_interleaving() {
    // The transcript view merges these with messages in chronological order, so this
    // query must hand them over in the order they happened, unlike the others.
    let (_d, store) = temp_store().await;
    for i in 0..3 {
        store
            .append_event(
                "message_sent",
                Some("caas"),
                Some("protocol"),
                json!({"n": i}),
            )
            .await
            .unwrap();
    }
    store
        .append_event(
            "message_sent",
            Some("caas"),
            Some("other"),
            json!({"n": 99}),
        )
        .await
        .unwrap();

    let evs = store.events_for_room("protocol", 10).await.unwrap();
    assert_eq!(evs.len(), 3, "only this room's events");
    assert_eq!(evs[0].detail["n"], 0, "oldest first");
    assert_eq!(evs[2].detail["n"], 2);
}

#[tokio::test]
async fn events_filter_by_agent_and_by_kind() {
    let (_d, store) = temp_store().await;
    store
        .append_event("ack", Some("caas"), Some("r"), json!({}))
        .await
        .unwrap();
    store
        .append_event("room_paused", Some("caas"), Some("r"), json!({}))
        .await
        .unwrap();
    store
        .append_event("ack", Some("dashboard"), Some("r"), json!({}))
        .await
        .unwrap();

    assert_eq!(store.events_for_agent("caas", 10).await.unwrap().len(), 2);
    assert_eq!(store.events_of_kind("ack", 10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn limit_is_respected() {
    let (_d, store) = temp_store().await;
    for i in 0..5 {
        store
            .append_event("ack", Some("caas"), Some("r"), json!({"n": i}))
            .await
            .unwrap();
    }
    assert_eq!(store.events(2).await.unwrap().len(), 2);
}

#[tokio::test]
async fn malformed_detail_json_does_not_poison_reads() {
    // detail_json is TEXT; a bad row should degrade to Null rather than failing the
    // whole query and hiding every other event on the page.
    let (_d, store) = temp_store().await;
    store
        .append_event("ack", Some("caas"), Some("r"), json!({"ok": true}))
        .await
        .unwrap();
    sqlx::query("INSERT INTO events (created_at, kind, agent, room, detail_json) VALUES (1, 'bad', 'x', 'r', 'not json')")
        .execute(store.pool_for_test())
        .await
        .unwrap();

    let evs = store.events(10).await.unwrap();
    assert_eq!(evs.len(), 2, "the bad row must not sink the query");
}
