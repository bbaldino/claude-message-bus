//! Read-only web views over the bus's own data. Performs no writes: it cannot be the
//! cause of a bug it is being used to investigate, and with no authentication on the
//! bus, anything this could do would be available to anything that can reach the port.

pub mod html;

use axum::extract::State;
use axum::response::Html;
use axum::{Router, routing::get};

use crate::bus::App;
use html::{esc, page};

pub fn routes() -> Router<App> {
    Router::new().route("/", get(overview))
}

async fn overview(State(app): State<App>) -> Html<String> {
    let agents = app.store.agents().await.unwrap_or_default();
    let rooms = app.store.rooms().await.unwrap_or_default();
    let events = app.store.events(20).await.unwrap_or_default();

    let mut b = String::new();
    b.push_str("<h1>overview</h1><h2>agents</h2><table>");
    for a in &agents {
        b.push_str(&format!(
            "<tr><td><a href=\"/agents/{n}\">{n}</a></td><td>{h}</td><td class=\"{c}\">{s}</td></tr>",
            n = esc(&a.name),
            h = esc(&a.host),
            c = if a.online { "" } else { "off" },
            s = if a.online { "online" } else { "offline" },
        ));
    }
    b.push_str("</table><h2>rooms</h2><table>");
    for r in &rooms {
        b.push_str(&format!(
            "<tr><td><a href=\"/rooms/{n}\">{n}</a></td><td>{m}</td></tr>",
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
