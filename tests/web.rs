// Drives the real server over HTTP against a temp SQLite. Asserts rendered content,
// not status codes: a page that returns 200 with an empty table has failed at its
// only job.
mod common;

use claude_bus::proto::FromBus;
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
async fn overview_lists_agents_and_rooms() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "lisa", "/w/caas", None, false, None)
            .await
            .unwrap();
        store.join_room("protocol", "caas").await.unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/").await;
    assert!(body.contains("caas"), "agent must appear: {body}");
    assert!(body.contains("protocol"), "room must appear");
}

#[tokio::test]
async fn a_script_tag_in_a_room_name_is_escaped() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .join_room("<script>alert(1)</script>", "caas")
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/").await;
    assert!(
        !body.contains("<script>alert(1)</script>"),
        "raw tag must not survive"
    );
    assert!(body.contains("&lt;script&gt;"), "must be escaped instead");
}

#[tokio::test]
async fn a_transcript_interleaves_messages_and_events_in_time_order() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room("protocol", "caas").await.unwrap();
        store
            .append_message("protocol", "caas", "FIRST_MESSAGE", false, false)
            .await
            .unwrap();
        // now_ms() is millisecond-resolution. Three inserts back-to-back against a
        // local SQLite temp file land well inside a single millisecond, so without a
        // gap here the three rows tie on `created_at` and the assertions below would
        // pass on the sort's tie-break (see `Entry::rank` in src/web/mod.rs) instead of
        // proving real chronological interleaving. A short sleep makes the timestamps
        // distinct so this test exercises what it claims to.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        store
            .append_event(
                "room_paused",
                Some("caas"),
                Some("protocol"),
                serde_json::json!({"count": 20}),
            )
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        store
            .append_message("protocol", "caas", "LAST_MESSAGE", false, false)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/rooms/protocol").await;
    let first = body.find("FIRST_MESSAGE").expect("first message rendered");
    let pause = body
        .find("room_paused")
        .expect("the pause event is shown inline");
    let last = body.find("LAST_MESSAGE").expect("last message rendered");

    assert!(
        first < pause,
        "the pause must appear after the first message"
    );
    assert!(
        pause < last,
        "and before the last — chronological, not grouped by type"
    );
}

#[tokio::test]
async fn a_script_tag_in_a_message_body_is_escaped() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room("protocol", "caas").await.unwrap();
        store
            .append_message(
                "protocol",
                "caas",
                "<script>alert('xss')</script>",
                false,
                false,
            )
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/rooms/protocol").await;
    assert!(
        !body.contains("<script>alert"),
        "an agent must not be able to inject script"
    );
    assert!(body.contains("&lt;script&gt;"));
}

#[tokio::test]
async fn room_links_on_the_overview_page_are_percent_encoded_in_the_href() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room("weird room?x=1&y=2", "caas").await.unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/").await;
    // The href must be percent-encoded so a browser treats the whole name as one path
    // segment, not `weird` followed by a `?x=1&y=2` query string.
    assert!(
        body.contains("href=\"/rooms/weird%20room%3Fx%3D1%26y%3D2\""),
        "href must be percent-encoded: {body}"
    );
    // The visible link text is only HTML-escaped, not percent-encoded, so it stays
    // human-readable.
    assert!(
        body.contains("weird room?x=1&amp;y=2"),
        "anchor text should read naturally: {body}"
    );
}

#[tokio::test]
async fn a_room_name_with_spaces_and_url_metacharacters_round_trips_to_its_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let name = "weird room?x=1&y=2#z";
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room(name, "caas").await.unwrap();
        store
            .append_message(name, "caas", "hello from a weird room", false, false)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/rooms/weird%20room%3Fx%3D1%26y%3D2%23z").await;
    assert!(
        body.contains("hello from a weird room"),
        "expected the room's transcript: {body}"
    );
    // The page title/h1 is the decoded name, HTML-escaped.
    assert!(
        body.contains("weird room?x=1&amp;y=2#z"),
        "expected the decoded name in the page: {body}"
    );
}

