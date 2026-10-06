pub mod bridge;
pub mod handler;
pub mod instructions;
pub mod status;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use rmcp::{RoleServer, ServiceExt, transport::IntoTransport, transport::stdio};
use tokio::sync::{Mutex, mpsc, watch};

use crate::config::{EnvSource, RealEnv};
use crate::proto::ToBus;
use bridge::BridgeConfig;
use handler::{Handler, Pending};
use status::{LocalStatus, StatusTx};

pub async fn run(bus_url: String, name: String) -> anyhow::Result<()> {
    run_on(stdio(), bus_url, name).await
}

/// Same as `run`, but takes the MCP transport directly instead of grabbing the
/// process's real stdin/stdout via `stdio()`. `rmcp::transport::stdio()` is
/// nothing more than `(tokio::io::stdin(), tokio::io::stdout())`, and rmcp has
/// a blanket `impl<Role, R, W> IntoTransport<Role, ..> for (R, W)` wherever `R:
/// AsyncRead` and `W: AsyncWrite` — so a `tokio::io::duplex()` half drops in
/// just as well as the process's real pipes. That's what lets tests drive this
/// exact handler and bridge code in-process, without spawning a child.
pub async fn run_on<T, E, A>(transport: T, bus_url: String, name: String) -> anyhow::Result<()>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    run_on_with_liveness(transport, bus_url, name, bridge::Liveness::default()).await
}

/// Same as `run_on`, but with an injectable liveness cadence so tests don't
/// have to wait out the production 30s/90s.
pub async fn run_on_with_liveness<T, E, A>(
    transport: T,
    bus_url: String,
    name: String,
    liveness: bridge::Liveness,
) -> anyhow::Result<()>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    run_on_with_options(
        transport,
        bus_url,
        name,
        AgentOptions {
            liveness,
            status_file: None,
        },
    )
    .await
}

/// The knobs `run_on_with_options` takes, so tests can inject them without a new
/// `run_on_with_*` per combination.
pub struct AgentOptions {
    pub liveness: bridge::Liveness,
    /// The hook's state file. `None` derives it from the environment
    /// (`CLAUDE_CODE_SESSION_ID` plus the state dir), `Some(None)` turns the poller
    /// off, and `Some(Some(path))` uses that file.
    pub status_file: Option<Option<PathBuf>>,
}

pub async fn run_on_with_options<T, E, A>(
    transport: T,
    bus_url: String,
    name: String,
    opts: AgentOptions,
) -> anyhow::Result<()>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    eprintln!("[agent] starting as \"{name}\", bus={bus_url}");

    let (to_bus, rx) = mpsc::unbounded_channel::<ToBus>();
    // The bridge needs its own sender to ack delivered messages — a receiver
    // cannot send.
    let ack_tx = to_bus.clone();
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    // Fed by the status tool (text) and the file poller (hook state); the bridge
    // sends the whole value on every change and again on every registration.
    let status: StatusTx = Arc::new(watch::channel(LocalStatus::default()).0);

    let handler = Handler {
        name: name.clone(),
        to_bus,
        pending: pending.clone(),
        next_req: Arc::new(AtomicU64::new(1)),
        status: status.clone(),
    };

    // Serve MCP before touching the network: session startup must never block
    // on the bus being reachable.
    let service = handler.serve(transport).await?;
    let peer = service.peer().clone();

    let env = RealEnv;
    let session_id = env.var("CLAUDE_CODE_SESSION_ID");
    let status_file = match opts.status_file {
        Some(explicit) => explicit,
        None => status::status_key(session_id.as_deref()).and_then(|key| {
            status::status_dir(env.var("XDG_STATE_HOME"), env.var("HOME"))
                .map(|d| d.join(format!("{key}.json")))
        }),
    };
    if let Some(path) = status_file {
        eprintln!("[agent] watching status file {}", path.display());
        tokio::spawn(bridge::poll_status_file(path, status.clone()));
    }
    let cfg = BridgeConfig {
        bus_url,
        name,
        host: env.hostname(),
        cwd: env
            .var("CLAUDE_PROJECT_DIR")
            .or_else(|| env.cwd())
            .unwrap_or_else(|| ".".to_string()),
        session_id,
        liveness: opts.liveness,
    };
    tokio::spawn(bridge::run(cfg, rx, ack_tx, peer, pending, status));

    service.waiting().await?;
    Ok(())
}
