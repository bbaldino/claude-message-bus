//! POC 2 — the Rust port of the channel probe.
//!
//! POC 1 proved the channel mechanism works and that there is no client-side magic to
//! replicate: the capability is plain JSON in the `initialize` result. The only open
//! question was whether `rmcp` can express it, or whether we hand-roll the stdio
//! JSON-RPC. This answers that.
//!
//! Two pieces are needed and both exist in rmcp 2.2.0:
//!   - `ServerCapabilities.experimental`, a `BTreeMap<String, JsonObject>`
//!   - `ServerNotification::CustomNotification`, which carries an arbitrary method name
//!
//! NOTE: stdout is the JSON-RPC transport. All logging goes to stderr and a file.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::sync::Arc;

use axum::{Router, extract::State, routing::post};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, CustomNotification, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    ServerNotification, Tool,
};
use rmcp::service::{Peer, RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServiceExt, transport::stdio};
use serde_json::{Value, json};

const PORT: u16 = 8789; // deliberately not 8788, so both probes can coexist

/// Absolute so the log lands next to the crate no matter what cwd Claude Code
/// spawns us with. Fine for a POC; the real binary will take a `--log` flag.
const LOG_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/rust-probe.log");

fn log(msg: &str) {
    let line = format!(
        "[{}] {msg}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    eprint!("{line}");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(LOG_PATH) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[derive(Clone)]
struct Probe;

impl rmcp::ServerHandler for Probe {
    fn get_info(&self) -> InitializeResult {
        // This map is the entire experiment: `claude/channel` under `experimental` is
        // what makes Claude Code register a notification listener for this server.
        let mut experimental: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();
        experimental.insert("claude/channel".to_string(), serde_json::Map::new());

        // These types are #[non_exhaustive], so they're built rather than
        // struct-literalled: builder for capabilities, field assignment for the rest.
        // from_build_env() reports rmcp's own version, not ours — set both explicitly.
        let mut server_info = Implementation::from_build_env();
        server_info.name = "rust-probe".into();
        server_info.version = env!("CARGO_PKG_VERSION").into();

        let mut info = InitializeResult::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_experimental_with(experimental)
            .enable_tools()
            .build();
        info.server_info = server_info;
        info.instructions = Some(
            "This is a Rust connectivity probe. Messages arrive as \
             <channel source=\"rust-probe\" probe_id=\"...\">. When one arrives, call \
             probe_reply with the probe_id and a short acknowledgement, then state in \
             plain text exactly which attributes the <channel> tag carried. \
             Do not modify any files."
                .to_string(),
        );
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        log("tools/list requested");
        let schema: serde_json::Map<String, Value> = json!({
            "type": "object",
            "properties": {
                "probe_id": { "type": "string", "description": "The probe_id from the channel tag" },
                "text": { "type": "string", "description": "A short acknowledgement" },
            },
            "required": ["probe_id", "text"],
        })
        .as_object()
        .cloned()
        .expect("schema is an object");

        Ok(ListToolsResult {
            tools: vec![Tool::new(
                Cow::Borrowed("probe_reply"),
                Cow::Borrowed("Acknowledge a probe message received over the channel"),
                Arc::new(schema),
            )],
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if request.name != "probe_reply" {
            return Err(McpError::invalid_params(
                format!("unknown tool: {}", request.name),
                None,
            ));
        }
        let args = request.arguments.unwrap_or_default();
        let probe_id = args.get("probe_id").and_then(Value::as_str).unwrap_or("?");
        let text = args.get("text").and_then(Value::as_str).unwrap_or("");
        log(&format!(
            "probe_reply called: probe_id={probe_id} text={text:?}"
        ));
        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "RUST_ECHO_MARKER probe_id={probe_id} text={text:?}"
        ))]))
    }
}

/// Anything POSTed here is pushed into the live session as a channel event.
async fn push(State(peer): State<Peer<RoleServer>>, body: String) -> String {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let probe_id = NEXT_ID
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .to_string();

    log(&format!(
        "emitting notification probe_id={probe_id} body={body:?}"
    ));

    let notification = CustomNotification::new(
        "notifications/claude/channel",
        Some(json!({
            "content": body,
            // meta keys must be identifiers: letters, digits, underscores only
            "meta": { "probe_id": probe_id, "lang": "rust" },
        })),
    );

    match peer
        .send_notification(ServerNotification::CustomNotification(notification))
        .await
    {
        Ok(()) => {
            // Resolves when written to the transport, NOT when Claude processes it.
            // A silent drop is indistinguishable from success here.
            log(&format!(
                "notification written to transport probe_id={probe_id}"
            ));
            "emitted\n".to_string()
        }
        Err(e) => {
            log(&format!("notification FAILED probe_id={probe_id}: {e}"));
            format!("failed: {e}\n")
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    log(&format!(
        "rust probe starting; cwd={:?} CLAUDE_PROJECT_DIR={:?}",
        std::env::current_dir().unwrap_or_default(),
        std::env::var("CLAUDE_PROJECT_DIR").ok()
    ));

    let service = Probe.serve(stdio()).await?;
    log("mcp connected over stdio");

    let peer = service.peer().clone();
    let app = Router::new().route("/", post(push)).with_state(peer);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", PORT)).await?;
    log(&format!("http listening on 127.0.0.1:{PORT}"));
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            log(&format!("http server error: {e}"));
        }
    });

    service.waiting().await?;
    Ok(())
}
