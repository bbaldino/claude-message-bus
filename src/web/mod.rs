//! Web views over the bus's own data.
//!
//! Read-only with exactly one exception: deleting an offline agent. Everything
//! else performs no writes, so the UI cannot be the cause of a bug it is being
//! used to investigate.
//!
//! The exception is deliberate and deliberately narrow. The bus has no
//! authentication and binds `0.0.0.0`, so anything this can do is available to
//! anything that can reach the port — which is why the delete refuses an agent
//! that is online, touches no messages or events, and records what it removed.
//! An unauthenticated caller can clear metadata for connections that are
//! already dead, and nothing more.
//!
//! Three things hold that boundary, and this doc is the canonical statement of
//! them:
//!
//! - **Offline only**, decided by the in-memory registry rather than the
//!   persisted `online` column, and held across the delete by
//!   `Registry::if_offline` so an agent cannot register into the gap between
//!   the check and the commit.
//! - **Known names only.** The `POST` looks the agent up itself; an unknown
//!   name renders a page and writes nothing, so the audit event cannot be
//!   forged and `events` cannot be grown by anyone who can reach the port.
//! - **Same-origin only.** A form POST is a "simple request" no browser
//!   preflights, so without a check any page the operator's browser loads could
//!   submit one — reaching a bus on `127.0.0.1` or behind NAT that is otherwise
//!   unreachable. A request whose `Origin` disagrees with the `Host` it was sent
//!   to is refused; one with no `Origin` at all (curl, scripts) is allowed,
//!   since it could already reach the port directly. This is not
//!   authentication: it narrows reach back to the network boundary the rest of
//!   the bus already assumes, nothing more.
//!
//! See `docs/superpowers/specs/2026-08-05-agent-delete-design.md`.

pub mod html;

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::response::Html;
use axum::{Router, routing::get};

use crate::bus::{App, Relayers};
use crate::store::AgentRow;
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
        "agent_deleted" => {
            let host = text("host");
            let mut s = format!("deleted {}", text("name"));
            if !host.is_empty() {
                s.push_str(&format!(" on {host}"));
            }
            // The agent row is gone, so this event is the only remaining record
            // of when it was last alive — dropping `last_seen` here would
            // destroy that irreversibly. `agents` is rendered for the same
            // reason in reverse: a delete that removed no agent row at all is
            // only self-evident in the log if the count is shown.
            if let Some(t) = num("last_seen") {
                s.push_str(&format!(" · last seen {}", fmt_time(t)));
            }
            s.push_str(&format!(
                " · {} agent rows, {} memberships, {} cursors",
                num("agents").unwrap_or(0),
                num("memberships").unwrap_or(0),
                num("cursors").unwrap_or(0),
            ));
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

/// Shorten a message body for a scannable table cell, on a character boundary.
///
/// Byte slicing would panic partway through a multi-byte character, and message bodies
/// are model output — non-ASCII is ordinary, not exotic. The full text is one click away
/// in the room transcript, so truncating here costs nothing.
fn truncate(s: &str, max: usize) -> String {
    let flat = s.replace('\n', " ");
    if flat.chars().count() <= max {
        return flat;
    }
    let kept: String = flat.chars().take(max).collect();
    format!("{}…", kept.trim_end())
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

/// The badge that distinguishes a person from a bot in an agent table.
///
/// A human is an ordinary agent carrying one boolean, which is what makes the rest of
/// the bus simple — but it also means nothing about the name, host, or cwd of a row
/// says whether anyone is actually reading it. Both tables that list agents render
/// this, so it lives here rather than being written out twice.
fn human_mark(is_human: bool) -> &'static str {
    if is_human {
        " <span class=\"human\">human</span>"
    } else {
        ""
    }
}

/// The badge marking an agent the bus is configured to accept relayed authority from.
///
/// Read from `App.relayers` at render time rather than from the agent's row: the grant is
/// bus configuration, not agent state, and a stored copy would disagree with the running
/// config the moment the flag changed.
fn relayer_mark(is_relayer: bool) -> &'static str {
    if is_relayer {
        " <span class=\"relayer\">relayer</span>"
    } else {
        ""
    }
}

