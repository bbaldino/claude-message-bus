//! `claude-bus status`: a one-shot table of every agent and its status.
//!
//! Connects as an observer, the way `tail` does, and asks for `list_agents`.
//! No new HTTP client, and no registration that would add a row to `agents`.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::proto::{AgentInfo, FromBus, ReplyResult, ToBus};

pub fn table(agents: &[AgentInfo], now: i64) -> String {
    if agents.is_empty() {
        return "no agents registered yet".to_string();
    }
    let mut rows: Vec<&AgentInfo> = agents.iter().collect();
    // Blocked on the human first: those rows ask something of the reader.
    rows.sort_by_key(|a| {
        let blocked = a.online
            && a.status.as_ref().map(|s| s.state) == Some(crate::proto::AgentState::BlockedOnHuman);
        (!blocked, !a.online, a.name.clone())
    });
    let width = rows
        .iter()
        .map(|a| a.name.chars().count())
        .max()
        .unwrap_or(0);
    rows.iter()
        .map(|a| {
            let status = match &a.status {
                Some(s) => crate::status_text::render_status(s, a.online, now),
                None if a.online => "online, no status".to_string(),
                None => "offline".to_string(),
            };
            format!("{:width$}  {status}", a.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn run(bus: String) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(&bus).await?;
    let (mut sink, mut stream) = ws.split();
    for cmd in [
        ToBus::Observe {
            name: format!("status-{}", std::process::id()),
        },
        ToBus::ListAgents { req_id: 1 },
    ] {
        sink.send(Message::text(serde_json::to_string(&cmd)?))
            .await?;
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let msg = tokio::time::timeout_at(deadline, stream.next())
            .await
            .map_err(|_| anyhow::anyhow!("the bus at {bus} did not answer within 10s"))?;
        let Some(msg) = msg else {
            anyhow::bail!("the bus closed the connection")
        };
        let Ok(text) = msg?.into_text() else { continue };
        match serde_json::from_str::<FromBus>(&text) {
            Ok(FromBus::Reply {
                req_id: 1,
                result: ReplyResult::Agents { agents },
            }) => {
                println!("{}", table(&agents, crate::store::now_ms()));
                return Ok(());
            }
            Ok(FromBus::Error { message, .. }) => anyhow::bail!("bus refused: {message}"),
            _ => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{AgentState, StatusView};

    #[test]
    fn an_empty_bus_says_so() {
        assert_eq!(table(&[], 0), "no agents registered yet");
    }

    #[test]
    fn blocked_agents_sort_first() {
        let mk = |name: &str, state: AgentState| AgentInfo {
            name: name.into(),
            host: "h".into(),
            online: true,
            version: None,
            status: Some(StatusView {
                state,
                since: 0,
                last_heartbeat: None,
                reason: None,
                text: None,
                text_at: None,
                quiet: false,
            }),
        };
        let out = table(
            &[
                mk("a", AgentState::Idle),
                mk("z", AgentState::BlockedOnHuman),
            ],
            0,
        );
        assert!(out.lines().next().unwrap().starts_with('z'), "{out}");
    }
}
