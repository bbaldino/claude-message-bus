//! The bus server. Owns the registry, rooms, message log, and file store.
//! Knows nothing about MCP — it speaks only the `proto` wire types.

pub(crate) mod commands;
pub mod delivery;
pub mod registry;
pub mod rooms;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::{Router, routing::get};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;

use crate::proto::{FromBus, ReplyResult, RoomUnread, ToBus};
use crate::store::Store;
use delivery::Guards;
use registry::Registry;

/// How often the bus pings each connected client, and how long it waits for
/// a pong before deciding the peer is gone.
///
/// Nothing on either side sent a `Ping` before this fix, so a peer whose
/// host vanished hard (lid closed, cable pulled, NAT black-holing) stayed
/// "online" — and kept being reported as `delivered` — until TCP itself
/// noticed, which is minutes away by default and can be indefinite behind a
/// black-holing NAT. This is what closes that gap.
///
/// `ping_interval` = 30s / `pong_timeout` = 90s (3 missed pings) for the
/// production default: frequent enough that a vanished peer is caught well
/// within a human's patience for "is this thing on", generous enough
/// (3 full cycles, not 1) that a busy-but-alive agent — one whose task is
/// momentarily starving the executor that would otherwise answer a ping —
/// is never mistaken for a dead one. `tokio-tungstenite` (both the axum
/// server side and the `tokio-tungstenite` client side used by
/// `agent::bridge` and `tail`) answers `Ping` with `Pong` automatically as
/// long as something keeps polling the stream, which both clients already
/// do in their main read loops — no client-side change was needed for pongs
/// to happen.
///
/// The timeout is only *re-checked* once per `ping_interval` (see
/// `timeout_ticker` in `connection`), not the instant `pong_timeout`
/// elapses, so the worst-case time to detect a vanished peer is
/// `pong_timeout + ping_interval` — up to ~120s at the production default,
/// not 90s.
#[derive(Clone, Copy)]
pub struct Keepalive {
    pub ping_interval: Duration,
    pub pong_timeout: Duration,
}

impl Default for Keepalive {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            pong_timeout: Duration::from_secs(90),
        }
    }
}

impl Keepalive {
    pub fn new(ping_interval: Duration, pong_timeout: Duration) -> Self {
        Self {
            ping_interval,
            pong_timeout,
        }
    }
}

/// Capacity of a connection's *control* channel — the queue for replies to
/// this connection's own commands (`Reply`, `Error`, `Registered`, `Unread`,
/// `Paused`), as opposed to `registry::CHANNEL_CAPACITY`, which bounds the
/// *routing* channel other connections fan messages into via
/// `Registry::send_to`.
///
/// These used to be the same channel. That let pressure on an agent as a
/// *recipient* (other agents' room fan-out filling its routing queue) starve
/// replies to that same agent's own outstanding requests as a *sender* —
/// the tool call would then time out after 10s and misreport the bus as
/// unreachable, exactly the lie the delivered/queued split exists to
/// prevent. Splitting the two channels means routing pressure can never
/// drop a reply.
///
/// Control traffic is strictly request/response: in the steady state there
/// is at most one outstanding reply per in-flight command, plus at most two
/// events for a single `Send` that trips the pause guard (`Paused` then its
/// resolving `Error`, sent back to back with nothing draining between
/// them). 16 comfortably covers Register + Registered, the single coalesced
/// reconnect `Unread` summary (see `send_unread_summaries` — one event for
/// the whole connection, not one per room), plus several commands pipelined
/// ahead of their replies, without ever growing unbounded — control traffic
/// originates from this connection's own actions, so it can't be driven
/// past that by another agent the way the routing queue can.
const CONTROL_CHANNEL_CAPACITY: usize = 16;

#[derive(Clone)]
pub(crate) struct App {
    pub(crate) store: Arc<Store>,
    pub(crate) registry: Registry,
    pub(crate) guards: Guards,
    pub(crate) keepalive: Keepalive,
}