#[tokio::test]
async fn a_room_name_with_non_ascii_characters_round_trips_to_its_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let name = "café-日本語";
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room(name, "caas").await.unwrap();
        store
            .append_message(name, "caas", "hello from a non-ascii room", false, false)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    // percent-encode the name the same way the production helper does, byte-by-byte.
    let encoded: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();

    let body = get(port, &format!("/rooms/{encoded}")).await;
    assert!(
        body.contains("hello from a non-ascii room"),
        "expected the room's transcript: {body}"
    );
    assert!(
        body.contains(name),
        "expected the decoded non-ascii name in the page: {body}"
    );
}

#[tokio::test]
async fn a_room_name_containing_a_percent_encoded_slash_round_trips() {
    // Whether `%2F` in a path segment reaches the handler as a literal `/` (rather than
    // being treated as introducing a new path segment, or rejected outright) is a
    // property of the router, not of this crate's encoder — routers commonly
    // special-case encoded slashes. Verified empirically rather than assumed: axum
    // 0.8.9 matches `/rooms/{name}` against the still-*encoded* request path (so `%2F`
    // does not introduce an extra segment boundary the way a literal `/` would), and
    // only percent-decodes the captured segment afterwards, when building `Path<String>`.
    // So a name containing a literal `/`, once percent-encoded by `encode_path_segment`,
    // does round-trip through this route.
    let dir = tempfile::tempdir().unwrap();
    let name = "a/b";
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room(name, "caas").await.unwrap();
        store
            .append_message(name, "caas", "hello from a slashy room", false, false)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/rooms/a%2Fb").await;
    assert!(
        body.contains("hello from a slashy room"),
        "expected the slashy room's transcript: {body}"
    );
    // Anchored to the `<h1>` rather than searching the whole page, so this still proves
    // the *decoded* name reached the handler: had decoding failed the heading would read
    // `<h1>a%2Fb`. Deliberately not matched against the closing tag — the heading also
    // carries a sort-direction note — but the check is no weaker for it.
    assert!(
        body.contains("<h1>a/b"),
        "expected the decoded name: {body}"
    );
}

#[tokio::test]
async fn an_agent_page_shows_its_registration_history() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "lisa", "/w/caas", None, false, None)
            .await
            .unwrap();
        store
            .append_event(
                "agent_registered",
                Some("caas"),
                None,
                serde_json::json!({"requested_name":"caas","effective_name":"caas","host":"lisa"}),
            )
            .await
            .unwrap();
        store
            .append_event(
                "agent_disconnected",
                Some("caas"),
                None,
                serde_json::json!({"reason":"keepalive_timeout"}),
            )
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/agents/caas").await;
    assert!(body.contains("agent_registered"));
    assert!(
        body.contains("keepalive_timeout"),
        "the disconnect reason is the diagnostic"
    );
}

#[tokio::test]
async fn the_agents_list_links_to_the_agent_page_with_a_percent_encoded_href() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("weird agent?x=1", "lisa", "/w", None, false, None)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/agents").await;
    assert!(
        body.contains("href=\"/agents/weird%20agent%3Fx%3D1\""),
        "href must be percent-encoded: {body}"
    );
    assert!(
        body.contains(">weird agent?x=1</a>"),
        "anchor text should read naturally (only HTML-escaping applies, and this name \
         has no HTML-special characters): {body}"
    );
}

#[tokio::test]
async fn a_files_page_lists_artifacts_with_uploader_and_size() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room("protocol", "caas").await.unwrap();
        store
            .put_file("protocol", "schema.json", b"{\"a\":1}", None, "caas")
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/rooms/protocol/files").await;
    assert!(body.contains("schema.json"));
    assert!(body.contains("caas"), "uploader must be shown");
}

#[tokio::test]
async fn the_event_log_filters_by_kind() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .append_event("ack", Some("caas"), Some("r"), serde_json::json!({}))
            .await
            .unwrap();
        store
            .append_event(
                "room_paused",
                Some("caas"),
                Some("r"),
                serde_json::json!({}),
            )
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let all = get(port, "/events").await;
    assert!(all.contains("ack") && all.contains("room_paused"));

    let filtered = get(port, "/events?kind=room_paused").await;
    assert!(filtered.contains("room_paused"));
    assert!(
        !filtered.contains(">ack<"),
        "the filter must actually exclude other kinds"
    );
}

