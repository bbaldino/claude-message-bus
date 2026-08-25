//! The wire protocol between an agent and the bus: JSON over WebSocket.
//!
//! Requests carry a `req_id` so replies can be correlated. This is what lets
//! the `send` tool block until the bus confirms delivery, rather than
//! optimistically reporting success for a message that was only queued.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Room { room: String },
    Agent { name: String },
}

/// agent → bus
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToBus {
    Register {
        name: String,
        host: String,
        cwd: String,
        session_id: Option<String>,
        /// Absent on the wire means `false`, which is what makes this change safe to
        /// deploy under running agents: Claude Code spawns a stdio MCP server once at
        /// session start and never respawns it, so agent binaries in flight when this
        /// ships keep sending the old payload shape indefinitely.
        #[serde(default)]
        human: bool,
        /// The agent binary's crate version. Absent on the wire means `None`, which is
        /// the signal worth surfacing rather than an error: Claude Code spawns a stdio
        /// MCP server once at session start and never respawns it, so a binary that
        /// predates this field keeps registering without one indefinitely.
        #[serde(default)]
        version: Option<String>,
    },
    Join {
        req_id: u64,
        room: String,
    },
    /// Identify a connection as an observer for its lifetime, the way
    /// `Register` identifies an agent. Unlike `Register`, this creates no row
    /// in `agents` and grants no membership anywhere — an observer is a
    /// spectator, not a participant. `name` is a display label only (used in
    /// bus-side diagnostics); it is never persisted and never contends with
    /// agent names for `Registry::attach`'s collision handling.
    Observe {
        name: String,
    },
    /// Start receiving `FromBus::Message` fan-out for `room`, without joining
    /// it: no `room_members` row is created, so the room's membership and any
    /// future `send`'s `delivered_to`/`queued_for` are unaffected. Valid only
    /// on a connection that identified via `Observe`.
    Watch {
        req_id: u64,
        room: String,
    },
    /// Stop watching a room. Observer-only.
    ///
    /// `Watch` alone would let a console accumulate every room it has ever
    /// selected, which both wastes fan-out and undoes the narrowing that made
    /// the observer subscriptions opt-in.
    Unwatch {
        req_id: u64,
        room: String,
    },
    Send {
        req_id: u64,
        target: Target,
        text: String,
        done: bool,
    },
    History {
        req_id: u64,
        room: String,
        limit: i64,
    },
    ListRooms {
        req_id: u64,
    },
    ListAgents {
        req_id: u64,
    },
    PutFile {
        req_id: u64,
        room: String,
        key: String,
        content_b64: String,
        content_type: Option<String>,
    },
    GetFile {
        req_id: u64,
        room: String,
        key: String,
    },
    ListFiles {
        req_id: u64,
        room: String,
    },
    Resume {
        req_id: u64,
        room: String,
    },
    /// Sent after a message has been injected into the session, advancing the
    /// agent's cursor for that room.
    Ack {
        room: String,
        last_delivered_id: i64,
    },
    /// Subscribe to agent connect/disconnect. Observer-only, opt-in: a `tail`
    /// watching one room must not start receiving fleet-wide traffic.
    WatchPresence {
        req_id: u64,
    },
    /// Subscribe to the event stream. `room: None` is the whole bus.
    WatchEvents {
        req_id: u64,
        room: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
pub struct HistoryItem {
    #[ts(type = "number")]
    pub id: i64,
    pub from: String,
    pub text: String,
    pub done: bool,
    #[ts(type = "number")]
    pub created_at: i64,
    /// Whether a human sent this. Carried on history as well as on the live event
    /// because a worker that was offline catches up through `history` — the reconnect
    /// path sends only an `Unread` summary, never a replay.
    #[serde(default)]
    pub human: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
pub struct RoomInfo {
    pub name: String,
    pub mode: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
pub struct AgentInfo {
    pub name: String,
    pub host: String,
    pub online: bool,
    /// The agent's reported crate version, or `None` for a binary predating the field.
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
pub struct FileInfo {
    pub key: String,
    #[ts(type = "number")]
    pub size: i64,
    pub content_type: Option<String>,
    pub updated_by: String,
}

/// One room's contribution to a reconnecting agent's `FromBus::Unread`
/// summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
pub struct RoomUnread {
    pub room: String,
    #[ts(type = "number")]
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplyResult {
    Sent {
        room: String,
        #[ts(type = "number")]
        msg_id: i64,
        delivered_to: Vec<String>,
        queued_for: Vec<String>,
    },
    Joined {
        room: String,
        members: Vec<String>,
    },
    Watching {
        room: String,
    },
    History {
        messages: Vec<HistoryItem>,
    },
    Rooms {
        rooms: Vec<RoomInfo>,
    },
    Agents {
        agents: Vec<AgentInfo>,
    },
    FileStored {
        key: String,
        #[ts(type = "number")]
        size: i64,
        sha256: String,
    },
    FileContent {
        key: String,
        content_b64: String,
        content_type: Option<String>,
    },
    Files {
        files: Vec<FileInfo>,
    },
    Resumed {
        room: String,
    },
}

/// bus → agent
///
/// `ts_rs::TS` is derived here, not only on the REST DTOs in `web::api`, and
/// that asymmetry was the root cause of three separate bugs in one phase. The
/// push protocol is snake_case on the wire — `rename_all` on an enum renames
/// variant *names*, not variant *fields* — while the DTOs are camelCase, and
/// every browser-side handler was a cast against a hand-written literal, which
/// is the one construct that silences the compiler about exactly this. With the
/// union generated, a rename in this file breaks the TypeScript build instead of
/// producing a silent `undefined` that passes every gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromBus {
    Registered {
        name: String,
        /// Whether this connection holds a relayer grant — its sends are stamped with
        /// its human's authority (`human="true"`).
        ///
        /// The bus is the only party that knows: the grant lives in configuration, and
        /// `Relayers::contains` matches the effective name, so a renamed collision holds
        /// no grant. Without this field an agent can only infer its own provenance from
        /// instructions that say `human="false"` means "another agent sent this" — right
        /// for every agent except a relayer, which is exactly the one that cannot tell.
        ///
        /// Absent on the wire means `false`, for the same reason `Register::human` has a
        /// default: Claude Code spawns a stdio MCP server once at session start and never
        /// respawns it, so a client that predates a new bus keeps parsing this frame.
        #[serde(default)]
        relayer: bool,
    },
    /// Acknowledges an `Observe`. The counterpart to `Registered`, on the
    /// same control channel — see `Observe`.
    Observing { name: String },
    Reply {
        #[ts(type = "number")]
        req_id: u64,
        result: ReplyResult,
    },
    /// A message to inject into the session as a channel event.
    Message {
        #[ts(type = "number")]
        id: i64,
        room: String,
        from: String,
        text: String,
        done: bool,
        /// Set by the bus from the sending connection, never by the sender. Absent on
        /// the wire means `false` so an agent binary that predates this field keeps
        /// deserializing — the same constraint that governs `Register.human`.
        #[serde(default)]
        human: bool,
    },
    /// Sent on reconnect instead of replaying the backlog. One event per
    /// connection, not per room — see `RoomUnread` — so a reconnecting
    /// agent's own control-plane queue can never be exhausted by the number
    /// of rooms it happens to belong to.
    Unread { rooms: Vec<RoomUnread> },
    /// The exchange cap tripped for this room.
    Paused { room: String, reason: String },
    Error {
        #[ts(type = "number | null")]
        req_id: Option<u64>,
        message: String,
    },
    /// An agent connected or disconnected. Only sent to observers that asked.
    Presence {
        name: String,
        host: String,
        online: bool,
        #[ts(type = "number")]
        last_seen: i64,
    },
    /// A bus event, as appended to the audit log. Only sent to observers that asked.
    Event {
        #[ts(type = "number")]
        id: i64,
        kind: String,
        agent: Option<String>,
        room: Option<String>,
        #[ts(type = "unknown")]
        detail: serde_json::Value,
        #[ts(type = "number")]
        created_at: i64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_bus_round_trips_through_json() {
        let cmd = ToBus::Send {
            req_id: 7,
            target: Target::Agent {
                name: "dashboard".into(),
            },
            text: "hello".into(),
            done: false,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"send\""), "tagged by type: {json}");
        let back: ToBus = serde_json::from_str(&json).unwrap();
        match back {
            ToBus::Send {
                req_id,
                target: Target::Agent { name },
                text,
                done,
            } => {
                assert_eq!(
                    (req_id, name.as_str(), text.as_str(), done),
                    (7, "dashboard", "hello", false)
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn sent_reply_distinguishes_delivered_from_queued() {
        // The whole point of the ack: the model must be told which happened.
        let reply = FromBus::Reply {
            req_id: 7,
            result: ReplyResult::Sent {
                room: "dm:caas|dashboard".into(),
                msg_id: 42,
                delivered_to: vec!["dashboard".into()],
                queued_for: vec!["nas".into()],
            },
        };
        let json = serde_json::to_string(&reply).unwrap();
        let back: FromBus = serde_json::from_str(&json).unwrap();
        match back {
            FromBus::Reply {
                result:
                    ReplyResult::Sent {
                        delivered_to,
                        queued_for,
                        ..
                    },
                ..
            } => {
                assert_eq!(delivered_to, vec!["dashboard".to_string()]);
                assert_eq!(queued_for, vec!["nas".to_string()]);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn observe_and_watch_round_trip_through_json() {
        let observe = ToBus::Observe {
            name: "tail-1234".into(),
        };
        let json = serde_json::to_string(&observe).unwrap();
        assert!(
            json.contains("\"type\":\"observe\""),
            "tagged by type: {json}"
        );
        assert_eq!(serde_json::from_str::<ToBus>(&json).unwrap(), observe);

        let watch = ToBus::Watch {
            req_id: 1,
            room: "protocol".into(),
        };
        let json = serde_json::to_string(&watch).unwrap();
        assert!(
            json.contains("\"type\":\"watch\""),
            "tagged by type: {json}"
        );
        assert_eq!(serde_json::from_str::<ToBus>(&json).unwrap(), watch);
    }

    #[test]
    fn unknown_variants_fail_loudly_rather_than_silently() {
        let err = serde_json::from_str::<ToBus>(r#"{"type":"teleport"}"#);
        assert!(err.is_err(), "unknown command must not deserialize");
    }

    #[test]
    fn message_carries_everything_the_channel_tag_needs() {
        let msg = FromBus::Message {
            id: 42,
            room: "protocol".into(),
            from: "caas".into(),
            text: "hi".into(),
            done: false,
            human: false,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "message");
        assert_eq!(json["id"], 42);
        assert_eq!(json["from"], "caas");
        assert_eq!(json["room"], "protocol");
    }
}
