// Drives the real server over HTTP against a temp SQLite. Asserts returned content,
// not just status codes: a response that is 200 with an empty list has failed at its
// only job.
mod common;

use claude_bus::store::Store;

async fn start(dir: &std::path::Path) -> u16 {
    let path = dir.to_path_buf();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { claude_bus::bus::serve_on(listener, path).await.unwrap() });
    common::wait_until_bus_ready(port).await;
    port
}

/// Like `start`, but with a configured relayer set. `serve_on` hardcodes an empty
/// one, and relayer reporting is exactly what needs a non-empty set to test.
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
            claude_bus::bus::participant::ParticipantConfig::default(),
        )
        .await
        .unwrap()
    });
    common::wait_until_bus_ready(port).await;
    port
}

/// Like `start`, but with the exchange guard's cap set directly (and the rate
/// limit off). `serve_on` uses the production cap of 20, which is a lot of
/// round trips to trip on purpose.
async fn start_with_cap(dir: &std::path::Path, cap: u32) -> u16 {
    let path = dir.to_path_buf();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        claude_bus::bus::serve_on_full(
            listener,
            path,
            claude_bus::bus::delivery::Guards::new(cap, 0),
            claude_bus::bus::Keepalive::default(),
            claude_bus::bus::registry::Registry::new(),
            claude_bus::bus::Relayers::default(),
            claude_bus::bus::participant::ParticipantConfig::default(),
        )
        .await
        .unwrap()
    });
    common::wait_until_bus_ready(port).await;
    port
}

async fn get(port: u16, path: &str) -> String {
    let url = format!("http://127.0.0.1:{port}{path}");
    // Minimal HTTP/1.1 GET so the test needs no HTTP client dependency.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let _ = url;
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}

/// Minimal HTTP/1.1 POST with an empty body, matching `get`'s no-dependency style.
/// Returns the whole raw response so a test can assert on the status line as well
/// as the body. Sends no `Origin`, like `curl` — which the delete route must allow,
/// since the same-origin check only exists to stop a *browser* being used as a
/// proxy into a network it can reach and the caller cannot.
async fn post(port: u16, path: &str) -> String {
    post_with_headers(port, path, &[]).await
}

/// `post`, plus arbitrary extra request headers — for asserting the same-origin
/// check, which is invisible to a request that carries no `Origin` at all.
async fn post_with_headers(port: u16, path: &str, extra: &[(&str, &str)]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let mut req = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n"
    );
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}

#[tokio::test]
async fn the_agents_api_does_not_report_ghosts_from_a_previous_bus() {
    // The regression: `online` is persisted, but the registry that knows who is actually
    // connected is in memory. A bus killed mid-connection leaves the column claiming an
    // agent is online; a later bus reading that column reports a ghost. This asserts the
    // API reflects the live registry (and the startup reconciliation), not the column.
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("ghost", "hardac", "/w/g", None, false, None)
            .await
            .unwrap();
        store.set_online("ghost", true).await.unwrap();
    }
    let port = start(dir.path()).await;

    let body = common::get_json(port, "/api/agents").await;
    let ghost = body
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "ghost")
        .unwrap_or_else(|| panic!("the agent is still known, just not connected: {body}"));
    assert_eq!(
        ghost["online"], false,
        "nothing is connected to this freshly started bus: {body}"
    );
}

#[tokio::test]
async fn the_agents_api_returns_json_in_camel_case() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent(
                "network-debug#2",
                "hardac",
                "/w/nd",
                Some("sess-1"),
                false,
                Some("0.3.3"),
            )
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/api/agents").await;

    assert!(
        body.contains("application/json"),
        "must be served as JSON: {body}"
    );
    assert!(body.contains("\"name\":\"network-debug#2\""), "got: {body}");
    assert!(body.contains("\"host\":\"hardac\""), "got: {body}");
    // camelCase on the wire even though the column is last_seen.
    assert!(
        body.contains("\"lastSeen\":"),
        "wire format must be camelCase: {body}"
    );
    assert!(body.contains("\"sessionId\":\"sess-1\""), "got: {body}");
    // mark_all_offline runs at startup, so a seeded agent is offline.
    assert!(body.contains("\"online\":false"), "got: {body}");
}

