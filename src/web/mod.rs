//! Read-only web views over the bus's own data. Performs no writes: it cannot be the
//! cause of a bug it is being used to investigate, and with no authentication on the
//! bus, anything this could do would be available to anything that can reach the port.

pub mod html;

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::response::Html;
use axum::{Router, routing::get};

use crate::bus::App;
use html::{encode_path_segment, esc, fmt_time, page};

/// Render an event's `detail_json` as a short human-readable phrase.
///
/// The raw JSON is faithful but unreadable at a glance, and these tables exist to be
/// scanned. Each arm surfaces the fields that make that kind of event worth recording —
/// most importantly `agent_registered`, where showing the requested name beside the
/// effective one is the whole point: a session that silently became `caas#2` is visible
/// here rather than inferred.
///
/// Unknown kinds fall back to the compact JSON rather than rendering nothing, so a kind
/// added later is still legible before anyone teaches this function about it.
fn summarize(kind: &str, detail: &serde_json::Value) -> String {
    let text = |k: &str| detail.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let num = |k: &str| detail.get(k).and_then(|v| v.as_i64());
    let names = |k: &str| {
        detail
            .get(k)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>())
            .unwrap_or_default()
    };

    match kind {
        "message_sent" => {
            let mut parts = Vec::new();
            if let Some(id) = num("msg_id") {
                parts.push(format!("#{id}"));
            }
            let delivered = names("delivered_to");
            let queued = names("queued_for");
            if !delivered.is_empty() {
                parts.push(format!("delivered to {}", delivered.join(", ")));
            }
            if !queued.is_empty() {
                parts.push(format!("queued for {}", queued.join(", ")));
            }
            if delivered.is_empty() && queued.is_empty() {
                parts.push("no recipients".to_string());
            }
            parts.join(" · ")
        }
        "ack" => match num("last_delivered_id") {
            Some(id) => format!("up to #{id}"),
            None => String::new(),
        },
        "agent_registered" => {
            let requested = text("requested_name");
            let effective = text("effective_name");
            let host = text("host");
            let mut s = if !requested.is_empty() && requested != effective {
                format!("requested {requested}, became {effective}")
            } else {
                String::new()
            };
            if !host.is_empty() {
                if !s.is_empty() {
                    s.push_str(" · ");
                }
                s.push_str(&format!("on {host}"));
            }
            s
        }
        "agent_disconnected" => text("reason").to_string(),
        "room_paused" => match num("count") {
            Some(c) => format!("after {c} exchanges"),
            None => String::new(),
        },
        "rate_limited" => match num("retry_in_ms") {
            Some(ms) => format!("retry in {ms}ms"),
            None => String::new(),
        },
        "file_stored" => {
            let key = text("key");
            match num("size") {
                Some(size) => format!("{key} ({size} bytes)"),
                None => key.to_string(),
            }
        }
        "file_fetched" => text("key").to_string(),
        "room_joined" | "resumed" => String::new(),
        _ => raw_detail(detail),
    }
}

/// The compact JSON for a non-empty detail object, or empty string for `{}`.
fn raw_detail(detail: &serde_json::Value) -> String {
    match detail.as_object() {
        Some(o) if o.is_empty() => String::new(),
        _ => detail.to_string(),
    }
}

/// What to actually show for an event's detail: the readable summary when there is one,
/// otherwise the raw JSON.
///
/// The fallback is not cosmetic. `summarize` reads only the fields it knows about per
/// kind, so an event carrying anything extra would otherwise render as a blank cell and
/// the recorded data would be invisible in the very view built to audit it. Falling back
/// to the JSON means an unrecognised payload is ugly rather than absent — the right trade
/// for a debugging tool, where silently dropping data is the worse failure.
fn detail_text(kind: &str, detail: &serde_json::Value) -> String {
    let summary = summarize(kind, detail);
    if summary.is_empty() {
        raw_detail(detail)
    } else {
        summary
    }
}

/// A full `<td>` for an event's detail: the readable text, with the complete raw JSON
/// preserved in `title` so the summary never costs fidelity for anyone who needs the
/// exact payload.
fn detail_cell(kind: &str, detail: &serde_json::Value) -> String {
    let raw = raw_detail(detail);
    if raw.is_empty() {
        return "<td class=\"detail\"></td>".to_string();
    }
    format!(
        "<td class=\"detail\" title=\"{t}\">{s}</td>",
        t = esc(&raw),
        s = esc(&detail_text(kind, detail)),
    )
}

