//! An interactive room client for a human.
//!
//! `tail` watches a room without joining it — it identifies via `Observe`, which
//! deliberately creates no `agents` row and no `room_members` row. This is the
//! participant counterpart: it registers as a human, joins the room, and sends what
//! is typed. Realtime needs nothing new; the bus already pushes `FromBus::Message`
//! over this same socket.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::config::EnvSource;
use crate::proto::{FromBus, ReplyResult, Target, ToBus};

/// How much history to print on joining. Matches the exchange cap, so a human
/// arriving sees at most one full unattended stretch of conversation.
const HISTORY_ON_JOIN: i64 = 20;

/// Who the human is talking to. A room reaches whoever joined it; an agent reaches that
/// agent whether or not it ever joined anything, because the DM path enrols both sides.
pub enum ChatTarget {
    Room(String),
    Agent(String),
}

pub async fn run(bus_url: String, target: ChatTarget, name: String) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(&bus_url).await?;
    let (mut sink, mut stream) = ws.split();

    // The room to join and read history from. For a DM this is computed the same way
    // the bus computes it from the send target, so both sides name the same room.
    let room = match &target {
        ChatTarget::Room(r) => r.clone(),
        ChatTarget::Agent(a) => crate::bus::rooms::dm_name(&name, a),
    };
    let send_target = match &target {
        ChatTarget::Room(r) => Target::Room { room: r.clone() },
        ChatTarget::Agent(a) => Target::Agent { name: a.clone() },
    };

    sink.send(Message::text(serde_json::to_string(&ToBus::Register {
        name: name.clone(),
        host: crate::config::RealEnv.hostname(),
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| ".".to_string()),
        session_id: None,
        human: true,
    })?))
    .await?;
    sink.send(Message::text(serde_json::to_string(&ToBus::Join {
        req_id: 1,
        room: room.clone(),
    })?))
    .await?;
    sink.send(Message::text(serde_json::to_string(&ToBus::History {
        req_id: 2,
        room: room.clone(),
        limit: HISTORY_ON_JOIN,
    })?))
    .await?;

    println!("— {room} as {name} — type to send, Ctrl-D to leave —");

    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    // Stdin is blocking, so it gets its own thread rather than starving the runtime.
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut req_id = 100u64;
    loop {
        tokio::select! {
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break };
                let Ok(text) = msg.into_text() else { continue };
                if text.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<FromBus>(&text) {
                    Ok(FromBus::Message { from, text, .. }) => println!("{from}: {text}"),
                    Ok(FromBus::Reply { result: ReplyResult::History { messages }, .. }) => {
                        for m in messages {
                            println!("{}: {}", m.from, m.text);
                        }
                        println!("— live —");
                    }
                    Ok(FromBus::Error { message, .. }) => eprintln!("! {message}"),
                    _ => {}
                }
            }
            line = line_rx.recv() => {
                let Some(line) = line else { break }; // Ctrl-D
                req_id += 1;
                sink.send(Message::text(serde_json::to_string(&ToBus::Send {
                    req_id,
                    target: send_target.clone(),
                    text: line,
                    done: false,
                })?))
                .await?;
            }
        }
    }
    Ok(())
}