/// Which agents in a JSON array are marked `isRelayer`, sorted.
fn relayer_names(agents: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = agents
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["isRelayer"] == true)
        .map(|a| a["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}

async fn seed_agents(dir: &std::path::Path, names: &[&str]) {
    let store = Store::open(dir).await.unwrap();
    for n in names {
        store
            .upsert_agent(n, "hardac", "/w", None, false, Some("0.3.0"))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_configured_relayer_is_marked_everywhere_an_agent_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    seed_agents(dir.path(), &["hub", "caas"]).await;
    let port = start_with_relayers(dir.path(), &["hub"]).await;

    let agents = common::get_json(port, "/api/agents").await;
    assert_eq!(relayer_names(&agents), ["hub"], "{agents}");

    let rail = common::get_json(port, "/api/rail").await;
    assert_eq!(relayer_names(&rail["agents"]), ["hub"], "{rail}");
    assert_eq!(rail["relayers"], serde_json::json!(["hub"]), "{rail}");

    let hub = common::get_json(port, "/api/agents/hub").await;
    assert_eq!(hub["isRelayer"], true, "{hub}");
    let caas = common::get_json(port, "/api/agents/caas").await;
    assert_eq!(caas["isRelayer"], false, "{caas}");
}

#[tokio::test]
async fn a_bus_with_no_relayers_reports_an_empty_set() {
    // An empty list, not a missing field: the console states "(none)" rather than
    // staying silent, and it can only do that if absence is explicit.
    let dir = tempfile::tempdir().unwrap();
    seed_agents(dir.path(), &["caas"]).await;
    let port = start(dir.path()).await;

    let rail = common::get_json(port, "/api/rail").await;
    assert_eq!(rail["relayers"], serde_json::json!([]), "{rail}");
    assert!(relayer_names(&rail["agents"]).is_empty(), "{rail}");
}

#[tokio::test]
async fn a_relayer_configured_under_a_name_no_agent_uses_is_still_listed() {
    // The failure this exists for. A mistyped `--relayer hubb` marks nothing, so
    // with the per-agent flag alone the bus would look identical to a correctly
    // configured one whose relayer is not connected. The set is what tells them apart.
    let dir = tempfile::tempdir().unwrap();
    seed_agents(dir.path(), &["hub"]).await;
    let port = start_with_relayers(dir.path(), &["hubb"]).await;

    let rail = common::get_json(port, "/api/rail").await;
    assert_eq!(rail["relayers"], serde_json::json!(["hubb"]), "{rail}");
    assert!(
        relayer_names(&rail["agents"]).is_empty(),
        "and nothing is marked, which is the tell: {rail}"
    );
}

#[tokio::test]
async fn the_agents_api_returns_an_empty_array_for_a_bus_with_no_agents() {
    // An empty fleet must be `[]`, not `null` and not an error — the frontend
    // maps over the result unconditionally.
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;

    let res = get(port, "/api/agents").await;
    assert!(res.starts_with("HTTP/1.1 200"), "got: {res}");
    let body = res.rsplit("\r\n\r\n").next().unwrap();
    assert_eq!(body.trim(), "[]", "got: {res}");
}

#[tokio::test]
async fn the_rail_summarises_rooms_and_agents() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "hardac", "/w/caas", None, false, Some("0.3.3"))
            .await
            .unwrap();
        store.join_room("protocol", "caas").await.unwrap();
        // The sender must not be a room member, or `unread_count`'s `from_agent
        // != ?3` filter means `caas` never counts its own message as unread and
        // no flag is derived at all. See src/store/mod.rs:497.
        store
            .append_message("protocol", "bbaldino", "hello", false, true)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/api/rail").await;

    assert!(body.contains("\"rooms\""), "got: {body}");
    assert!(body.contains("\"agents\""), "got: {body}");
    assert!(
        body.contains("\"protocol\""),
        "the room must appear: {body}"
    );
    assert!(body.contains("\"caas\""), "the agent must appear: {body}");
    // 12 five-minute buckets, oldest first, always full length.
    assert!(
        body.contains("\"buckets\":["),
        "buckets must be present: {body}"
    );
    // The agent is offline (mark_all_offline runs at startup), and the room has
    // an unread message for it, so the room is blocked.
    assert!(body.contains("\"blocked\""), "flag must be derived: {body}");
}

#[tokio::test]
async fn meta_reports_the_host_and_version() {
    use claude_bus::config::EnvSource;
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;

    let body = get(port, "/api/meta").await;

    assert!(body.contains("\"version\""), "got: {body}");
    assert!(
        body.contains(env!("CARGO_PKG_VERSION")),
        "must report the running version: {body}"
    );
    // The pill is "hardac · 0.3.3" — both halves, or 2b cannot render it. The
    // test was named for the host long before it asserted one.
    assert!(body.contains("\"host\""), "got: {body}");
    assert!(
        body.contains(&claude_bus::config::RealEnv.hostname()),
        "must report the host the bus is running on: {body}"
    );
}

/// Drive a real bus to the exchange cap, then clear the pause the way the
/// operator actually does — by typing in their own session — and check the rail
/// agrees. Deliberately end to end rather than seeding events: the whole defect
/// was that the two paths which actually clear a pause wrote no event at all, so
/// every store-level test that seeded `resumed` by hand passed while the shipped
/// flag never cleared.
#[tokio::test]
async fn a_humans_message_clears_needs_you_on_the_rail() {
    use claude_bus::proto::{Target, ToBus};

    let dir = tempfile::tempdir().unwrap();
    let port = start_with_cap(dir.path(), 1).await;

    // An agent talks until the room pauses.
    let mut caas = common::connect(port, "caas").await;
    common::next_event(&mut caas).await; // Registered
    for _ in 0..2 {
        common::send(
            &mut caas,
            &ToBus::Send {
                req_id: 1,
                target: Target::Room {
                    room: "protocol".into(),
                },
                text: "still going".into(),
                done: false,
            },
        )
        .await;
        common::next_event(&mut caas).await;
    }

    let body = get(port, "/api/rail").await;
    assert!(
        body.contains("needsYou"),
        "the cap tripped, so the rail must ask for a person: {body}"
    );

    // The human types in their session. No `resume` call — this is the ordinary
    // way a paused room continues.
    let mut human = common::connect_human(port, "bbaldino").await;
    common::next_event(&mut human).await; // Registered
    common::send(
        &mut human,
        &ToBus::Send {
            req_id: 1,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "carry on".into(),
            done: false,
        },
    )
    .await;
    common::next_event(&mut human).await; // Sent

    let body = get(port, "/api/rail").await;
    assert!(
        !body.contains("needsYou"),
        "the room resumed the moment the human spoke; the rail must not still \
         be asking for one: {body}"
    );
}

/// The other clearing path, and the usual one: `contrib/human-active-hook.sh`
/// POSTs here on every prompt the operator submits, and DEPLOY.md documents it
/// as the standard deployment.
#[tokio::test]
async fn the_human_active_hook_clears_needs_you_on_the_rail() {
    use claude_bus::proto::{Target, ToBus};

    let dir = tempfile::tempdir().unwrap();
    let port = start_with_cap(dir.path(), 1).await;

    let mut caas = common::connect(port, "caas").await;
    common::next_event(&mut caas).await; // Registered
    for _ in 0..2 {
        common::send(
            &mut caas,
            &ToBus::Send {
                req_id: 1,
                target: Target::Room {
                    room: "protocol".into(),
                },
                text: "still going".into(),
                done: false,
            },
        )
        .await;
        common::next_event(&mut caas).await;
    }

    let body = get(port, "/api/rail").await;
    assert!(
        body.contains("needsYou"),
        "the cap must have tripped: {body}"
    );

    let res = post(port, "/human-active?agent=caas").await;
    assert!(res.starts_with("HTTP/1.1 200"), "got: {res}");

    let body = get(port, "/api/rail").await;
    assert!(
        !body.contains("needsYou"),
        "the hook un-paused the room, so the flag must be gone: {body}"
    );
}

/// SQLite reads a negative `LIMIT` as *unlimited*, so `?limit=-1` on either list
/// endpoint used to dump the whole table — from an unauthenticated endpoint, on a
/// bus that binds 0.0.0.0. Clamping to at least 1 is what makes the parameter mean
/// what it says.
#[tokio::test]
async fn a_negative_limit_does_not_dump_the_whole_table() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        for i in 0..5 {
            store
                .append_message("protocol", "caas", &format!("m{i}"), false, false)
                .await
                .unwrap();
            store
                .append_event("ack", Some("caas"), Some("protocol"), serde_json::json!({}))
                .await
                .unwrap();
        }
    }
    let port = start(dir.path()).await;

    let body = get(port, "/api/rooms/protocol/messages?limit=-1").await;
    assert_eq!(
        body.matches("\"body\"").count(),
        1,
        "a negative limit must clamp to one row, not every row: {body}"
    );

    let body = get(port, "/api/events?limit=-1").await;
    assert_eq!(
        body.matches("\"kind\"").count(),
        1,
        "a negative limit must clamp to one row, not every row: {body}"
    );
}