#[tokio::test]
async fn the_event_log_filters_by_agent() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .append_event(
                "ack",
                Some("caas"),
                Some("r"),
                serde_json::json!({"tag": "from-caas"}),
            )
            .await
            .unwrap();
        store
            .append_event(
                "ack",
                Some("other"),
                Some("r"),
                serde_json::json!({"tag": "from-other"}),
            )
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let filtered = get(port, "/events?agent=caas").await;
    assert!(filtered.contains("from-caas"));
    assert!(
        !filtered.contains("from-other"),
        "the agent filter must exclude events from other agents"
    );
}

#[tokio::test]
async fn the_event_log_filters_by_room() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .append_event(
                "ack",
                Some("caas"),
                Some("protocol"),
                serde_json::json!({"tag": "in-protocol"}),
            )
            .await
            .unwrap();
        store
            .append_event(
                "ack",
                Some("caas"),
                Some("other-room"),
                serde_json::json!({"tag": "in-other-room"}),
            )
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let filtered = get(port, "/events?room=protocol").await;
    assert!(filtered.contains("in-protocol"));
    assert!(
        !filtered.contains("in-other-room"),
        "the room filter must exclude events from other rooms"
    );
}

#[tokio::test]
async fn the_event_log_agent_cell_links_to_its_own_filtered_view() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .append_event(
                "ack",
                Some("weird agent?x=1"),
                Some("r"),
                serde_json::json!({"tag": "from-caas"}),
            )
            .await
            .unwrap();
        store
            .append_event(
                "ack",
                Some("other"),
                Some("r"),
                serde_json::json!({"tag": "from-other"}),
            )
            .await
            .unwrap();
        // Nullable: an event with no agent must not render as a link.
        store
            .append_event("resumed", None, None, serde_json::json!({}))
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/events").await;
    assert!(
        body.contains("href=\"/events?agent=weird%20agent%3Fx%3D1\">weird agent?x=1</a>"),
        "the agent cell must link to its own filtered view, percent-encoded in the href \
         and HTML-escaped as anchor text, exactly like the kind column: {body}"
    );
    assert!(
        !body.contains("href=\"/events?agent=\""),
        "an event with no agent must not become a link to an empty filter: {body}"
    );

    let filtered = get(port, "/events?agent=weird%20agent%3Fx%3D1").await;
    assert!(filtered.contains("from-caas"));
    assert!(
        !filtered.contains("from-other"),
        "following the agent link must exclude events from other agents"
    );
}

#[tokio::test]
async fn the_event_log_room_cell_links_to_its_own_filtered_view() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .append_event(
                "ack",
                Some("caas"),
                Some("weird room&y=2"),
                serde_json::json!({"tag": "in-protocol"}),
            )
            .await
            .unwrap();
        store
            .append_event(
                "ack",
                Some("caas"),
                Some("other-room"),
                serde_json::json!({"tag": "in-other-room"}),
            )
            .await
            .unwrap();
        // Nullable: an event with no room must not render as a link.
        store
            .append_event("resumed", None, None, serde_json::json!({}))
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/events").await;
    assert!(
        body.contains("href=\"/events?room=weird%20room%26y%3D2\">weird room&amp;y=2</a>"),
        "the room cell must link to its own filtered view, percent-encoded in the href \
         and HTML-escaped as anchor text, exactly like the kind column: {body}"
    );
    assert!(
        !body.contains("href=\"/events?room=\""),
        "an event with no room must not become a link to an empty filter: {body}"
    );

    let filtered = get(port, "/events?room=weird%20room%26y%3D2").await;
    assert!(filtered.contains("in-protocol"));
    assert!(
        !filtered.contains("in-other-room"),
        "following the room link must exclude events from other rooms"
    );
}