pub fn routes() -> Router<App> {
    Router::new()
        .route("/", get(overview))
        .route("/rooms", get(rooms))
        .route("/rooms/{name}", get(room))
        .route("/rooms/{name}/files", get(room_files))
        .route("/agents", get(agents))
        .route("/agents/{name}", get(agent))
        .route("/events", get(events_page))
}

async fn overview(State(app): State<App>) -> Html<String> {
    let agents = app.store.agents().await.unwrap_or_default();
    let rooms = app.store.rooms().await.unwrap_or_default();
    let events = app.store.events(20).await.unwrap_or_default();

    // See `agents()`: liveness comes from the registry, not the persisted column.
    let live = app.registry.online().await;
    let mut b = String::new();
    b.push_str("<h1>overview</h1><h2>agents</h2><table><tr><th>name<th>host<th>state</tr>");
    for a in &agents {
        let online = live.contains(&a.name);
        b.push_str(&format!(
            "<tr><td><a href=\"/agents/{p}\">{n}</a></td><td>{h}</td><td class=\"{c}\">{s}</td></tr>",
            p = encode_path_segment(&a.name),
            n = esc(&a.name),
            h = esc(&a.host),
            c = if online { "" } else { "off" },
            s = if online { "online" } else { "offline" },
        ));
    }
    b.push_str("</table><h2>rooms</h2><table><tr><th>room<th>members</tr>");
    for r in &rooms {
        b.push_str(&format!(
            "<tr><td><a href=\"/rooms/{p}\">{n}</a></td><td>{m}</td></tr>",
            p = encode_path_segment(&r.name),
            n = esc(&r.name),
            m = esc(&r.members.join(", ")),
        ));
    }
    // The sort direction is stated because it is not guessable from the data: a send and
    // the ack it provokes land milliseconds apart, so newest-first puts the ack *above*
    // the message it acknowledges, which reads as backwards until you know the rule.
    b.push_str(
        "</table><h2>recent events <span class=\"note\">newest first</span></h2>\
         <table><tr><th>when<th>kind<th>agent<th>room<th>detail</tr>",
    );
    for e in &events {
        b.push_str(&format!(
            "<tr><td class=\"when\">{w}</td><td>{k}</td><td>{a}</td><td>{r}</td>{d}</tr>",
            w = esc(&fmt_time(e.created_at)),
            k = esc(&e.kind),
            a = esc(e.agent.as_deref().unwrap_or("")),
            r = esc(e.room.as_deref().unwrap_or("")),
            d = detail_cell(&e.kind, &e.detail),
        ));
    }
    b.push_str("</table>");
    Html(page("overview", &b))
}

/// One row of a transcript, from either source, sorted into a single timeline.
enum Entry {
    Message {
        at: i64,
        from: String,
        body: String,
    },
    Event {
        at: i64,
        kind: String,
        detail: String,
    },
}

impl Entry {
    fn at(&self) -> i64 {
        match self {
            Entry::Message { at, .. } | Entry::Event { at, .. } => *at,
        }
    }

    /// Tie-break for entries with the same `created_at`. Message ids and event ids are
    /// independent, unrelated autoincrement sequences (separate tables), so there is no
    /// way to compare them directly to break a tie "correctly" — and `now_ms()` is
    /// millisecond resolution, so two rows written back-to-back on a fast machine share
    /// a timestamp routinely, not as an edge case.
    ///
    /// Deliberate choice: messages sort before events at an equal timestamp. Events in
    /// this log are overwhelmingly the bus's *reaction* to something an agent did (a
    /// message pushing a room over its exchange cap and triggering `room_paused`, for
    /// example) — so on a tie, showing the message first and the event it likely
    /// provoked second matches the causal story a reader is reconstructing, even though
    /// the clock alone can't prove the causality. This is a tie-break of convenience,
    /// not a verified ordering guarantee.
    fn rank(&self) -> u8 {
        match self {
            Entry::Message { .. } => 0,
            Entry::Event { .. } => 1,
        }
    }
}

