// Drives the real server over HTTP against a temp SQLite. Asserts rendered content,
// not status codes: a page that returns 200 with an empty table has failed at its
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
