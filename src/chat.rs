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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatTarget {
    Room(String),
    Agent(String),
}

/// Resolve `chat`'s `(<room> | --to <agent>)` arguments into a single target.
/// `None` is the usage-error case — both given, or neither — which `main` turns into its
/// `exit(2)`; kept pure and returning `Option` rather than exiting itself so it can be
/// unit-tested without touching process state.
pub fn chat_target(positional: Option<String>, to: Option<String>) -> Option<ChatTarget> {
    match (positional, to) {
        (Some(room), None) => Some(ChatTarget::Room(room)),
        (None, Some(agent)) => Some(ChatTarget::Agent(agent)),
        _ => None,
    }
}

/// The room a chat session joins and reads history from, for a given target and this
/// client's own name. For a DM this must land on the same room the bus computes from the
/// send target (`bus::rooms::resolve`), so both sides agree on where the conversation lives.
pub fn room_for(target: &ChatTarget, name: &str) -> String {
    match target {
        ChatTarget::Room(r) => r.clone(),
        ChatTarget::Agent(a) => crate::bus::rooms::dm_name(name, a),
    }
}

pub async fn run(bus_url: String, target: ChatTarget, name: String) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(&bus_url).await?;
    let (mut sink, mut stream) = ws.split();

    let room = room_for(&target, &name);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_only_resolves_to_a_room_target() {
        assert_eq!(
            chat_target(Some("standup".to_string()), None),
            Some(ChatTarget::Room("standup".to_string()))
        );
    }

    #[test]
    fn to_only_resolves_to_an_agent_target() {
        assert_eq!(
            chat_target(None, Some("caas".to_string())),
            Some(ChatTarget::Agent("caas".to_string()))
        );
    }

    #[test]
    fn both_given_is_a_usage_error() {
        assert_eq!(
            chat_target(Some("standup".to_string()), Some("caas".to_string())),
            None
        );
    }

    #[test]
    fn neither_given_is_a_usage_error() {
        assert_eq!(chat_target(None, None), None);
    }

    #[test]
    fn room_for_a_room_target_is_the_room_itself() {
        let target = ChatTarget::Room("standup".to_string());
        assert_eq!(room_for(&target, "dashboard"), "standup");
    }

    #[test]
    fn room_for_an_agent_target_matches_what_the_bus_resolves_from_the_send_target() {
        // The DM path only works if the client and the bus land on the same room name:
        // the client picks it here to join and read history, while the bus picks it
        // independently in `bus::rooms::resolve` from the `Send`'s `Target::Agent`. If
        // the two ever computed it differently, a human would join a room the bus never
        // delivers into.
        let target = ChatTarget::Agent("caas".to_string());
        let name = "dashboard";
        let via_chat = room_for(&target, name);
        let via_bus = crate::bus::rooms::resolve(
            &crate::proto::Target::Agent {
                name: "caas".to_string(),
            },
            name,
        );
        assert_eq!(via_chat, via_bus);
    }
}