async fn room(State(app): State<App>, Path(name): Path<String>) -> Html<String> {
    let msgs = app.store.history(&name, 500).await.unwrap_or_default();
    let evs = app
        .store
        .events_for_room(&name, 500)
        .await
        .unwrap_or_default();

    let mut entries: Vec<Entry> = Vec::with_capacity(msgs.len() + evs.len());
    for m in msgs {
        entries.push(Entry::Message {
            at: m.created_at,
            from: m.from_agent,
            body: m.body,
        });
    }
    for e in evs {
        let detail = detail_text(&e.kind, &e.detail);
        entries.push(Entry::Event {
            at: e.created_at,
            kind: e.kind,
            detail,
        });
    }
    // The whole point of the page: one timeline, not two lists. See `Entry::rank` for
    // how same-millisecond ties are broken.
    entries.sort_by_key(|e| (e.at(), e.rank()));

    let mut b = format!(
        // Opposite direction to the event tables, and deliberately so: a transcript is
        // read as a conversation, top to bottom. Labelled because the other pages sort
        // the other way and switching between them without a cue is disorienting.
        "<h1>{n} <span class=\"note\">oldest first</span></h1>\
         <p><a href=\"/rooms/{p}/files\">files</a></p>\
         <table><tr><th>when<th>who<th>what</tr>",
        n = esc(&name),
        p = encode_path_segment(&name),
    );
    for e in &entries {
        match e {
            Entry::Message { at, from, body } => b.push_str(&format!(
                "<tr><td class=\"when\">{w}</td><td>{f}</td><td class=\"msg\">{t}</td></tr>",
                w = esc(&fmt_time(*at)),
                f = esc(from),
                t = esc(body),
            )),
            Entry::Event { at, kind, detail } => b.push_str(&format!(
                "<tr><td class=\"when\">{w}</td><td class=\"ev\">{k}</td>\
                 <td class=\"ev\">{d}</td></tr>",
                w = esc(&fmt_time(*at)),
                k = esc(kind),
                d = esc(detail),
            )),
        }
    }
    b.push_str("</table>");
    Html(page(&name, &b))
}

async fn room_files(State(app): State<App>, Path(name): Path<String>) -> Html<String> {
    let files = app.store.list_files(&name).await.unwrap_or_default();
    let mut b = format!(
        "<h1><a href=\"/rooms/{p}\">{n}</a> · files</h1><table><tr><th>key<th>size<th>by<th>sha256</tr>",
        p = encode_path_segment(&name),
        n = esc(&name),
    );
    for f in &files {
        b.push_str(&format!(
            "<tr><td>{k}</td><td>{s}</td><td>{u}</td><td>{h}</td></tr>",
            k = esc(&f.key),
            s = f.size,
            u = esc(&f.updated_by),
            h = esc(&f.sha256[..16.min(f.sha256.len())]),
        ));
    }
    b.push_str("</table>");
    Html(page(&format!("{name} · files"), &b))
}

async fn agents(State(app): State<App>) -> Html<String> {
    let agents = app.store.agents().await.unwrap_or_default();
    // Liveness comes from the in-memory registry, not the persisted `online` column —
    // the same source the `agents` MCP tool uses. The column is a cache that can only be
    // written by a graceful teardown, so a bus killed mid-connection leaves it claiming
    // agents are online that are not. Reading the registry keeps this page and the tool
    // from disagreeing about who is connected.
    let live = app.registry.online().await;
    let mut b = String::from("<h1>agents</h1><table><tr><th>name<th>host<th>state</tr>");
    for a in &agents {
        let online = live.contains(&a.name);
        b.push_str(&format!(
            "<tr><td><a href=\"/agents/{p}\">{n}</a></td><td>{h}</td><td class=\"{c}\">{s}</td></tr>",
            p = encode_path_segment(&a.name),
            n = esc(&a.name),
            h = esc(&a.host),
            c = if online { "" } else { "off" },
            s = if online { "online" } else { "offline" },
        ));
    }
    b.push_str("</table>");
    Html(page("agents", &b))
}

async fn agent(State(app): State<App>, Path(name): Path<String>) -> Html<String> {
    let rooms = app.store.rooms().await.unwrap_or_default();
    let mine: Vec<&str> = rooms
        .iter()
        .filter(|r| r.members.contains(&name))
        .map(|r| r.name.as_str())
        .collect();
    let evs = app
        .store
        .events_for_agent(&name, 200)
        .await
        .unwrap_or_default();

    let mut b = format!("<h1>{}</h1><h2>rooms</h2><ul>", esc(&name));
    for r in &mine {
        b.push_str(&format!(
            "<li><a href=\"/rooms/{p}\">{n}</a></li>",
            p = encode_path_segment(r),
            n = esc(r),
        ));
    }
    b.push_str(
        "</ul><h2>activity <span class=\"note\">newest first</span></h2>\
         <table><tr><th>when<th>kind<th>room<th>detail</tr>",
    );
    for e in &evs {
        b.push_str(&format!(
            "<tr><td class=\"when\">{w}</td><td>{k}</td><td>{r}</td>{d}</tr>",
            w = esc(&fmt_time(e.created_at)),
            k = esc(&e.kind),
            r = esc(e.room.as_deref().unwrap_or("")),
            d = detail_cell(&e.kind, &e.detail),
        ));
    }
    b.push_str("</table>");
    Html(page(&name, &b))
}

