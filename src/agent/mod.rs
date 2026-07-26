pub mod bridge;
pub mod handler;
pub mod instructions;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use rmcp::{ServiceExt, transport::stdio};
use tokio::sync::{Mutex, mpsc};

use crate::config::{EnvSource, RealEnv};
use crate::proto::ToBus;
use bridge::BridgeConfig;
use handler::{Handler, Pending};

pub async fn run(bus_url: String, name: String) -> anyhow::Result<()> {
    eprintln!("[agent] starting as \"{name}\", bus={bus_url}");

    let (to_bus, rx) = mpsc::unbounded_channel::<ToBus>();
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

    let handler = Handler {
        name: name.clone(),
        to_bus,
        pending: pending.clone(),
        next_req: Arc::new(AtomicU64::new(1)),
    };

    // Serve MCP before touching the network: session startup must never block
    // on the bus being reachable.
    let service = handler.serve(stdio()).await?;
    let peer = service.peer().clone();

    let env = RealEnv;
    let cfg = BridgeConfig {
        bus_url,
        name,
        host: env.hostname(),
        cwd: env
            .var("CLAUDE_PROJECT_DIR")
            .or_else(|| env.cwd())
            .unwrap_or_else(|| ".".to_string()),
        session_id: env.var("CLAUDE_CODE_SESSION_ID"),
    };
    tokio::spawn(bridge::run(cfg, rx, peer, pending));

    service.waiting().await?;
    Ok(())
}