/// The third way a pause disappears: the bus restarts. `Guards` keeps its
/// counters in memory, so a fresh process has no paused rooms while every
/// `room_paused` row it inherits is still the latest word in the log. Left
/// alone, every room that was ever paused comes back flagged and nothing can
/// clear it, because there is no pause left for a human to lift.
#[tokio::test]
async fn a_restart_clears_needs_you_for_rooms_whose_pause_it_did_not_inherit() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room("protocol", "caas").await.unwrap();
        store
            .append_event(
                "room_paused",
                Some("caas"),
                Some("protocol"),
                serde_json::json!({ "count": 20 }),
            )
            .await
            .unwrap();
        assert_eq!(
            store.paused_rooms().await.unwrap(),
            vec!["protocol".to_string()],
            "the fixture must actually leave a pause behind"
        );
    }

    let port = start(dir.path()).await;

    let body = get(port, "/api/rail").await;
    assert!(
        !body.contains("needsYou"),
        "this process has no such pause, so the rail must not report one: {body}"
    );
}

/// The hook fires on every prompt the operator types, almost always for rooms
/// that were never paused. Those must not each leave a `resumed` in the log
/// claiming a pause was lifted — the audit trail is the thing being fixed here,
/// and filling it with false resumes would be its own defect.
#[tokio::test]
async fn the_human_active_hook_writes_nothing_for_a_room_that_was_not_paused() {
    let dir = tempfile::tempdir().unwrap();
    let port = start_with_cap(dir.path(), 20).await;
    let path = dir.path().to_path_buf();

    let mut caas = common::connect(port, "caas").await;
    common::next_event(&mut caas).await; // Registered
    common::send(
        &mut caas,
        &claude_bus::proto::ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    common::next_event(&mut caas).await; // Joined

    let res = post(port, "/human-active?agent=caas").await;
    assert!(res.starts_with("HTTP/1.1 200"), "got: {res}");

    let store = Store::open(&path).await.unwrap();
    let resumed = store.events_of_kind("resumed", 10).await.unwrap();
    assert!(
        resumed.is_empty(),
        "nothing was paused, so nothing was resumed: {resumed:?}"
    );
}

/// The routing table above `resolve`, which nothing else exercises: `resolve` is
/// unit-tested in `src/web/assets.rs`, but a path that never reaches it fails
/// invisibly. `/` in particular matches no `/{*rest}` catch-all (matchit requires
/// a non-empty remainder) and so needs its own route.
///
/// Asserts shape rather than content: CI's Rust job has only `.gitkeep` in
/// `ui/dist`, so there is no built bundle here. What must hold either way is
/// that every form behaves identically and that none of them is a bare
/// route-miss.
#[tokio::test]
async fn the_app_routes_all_reach_the_bundle_handler() {
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;

    let mut statuses = Vec::new();
    for path in ["/", "/agents/caas", "/rooms/protocol"] {
        let res = get(port, path).await;
        // A bare 404 with no body is axum's router fallback — the request never
        // reached a handler. The handler's own miss says why.
        assert_ne!(
            status_of(&res),
            "404",
            "{path} must reach the bundle handler, not fall off the router: {res}"
        );
        statuses.push((path, status_of(&res)));
    }

    let first = statuses[0].1.clone();
    for (path, status) in &statuses {
        assert_eq!(
            *status, first,
            "{path} must behave like / — they are all the same app: {statuses:?}"
        );
    }

    // With no bundle built (the state of a fresh clone and of CI's Rust job) the
    // handler explains itself rather than 404ing. With one built it serves the
    // shell. Either is fine; silence is not.
    let res = get(port, "/").await;
    assert!(
        res.contains("was not built") || res.contains("<!doctype") || res.contains("<!DOCTYPE"),
        "/ must serve the shell or say why it cannot: {res}"
    );
}

/// The console used to be mounted at `/app`. Bookmarks and proxy configs from
/// then must land on the same screen at the root, not on a 404.
#[tokio::test]
async fn the_old_app_mount_redirects_to_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;

    for (from, to) in [
        ("/app", "/"),
        ("/app/", "/"),
        ("/app/rooms/protocol", "/rooms/protocol"),
    ] {
        let res = get(port, from).await;
        assert_eq!(status_of(&res), "308", "{from} must redirect: {res}");
        assert!(
            res.to_ascii_lowercase()
                .contains(&format!("location: {to}\r\n")),
            "{from} must redirect to {to}: {res}"
        );
    }
}

