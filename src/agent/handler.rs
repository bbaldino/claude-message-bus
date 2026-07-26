//! The MCP surface Claude Code talks to. This is the only file that knows both
//! `rmcp` and our protocol; if the channels contract changes, it changes here.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, Implementation, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, model::JsonObject};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::agent::instructions;
use crate::proto::{FromBus, ToBus};

pub type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<FromBus>>>>;

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
                    Cow::Borrowed("List known agents and whether they are online"),
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
        // Implemented in Task 11; a stub keeps the contract test honest about
        // tools/list without pretending the tools work yet.
        let _ = request;
        Ok(CallToolResult::error(vec![ContentBlock::text(
            "not yet implemented",
        )]))
    }
}
