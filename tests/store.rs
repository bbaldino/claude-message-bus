use claude_bus::store::Store;

async fn temp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).await.expect("open store");
    (dir, store)
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