/// An unknown `/api` path must stay a 404, not be answered with the app shell
/// just because the app claims every otherwise-unrouted path.
#[tokio::test]
async fn an_unknown_api_path_is_a_404_not_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;

    let res = get(port, "/api/no-such-thing").await;
    assert_eq!(status_of(&res), "404", "{res}");
}

fn status_of(res: &str) -> String {
    res.lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn the_transcript_returns_a_rooms_messages() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .append_message("protocol", "caas", "first", false, false)
            .await
            .unwrap();
        store
            .append_message("protocol", "bbaldino", "second", true, true)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/api/rooms/protocol/messages?limit=10").await;

    assert!(body.contains("\"first\""), "got: {body}");
    assert!(body.contains("\"second\""), "got: {body}");
    assert!(
        body.contains("\"human\":true"),
        "human authority must survive: {body}"
    );
    assert!(
        body.contains("\"done\":true"),
        "the done marker must survive: {body}"
    );
}

#[tokio::test]
async fn the_transcript_pages_backward_with_before() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        for i in 0..5 {
            store
                .append_message("protocol", "caas", &format!("msg{i}"), false, false)
                .await
                .unwrap();
        }
    }
    let port = start(dir.path()).await;

    let page1_res = get(port, "/api/rooms/protocol/messages?limit=2").await;
    let page1_body = page1_res.rsplit("\r\n\r\n").next().unwrap();
    let page1: serde_json::Value = serde_json::from_str(page1_body).unwrap();
    let page1 = page1.as_array().unwrap();
    assert_eq!(page1.len(), 2, "got: {page1_res}");
    assert_eq!(page1[0]["body"], "msg3", "got: {page1_res}");
    assert_eq!(page1[1]["body"], "msg4", "got: {page1_res}");
    let oldest_on_page1 = page1[0]["id"].as_i64().unwrap();

    let page2_res = get(
        port,
        &format!("/api/rooms/protocol/messages?limit=2&before={oldest_on_page1}"),
    )
    .await;
    let page2_body = page2_res.rsplit("\r\n\r\n").next().unwrap();
    let page2: serde_json::Value = serde_json::from_str(page2_body).unwrap();
    let page2 = page2.as_array().unwrap();
    assert_eq!(page2.len(), 2, "got: {page2_res}");
    assert_eq!(page2[0]["body"], "msg1", "oldest first: {page2_res}");
    assert_eq!(page2[1]["body"], "msg2", "got: {page2_res}");

    // No id repeats across the two pages, and no id is skipped between them.
    let page1_ids: Vec<i64> = page1.iter().map(|m| m["id"].as_i64().unwrap()).collect();
    let page2_ids: Vec<i64> = page2.iter().map(|m| m["id"].as_i64().unwrap()).collect();
    for id in &page2_ids {
        assert!(
            !page1_ids.contains(id),
            "page 2 must not repeat a page 1 id: {page1_ids:?} vs {page2_ids:?}"
        );
    }
    assert_eq!(
        page2_ids[1] + 1,
        page1_ids[0],
        "no gap between the two pages: {page2_ids:?} then {page1_ids:?}"
    );
}