pub async fn serve(port: u16, data_dir: PathBuf) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    eprintln!("claude-bus listening on 0.0.0.0:{port}");
    serve_on(listener, data_dir).await
}

/// Split out so tests can bind port 0 and learn the assigned port.
pub async fn serve_on(listener: tokio::net::TcpListener, data_dir: PathBuf) -> anyhow::Result<()> {
    serve_on_with(listener, data_dir, Guards::default()).await
}

/// Guards are injected rather than read from configuration so tests can disable
/// the rate limit without the production path branching on a test-only signal.
pub async fn serve_on_with(
    listener: tokio::net::TcpListener,
    data_dir: PathBuf,
    guards: Guards,
) -> anyhow::Result<()> {
    serve_on_with_keepalive(listener, data_dir, guards, Keepalive::default()).await
}

/// Keepalive is injected the same way `Guards` is: production gets the real
/// 30s/90s cadence, tests get millisecond-scale intervals so the "a vanished
/// peer stops being reported as online" behavior is testable without
/// sleeping for the production timeout.
pub async fn serve_on_with_keepalive(
    listener: tokio::net::TcpListener,
    data_dir: PathBuf,
    guards: Guards,
    keepalive: Keepalive,
) -> anyhow::Result<()> {
    serve_on_full(listener, data_dir, guards, keepalive, Registry::new()).await
}

/// `Registry` is injected for the same reason `Guards` and `Keepalive` are:
/// it lets a test reach in and call `Registry::send_to` directly against a
/// connection the running bus has live, without needing to win a race over
/// the wire against the writer task that's draining it. That is what proves
/// a full *routing* queue (other connections' fan-out) can never starve the
/// same connection's own *control*-channel replies.
pub async fn serve_on_full(
    listener: tokio::net::TcpListener,
    data_dir: PathBuf,
    guards: Guards,
    keepalive: Keepalive,
    registry: Registry,
) -> anyhow::Result<()> {
    let app = App {
        store: Arc::new(Store::open(&data_dir).await?),
        registry,
        guards,
        keepalive,
    };
    let router = Router::new()
        .route("/ws", get(upgrade))
        .route("/human-active", axum::routing::post(human_active))
        .merge(crate::web::routes())
        .with_state(app);
    axum::serve(listener, router).await?;
    Ok(())
}

#[derive(serde::Deserialize)]
struct HumanActiveQuery {
    agent: String,
}

/// Called by the optional UserPromptSubmit hook. The human typing is the only
/// accurate signal that a conversation is still supervised.
async fn human_active(
    State(app): State<App>,
    axum::extract::Query(q): axum::extract::Query<HumanActiveQuery>,
) -> &'static str {
    let rooms: Vec<String> = app
        .store
        .rooms()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.members.contains(&q.agent))
        .map(|r| r.name)
        .collect();
    app.guards.reset_all_for(&rooms).await;
    "ok"
}

async fn upgrade(ws: WebSocketUpgrade, State(app): State<App>) -> Response {
    ws.on_upgrade(move |socket| connection(socket, app))
}