/// The configured relayer set, for the note beneath each agent table.
///
/// Printed even when empty. A mistyped flag yields a set that badges nothing, which
/// without this line is indistinguishable from a correct configuration whose relayer
/// happens to be disconnected — so the line is what makes the mistake diagnosable.
fn relayer_note(relayers: &Relayers) -> String {
    let names = relayers.names();
    if names.is_empty() {
        "relayers: (none)".to_string()
    } else {
        format!("relayers: {}", esc(&names.join(", ")))
    }
}

/// How an agent's reported version renders, and whether it differs from this bus.
///
/// A differing version is the whole signal: Claude Code never respawns a stdio MCP
/// server, so a session started before an upgrade keeps its old binary until someone
/// restarts it, and this is what makes those sessions findable. `None` means a binary
/// predating the field, which is also worth flagging.
///
/// The badge says "differs from this bus", not "broken" — an agent built from a branch
/// would be flagged too, and the version shown beside it tells the reader which case
/// they are looking at.
fn version_cell(version: Option<&str>) -> String {
    let current = env!("CARGO_PKG_VERSION");
    match version {
        Some(v) if v == current => esc(v),
        Some(v) => format!("{} <span class=\"stale\">differs</span>", esc(v)),
        None => "unknown <span class=\"stale\">differs</span>".to_string(),
    }
}