#[tokio::test]
async fn the_events_endpoint_scopes_to_a_room_or_the_whole_bus() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .append_event(
                "room_joined",
                Some("caas"),
                Some("protocol"),
                serde_json::json!({}),
            )
            .await
            .unwrap();
        store
            .append_event(
                "room_joined",
                Some("dash"),
                Some("other"),
                serde_json::json!({}),
            )
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let scoped = get(port, "/api/events?room=protocol&limit=50").await;
    assert!(scoped.contains("\"protocol\""), "got: {scoped}");
    assert!(
        !scoped.contains("\"other\""),
        "must not leak other rooms: {scoped}"
    );

    let whole = get(port, "/api/events?limit=50").await;
    assert!(whole.contains("\"protocol\""), "got: {whole}");
    assert!(
        whole.contains("\"other\""),
        "whole-bus scope sees everything: {whole}"
    );
}

#[tokio::test]
async fn the_events_endpoint_filters_by_kind() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .append_event(
                "room_joined",
                Some("caas"),
                Some("protocol"),
                serde_json::json!({}),
            )
            .await
            .unwrap();
        store
            .append_event("ack", Some("caas"), Some("protocol"), serde_json::json!({}))
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/api/events?kind=ack&limit=50").await;

    assert!(body.contains("\"ack\""), "got: {body}");
    assert!(
        !body.contains("room_joined"),
        "kind must narrow the fetch: {body}"
    );
}

#[tokio::test]
async fn the_events_endpoint_combines_room_and_kind_with_and_semantics() {
    // The combination the other three tests don't reach: room-only and kind-only each
    // pass even if the SQL's AND is secretly an OR (the other clause is NULL and always
    // true), or if one filter is silently ignored. Only asserting the row that matches
    // both would still pass against either bug — the two negatives below are the point.
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        // Matches both filters.
        store
            .append_event(
                "ack",
                Some("caas"),
                Some("protocol"),
                serde_json::json!({"tag": "match"}),
            )
            .await
            .unwrap();
        // Right room, wrong kind.
        store
            .append_event(
                "room_paused",
                Some("caas"),
                Some("protocol"),
                serde_json::json!({"tag": "same-room-different-kind"}),
            )
            .await
            .unwrap();
        // Right kind, wrong room.
        store
            .append_event(
                "ack",
                Some("caas"),
                Some("other"),
                serde_json::json!({"tag": "same-kind-different-room"}),
            )
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/api/events?room=protocol&kind=ack&limit=50").await;

    assert!(body.contains("\"match\""), "got: {body}");
    assert!(
        !body.contains("same-room-different-kind"),
        "the room match alone must not be enough: {body}"
    );
    assert!(
        !body.contains("same-kind-different-room"),
        "the kind match alone must not be enough: {body}"
    );
}

