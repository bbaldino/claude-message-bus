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
    },
    Join {
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryItem {
    pub id: i64,
    pub from: String,
    pub text: String,
    pub done: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomInfo {
    pub name: String,
    pub mode: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentInfo {
    pub name: String,
    pub host: String,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileInfo {
    pub key: String,
    pub size: i64,
    pub content_type: Option<String>,
    pub updated_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplyResult {
    Sent {
        room: String,
        msg_id: i64,
        delivered_to: Vec<String>,
        queued_for: Vec<String>,
    },
    Joined {
        room: String,
        members: Vec<String>,
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromBus {
    Registered {
        name: String,
    },
    Reply {
        req_id: u64,
        result: ReplyResult,
    },
    /// A message to inject into the session as a channel event.
    Message {
        id: i64,
        room: String,
        from: String,
        text: String,
        done: bool,
    },
    /// Sent on reconnect instead of replaying the backlog.
    Unread {
        room: String,
        count: i64,
    },
    /// The exchange cap tripped for this room.
    Paused {
        room: String,
        reason: String,
    },
    Error {
        req_id: Option<u64>,
        message: String,
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
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "message");
        assert_eq!(json["id"], 42);
        assert_eq!(json["from"], "caas");
        assert_eq!(json["room"], "protocol");
    }
}
