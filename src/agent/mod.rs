pub mod handler;
pub mod instructions;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use rmcp::{ServiceExt, transport::stdio};
use tokio::sync::{Mutex, mpsc};

use crate::proto::ToBus;
use handler::{Handler, Pending};

pub async fn run(bus_url: String, name: String) -> anyhow::Result<()> {
    eprintln!("[agent] starting as \"{name}\", bus={bus_url}");

    let (to_bus, _rx) = mpsc::unbounded_channel::<ToBus>();
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

    let handler = Handler {
        name: name.clone(),
        to_bus,
        pending,
        next_req: Arc::new(AtomicU64::new(1)),
    };

    // Serve MCP before touching the network so session startup never blocks.
    let service = handler.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