#[tokio::test]
async fn agent_detail_reports_rooms_with_counts_and_a_true_event_total() {
    use claude_bus::proto::{Target, ToBus};

    let (_d, port) = common::start_bus().await;
    let mut caas = common::connect(port, "caas").await;
    common::next_event(&mut caas).await; // Registered
    common::send(
        &mut caas,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    common::next_event(&mut caas).await;
    for i in 0..3 {
        common::send(
            &mut caas,
            &ToBus::Send {
                req_id: 10 + i,
                target: Target::Room {
                    room: "protocol".into(),
                },
                text: format!("m{i}"),
                done: false,
            },
        )
        .await;
        common::next_event(&mut caas).await;
    }

    let body = common::get_json(port, "/api/agents/caas").await;
    assert_eq!(body["name"], "caas");
    assert_eq!(body["rooms"][0]["name"], "protocol");
    assert_eq!(body["rooms"][0]["messageCount"], 3);
    // The list is capped but the total is not — the header states the truth.
    assert!(body["eventTotal"].as_i64().unwrap() >= 1);
    assert_eq!(body["buckets"].as_array().unwrap().len(), 20);
}

#[tokio::test]
async fn agent_detail_for_a_tombstone_has_no_rooms_and_still_renders() {
    // An agent that registered and never joined anything: the shape the screen's
    // "never joined a room" state depends on.
    let (_d, port) = common::start_bus().await;
    let mut ghost = common::connect(port, "ghost").await;
    common::next_event(&mut ghost).await;
    drop(ghost);

    let body = common::get_json(port, "/api/agents/ghost").await;
    assert_eq!(body["name"], "ghost");
    assert!(body["rooms"].as_array().unwrap().is_empty());
    assert_eq!(body["buckets"].as_array().unwrap().len(), 20);
}

#[tokio::test]
async fn agent_detail_for_an_unknown_name_is_404() {
    let (_d, port) = common::start_bus().await;
    let status = common::get_status(port, "/api/agents/nobody").await;
    assert_eq!(status, 404);
}

/// `agent_detail_reports_rooms_with_counts_and_a_true_event_total` sends the agent
/// well under fifty events, so there `events.len()` and `COUNT(*)` are numerically
/// identical — that test cannot tell a real total from `.len()` in disguise. This
/// one pushes past `AGENT_EVENT_LIMIT` so the two diverge, which is the only way to
/// prove the total isn't the slice's length.
#[tokio::test]
async fn agent_detail_reports_the_true_total_beyond_the_event_cap() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "hardac", "/w/caas", None, false, None)
            .await
            .unwrap();
        for _ in 0..60 {
            store
                .append_event("ack", Some("caas"), None, serde_json::json!({}))
                .await
                .unwrap();
        }
    }
    let port = start(dir.path()).await;

    let body = common::get_json(port, "/api/agents/caas").await;
    assert_eq!(
        body["events"].as_array().unwrap().len(),
        50,
        "the list must stay capped at fifty: {body}"
    );
    assert!(
        body["eventTotal"].as_i64().unwrap() > 50,
        "the total must reflect all sixty rows, not just the capped slice: {body}"
    );
}

/// The SQL is driven by `room_members`, with messages only `LEFT JOIN`ed in — a room
/// joined but never spoken in must still appear, with a zero count, rather than being
/// dropped by an inner join.
#[tokio::test]
async fn agent_detail_reports_a_joined_room_with_zero_messages() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "hardac", "/w/caas", None, false, None)
            .await
            .unwrap();
        store.join_room("protocol", "caas").await.unwrap();
    }
    let port = start(dir.path()).await;

    let body = common::get_json(port, "/api/agents/caas").await;
    assert_eq!(body["rooms"][0]["name"], "protocol");
    assert_eq!(
        body["rooms"][0]["messageCount"], 0,
        "membership alone must produce a row, with a zero count: {body}"
    );
}