async fn connection(socket: WebSocket, app: App) {
    let (mut sink, mut stream) = socket.split();

    // Two separate queues, not one. `routing_tx` is what `Registry` hands
    // out to *other* connections via `send_to` — it carries fan-out
    // `Message` events addressed to this agent. `control_tx` is used only
    // by this connection's own read loop below, for replies to this
    // connection's own commands. Keeping them apart means a flood of
    // inbound routing traffic (this agent being a popular recipient) can
    // never fill the queue a reply to this agent's *own* request needs to
    // land in — see `CONTROL_CHANNEL_CAPACITY` for why that matters.
    let (routing_tx, mut routing_rx) = mpsc::channel::<FromBus>(registry::CHANNEL_CAPACITY);
    let (control_tx, mut control_rx) = mpsc::channel::<FromBus>(CONTROL_CHANNEL_CAPACITY);

    // The writer task owns the sink for the connection's whole lifetime, so
    // it — not the read loop below — is the one that actually emits `Ping`
    // frames on a timer, interleaved with real `FromBus` events drained from
    // both queues. Control traffic is preferred over routing traffic when
    // both are ready: a reply that unblocks a waiting tool call matters more
    // than another routed message, and briefly starving routing traffic is
    // harmless — `Registry::send_to`'s boolean already reports that
    // honestly as `queued`.
    let ping_interval = app.keepalive.ping_interval;
    let writer = tokio::spawn(async move {
        let mut ping_ticker = tokio::time::interval(ping_interval);
        ping_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping_ticker.tick().await; // the first tick fires immediately; skip it
        loop {
            // `ping_ticker` goes first in this `biased` order even though
            // it's the *least* important branch to service. Under `biased`,
            // an earlier arm being (usually) `Pending` is what lets later
            // arms run at all — polling the near-always-`Pending` timer
            // first costs nothing, whereas putting a channel first would
            // mean: on a connection with sustained fan-out and no idle gap,
            // `control_rx`/`routing_rx` are *always* ready, so the ticker
            // arm would never even get polled, its timer would never fire,
            // no `Ping` would ever go out, no `Pong` would ever come back,
            // and `timeout_ticker` in the read loop below would eventually
            // tear down a connection that was alive and merely busy. Control
            // is still preferred over routing whenever both have data —
            // that ordering is unaffected, since the timer arm is a no-op
            // the overwhelming majority of the time it's polled.
            let event = tokio::select! {
                biased;
                _ = ping_ticker.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                    continue;
                }
                event = control_rx.recv() => event,
                event = routing_rx.recv() => event,
            };
            let Some(event) = event else { break };
            let Ok(json) = serde_json::to_string(&event) else {
                continue;
            };
            if sink.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    let mut me: Option<String> = None;
    // Set instead of `me` when this connection identified via `Observe`
    // rather than `Register`. The two are mutually exclusive for the
    // connection's whole lifetime — see the guards in the `Register` and
    // `Observe` handling below. Kept as an opaque `ObserverId`, not a name:
    // `handle_observer` never needs a display string, only the registry
    // token that `watch`/`notify_watchers` key off of.
    let mut observer: Option<registry::ObserverId> = None;

    // A peer that stops answering pings must not be allowed to keep the read
    // loop parked in `stream.next().await` forever — on a black-holed
    // connection that read may never return on its own, TCP alone can take
    // ~15 minutes to notice by default, and indefinitely behind a
    // black-holing NAT. `timeout_ticker` fires independently of whatever
    // `stream.next()` is doing, so the peer is judged solely on whether a
    // pong actually arrived, not on whether the socket happens to wake up.
    let mut last_pong = tokio::time::Instant::now();
    let mut timeout_ticker = tokio::time::interval(app.keepalive.ping_interval);
    timeout_ticker.tick().await; // skip the immediate first tick

    // Which of the two ways out of the loop below actually happened, for the
    // `agent_disconnected` event at teardown. Defaults to the ordinary case
    // (the socket closing); the keepalive-timeout branch overwrites it right
    // before its own `break` so a ghost agent (one whose socket vanished
    // without closing) is distinguishable in the log from a clean hangup.
    let mut disconnect_reason: &'static str = "socket_closed";

    loop {
        tokio::select! {
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break };
                let text = match msg {
                    Message::Pong(_) => {
                        last_pong = tokio::time::Instant::now();
                        continue;
                    }
                    Message::Text(text) => text,
                    _ => continue,
                };

                let cmd: ToBus = match serde_json::from_str(&text) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = control_tx.try_send(FromBus::Error {
                            req_id: None,
                            message: format!("unparseable command: {e}"),
                        });
                        continue;
                    }
                };

                if let ToBus::Register {
                    name,
                    host,
                    cwd,
                    session_id,
                } = &cmd
                {
                    // A connection registers exactly once. Accepting a second
                    // Register would mint a fresh effective name via `attach` (e.g.
                    // `caas#2`) while leaving the original identity's connection
                    // entry in the registry untouched — that first name would never
                    // be detached, so it would stay "online" forever, silently
                    // swallowing anything addressed to it after this socket closes.
                    if let Some(existing) = &me {
                        let _ = control_tx.try_send(FromBus::Error {
                            req_id: None,
                            message: format!(
                                "already registered as {existing}; a connection may only register once"
                            ),
                        });
                        continue;
                    }
                    if observer.is_some() {
                        let _ = control_tx.try_send(FromBus::Error {
                            req_id: None,
                            message:
                                "already identified as an observer; a connection may only \
                                 identify once"
                                    .into(),
                        });
                        continue;
                    }

                    // `Registry` only ever needs the routing sender: it is
                    // what other connections use to fan messages in, never
                    // to answer this connection's own commands.
                    let effective = app.registry.attach(name, host, routing_tx.clone()).await;
                    let _ = app
                        .store
                        .upsert_agent(&effective, host, cwd, session_id.as_deref())
                        .await;
                    let _ = app
                        .store
                        .append_event(
                            "agent_registered",
                            Some(&effective),
                            None,
                            json!({
                                "requested_name": name,
                                "effective_name": &effective,
                                "host": host,
                                "session_id": session_id,
                            }),
                        )
                        .await;
                    me = Some(effective.clone());
                    let _ = control_tx.try_send(FromBus::Registered {
                        name: effective.clone(),
                    });
                    send_unread_summaries(&app, &effective, &control_tx).await;
                    continue;
                }

                // A viewer is not a participant: `Observe` gives a connection
                // an identity for its lifetime (satisfying "register before
                // sending commands" below) without ever calling `Store` or
                // `Registry::attach` — no `agents` row, no name that could
                // collide with or consume a suffix meant for a real agent.
                // See `ObserverId` and `Registry::attach_observer`.
                if let ToBus::Observe { name } = &cmd {
                    if let Some(existing) = &me {
                        let _ = control_tx.try_send(FromBus::Error {
                            req_id: None,
                            message: format!(
                                "already registered as {existing}; a connection may only \
                                 identify once"
                            ),
                        });
                        continue;
                    }
                    if observer.is_some() {
                        let _ = control_tx.try_send(FromBus::Error {
                            req_id: None,
                            message:
                                "already identified as an observer; a connection may only \
                                 identify once"
                                    .into(),
                        });
                        continue;
                    }

                    let id = app.registry.attach_observer(routing_tx.clone()).await;
                    observer = Some(id);
                    eprintln!("observer connected: {name}");
                    let _ = control_tx.try_send(FromBus::Observing { name: name.clone() });
                    continue;
                }

                let Some(name) = me.clone() else {
                    if let Some(id) = observer {
                        handle_observer(&app, id, cmd, &control_tx).await;
                        continue;
                    }
                    let _ = control_tx.try_send(FromBus::Error {
                        req_id: None,
                        message: "register before sending commands".into(),
                    });
                    continue;
                };

                commands::handle(&app, &name, cmd, &control_tx).await;
            }
            _ = timeout_ticker.tick() => {
                if last_pong.elapsed() > app.keepalive.pong_timeout {
                    if let Some(name) = &me {
                        eprintln!(
                            "keepalive timeout: no pong from {name} within {:?}, closing",
                            app.keepalive.pong_timeout
                        );
                    }
                    disconnect_reason = "keepalive_timeout";
                    break;
                }
            }
        }
    }

    if let Some(name) = me {
        app.registry.detach(&name).await;
        let _ = app.store.set_online(&name, false).await;
        let _ = app
            .store
            .append_event(
                "agent_disconnected",
                Some(&name),
                None,
                json!({ "reason": disconnect_reason }),
            )
            .await;
        eprintln!("disconnected: {name}");
    } else if let Some(id) = observer {
        // No `Store` state was ever created for an observer (see
        // `Registry::attach_observer`), so unlike an agent's teardown above
        // there is nothing to mark offline anywhere — dropping the in-memory
        // registry entry is the whole story. Nothing in `agents` or
        // `room_members` ever mentioned this connection to begin with.
        app.registry.detach_observer(id).await;
        eprintln!("observer disconnected");
    }
    writer.abort();
}

