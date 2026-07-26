//! Read-only viewer. Registers as an observer, prints history, then streams.
//!
//! Claude Code shows a session its inbound channel events but hides the text it
//! sends back, so no participant's terminal shows both halves of a
//! conversation. This does.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::proto::{FromBus, ReplyResult, ToBus};

pub async fn run(bus_url: String, room: Option<String>) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(&bus_url).await?;
    let (mut sink, mut stream) = ws.split();

    let observer = format!("tail-{}", std::process::id());
    sink.send(Message::text(serde_json::to_string(&ToBus::Register {
        name: observer.clone(),
        host: "observer".into(),
        cwd: ".".into(),
        session_id: None,
    })?))
    .await?;

    if let Some(room) = &room {
        sink.send(Message::text(serde_json::to_string(&ToBus::Join {
            req_id: 1,
            room: room.clone(),
        })?))
        .await?;
        sink.send(Message::text(serde_json::to_string(&ToBus::History {
            req_id: 2,
            room: room.clone(),
            limit: 50,
        })?))
        .await?;
        println!("— watching {room} —");
    } else {
        sink.send(Message::text(serde_json::to_string(&ToBus::ListRooms {
            req_id: 3,
        })?))
        .await?;
    }

    while let Some(msg) = stream.next().await {
        let Ok(text) = msg?.into_text() else { continue };
        if text.trim().is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<FromBus>(&text) else {
            continue;
        };
        match event {
            FromBus::Message {
                room,
                from,
                text,
                done,
                ..
            } => {
                println!(
                    "{from} → {room}: {text}{}",
                    if done { "  [done]" } else { "" }
                );
            }
            FromBus::Reply {
                result: ReplyResult::History { messages },
                ..
            } => {
                for m in messages {
                    println!("{}: {}", m.from, m.text);
                }
                println!("— live —");
            }
            FromBus::Reply {
                result: ReplyResult::Rooms { rooms },
                ..
            } => {
                if rooms.is_empty() {
                    println!("no rooms yet");
                } else {
                    println!("rooms:");
                    for r in rooms {
                        println!("  {} — {}", r.name, r.members.join(", "));
                    }
                }
                println!("\npass a room name to watch one: claude-bus tail <room>");
                return Ok(());
            }
            FromBus::Paused { room, reason } => println!("!! {room} paused: {reason}"),
            FromBus::Error { message, .. } => eprintln!("error: {message}"),
            _ => {}
        }
    }
    Ok(())
}