#[tokio::test]
async fn deletion_preview_counts_real_rows() {
    use claude_bus::proto::ToBus;

    let (_d, port) = common::start_bus().await;
    let mut caas = common::connect(port, "caas").await;
    common::next_event(&mut caas).await;
    common::send(
        &mut caas,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    common::next_event(&mut caas).await;
    drop(caas);
    common::wait_until(|| async { !common::agent_is_online(port, "caas").await }).await;

    let body = common::get_json(port, "/api/agents/caas/deletion").await;
    assert_eq!(body["registration"], 1);
    assert_eq!(body["memberships"], 1);
    assert_eq!(body["online"], false);
    assert!(body["host"].as_str().is_some());
}

#[tokio::test]
async fn deleting_an_offline_agent_removes_it() {
    let (_d, port) = common::start_bus().await;
    let mut caas = common::connect(port, "caas").await;
    common::next_event(&mut caas).await;
    drop(caas);
    common::wait_until(|| async { !common::agent_is_online(port, "caas").await }).await;

    let status = common::delete_same_origin(port, "/api/agents/caas").await;
    assert_eq!(status, 204);
    assert_eq!(common::get_status(port, "/api/agents/caas").await, 404);
}

#[tokio::test]
async fn deleting_with_no_origin_header_succeeds() {
    // A request with no Origin at all — curl, a script — is allowed, mirroring
    // the HTML path's `POST`: it could already reach the port directly, so
    // refusing it buys nothing. Ships untested in neither direction until
    // now, which is exactly what let the stricter, brief-supplied "Origin and
    // Host both required" check land unnoticed.
    let (_d, port) = common::start_bus().await;
    let mut caas = common::connect(port, "caas").await;
    common::next_event(&mut caas).await;
    drop(caas);
    common::wait_until(|| async { !common::agent_is_online(port, "caas").await }).await;

    let status = common::delete_no_origin(port, "/api/agents/caas").await;
    assert_eq!(status, 204);
    assert_eq!(common::get_status(port, "/api/agents/caas").await, 404);
}

#[tokio::test]
async fn deleting_an_online_agent_is_refused_and_changes_nothing() {
    // The race this endpoint exists to lose safely: the client believed the
    // agent was offline, and it is not.
    let (_d, port) = common::start_bus().await;
    let mut caas = common::connect(port, "caas").await;
    common::next_event(&mut caas).await; // still connected

    let status = common::delete_same_origin(port, "/api/agents/caas").await;
    assert_eq!(status, 409);
    assert_eq!(common::get_status(port, "/api/agents/caas").await, 200);
}

#[tokio::test]
async fn a_cross_origin_delete_is_refused() {
    // The bus binds 0.0.0.0 with no authentication, so anything that can reach
    // the port can send this.
    let (_d, port) = common::start_bus().await;
    let mut caas = common::connect(port, "caas").await;
    common::next_event(&mut caas).await;
    drop(caas);
    common::wait_until(|| async { !common::agent_is_online(port, "caas").await }).await;

    let status = common::delete_with_origin(port, "/api/agents/caas", "http://evil.example").await;
    assert_eq!(status, 403);
    assert_eq!(common::get_status(port, "/api/agents/caas").await, 200);
}

/// The HTML delete has two tests for this (`posting_the_delete_records_an_audit_event`
/// and `posting_the_delete_for_an_unknown_agent_writes_no_audit_event`); the JSON
/// delete had neither, even though its unknown-name guard is the same
/// safety-critical line — what stops any name reaching this URL from forging the
/// one record this feature promises will outlive the agent.
#[tokio::test]
async fn deleting_an_offline_agent_via_json_records_an_audit_event() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("network-debug#2", "hardac", "/w/nd", None, false, None)
            .await
            .unwrap();
        // Deliberately non-empty, for the same reason `posting_the_delete_records_an_audit_event`
        // is: with no membership every count renders as 0 whether the event carries
        // it or not, so a test with an empty footprint cannot tell a real count from
        // a dropped one.
        store
            .join_room("protocol", "network-debug#2")
            .await
            .unwrap();
        store
            .set_cursor("protocol", "network-debug#2", 4)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let status = common::delete_same_origin(port, "/api/agents/network-debug%232").await;
    assert_eq!(status, 204);

    let events = common::get_json(port, "/api/events").await;
    let deleted = events
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "agent_deleted")
        .unwrap_or_else(|| panic!("no agent_deleted event: {events}"));
    assert_eq!(deleted["agent"], "network-debug#2");
    assert_eq!(deleted["detail"]["name"], "network-debug#2");
    assert_eq!(deleted["detail"]["host"], "hardac");
    assert_eq!(deleted["detail"]["memberships"], 1);
    assert_eq!(deleted["detail"]["cursors"], 1);
}

/// Without this guard, any name at all would pass the JSON delete's liveness
/// check trivially (an unknown agent is, trivially, offline), delete nothing,
/// and still write an `agent_deleted` event — forging the one record this
/// feature promises will outlive the agent, and letting anyone who can reach
/// the port grow `events` unboundedly through the URL path alone.
#[tokio::test]
async fn deleting_an_unknown_agent_via_json_writes_no_audit_event() {
    let (_d, port) = common::start_bus().await;

    let status = common::delete_same_origin(port, "/api/agents/nobody").await;
    assert_eq!(status, 404);

    let events = common::get_json(port, "/api/events").await;
    assert!(
        !events
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "agent_deleted"),
        "an unknown name must forge no audit event: {events}"
    );
}

/// `AgentIdentity` (the TS side) has to tell "cannot compare against this bus"
/// apart from "this agent's own version is unknown" — the second is a real
/// signal (a binary predating the version field) that must survive the column
/// -> DTO hop faithfully. This is the only genuinely unverified link in that
/// path.
#[tokio::test]
async fn agent_detail_returns_a_known_version_faithfully() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "hardac", "/w/caas", None, false, Some("0.0.1"))
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = common::get_json(port, "/api/agents/caas").await;
    assert_eq!(body["version"], "0.0.1");
}

#[tokio::test]
async fn agent_detail_returns_a_null_version_faithfully() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "hardac", "/w/caas", None, false, None)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = common::get_json(port, "/api/agents/caas").await;
    assert_eq!(body["version"], serde_json::Value::Null);
}