/// On reconnect an agent gets counts, never the backlog: replaying yesterday's
/// conversation into a fresh session wastes context and derails whatever the
/// human actually sat down to do.
async fn send_unread_summaries(app: &App, name: &str, control_tx: &registry::Sender) {
    let Ok(rooms) = app.store.rooms().await else {
        return;
    };
    // One event for the whole connection, not one per room: an agent that
    // belongs to many rooms with unread messages must not be able to
    // exhaust its own control-plane queue at Register time just by having
    // joined a lot of rooms — the exact defect this round fixed, via a
    // different trigger. See `RoomUnread` / `FromBus::Unread`.
    let mut unread = Vec::new();
    for room in rooms {
        if !room.members.iter().any(|m| m == name) {
            continue;
        }
        if let Ok(count) = app.store.unread_count(&room.name, name).await
            && count > 0
        {
            unread.push(RoomUnread {
                room: room.name.clone(),
                count,
            });
        }
    }
    if !unread.is_empty() {
        let _ = control_tx.try_send(FromBus::Unread { rooms: unread });
    }
}

/// The observer counterpart to `handle`. Deliberately a much smaller surface:
/// an observer may watch rooms and read (`History`, `ListRooms`) but cannot
/// join, send, or do anything else that would create or imply membership —
/// see the module doc on `Observe`/`Watch` for why. Every other `ToBus`
/// variant is rejected here rather than silently accepted and ignored, so a
/// future new command doesn't accidentally become observer-usable just by
/// falling through.
async fn handle_observer(
    app: &App,
    id: registry::ObserverId,
    cmd: ToBus,
    control_tx: &registry::Sender,
) {
    match cmd {
        ToBus::Watch { req_id, room } => {
            app.registry.watch(id, &room).await;
            let _ = control_tx.try_send(FromBus::Reply {
                req_id,
                result: ReplyResult::Watching { room },
            });
        }

        ToBus::History {
            req_id,
            room,
            limit,
        } => commands::reply_history(app, control_tx, req_id, &room, limit).await,

        ToBus::ListRooms { req_id } => commands::reply_list_rooms(app, control_tx, req_id).await,

        other => {
            let _ = control_tx.try_send(FromBus::Error {
                req_id: req_id_of(&other),
                message: "observers may only watch, list_rooms, or history — a viewer is not \
                          a participant"
                    .into(),
            });
        }
    }
}

/// Best-effort `req_id` extraction for commands an observer is not allowed to
/// issue, purely so the rejection in `handle_observer` can still correlate
/// back to the caller's request where the variant carries one.
fn req_id_of(cmd: &ToBus) -> Option<u64> {
    match cmd {
        ToBus::Join { req_id, .. }
        | ToBus::Send { req_id, .. }
        | ToBus::History { req_id, .. }
        | ToBus::ListRooms { req_id }
        | ToBus::ListAgents { req_id }
        | ToBus::PutFile { req_id, .. }
        | ToBus::GetFile { req_id, .. }
        | ToBus::ListFiles { req_id, .. }
        | ToBus::Resume { req_id, .. }
        | ToBus::Watch { req_id, .. } => Some(*req_id),
        ToBus::Register { .. } | ToBus::Observe { .. } | ToBus::Ack { .. } => None,
    }
}