/// The raw event log, optionally narrowed by `kind`, `agent`, and/or `room` query
/// params (`GET /events?kind=...&agent=...&room=...`). Time filtering is deliberately
/// out of scope for this task — it would need a new store query (`events` and its
/// siblings only take a row limit, not a time range) and is left for a future task.
///
/// The store has no single query that filters on more than one of kind/agent/room at
/// once, and adding one is out of scope here (this task touches `src/web/` only). So
/// exactly one filter is pushed down to SQL via whichever of `events_for_room`,
/// `events_for_agent`, or `events_of_kind` applies (room takes priority, then agent,
/// then kind, on the assumption that narrowing to a room or an agent is usually the
/// more deliberate query — an arbitrary but harmless choice, since every provided
/// filter is then re-applied in memory below regardless of which one was pushed down).
/// The net effect is straightforward AND semantics: with `kind` and `agent` both set,
/// you get events that match both. The one caveat worth knowing: the SQL-pushed-down
/// filter already caps the candidate set at 500 rows *before* the remaining filters
/// narrow it further, so a combination that's rare within that filter's most recent
/// 500 rows can come back thinner than the same combination would over the full log.
async fn events_page(
    State(app): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> Html<String> {
    let kind = q.get("kind").map(String::as_str);
    let agent = q.get("agent").map(String::as_str);
    let room = q.get("room").map(String::as_str);

    let mut evs = if let Some(r) = room {
        // events_for_room returns oldest-first (it feeds the transcript view); flip it
        // so this page is newest-first like every other filter path.
        let mut v = app.store.events_for_room(r, 500).await.unwrap_or_default();
        v.reverse();
        v
    } else if let Some(a) = agent {
        app.store.events_for_agent(a, 500).await.unwrap_or_default()
    } else if let Some(k) = kind {
        app.store.events_of_kind(k, 500).await.unwrap_or_default()
    } else {
        app.store.events(500).await.unwrap_or_default()
    };
    if let Some(k) = kind {
        evs.retain(|e| e.kind == k);
    }
    if let Some(a) = agent {
        evs.retain(|e| e.agent.as_deref() == Some(a));
    }
    if let Some(r) = room {
        evs.retain(|e| e.room.as_deref() == Some(r));
    }

    let mut b = String::from(
        "<h1>events <span class=\"note\">newest first</span></h1>\
         <table><tr><th>when<th>kind<th>agent<th>room<th>detail</tr>",
    );
    for e in &evs {
        // `agent` and `room` are nullable, so an event without one gets a plain empty
        // cell rather than a link to `/events?agent=` or `/events?room=`, which would
        // filter to nothing meaningful.
        let agent_cell = match e.agent.as_deref() {
            Some(a) => format!(
                "<a href=\"/events?agent={ap}\">{a}</a>",
                ap = encode_path_segment(a),
                a = esc(a),
            ),
            None => String::new(),
        };
        let room_cell = match e.room.as_deref() {
            Some(r) => format!(
                "<a href=\"/events?room={rp}\">{r}</a>",
                rp = encode_path_segment(r),
                r = esc(r),
            ),
            None => String::new(),
        };
        b.push_str(&format!(
            "<tr><td class=\"when\">{w}</td><td><a href=\"/events?kind={kp}\">{k}</a></td>\
             <td>{a}</td><td>{r}</td>{d}</tr>",
            w = esc(&fmt_time(e.created_at)),
            // A query *value* isn't a path segment, but percent-encoding it against the
            // same unreserved set (`encode_path_segment`) is still correct here: that
            // encoding escapes every byte outside `A-Za-z0-9-._~`, which is a superset
            // of what a query value strictly requires (at minimum `&`, `=`, `#`) — and
            // over-escaping a URL component is always valid per RFC 3986, just more
            // verbose than the minimal query-specific escaping would be. Reusing it
            // avoids adding a near-duplicate helper for one call site.
            kp = encode_path_segment(&e.kind),
            k = esc(&e.kind),
            a = agent_cell,
            r = room_cell,
            d = detail_cell(&e.kind, &e.detail),
        ));
    }
    b.push_str("</table>");
    Html(page("events", &b))
}

async fn rooms(State(app): State<App>) -> Html<String> {
    let rooms = app.store.rooms().await.unwrap_or_default();
    let mut b = String::from("<h1>rooms</h1><table><tr><th>room<th>members</tr>");
    for r in &rooms {
        b.push_str(&format!(
            "<tr><td><a href=\"/rooms/{p}\">{n}</a></td><td>{m}</td></tr>",
            p = encode_path_segment(&r.name),
            n = esc(&r.name),
            m = esc(&r.members.join(", ")),
        ));
    }
    b.push_str("</table>");
    Html(page("rooms", &b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_send_summary_distinguishes_delivered_from_queued() {
        // The delivered-vs-queued distinction is one of the defects this log exists to
        // expose, so it must survive into what a human actually reads.
        let d = json!({"msg_id": 4, "delivered_to": [], "queued_for": ["beta"]});
        let s = summarize("message_sent", &d);
        assert!(s.contains("queued for beta"), "{s}");
        assert!(!s.contains("delivered to"), "nothing was delivered: {s}");

        let d = json!({"msg_id": 5, "delivered_to": ["alpha"], "queued_for": []});
        let s = summarize("message_sent", &d);
        assert!(s.contains("delivered to alpha"), "{s}");
        assert!(!s.contains("queued for"), "{s}");
    }

    #[test]
    fn a_name_collision_is_visible_in_the_summary() {
        // caas -> caas#2 must be readable at a glance rather than inferred.
        let d = json!({"requested_name": "caas", "effective_name": "caas#2", "host": "nas"});
        let s = summarize("agent_registered", &d);
        assert!(s.contains("requested caas"), "{s}");
        assert!(s.contains("became caas#2"), "{s}");
    }

    #[test]
    fn an_uncollided_registration_does_not_claim_a_collision() {
        let d = json!({"requested_name": "caas", "effective_name": "caas", "host": "nas"});
        let s = summarize("agent_registered", &d);
        assert!(!s.contains("became"), "no collision happened: {s}");
        assert!(s.contains("nas"), "the host is still worth showing: {s}");
    }

    #[test]
    fn a_disconnect_summary_carries_the_reason() {
        let s = summarize(
            "agent_disconnected",
            &json!({"reason": "keepalive_timeout"}),
        );
        assert_eq!(s, "keepalive_timeout");
    }

    #[test]
    fn detailless_kinds_render_empty_rather_than_an_empty_object() {
        assert_eq!(summarize("room_joined", &json!({})), "");
        assert_eq!(summarize("resumed", &json!({})), "");
    }

    #[test]
    fn a_known_kind_carrying_unknown_fields_still_shows_them() {
        // summarize() reads only the fields it knows per kind. Without the fallback an
        // `ack` carrying anything else would render as a blank cell, making recorded
        // data invisible in the view built to audit it — silently dropping data is the
        // worse failure for a debugging tool.
        let d = json!({"tag": "from-caas"});
        assert_eq!(summarize("ack", &d), "", "no known field to summarize");
        assert!(
            detail_text("ack", &d).contains("from-caas"),
            "the payload must survive into what is rendered"
        );
    }

    #[test]
    fn the_detail_cell_keeps_the_raw_json_even_when_it_shows_a_summary() {
        // The summary must not cost fidelity: the exact payload stays available.
        let d = json!({"reason": "keepalive_timeout"});
        let cell = detail_cell("agent_disconnected", &d);
        assert!(cell.contains("keepalive_timeout"), "{cell}");
        assert!(
            cell.contains("title="),
            "raw JSON must be preserved: {cell}"
        );
        assert!(
            cell.contains("&quot;reason&quot;"),
            "escaped in the attr: {cell}"
        );
    }

    #[test]
    fn an_empty_detail_renders_an_empty_cell_with_no_title() {
        let cell = detail_cell("room_joined", &json!({}));
        assert!(!cell.contains("title="), "nothing to preserve: {cell}");
    }

    #[test]
    fn an_unknown_kind_falls_back_to_json_rather_than_hiding_it() {
        // A kind added later must stay legible before this function learns about it.
        let s = summarize("something_new", &json!({"a": 1}));
        assert!(s.contains("\"a\""), "{s}");
        assert_eq!(summarize("something_new", &json!({})), "");
    }

    #[test]
    fn a_summary_of_hostile_input_is_still_escaped_by_the_caller() {
        // summarize does not escape — it is the render sites' job. Assert the raw text
        // passes through so a future change that starts escaping here (double-escaping
        // at the call site) is caught.
        let s = summarize("agent_disconnected", &json!({"reason": "<script>"}));
        assert_eq!(s, "<script>");
        assert_eq!(esc(&s), "&lt;script&gt;");
    }
}