#[tokio::test]
async fn room_files_lists_what_was_stored() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .put_file(
                "protocol",
                "digest-report.json",
                b"{}",
                Some("application/json"),
                "caas",
            )
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = common::get_json(port, "/api/rooms/protocol/files").await;
    let files = body.as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["key"], "digest-report.json");
    assert_eq!(files[0]["size"], 2);
    assert_eq!(files[0]["contentType"], "application/json");
    assert_eq!(files[0]["updatedBy"], "caas");
    assert!(files[0]["updatedAt"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn a_room_with_no_files_returns_an_empty_list_not_an_error() {
    // "No files" and "could not find out" must be different answers, and the
    // client renders them differently — so the empty case must be a 200.
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;

    assert_eq!(
        common::get_status(port, "/api/rooms/anything/files").await,
        200
    );
    let body = common::get_json(port, "/api/rooms/anything/files").await;
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn room_files_survives_a_key_with_awkward_characters() {
    // Keys are agent-supplied; a slash or a space in one must round-trip.
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .put_file("protocol", "reports/q3 summary.txt", b"hi", None, "caas")
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = common::get_json(port, "/api/rooms/protocol/files").await;
    assert_eq!(body[0]["key"], "reports/q3 summary.txt");
    assert!(body[0]["contentType"].is_null());
}

#[tokio::test]
async fn hiding_a_room_is_reflected_in_the_rail() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.ensure_room("protocol").await.unwrap();
    }
    let port = start(dir.path()).await;

    let res = common::post_json(
        port,
        "/api/rooms/protocol/hidden",
        serde_json::json!({ "hidden": true }),
    )
    .await;
    assert_eq!(res, 204);

    let rail = common::get_json(port, "/api/rail").await;
    let room = rail["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "protocol")
        .unwrap();
    assert_eq!(
        room["hidden"], true,
        "the rail must ship the flag, not filter the room out"
    );
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
    let port = start(dir.path()).await;
    let rail = common::get_json(port, "/api/rail").await;
    assert_eq!(rail["rooms"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn hiding_an_unknown_room_is_a_404_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;
    let res = common::post_json(
        port,
        "/api/rooms/nope/hidden",
        serde_json::json!({ "hidden": true }),
    )
    .await;
    assert_eq!(res, 404);
    let rail = common::get_json(port, "/api/rail").await;
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
    let port = start(dir.path()).await;
    let res = common::post_json_with_origin(
        port,
        "/api/rooms/protocol/hidden",
        serde_json::json!({ "hidden": true }),
        "https://evil.example",
    )
    .await;
    assert_eq!(res, 403);
}

#[tokio::test]
async fn hiding_a_room_writes_exactly_one_event_naming_the_room_and_no_agent() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.ensure_room("protocol").await.unwrap();
    }
    let port = start(dir.path()).await;

    let res = common::post_json(
        port,
        "/api/rooms/protocol/hidden",
        serde_json::json!({ "hidden": true }),
    )
    .await;
    assert_eq!(res, 204);

    let events = common::get_json(port, "/api/events?room=protocol&kind=room_hidden").await;
    let hidden = events.as_array().unwrap();
    assert_eq!(
        hidden.len(),
        1,
        "exactly one room_hidden event, got: {events}"
    );
    assert_eq!(hidden[0]["room"], "protocol");
    assert!(
        hidden[0]["agent"].is_null(),
        "a manual hide is not attributed to any agent: {events}"
    );
}

/// A double-click (or a retried POST) asking for a state the room is already
/// in must not forge a second event — the same reasoning `agent_deleted`'s
/// unknown-name guard applies to a delete that removed no row. Without the
/// value guard on the store's `UPDATE`, `rows_affected` reads 1 whether or
/// not the flag actually changed, so the handler cannot tell a real
/// transition from a no-op and writes a `room_hidden` for both.
#[tokio::test]
async fn hiding_an_already_hidden_room_writes_no_second_event() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.ensure_room("protocol").await.unwrap();
    }
    let port = start(dir.path()).await;

    let first = common::post_json(
        port,
        "/api/rooms/protocol/hidden",
        serde_json::json!({ "hidden": true }),
    )
    .await;
    assert_eq!(first, 204);
    let second = common::post_json(
        port,
        "/api/rooms/protocol/hidden",
        serde_json::json!({ "hidden": true }),
    )
    .await;
    assert_eq!(
        second, 204,
        "the caller asked for a state and got it, even though nothing changed"
    );

    let events = common::get_json(port, "/api/events?room=protocol&kind=room_hidden").await;
    assert_eq!(
        events.as_array().unwrap().len(),
        1,
        "a repeated hide must not forge a second event: {events}"
    );
}

/// Symmetric with the hide case: unhiding a room that was never hidden is a
/// no-op state-wise, and must not forge a `room_unhidden` claiming a
/// transition that never happened.
#[tokio::test]
async fn unhiding_a_never_hidden_room_writes_no_event() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.ensure_room("protocol").await.unwrap();
    }
    let port = start(dir.path()).await;

    let res = common::post_json(
        port,
        "/api/rooms/protocol/hidden",
        serde_json::json!({ "hidden": false }),
    )
    .await;
    assert_eq!(res, 204);

    let events = common::get_json(port, "/api/events?room=protocol&kind=room_unhidden").await;
    assert!(
        events.as_array().unwrap().is_empty(),
        "unhiding a room that was never hidden must forge no event: {events}"
    );
}
