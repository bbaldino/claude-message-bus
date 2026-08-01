//! Read-only viewer. Registers as an observer, prints history, then streams.
//!
//! Claude Code shows a session its inbound channel events but hides the text it
//! sends back, so no participant's terminal shows both halves of a
//! conversation. This does.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::proto::{FromBus, ReplyResult, ToBus};

/// Formats one inbound `FromBus::Message` for display. `tail` is the
/// authoritative view of a conversation, so it marks human-sent messages the
/// same way the `history` MCP tool does (see `agent::handler`) — appending
/// `" (human)"` after the sender name — rather than inventing a second
/// convention for the same fact.
fn format_message_line(room: &str, from: &str, text: &str, done: bool, human: bool) -> String {
    format!(
        "{from}{} → {room}: {text}{}",
        if human { " (human)" } else { "" },
        if done { "  [done]" } else { "" }
    )
}

pub async fn run(bus_url: String, room: Option<String>) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(&bus_url).await?;
    let (mut sink, mut stream) = ws.split();

    // `Observe`, not `Register`: a viewer is not a participant. This gives
    // the connection an identity for its lifetime — satisfying the bus's
    // "register before sending commands" gate — without ever creating an
    // `agents` row or a `room_members` row. See `ToBus::Observe`.
    let observer = format!("tail-{}", std::process::id());
    sink.send(Message::text(serde_json::to_string(&ToBus::Observe {
        name: observer.clone(),
    })?))
    .await?;

    if let Some(room) = &room {
        // `Watch`, not `Join`: this room's live traffic reaches us without
        // ever making us a member of it.
        sink.send(Message::text(serde_json::to_string(&ToBus::Watch {
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
                human,
                ..
            } => {
                println!("{}", format_message_line(&room, &from, &text, done, human));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_human_senders() {
        let line = format_message_line("general", "bbaldino", "hello", false, true);
        assert_eq!(line, "bbaldino (human) → general: hello");
    }

    #[test]
    fn does_not_mark_agent_senders() {
        let line = format_message_line("general", "claude-code", "hello", false, false);
        assert_eq!(line, "claude-code → general: hello");
    }

    #[test]
    fn appends_done_marker_after_human_marker() {
        let line = format_message_line("general", "bbaldino", "hello", true, true);
        assert_eq!(line, "bbaldino (human) → general: hello  [done]");
    }
}