#[tokio::test]
async fn the_event_log_combines_kind_and_agent_filters_with_and_semantics() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        // Matches both filters.
        store
            .append_event(
                "room_paused",
                Some("caas"),
                Some("r"),
                serde_json::json!({"tag": "match"}),
            )
            .await
            .unwrap();
        // Right kind, wrong agent.
        store
            .append_event(
                "room_paused",
                Some("other"),
                Some("r"),
                serde_json::json!({"tag": "wrong-agent"}),
            )
            .await
            .unwrap();
        // Right agent, wrong kind.
        store
            .append_event(
                "ack",
                Some("caas"),
                Some("r"),
                serde_json::json!({"tag": "wrong-kind"}),
            )
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let filtered = get(port, "/events?kind=room_paused&agent=caas").await;
    // Detail JSON is rendered through `esc`, so the literal quotes come out as
    // `&quot;` — assert on the tag value itself, not the raw JSON punctuation.
    assert!(filtered.contains("match"));
    assert!(
        !filtered.contains("wrong-agent"),
        "combined filters must exclude events matching only the kind"
    );
    assert!(
        !filtered.contains("wrong-kind"),
        "combined filters must exclude events matching only the agent"
    );
}

#[tokio::test]
async fn the_agents_page_does_not_show_ghosts_from_a_previous_bus() {
    // The regression: `online` is persisted, but the registry that knows who is actually
    // connected is in memory. A bus killed mid-connection leaves the column claiming an
    // agent is online; a later bus reading that column reports a ghost. This asserts the
    // page reflects the live registry (and the startup reconciliation), not the column.
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

    let body = get(port, "/agents").await;
    assert!(
        body.contains("ghost"),
        "the agent is still known, just not connected: {body}"
    );
    assert!(
        !body.contains(">online<"),
        "nothing is connected to this freshly started bus: {body}"
    );
    assert!(
        body.contains(">offline<"),
        "it should be shown as offline: {body}"
    );
}

#[tokio::test]
async fn the_overview_shows_recent_message_text_across_rooms() {
    // The event log records that a message was sent and whether it was delivered, but
    // not what it said. Without this the overview can't answer "what are they talking
    // about" without guessing which room to open.
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room("protocol", "caas").await.unwrap();
        store.join_room("other", "dashboard").await.unwrap();
        store
            .append_message("protocol", "caas", "SETTLED_ON_THE_SCHEMA", false, false)
            .await
            .unwrap();
        store
            .append_message("other", "dashboard", "UNRELATED_CHATTER", false, false)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/").await;
    assert!(
        body.contains("SETTLED_ON_THE_SCHEMA"),
        "message text must appear on the overview: {body}"
    );
    assert!(
        body.contains("UNRELATED_CHATTER"),
        "and it spans rooms, not just one: {body}"
    );
}

#[tokio::test]
async fn a_long_message_is_truncated_without_splitting_a_character() {
    // Message bodies are model output, so non-ASCII is ordinary. Byte slicing would
    // panic partway through a multi-byte character and take the whole page down.
    let dir = tempfile::tempdir().unwrap();
    let long = "é".repeat(400);
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room("protocol", "caas").await.unwrap();
        store
            .append_message("protocol", "caas", &long, false, false)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/").await;
    assert!(body.contains('…'), "should be visibly truncated: {body}");
    assert!(
        !body.contains(&long),
        "the full 400-character body must not be inlined on the overview"
    );
}

#[tokio::test]
async fn a_human_is_marked_distinctly_in_the_agent_list() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "hardac", "/w", None, false, None)
            .await
            .unwrap();
        store
            .upsert_agent("bbaldino", "hardac", "/w", None, true, None)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/agents").await;
    assert!(
        body.contains("bbaldino"),
        "the human must be listed: {body}"
    );
    assert!(
        body.contains("human"),
        "and marked as one rather than looking like a bot: {body}"
    );
    // The marker must not be applied to everyone.
    let human_marks = body.matches("class=\"human\"").count();
    assert_eq!(
        human_marks, 1,
        "exactly one row should carry the marker: {body}"
    );
}

