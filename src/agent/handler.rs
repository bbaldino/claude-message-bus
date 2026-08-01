//! The MCP surface Claude Code talks to. This is the only file that knows both
//! `rmcp` and our protocol; if the channels contract changes, it changes here.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use base64::Engine;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, Implementation, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, model::JsonObject};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::agent::instructions;
use crate::proto::{FromBus, ReplyResult, ToBus};

pub type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<FromBus>>>>;

/// The single source of truth for the tool names this server exposes.
/// `list_tools` below is the other half of the contract — its tool literals
/// must match this list exactly, which `tests/agent_contract.rs` asserts.
/// `claude-bus init` derives its permission allowlist from this same const,
/// rather than hardcoding the nine names a third time, so adding a tool here
/// without wiring it into `list_tools` fails the suite instead of silently
/// stalling an unattended agent-to-agent exchange on a permission prompt.
pub const BUS_TOOL_NAMES: [&str; 9] = [
    "send",
    "history",
    "rooms",
    "agents",
    "join",
    "put_file",
    "get_file",
    "list_files",
    "resume",
];

#[derive(Clone)]
pub struct Handler {
    pub name: String,
    pub to_bus: mpsc::UnboundedSender<ToBus>,
    pub pending: Pending,
    pub next_req: Arc<std::sync::atomic::AtomicU64>,
}

fn schema(v: Value) -> Arc<JsonObject> {
    Arc::new(v.as_object().cloned().expect("schema must be an object"))
}

impl Handler {
    /// Issue a request and wait for the bus's reply. This is what makes `send`
    /// able to report delivered-vs-queued instead of optimistically claiming
    /// success for a message that was only queued.
    async fn request<F>(&self, build: F) -> Result<ReplyResult, String>
    where
        F: FnOnce(u64) -> ToBus,
    {
        let req_id = self
            .next_req
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(req_id, tx);

        if self.to_bus.send(build(req_id)).is_err() {
            self.pending.lock().await.remove(&req_id);
            return Err("not connected to the bus".to_string());
        }

        match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
            Ok(Ok(FromBus::Reply { result, .. })) => Ok(result),
            Ok(Ok(FromBus::Error { message, .. })) => Err(message),
            Ok(Ok(other)) => Err(format!("unexpected bus reply: {other:?}")),
            Ok(Err(_)) => Err("bus reply channel closed".to_string()),
            Err(_) => {
                self.pending.lock().await.remove(&req_id);
                Err("the bus did not reply within 10s; it may be unreachable".to_string())
            }
        }
    }
}