/// One row of an agent table: name (linked), host, version, and online/offline state.
/// Shared by `overview()`'s `/` table and `agents()`'s `/agents` table — the two used to
/// carry byte-identical `format!` blocks maintained separately, which is exactly the
/// shape where a later change updates one table and not the other.
fn agent_row(a: &AgentRow, online: bool, is_relayer: bool) -> String {
    format!(
        "<tr><td><a href=\"/agents/{p}\">{n}</a>{mark}{relay}</td><td>{h}</td><td>{v}</td>\
         <td class=\"when\">{w}</td><td class=\"{c}\">{s}</td></tr>",
        p = encode_path_segment(&a.name),
        n = esc(&a.name),
        mark = human_mark(a.is_human),
        relay = relayer_mark(is_relayer),
        h = esc(&a.host),
        v = version_cell(a.version.as_deref()),
        w = esc(&fmt_time(a.last_seen)),
        c = if online { "" } else { "off" },
        s = if online { "online" } else { "offline" },
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
        .route(
            "/agents/{name}/delete",
            get(delete_agent_confirm).post(delete_agent_perform),
        )
        .route("/events", get(events_page))
}

async fn overview(State(app): State<App>) -> Html<String> {
    let agents = app.store.agents().await.unwrap_or_default();
    let rooms = app.store.rooms().await.unwrap_or_default();
    let messages = app.store.recent_messages(20).await.unwrap_or_default();
    let events = app.store.events(20).await.unwrap_or_default();

    // See `agents()`: liveness comes from the registry, not the persisted column.
    let live = app.registry.online().await;
    let mut b = String::new();
    b.push_str(
        "<h1>overview</h1><h2>agents</h2><table><tr><th>name<th>host<th>version<th>last seen<th>state</tr>",
    );
    for a in &agents {
        let online = live.contains(&a.name);
        b.push_str(&agent_row(a, online, app.relayers.contains(&a.name)));
    }
    b.push_str(&format!(
        "</table><p class=\"note\">this bus is running {} — {}</p>",
        esc(env!("CARGO_PKG_VERSION")),
        relayer_note(&app.relayers),
    ));
    b.push_str("<h2>rooms</h2><table><tr><th>room<th>members</tr>");
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
    // What is actually being said, not just that something was said. The event log
    // records that a message was sent and whether it was delivered or queued, but not
    // its text — so without this the overview can't answer "what are they talking
    // about" without guessing a room to click into.
    b.push_str(
        "</table><h2>recent messages <span class=\"note\">newest first</span></h2>\
         <table><tr><th>when<th>room<th>from<th>message</tr>",
    );
    for m in &messages {
        b.push_str(&format!(
            "<tr><td class=\"when\">{w}</td><td><a href=\"/rooms/{p}\">{r}</a></td>\
             <td>{f}</td><td>{t}</td></tr>",
            w = esc(&fmt_time(m.created_at)),
            p = encode_path_segment(&m.room),
            r = esc(&m.room),
            f = esc(&m.from_agent),
            t = esc(&truncate(&m.body, 90)),
        ));
    }
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
    let mut b = String::from(
        "<h1>agents</h1><table><tr><th>name<th>host<th>version<th>last seen<th>state</tr>",
    );
    for a in &agents {
        let online = live.contains(&a.name);
        b.push_str(&agent_row(a, online, app.relayers.contains(&a.name)));
    }
    b.push_str("</table>");
    b.push_str(&format!(
        "<p class=\"note\">this bus is running {} — {}</p>",
        esc(env!("CARGO_PKG_VERSION")),
        relayer_note(&app.relayers),
    ));
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

    let mut b = format!(
        "<h1>{n}</h1><p><a href=\"/agents/{p}/delete\">delete this agent</a></p>\
         <h2>rooms</h2><ul>",
        n = esc(&name),
        p = encode_path_segment(&name),
    );
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

/// A delete page: body wrapped in the shared chrome, titled with the agent's
/// name rather than a generic "delete agent" so a tab full of these is
/// distinguishable — matching the neighbouring `agent()` handler.
fn delete_page(name: &str, body: &str) -> Html<String> {
    Html(page(&format!("delete {name}"), body))
}

/// The page both delete routes render for a name that has no `agents` row.
///
/// Shared so the `GET` and the `POST` cannot drift: the `POST` needs it not
/// just for tidiness but for correctness — without the guard, any name at all
/// deletes nothing and still writes an `agent_deleted` event, forging the one
/// record this feature promises will outlive the agent.
fn no_such_agent(name: &str) -> Html<String> {
    delete_page(
        name,
        &format!("<h1>delete agent</h1><p>no agent named {}</p>", esc(name)),
    )
}

/// The refusal page for an agent that is connected right now.
fn agent_is_online_page(name: &str, why: &str) -> Html<String> {
    delete_page(
        name,
        &format!(
            "<h1>delete {n}</h1><p>{why}</p><p><a href=\"/agents/{p}\">back</a></p>",
            n = esc(name),
            p = encode_path_segment(name),
        ),
    )
}

/// Confirmation page for deleting an agent. Renders the blast radius before
/// anything is removed, and renders no button at all when the agent is online —
/// a button known to fail is worse than none.
async fn delete_agent_confirm(State(app): State<App>, Path(name): Path<String>) -> Html<String> {
    let known = app
        .store
        .agents()
        .await
        .unwrap_or_default()
        .into_iter()
        .any(|a| a.name == name);
    if !known {
        return no_such_agent(&name);
    }

    if app.registry.is_online(&name).await {
        return agent_is_online_page(
            &name,
            &format!(
                "{n} is online. Only offline agents can be deleted — deleting a connected \
                 agent would drop the room memberships it is still receiving messages through.",
                n = esc(&name),
            ),
        );
    }

    // A failed read must not render as an empty blast radius. This listing is
    // the only safeguard on the action, and `unwrap_or_default` here would make
    // a database error indistinguishable from "this agent belongs to nothing"
    // while still offering the button.
    let fp = match app.store.agent_footprint(&name).await {
        Ok(fp) => fp,
        Err(e) => {
            return delete_page(
                &name,
                &format!(
                    "<h1>delete {n}</h1><p>could not read what deleting {n} would remove: \
                     {e}</p><p>Nothing was deleted. Without that listing there is no way to \
                     see the blast radius, so no button is offered.</p>\
                     <p><a href=\"/agents/{p}\">back</a></p>",
                    n = esc(&name),
                    e = esc(&e.to_string()),
                    p = encode_path_segment(&name),
                ),
            );
        }
    };

    let mut b = format!("<h1>delete {}</h1>", esc(&name));
    b.push_str("<h2>this will remove</h2><ul>");
    b.push_str(&format!("<li>the agent row for {}</li>", esc(&name)));
    for r in &fp.rooms {
        b.push_str(&format!("<li>membership of room {}</li>", esc(r)));
    }
    b.push_str(&format!(
        "<li>{n} cursor{s}</li></ul>",
        n = fp.cursors,
        s = if fp.cursors == 1 { "" } else { "s" },
    ));
    b.push_str(
        "<p class=\"note\">messages and events are kept: room transcripts stay \
                readable and the audit trail outlives the agent.</p>",
    );
    b.push_str(&format!(
        "<form method=\"post\" action=\"/agents/{p}/delete\">\
         <button type=\"submit\">delete {n}</button></form>\
         <p><a href=\"/agents/{p}\">cancel</a></p>",
        p = encode_path_segment(&name),
        n = esc(&name),
    ));
    delete_page(&name, &b)
}

/// Whether a request's `Origin` names the same origin as the `Host` it was sent
/// to, i.e. whether this POST came from a page served by this bus.
///
/// Compared against `Host` rather than a configured origin so the bus still
/// works behind a reverse proxy or under any name the operator reaches it by.
/// Anything that is not an `http`/`https` origin — including the literal
/// `null` a sandboxed frame sends — is not same-origin and is refused.
fn origin_matches_host(origin: &str, host: &str) -> bool {
    let (scheme, authority) = match origin.split_once("://") {
        Some(("http", a)) => ("http", a),
        Some(("https", a)) => ("https", a),
        _ => return false,
    };
    if authority == host {
        return true;
    }
    // A browser omits the default port from `Origin`; a `Host` header may still
    // carry it (and vice versa), and those two spellings are the same origin.
    let default_port = if scheme == "https" { ":443" } else { ":80" };
    host.strip_suffix(default_port) == Some(authority)
        || authority.strip_suffix(default_port) == Some(host)
}

/// Perform the delete.
///
/// Three guards, in the order that lets each one narrow what the next has to
/// handle:
///
/// 1. **Cross-origin.** An HTML form POST is a "simple request": any page a
///    browser loads can auto-submit one at this URL with no preflight, which
///    would extend the bus's reach from "anything that can reach the port" to
///    "anything the operator's browser will load" — including a bus on
///    `127.0.0.1` or behind NAT. A request with *no* `Origin` (curl, the test
///    harness) is allowed: those were already able to reach the port directly,
///    so refusing them buys nothing.
/// 2. **Unknown name.** Looked up once, here, rather than trusted from the
///    `GET`: it is what stops any name at all from forging an `agent_deleted`
///    event, and the row it returns supplies the `host` and `last_seen` the
///    event must carry — the last trace of when this agent was alive.
/// 3. **Liveness**, held across the delete by `Registry::if_offline`. Checking
///    and then deleting is a race an agent can lose: `attach` inserts into the
///    registry *before* the `agents` row is written, so a `Register` landing in
///    between would leave a live agent with nothing in the database.
async fn delete_agent_perform(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::http::header;
    use axum::response::IntoResponse;

    let header_str = |h: header::HeaderName| {
        headers
            .get(h)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };

    if let Some(origin) = header_str(header::ORIGIN) {
        let host = header_str(header::HOST).unwrap_or_default();
        if !origin_matches_host(&origin, &host) {
            return delete_page(
                &name,
                &format!(
                    "<h1>delete {n}</h1><p>nothing was deleted: this request came from \
                     {o}, which is not this bus. A delete must be submitted from a page \
                     this bus served.</p><p><a href=\"/agents/{p}\">back</a></p>",
                    n = esc(&name),
                    o = esc(&origin),
                    p = encode_path_segment(&name),
                ),
            )
            .into_response();
        }
    }

    let row = match app.store.agents().await {
        Ok(rows) => rows.into_iter().find(|a| a.name == name),
        Err(e) => {
            return delete_page(
                &name,
                &format!(
                    "<h1>delete {n}</h1><p>nothing was deleted: {e}</p>",
                    n = esc(&name),
                    e = esc(&e.to_string()),
                ),
            )
            .into_response();
        }
    };
    let Some(row) = row else {
        return no_such_agent(&name).into_response();
    };

    // The store call inside must not touch the registry — the connection lock
    // is held for its whole duration.
    let deleted = app
        .registry
        .if_offline(&name, || app.store.forget_agent(&name))
        .await;

    let Some(result) = deleted else {
        return agent_is_online_page(
            &name,
            &format!(
                "{n} came online while this page was open, so nothing was deleted. \
                 Only offline agents can be deleted.",
                n = esc(&name),
            ),
        )
        .into_response();
    };

    match result {
        Ok(counts) => {
            // The only surviving record that this agent ever existed, so a
            // failure to write it is worth saying out loud — `eprintln!` to
            // match the disconnect paths next door.
            if let Err(e) = app
                .store
                .append_event(
                    "agent_deleted",
                    Some(&name),
                    None,
                    serde_json::json!({
                        "name": name,
                        "host": row.host,
                        "last_seen": row.last_seen,
                        "agents": counts.agents,
                        "memberships": counts.memberships,
                        "cursors": counts.cursors,
                    }),
                )
                .await
            {
                eprintln!("agent_deleted event for {name} was not recorded: {e}");
            }
            axum::response::Redirect::to("/agents").into_response()
        }
        Err(e) => delete_page(
            &name,
            &format!(
                "<h1>delete {n}</h1><p>nothing was deleted: {e}</p>",
                n = esc(&name),
                e = esc(&e.to_string()),
            ),
        )
        .into_response(),
    }
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
    fn the_delete_summary_keeps_the_last_trace_of_a_deleted_agent() {
        // The agents row is gone by the time this renders, so last_seen and the
        // agent-row count exist nowhere else. Dropping either from the summary
        // makes the log unable to answer "when was it last alive" or "did that
        // delete actually remove an agent".
        let d = json!({
            "name": "network-debug#2", "host": "hardac", "last_seen": 1_785_000_000_123i64,
            "agents": 1, "memberships": 2, "cursors": 3,
        });
        let s = summarize("agent_deleted", &d);
        assert!(s.contains("network-debug#2"), "{s}");
        assert!(s.contains("hardac"), "{s}");
        assert!(s.contains("last seen"), "{s}");
        assert!(s.contains("1 agent rows"), "{s}");
        assert!(s.contains("2 memberships"), "{s}");
        assert!(s.contains("3 cursors"), "{s}");
    }

    #[test]
    fn a_same_origin_form_post_is_accepted() {
        assert!(origin_matches_host("http://nas:7777", "nas:7777"));
        assert!(origin_matches_host("https://bus.example", "bus.example"));
        // A browser omits the default port from Origin; Host may still carry it.
        assert!(origin_matches_host("http://nas", "nas:80"));
        assert!(origin_matches_host("https://nas", "nas:443"));
    }

    #[test]
    fn a_cross_origin_form_post_is_refused() {
        // The attack: a page the operator's browser loads auto-submits a form
        // at a bus it could not otherwise reach.
        assert!(!origin_matches_host("http://evil.example", "nas:7777"));
        // Same host, different port is a different origin.
        assert!(!origin_matches_host("http://nas:8080", "nas:7777"));
        // Scheme matters, and so does the default-port pairing.
        assert!(!origin_matches_host("http://nas", "nas:443"));
        // A sandboxed frame sends this, and it is not this bus.
        assert!(!origin_matches_host("null", "nas:7777"));
        assert!(!origin_matches_host("file://", "nas:7777"));
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
