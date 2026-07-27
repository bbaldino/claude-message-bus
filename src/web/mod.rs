//! Read-only web views over the bus's own data. Performs no writes: it cannot be the
//! cause of a bug it is being used to investigate, and with no authentication on the
//! bus, anything this could do would be available to anything that can reach the port.

pub mod html;

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::response::Html;
use axum::{Router, routing::get};

use crate::bus::App;
use html::{encode_path_segment, esc, page};

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

    let mut b = String::new();
    b.push_str("<h1>overview</h1><h2>agents</h2><table>");
    for a in &agents {
        b.push_str(&format!(
            "<tr><td><a href=\"/agents/{p}\">{n}</a></td><td>{h}</td><td class=\"{c}\">{s}</td></tr>",
            p = encode_path_segment(&a.name),
            n = esc(&a.name),
            h = esc(&a.host),
            c = if a.online { "" } else { "off" },
            s = if a.online { "online" } else { "offline" },
        ));
    }
    b.push_str("</table><h2>rooms</h2><table>");
    for r in &rooms {
        b.push_str(&format!(
            "<tr><td><a href=\"/rooms/{p}\">{n}</a></td><td>{m}</td></tr>",
            p = encode_path_segment(&r.name),
            n = esc(&r.name),
            m = esc(&r.members.join(", ")),
        ));
    }
    b.push_str("</table><h2>recent events</h2><table>");
    for e in &events {
        b.push_str(&format!(
            "<tr><td>{k}</td><td>{a}</td><td>{r}</td></tr>",
            k = esc(&e.kind),
            a = esc(e.agent.as_deref().unwrap_or("")),
            r = esc(e.room.as_deref().unwrap_or("")),
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
        entries.push(Entry::Event {
            at: e.created_at,
            kind: e.kind,
            detail: e.detail.to_string(),
        });
    }
    // The whole point of the page: one timeline, not two lists. See `Entry::rank` for
    // how same-millisecond ties are broken.
    entries.sort_by_key(|e| (e.at(), e.rank()));

    let mut b = format!(
        "<h1>{n}</h1><p><a href=\"/rooms/{p}/files\">files</a></p><table>",
        n = esc(&name),
        p = encode_path_segment(&name),
    );
    for e in &entries {
        match e {
            Entry::Message { from, body, .. } => b.push_str(&format!(
                "<tr><td>{f}</td><td class=\"msg\">{t}</td></tr>",
                f = esc(from),
                t = esc(body),
            )),
            Entry::Event { kind, detail, .. } => b.push_str(&format!(
                "<tr><td class=\"ev\">{k}</td><td class=\"ev\">{d}</td></tr>",
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
    let mut b = String::from("<h1>agents</h1><table><tr><th>name<th>host<th>state</tr>");
    for a in &agents {
        b.push_str(&format!(
            "<tr><td><a href=\"/agents/{p}\">{n}</a></td><td>{h}</td><td class=\"{c}\">{s}</td></tr>",
            p = encode_path_segment(&a.name),
            n = esc(&a.name),
            h = esc(&a.host),
            c = if a.online { "" } else { "off" },
            s = if a.online { "online" } else { "offline" },
        ));
    }
    b.push_str("</table>");
    Html(page("agents", &b))
}

async fn agent(State(app): State<App>, Path(name): Path<String>) -> Html<String> {
    let rooms = app.store.rooms().await.unwrap_or_default();
    let mine: Vec<&str> = rooms
        .iter()
        .filter(|r| r.members.iter().any(|m| *m == name))
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
    b.push_str("</ul><h2>activity</h2><table><tr><th>kind<th>room<th>detail</tr>");
    for e in &evs {
        b.push_str(&format!(
            "<tr><td>{k}</td><td>{r}</td><td>{d}</td></tr>",
            k = esc(&e.kind),
            r = esc(e.room.as_deref().unwrap_or("")),
            d = esc(&e.detail.to_string()),
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

    let mut b = String::from("<h1>events</h1><table><tr><th>kind<th>agent<th>room<th>detail</tr>");
    for e in &evs {
        b.push_str(&format!(
            "<tr><td><a href=\"/events?kind={kp}\">{k}</a></td><td>{a}</td><td>{r}</td><td>{d}</td></tr>",
            // A query *value* isn't a path segment, but percent-encoding it against the
            // same unreserved set (`encode_path_segment`) is still correct here: that
            // encoding escapes every byte outside `A-Za-z0-9-._~`, which is a superset
            // of what a query value strictly requires (at minimum `&`, `=`, `#`) — and
            // over-escaping a URL component is always valid per RFC 3986, just more
            // verbose than the minimal query-specific escaping would be. Reusing it
            // avoids adding a near-duplicate helper for one call site.
            kp = encode_path_segment(&e.kind),
            k = esc(&e.kind),
            a = esc(e.agent.as_deref().unwrap_or("")),
            r = esc(e.room.as_deref().unwrap_or("")),
            d = esc(&e.detail.to_string()),
        ));
    }
    b.push_str("</table>");
    Html(page("events", &b))
}

async fn rooms(State(app): State<App>) -> Html<String> {
    let rooms = app.store.rooms().await.unwrap_or_default();
    let mut b = String::from("<h1>rooms</h1><table>");
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
