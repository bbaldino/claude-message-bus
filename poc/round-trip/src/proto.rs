//! Wire protocol between an agent and the bus. JSON over WebSocket.

use serde::{Deserialize, Serialize};

/// agent → bus
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToBus {
    Register { name: String },
    Send { to: String, text: String },
}

/// bus → agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromBus {
    Registered {
        name: String,
    },
    Message {
        id: u64,
        from: String,
        text: String,
    },
    /// Broadcast whenever presence changes, so each agent can keep a local
    /// roster without needing request/response plumbing.
    Agents {
        online: Vec<String>,
    },
    Error {
        message: String,
    },
}
