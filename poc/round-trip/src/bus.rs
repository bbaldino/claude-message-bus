//! The bus server. In-memory for POC 3 — no SQLite, no rooms, no file store.
//! Just enough to route point-to-point messages and hold them for offline agents.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::{Router, routing::get};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::proto::{FromBus, ToBus};

#[derive(Default)]
struct BusState {
    online: HashMap<String, mpsc::UnboundedSender<FromBus>>,
    /// Messages for agents that aren't connected yet. Delivered on register.
    pending: HashMap<String, Vec<FromBus>>,
    next_id: u64,
}

type Shared = Arc<Mutex<BusState>>;

impl BusState {
    fn roster(&self) -> Vec<String> {
        let mut v: Vec<String> = self.online.keys().cloned().collect();
        v.sort();
        v
    }

    fn broadcast_roster(&self) {
        let msg = FromBus::Agents {
            online: self.roster(),
        };
        for tx in self.online.values() {
            let _ = tx.send(msg.clone());
        }
    }
}

pub async fn serve(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let state: Shared = Arc::default();
    let app = Router::new()
        .route("/ws", get(ws_upgrade))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    eprintln!("bus listening on 0.0.0.0:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<Shared>) -> Response {
    ws.on_upgrade(move |socket| handle(socket, state))
}

async fn handle(socket: WebSocket, state: Shared) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<FromBus>();
    let mut name: Option<String> = None;

    // Pump outbound messages to this agent.
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(_) => continue,
            };
            if sink.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else { continue };
        let Ok(cmd) = serde_json::from_str::<ToBus>(&text) else {
            let _ = tx.send(FromBus::Error {
                message: format!("unparseable: {text}"),
            });
            continue;
        };

        match cmd {
            ToBus::Register { name: n } => {
                let pending = {
                    let mut s = state.lock().expect("bus state poisoned");
                    s.online.insert(n.clone(), tx.clone());
                    s.pending.remove(&n).unwrap_or_default()
                };
                name = Some(n.clone());
                eprintln!("registered: {n}");
                let _ = tx.send(FromBus::Registered { name: n });
                // Deliver anything that arrived while this agent was away.
                for m in pending {
                    let _ = tx.send(m);
                }
                state.lock().expect("bus state poisoned").broadcast_roster();
            }
            ToBus::Send { to, text } => {
                let from = name.clone().unwrap_or_else(|| "unknown".to_string());
                let mut s = state.lock().expect("bus state poisoned");
                s.next_id += 1;
                let envelope = FromBus::Message {
                    id: s.next_id,
                    from: from.clone(),
                    text: text.clone(),
                };
                eprintln!("{from} → {to}: {text}");
                match s.online.get(&to) {
                    Some(peer) => {
                        let _ = peer.send(envelope);
                    }
                    None => {
                        // Not connected: hold it. This is the "agent B was closed,
                        // gets the backlog when you open it tomorrow" case.
                        s.pending.entry(to.clone()).or_default().push(envelope);
                        let _ = tx.send(FromBus::Error {
                            message: format!("{to} is offline; message queued"),
                        });
                    }
                }
            }
        }
    }

    if let Some(n) = name {
        let mut s = state.lock().expect("bus state poisoned");
        s.online.remove(&n);
        eprintln!("disconnected: {n}");
        s.broadcast_roster();
    }
    writer.abort();
}