impl rmcp::ServerHandler for Handler {
    // InitializeResult and Implementation are #[non_exhaustive], so struct
    // literal syntax is unavailable outside rmcp and field assignment after
    // Default::default() is the only route. Clippy's lint assumes a literal was
    // possible; here it was not.
    #[allow(clippy::field_reassign_with_default)]
    fn get_info(&self) -> InitializeResult {
        // The presence of this key is what makes Claude Code register a
        // notification listener and treat this server as a channel.
        let mut experimental: BTreeMap<String, JsonObject> = BTreeMap::new();
        experimental.insert("claude/channel".to_string(), serde_json::Map::new());

        // rmcp model types are #[non_exhaustive]: build, do not struct-literal.
        let mut server_info = Implementation::from_build_env();
        server_info.name = "msgbus".into();
        server_info.version = env!("CARGO_PKG_VERSION").into();

        let mut info = InitializeResult::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_experimental_with(experimental)
            .enable_tools()
            .build();
        info.server_info = server_info;
        info.instructions = Some(instructions::for_agent(&self.name));
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: vec![
                Tool::new(
                    Cow::Borrowed("send"),
                    Cow::Borrowed(
                        "Send a message to another agent (to) or a room (room). Waits for \
                         the bus to confirm whether it was delivered or queued.",
                    ),
                    schema(json!({
                        "type": "object",
                        "properties": {
                            "to": { "type": "string", "description": "Recipient agent name (direct message)" },
                            "room": { "type": "string", "description": "Room name (broadcast to members)" },
                            "text": { "type": "string", "description": "The message body" },
                            "done": { "type": "boolean", "description": "Mark the topic settled; no reply expected" }
                        },
                        "required": ["text"]
                    })),
                ),
                Tool::new(
                    Cow::Borrowed("history"),
                    Cow::Borrowed("Fetch recent messages from a room"),
                    schema(json!({
                        "type": "object",
                        "properties": {
                            "room": { "type": "string" },
                            "limit": { "type": "integer", "description": "Default 20" }
                        },
                        "required": ["room"]
                    })),
                ),
                Tool::new(
                    Cow::Borrowed("rooms"),
                    Cow::Borrowed("List rooms and their members"),
                    schema(json!({ "type": "object", "properties": {} })),
                ),
                Tool::new(
                    Cow::Borrowed("agents"),
                    Cow::Borrowed(
                        "List known agents, whether they are online, and the claude-bus \
                         version each reported at registration. Call this to find which \
                         sessions are running a version that differs from this bus's own \
                         (an `unknown` version is a binary that predates version reporting \
                         entirely) — those sessions need restarting to pick up the current \
                         binary.",
                    ),
                    schema(json!({ "type": "object", "properties": {} })),
                ),
                Tool::new(
                    Cow::Borrowed("join"),
                    Cow::Borrowed("Join a room, creating it if it does not exist"),
                    schema(json!({
                        "type": "object",
                        "properties": { "room": { "type": "string" } },
                        "required": ["room"]
                    })),
                ),
                Tool::new(
                    Cow::Borrowed("put_file"),
                    Cow::Borrowed(
                        "Store an artifact in a room. Provide exactly one of `content` \
                         (inline text) or `path` (read from local disk).",
                    ),
                    schema(json!({
                        "type": "object",
                        "properties": {
                            "room": { "type": "string" },
                            "key": { "type": "string", "description": "Name within the room, e.g. schema.json" },
                            "content": { "type": "string" },
                            "path": { "type": "string" },
                            "content_type": { "type": "string" }
                        },
                        "required": ["room", "key"]
                    })),
                ),
                Tool::new(
                    Cow::Borrowed("get_file"),
                    Cow::Borrowed("Retrieve an artifact's contents from a room"),
                    schema(json!({
                        "type": "object",
                        "properties": {
                            "room": { "type": "string" },
                            "key": { "type": "string" }
                        },
                        "required": ["room", "key"]
                    })),
                ),
                Tool::new(
                    Cow::Borrowed("list_files"),
                    Cow::Borrowed("List artifacts stored in a room"),
                    schema(json!({
                        "type": "object",
                        "properties": { "room": { "type": "string" } },
                        "required": ["room"]
                    })),
                ),
                Tool::new(
                    Cow::Borrowed("resume"),
                    Cow::Borrowed(
                        "Clear a room's exchange-cap pause. Only call this after your \
                         human has said to continue.",
                    ),
                    schema(json!({
                        "type": "object",
                        "properties": { "room": { "type": "string" } },
                        "required": ["room"]
                    })),
                ),
            ],
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let args = request.arguments.unwrap_or_default();
        let s = |k: &str| args.get(k).and_then(Value::as_str).map(String::from);
        let text_of = |v: String| Ok(CallToolResult::success(vec![ContentBlock::text(v)]));

        match request.name.as_ref() {
            "send" => {
                let Some(body) = s("text") else {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "`text` is required",
                    )]));
                };
                let target = match (s("to"), s("room")) {
                    (Some(name), None) => crate::proto::Target::Agent { name },
                    (None, Some(room)) => crate::proto::Target::Room { room },
                    _ => {
                        return Ok(CallToolResult::error(vec![ContentBlock::text(
                            "provide exactly one of `to` (direct message) or `room` (broadcast)",
                        )]));
                    }
                };
                let done = args.get("done").and_then(Value::as_bool).unwrap_or(false);
                let reply = self
                    .request(|req_id| ToBus::Send {
                        req_id,
                        target,
                        text: body.clone(),
                        done,
                    })
                    .await;
                match reply {
                    Ok(ReplyResult::Sent {
                        room,
                        delivered_to,
                        queued_for,
                        ..
                    }) => {
                        // Echo the full text: Claude Code hides outbound channel
                        // text from the terminal, so this keeps the transcript
                        // self-contained on replay.
                        let mut status = String::new();
                        if !delivered_to.is_empty() {
                            status.push_str(&format!("delivered to {}", delivered_to.join(", ")));
                        }
                        if !queued_for.is_empty() {
                            if !status.is_empty() {
                                status.push_str("; ");
                            }
                            status.push_str(&format!(
                                "queued for {} (offline)",
                                queued_for.join(", ")
                            ));
                        }
                        if status.is_empty() {
                            status.push_str("nobody else is in this room yet");
                        }
                        text_of(format!("[{room}] {status}\nsent: {body}"))
                    }
                    Ok(other) => text_of(format!("unexpected reply: {other:?}")),
                    Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
                }
            }

            "history" => {
                let Some(room) = s("room") else {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "`room` is required",
                    )]));
                };
                let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(20);
                match self
                    .request(|req_id| ToBus::History {
                        req_id,
                        room: room.clone(),
                        limit,
                    })
                    .await
                {
                    Ok(ReplyResult::History { messages }) if messages.is_empty() => {
                        text_of(format!("no messages yet in {room}"))
                    }
                    Ok(ReplyResult::History { messages }) => text_of(
                        messages
                            .into_iter()
                            .map(|m| {
                                format!(
                                    "[{}] {}{}: {}",
                                    m.id,
                                    m.from,
                                    if m.human { " (human)" } else { "" },
                                    m.text
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    Ok(other) => text_of(format!("unexpected reply: {other:?}")),
                    Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
                }
            }

            "rooms" => match self.request(|req_id| ToBus::ListRooms { req_id }).await {
                Ok(ReplyResult::Rooms { rooms }) if rooms.is_empty() => {
                    text_of("no rooms yet".to_string())
                }
                Ok(ReplyResult::Rooms { rooms }) => text_of(
                    rooms
                        .into_iter()
                        .map(|r| format!("{} [{}] — {}", r.name, r.mode, r.members.join(", ")))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                Ok(other) => text_of(format!("unexpected reply: {other:?}")),
                Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
            },

            "agents" => match self.request(|req_id| ToBus::ListAgents { req_id }).await {
                Ok(ReplyResult::Agents { agents }) if agents.is_empty() => {
                    text_of("no agents registered yet".to_string())
                }
                Ok(ReplyResult::Agents { agents }) => text_of(
                    agents
                        .into_iter()
                        .map(|a| {
                            format!(
                                "{}@{} — {} — {}",
                                a.name,
                                a.host,
                                if a.online { "online" } else { "offline" },
                                a.version.as_deref().unwrap_or("unknown")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                Ok(other) => text_of(format!("unexpected reply: {other:?}")),
                Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
            },

            "join" => {
                let Some(room) = s("room") else {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "`room` is required",
                    )]));
                };
                match self
                    .request(|req_id| ToBus::Join {
                        req_id,
                        room: room.clone(),
                    })
                    .await
                {
                    Ok(ReplyResult::Joined { room, members }) => {
                        text_of(format!("joined {room}; members: {}", members.join(", ")))
                    }
                    Ok(other) => text_of(format!("unexpected reply: {other:?}")),
                    Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
                }
            }

            "put_file" => {
                let (Some(room), Some(key)) = (s("room"), s("key")) else {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "`room` and `key` are required",
                    )]));
                };
                let bytes = match (s("content"), s("path")) {
                    (Some(c), None) => c.into_bytes(),
                    (None, Some(p)) => match std::fs::read(&p) {
                        Ok(b) => b,
                        Err(e) => {
                            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                                "cannot read {p}: {e}"
                            ))]));
                        }
                    },
                    _ => {
                        return Ok(CallToolResult::error(vec![ContentBlock::text(
                            "provide exactly one of `content` or `path`",
                        )]));
                    }
                };
                let content_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let content_type = s("content_type");
                match self
                    .request(|req_id| ToBus::PutFile {
                        req_id,
                        room: room.clone(),
                        key: key.clone(),
                        content_b64: content_b64.clone(),
                        content_type: content_type.clone(),
                    })
                    .await
                {
                    Ok(ReplyResult::FileStored { key, size, .. }) => {
                        text_of(format!("stored {key} in {room} ({size} bytes)"))
                    }
                    Ok(other) => text_of(format!("unexpected reply: {other:?}")),
                    Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
                }
            }

            "get_file" => {
                let (Some(room), Some(key)) = (s("room"), s("key")) else {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "`room` and `key` are required",
                    )]));
                };
                match self
                    .request(|req_id| ToBus::GetFile {
                        req_id,
                        room: room.clone(),
                        key: key.clone(),
                    })
                    .await
                {
                    Ok(ReplyResult::FileContent {
                        key, content_b64, ..
                    }) => match base64::engine::general_purpose::STANDARD.decode(&content_b64) {
                        Ok(bytes) => match String::from_utf8(bytes) {
                            Ok(text) => text_of(text),
                            Err(e) => text_of(format!(
                                "{key} is {} bytes of binary data (not valid UTF-8: {e})",
                                e.as_bytes().len()
                            )),
                        },
                        Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                            "bad base64 from bus: {e}"
                        ))])),
                    },
                    Ok(other) => text_of(format!("unexpected reply: {other:?}")),
                    Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
                }
            }

            "list_files" => {
                let Some(room) = s("room") else {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "`room` is required",
                    )]));
                };
                match self
                    .request(|req_id| ToBus::ListFiles {
                        req_id,
                        room: room.clone(),
                    })
                    .await
                {
                    Ok(ReplyResult::Files { files }) if files.is_empty() => {
                        text_of(format!("no files in {room}"))
                    }
                    Ok(ReplyResult::Files { files }) => text_of(
                        files
                            .into_iter()
                            .map(|f| format!("{} — {} bytes, by {}", f.key, f.size, f.updated_by))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    Ok(other) => text_of(format!("unexpected reply: {other:?}")),
                    Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
                }
            }

            "resume" => {
                let Some(room) = s("room") else {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "`room` is required",
                    )]));
                };
                match self
                    .request(|req_id| ToBus::Resume {
                        req_id,
                        room: room.clone(),
                    })
                    .await
                {
                    Ok(ReplyResult::Resumed { room }) => {
                        text_of(format!("{room} resumed; the exchange counter is cleared"))
                    }
                    Ok(other) => text_of(format!("unexpected reply: {other:?}")),
                    Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
                }
            }

            other => Err(McpError::invalid_params(
                format!("unknown tool: {other}"),
                None,
            )),
        }
    }
}