#[tokio::test]
async fn the_agents_page_shows_versions_and_flags_mismatches() {
    let dir = tempfile::tempdir().unwrap();
    let current = env!("CARGO_PKG_VERSION");
    {
        let store = Store::open(dir.path()).await.unwrap();
        // Matches the bus: should NOT be flagged.
        store
            .upsert_agent("current", "hardac", "/w", None, false, Some(current))
            .await
            .unwrap();
        // Behind the bus: should be flagged.
        store
            .upsert_agent("stale", "hardac", "/w", None, false, Some("0.0.1"))
            .await
            .unwrap();
        // Predates the field entirely: should be flagged, and shown as unknown.
        store
            .upsert_agent("ancient", "hardac", "/w", None, false, None)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/agents").await;
    assert!(
        body.contains("0.0.1"),
        "a reported version must be shown: {body}"
    );
    assert!(
        body.contains("unknown"),
        "an agent that reported nothing must read as unknown, not blank: {body}"
    );
    assert!(
        body.contains("this bus is running"),
        "the page must state its own running version, not just happen to contain the \
         string somewhere (e.g. in an agent's own version cell): {body}"
    );
    assert!(
        body.contains(current),
        "the bus's own version must be on the page to compare against: {body}"
    );
    // Exactly the two that differ from the bus carry the marker.
    assert_eq!(
        body.matches("class=\"stale\"").count(),
        2,
        "only the differing agents should be flagged: {body}"
    );

    // `/` renders the same agent table via a shared row helper — nothing previously
    // exercised that page's version column or note, which is exactly the shape where a
    // later change updates `/agents` and silently leaves `/` behind.
    let overview_body = get(port, "/").await;
    assert!(
        overview_body.contains("0.0.1"),
        "a reported version must be shown on the overview too: {overview_body}"
    );
    assert!(
        overview_body.contains("this bus is running"),
        "the overview must state its own running version: {overview_body}"
    );
    assert!(
        overview_body.contains(current),
        "the bus's own version must be on the overview to compare against: {overview_body}"
    );
}

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

#[tokio::test]
async fn the_confirm_page_lists_what_will_be_removed() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("network-debug#2", "hardac", "/w/nd", None, false, None)
            .await
            .unwrap();
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

    // `#` must be percent-encoded or the path silently truncates at the fragment.
    let body = get(port, "/agents/network-debug%232/delete").await;

    assert!(body.contains("network-debug#2"), "name must appear: {body}");
    assert!(
        body.contains("protocol"),
        "the membership at risk must be listed"
    );
    assert!(body.contains("1 cursor"), "the cursor count must appear");
    assert!(
        body.contains("messages and events are kept"),
        "the page must say what survives"
    );
    assert!(
        body.contains("<form"),
        "an offline agent must get a real button"
    );
}

/// The footprint listing is the only safeguard on this action, so a failed read
/// must not render as an empty one. `unwrap_or_default` made a database error
/// indistinguishable from "this agent belongs to nothing" — and still offered
/// the button.
#[tokio::test]
async fn the_confirm_page_offers_no_button_when_the_footprint_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("network-debug#2", "hardac", "/w/nd", None, false, None)
            .await
            .unwrap();
        store
            .join_room("protocol", "network-debug#2")
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    // Break the footprint query out from under the running bus, the same way
    // `tests/events.rs` breaks the event log.
    {
        let store = Store::open(dir.path()).await.unwrap();
        sqlx::query("DROP TABLE cursors")
            .execute(store.pool_for_test())
            .await
            .unwrap();
    }

    let body = get(port, "/agents/network-debug%232/delete").await;
    assert!(
        !body.contains("<form"),
        "no button may be offered when the blast radius is unknown: {body}"
    );
    assert!(
        body.contains("could not read"),
        "the failure must be stated rather than rendered as an empty list: {body}"
    );
}

