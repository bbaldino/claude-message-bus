mod common;

use claude_bus::proto::{AgentState, FromBus, ToBus};

fn status(state: AgentState) -> ToBus {
    ToBus::Status {
        state,
        state_age_ms: Some(0),
        heartbeat_age_ms: Some(0),
        reason: None,
        text: None,
        text_age_ms: None,
        waiting_on: None,
    }
}

async fn events_of(port: u16, kind: &str) -> Vec<serde_json::Value> {
    let all = common::get_json(port, &format!("/api/events?kind={kind}")).await;
    all.as_array().unwrap().clone()
}

#[tokio::test]
async fn a_status_reaches_presence_watchers() {
    let (_dir, port) = common::start_bus().await;
    let mut obs = common::connect_observer(port, "console").await;
    common::send(&mut obs, &ToBus::WatchPresence { req_id: 1 }).await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await; // Registered
    common::send(&mut a, &status(AgentState::Working)).await;
    let mut saw = false;
    for _ in 0..10 {
        if let FromBus::Status { name, status } = common::next_event(&mut obs).await {
            assert_eq!(name, "caas");
            assert_eq!(status.state, AgentState::Working);
            saw = true;
            break;
        }
    }
    assert!(saw, "the observer never received the status push");
}

#[tokio::test]
async fn entering_and_leaving_blocked_writes_exactly_two_events() {
    let (_dir, port) = common::start_bus().await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await;
    common::send(&mut a, &status(AgentState::Working)).await;
    let mut b = status(AgentState::BlockedOnHuman);
    if let ToBus::Status { reason, .. } = &mut b {
        *reason = Some("permission_prompt".into());
    }
    common::send(&mut a, &b).await;
    common::send(&mut a, &b).await; // a repeat must not log a second entry
    common::send(&mut a, &status(AgentState::Working)).await;
    common::send(&mut a, &status(AgentState::Idle)).await; // not logged at all
    assert!(
        common::wait_until(|| async { events_of(port, "blocked_on_human").await.len() == 2 }).await,
        "expected one entry and one exit"
    );
    let ev = events_of(port, "blocked_on_human").await;
    let entered: Vec<bool> = ev
        .iter()
        .map(|e| e["detail"]["entered"].as_bool().unwrap())
        .collect();
    assert!(
        entered.contains(&true) && entered.contains(&false),
        "{ev:?}"
    );
    for e in &ev {
        let detail = e["detail"].as_object().unwrap();
        let mut keys: Vec<&str> = detail.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["entered", "reason", "via"], "{ev:?}");
        assert_eq!(detail["via"], "hook", "{ev:?}");
    }
}

#[tokio::test]
async fn disconnecting_while_blocked_closes_the_wait_and_keeps_the_status() {
    let (_dir, port) = common::start_bus().await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await;
    common::send(&mut a, &status(AgentState::BlockedOnHuman)).await;
    assert!(
        common::wait_until(|| async { events_of(port, "blocked_on_human").await.len() == 1 }).await
    );
    drop(a);
    assert!(
        common::wait_until(|| async {
            events_of(port, "blocked_on_human")
                .await
                .iter()
                .any(|e| e["detail"]["via"] == "disconnect")
        })
        .await,
        "a disconnect while blocked must close the wait"
    );
    let ev = events_of(port, "blocked_on_human").await;
    let exit = ev
        .iter()
        .find(|e| e["detail"]["via"] == "disconnect")
        .unwrap();
    let detail = exit["detail"].as_object().unwrap();
    let mut keys: Vec<&str> = detail.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["entered", "reason", "via"], "{exit:?}");
    assert!(detail["reason"].is_null(), "{exit:?}");
}

#[tokio::test]
async fn deleting_an_agent_forgets_its_status() {
    let (_dir, port) = common::start_bus().await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await;
    common::send(&mut a, &status(AgentState::Idle)).await;
    drop(a);
    assert!(common::wait_until(|| async { !common::agent_is_online(port, "caas").await }).await);
    assert_eq!(
        common::delete_same_origin(port, "/api/agents/caas").await,
        204
    );
    let mut again = common::connect(port, "caas").await;
    common::next_event(&mut again).await;
    let agents = common::get_json(port, "/api/agents").await;
    let caas = agents
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "caas")
        .unwrap();
    assert!(
        caas["status"].is_null(),
        "a re-registered name inherited a status: {caas}"
    );
}

#[tokio::test]
async fn status_never_counts_toward_the_exchange_cap() {
    // A cap of 2. Fifty statuses, then one real send, which must still be allowed.
    let (_dir, port, _path) =
        common::start_bus_with_guards_dir(claude_bus::bus::delivery::Guards::new(2, 0)).await;
    let mut a = common::connect(port, "a").await;
    common::next_event(&mut a).await;
    let mut b = common::connect(port, "b").await;
    common::next_event(&mut b).await;
    for _ in 0..50 {
        common::send(&mut a, &status(AgentState::Working)).await;
    }
    common::send(
        &mut a,
        &ToBus::Send {
            req_id: 9,
            target: claude_bus::proto::Target::Agent { name: "b".into() },
            text: "real".into(),
            done: false,
        },
    )
    .await;
    loop {
        match common::next_event(&mut a).await {
            FromBus::Reply { req_id: 9, .. } => break,
            FromBus::Error {
                req_id: Some(9),
                message,
            } => panic!("send refused: {message}"),
            _ => continue,
        }
    }
}

#[tokio::test]
async fn every_reader_reports_the_status() {
    let (_dir, port) = common::start_bus().await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await;
    let mut s = status(AgentState::Idle);
    if let ToBus::Status { text, .. } = &mut s {
        *text = Some("<script>alert(1)</script>".into());
    }
    common::send(&mut a, &s).await;
    assert!(
        common::wait_until(|| async {
            common::get_json(port, "/api/agents").await[0]["status"]["state"] == "idle"
        })
        .await
    );
    let rail = common::get_json(port, "/api/rail").await;
    assert_eq!(rail["agents"][0]["status"]["state"], "idle", "{rail}");
    let detail = common::get_json(port, "/api/agents/caas").await;
    assert_eq!(
        detail["status"]["text"], "<script>alert(1)</script>",
        "stored verbatim; escaping is the renderer's job: {detail}"
    );
}

#[tokio::test]
async fn an_observer_can_list_agents_with_status() {
    let (_dir, port) = common::start_bus().await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await;
    common::send(&mut a, &status(AgentState::Working)).await;
    let mut obs = common::connect_observer(port, "status-cli").await;
    common::send(&mut obs, &ToBus::ListAgents { req_id: 4 }).await;
    loop {
        if let FromBus::Reply {
            req_id: 4,
            result: claude_bus::proto::ReplyResult::Agents { agents },
        } = common::next_event(&mut obs).await
        {
            let caas = agents.iter().find(|x| x.name == "caas").unwrap();
            if caas.status.is_some() {
                break;
            }
            common::send(&mut obs, &ToBus::ListAgents { req_id: 4 }).await;
        }
    }
}
