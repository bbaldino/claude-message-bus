//! The bus's HTTP surface: the JSON API under `/api`, the HTTP participant
//! endpoints, and the single-page app, which answers every other `GET`.
//!
//! The console's writes are deliberately few — hiding a room, and deleting an
//! offline agent — so the UI cannot be the cause of a bug it is being used to
//! investigate.
//!
//! The delete is the only one that removes anything, and it is deliberately
//! narrow. The bus has no
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
//! - **Known names only.** The `DELETE` looks the agent up itself; an unknown
//!   name writes nothing, so the audit event cannot be forged and `events`
//!   cannot be grown by anyone who can reach the port.
//! - **Same-origin only.** A request whose `Origin` disagrees with the `Host`
//!   it was sent to is refused; one with no `Origin` at all (curl, scripts) is
//!   allowed, since it could already reach the port directly. This is not
//!   authentication: it narrows reach back to the network boundary the rest of
//!   the bus already assumes, nothing more.
//!
//! See `docs/superpowers/specs/2026-08-05-agent-delete-design.md`.

mod api;
mod assets;
mod participants;

use axum::{
    Router,
    routing::{get, post},
};

use crate::bus::App;

/// Whether a request's `Origin` names the same origin as the `Host` it was sent
/// to, i.e. whether this POST came from a page served by this bus.
///
/// Compared against `Host` rather than a configured origin so the bus still
/// works behind a reverse proxy or under any name the operator reaches it by.
/// Anything that is not an `http`/`https` origin — including the literal
/// `null` a sandboxed frame sends — is not same-origin and is refused.
pub(crate) fn origin_matches_host(origin: &str, host: &str) -> bool {
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

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/agents", get(api::agents))
        .route(
            "/api/agents/{name}",
            get(api::agent_detail).delete(api::agent_delete),
        )
        .route(
            "/api/agents/{name}/deletion",
            get(api::agent_deletion_preview),
        )
        .route("/api/rail", get(api::rail))
        .route("/api/meta", get(api::meta))
        .route("/api/rooms/{name}/messages", get(api::room_messages))
        .route("/api/rooms/{name}/files", get(api::room_files))
        .route("/api/rooms/{name}/hidden", post(api::room_set_hidden))
        .route("/api/events", get(api::events))
        .route("/api/participants", post(participants::register))
        .route("/api/participants/send", post(participants::send))
        .route("/api/participants/receive", get(participants::receive))
        .route("/api/participants/resume", post(participants::resume))
        // The console used to live under `/app` while the server-rendered pages
        // held `/`. Bookmarks and reverse-proxy configs from that era still point
        // there, so send them to the same place at the root rather than 404ing.
        // `/app/` is registered separately because matchit requires a non-empty
        // remainder for a catch-all.
        .route("/app", get(assets::legacy_app_redirect))
        .route("/app/", get(assets::legacy_app_redirect))
        .route("/app/{*rest}", get(assets::legacy_app_redirect))
        // Static routes above (and `/ws`, `/human-active` in `bus`) outrank the
        // catch-all, so this only sees paths nothing else claimed. `/` needs its
        // own route for the same matchit reason as `/app/`.
        .route("/", get(assets::app_root))
        .route("/{*rest}", get(assets::app_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_same_origin_request_is_accepted() {
        assert!(origin_matches_host("http://nas:7777", "nas:7777"));
        assert!(origin_matches_host("https://bus.example", "bus.example"));
        // A browser omits the default port from Origin; Host may still carry it.
        assert!(origin_matches_host("http://nas", "nas:80"));
        assert!(origin_matches_host("https://nas", "nas:443"));
    }

    #[test]
    fn a_cross_origin_request_is_refused() {
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
}