#[tokio::test]
async fn the_confirm_page_refuses_an_online_agent() {
    let (_d, port, _path) = common::start_bus_with_dir().await;
    let _ws = common::connect(port, "caas").await;
    assert!(common::agent_is_online(port, "caas").await);

    let body = get(port, "/agents/caas/delete").await;

    assert!(
        body.contains("online"),
        "the refusal reason must be shown: {body}"
    );
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

#[tokio::test]
async fn posting_the_delete_removes_the_agent_and_redirects() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("network-debug#2", "hardac", "/w/nd", None, false, None)
            .await
            .unwrap();
        store
            .join_room("protocol", "network-debug#2")
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let res = post(port, "/agents/network-debug%232/delete").await;
    // Matched on the status line, not anywhere in the response: "303" also
    // appears in a rendered agent name, a port number, or a timestamp, so a
    // substring match would pass for a page that deleted nothing.
    assert!(
        res.starts_with("HTTP/1.1 303"),
        "must redirect after a POST: {res}"
    );

    let agents = get(port, "/agents").await;
    assert!(
        !agents.contains("network-debug#2"),
        "the deleted agent must be gone from the list: {agents}"
    );

    // `/rooms` is the page that renders each room's *members*; the per-room
    // transcript does not, so asserting against it would pass for any input.
    let rooms = get(port, "/rooms").await;
    assert!(
        !rooms.contains("network-debug#2"),
        "the stranded membership must be gone too: {rooms}"
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
        // Deliberately non-empty: with no membership and no cursor every count
        // renders as 0 whether the event carries them or not, so the test would
        // pass against an event that dropped them entirely.
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
    post(port, "/agents/network-debug%232/delete").await;

    let events = get(port, "/events").await;
    assert!(events.contains("agent_deleted"), "got: {events}");
    assert!(
        events.contains("network-debug#2"),
        "the audit event must name the deleted agent, since its row is gone"
    );
    assert!(
        events.contains("hardac"),
        "the host must survive the row: {events}"
    );
    // The row is gone, so this event is the only remaining record of when the
    // agent was last alive and of what the delete actually removed.
    assert!(
        events.contains("last seen"),
        "last_seen must be recorded: {events}"
    );
    assert!(
        events.contains("1 agent rows"),
        "the agent-row count makes a phantom delete self-evident: {events}"
    );
    assert!(
        events.contains("1 memberships"),
        "the membership count must be non-zero and rendered: {events}"
    );
    assert!(
        events.contains("1 cursors"),
        "the cursor count must be non-zero and rendered: {events}"
    );
}

/// Without an unknown-name guard on the `POST`, any name at all passes the
/// offline check trivially, deletes nothing, and still writes an
/// `agent_deleted` event — forging the one record this feature promises will
/// outlive the agent, and letting anyone who can reach the port grow `events`
/// unboundedly, several KB at a time, through the URL path alone.
#[tokio::test]
async fn posting_the_delete_for_an_unknown_agent_writes_no_audit_event() {
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;

    let res = post(port, "/agents/nobody/delete").await;
    assert!(
        res.contains("no agent named nobody"),
        "the POST must render the same refusal the GET does: {res}"
    );

    let events = get(port, "/events").await;
    assert!(
        !events.contains("agent_deleted"),
        "an unknown name must forge no audit event: {events}"
    );
    assert!(
        !events.contains("nobody"),
        "and must write nothing naming the caller's string: {events}"
    );
}

/// A form POST is a "simple request" — no preflight — so any page the
/// operator's browser loads could otherwise submit one to a bus on `127.0.0.1`
/// or behind NAT that the attacker cannot reach directly.
#[tokio::test]
async fn a_cross_origin_post_is_refused_and_deletes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("network-debug#2", "hardac", "/w/nd", None, false, None)
            .await
            .unwrap();
        store
            .join_room("protocol", "network-debug#2")
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let res = post_with_headers(
        port,
        "/agents/network-debug%232/delete",
        &[("Origin", "http://evil.example")],
    )
    .await;
    assert!(
        !res.starts_with("HTTP/1.1 303"),
        "a cross-origin POST must not succeed: {res}"
    );
    assert!(
        res.contains("evil.example"),
        "the refusal must say what was rejected: {res}"
    );

    let agents = get(port, "/agents").await;
    assert!(
        agents.contains("network-debug#2"),
        "the agent must survive a refused POST: {agents}"
    );
    let rooms = get(port, "/rooms").await;
    assert!(
        rooms.contains("network-debug#2"),
        "and so must its membership: {rooms}"
    );
    let events = get(port, "/events").await;
    assert!(
        !events.contains("agent_deleted"),
        "a refused POST must record no delete: {events}"
    );
}

