//! Read-only web views over the bus's own data. Performs no writes: it cannot be the
//! cause of a bug it is being used to investigate, and with no authentication on the
//! bus, anything this could do would be available to anything that can reach the port.

pub mod html;

use axum::extract::{Path, State};
use axum::response::Html;
use axum::{Router, routing::get};

use crate::bus::App;
use html::{encode_path_segment, esc, page};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/", get(overview))
        .route("/rooms", get(rooms))
        .route("/rooms/{name}", get(room))
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

    let mut b = format!("<h1>{}</h1><table>", esc(&name));
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