#[tokio::test]
async fn posting_the_delete_refuses_an_agent_that_came_online_after_the_confirm_page() {
    let (_d, port, path) = common::start_bus_with_dir().await;
    // Offline when the confirm page would have been rendered...
    {
        // Await the registration before dropping. `connect` returns as soon as
        // the Register frame is written, so dropping immediately leaves this
        // connection's attach racing its own detach — and then the wait below
        // can succeed because the agent never registered rather than because it
        // disconnected, which is not the state this test means to set up.
        let mut ws = common::connect(port, "caas").await;
        common::next_event(&mut ws).await; // Registered
        drop(ws);
    }
    assert!(
        common::wait_until(|| async move { !common::agent_is_online(port, "caas").await }).await
    );

    // ...and online by the time the POST lands.
    let mut ws = common::connect(port, "caas").await;
    // This session must own the BARE name. If a lingering connection still held
    // it, the bus would hand this one `caas#2` — every assertion below would
    // then be about a different agent than the one the POST targets, and the
    // test would pass or fail for reasons unrelated to the liveness re-check.
    match common::next_event(&mut ws).await {
        FromBus::Registered { name } => assert_eq!(
            name, "caas",
            "reconnect must own the bare name, not a collision suffix"
        ),
        other => panic!("expected Registered, got {other:?}"),
    }
    assert!(common::agent_is_online(port, "caas").await);

    // The memberships and cursors are the point of the rule: they are what a
    // live agent is still receiving messages through, and nothing repairs them
    // if they are dropped out from under a connected session.
    let store = Store::open(&path).await.unwrap();
    store.join_room("protocol", "caas").await.unwrap();
    store.set_cursor("protocol", "caas", 9).await.unwrap();

    let res = post(port, "/agents/caas/delete").await;
    assert!(
        res.contains("online"),
        "the POST must re-check liveness: {res}"
    );

    let agents = get(port, "/agents").await;
    assert!(agents.contains("caas"), "the live agent must survive");
    assert_eq!(
        store.room_members("protocol").await.unwrap(),
        vec!["caas".to_string()],
        "the live agent's membership must survive"
    );
    assert_eq!(
        store.cursor("protocol", "caas").await.unwrap(),
        9,
        "and so must its cursor"
    );
}

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
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;

    let body = get(port, "/api/meta").await;

    assert!(body.contains("\"version\""), "got: {body}");
    assert!(
        body.contains(env!("CARGO_PKG_VERSION")),
        "must report the running version: {body}"
    );
}

/// The routing table above `resolve`, which nothing else exercises: `resolve` is
/// unit-tested in `src/web/assets.rs`, but a path that never reaches it fails
/// invisibly. `/app/` in particular matches neither `/app` nor `/app/{*rest}`
/// (matchit requires a non-empty catch-all remainder) and so needs its own
/// route — and `/app/` is the canonical URL, since `ui/vite.config.ts` sets
/// `base: '/app/'` and that is what a `location /app/` proxy produces.
///
/// Asserts shape rather than content: CI's Rust job has only `.gitkeep` in
/// `ui/dist`, so there is no built bundle here. What must hold either way is
/// that all three forms behave identically and that none of them is a bare
/// route-miss.
#[tokio::test]
async fn the_app_routes_all_reach_the_bundle_handler() {
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;

    let mut statuses = Vec::new();
    for path in ["/app", "/app/", "/app/agents/caas"] {
        let res = get(port, path).await;
        let status = res
            .lines()
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_string();

        // A bare 404 with no body is axum's router fallback — the request never
        // reached a handler. The handler's own miss says why.
        assert_ne!(
            status, "404",
            "{path} must reach the bundle handler, not fall off the router: {res}"
        );
        statuses.push((path, status));
    }

    let first = statuses[0].1.clone();
    for (path, status) in &statuses {
        assert_eq!(
            *status, first,
            "{path} must behave like /app — all three are the same app: {statuses:?}"
        );
    }

    // With no bundle built (the state of a fresh clone and of CI's Rust job) the
    // handler explains itself rather than 404ing. With one built it serves the
    // shell. Either is fine; silence is not.
    let res = get(port, "/app/").await;
    assert!(
        res.contains("was not built") || res.contains("<!doctype") || res.contains("<!DOCTYPE"),
        "/app/ must serve the shell or say why it cannot: {res}"
    );
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
