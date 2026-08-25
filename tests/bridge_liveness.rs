//! The bridge's own liveness detection.
//!
//! The bug these exist for: the bus dropped a sleeping laptop's agent on a
//! keepalive timeout and closed its side. That FIN was lost, the bus never
//! wrote to the socket again, and the client sat in `connect_once` — which
//! wakes only on "the model wants to send" or "bytes arrived" — forever. The
//! reconnect loop it already had never got a turn. Six days offline, process
//! alive, socket still ESTAB.

mod common;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

use claude_bus::agent::bridge::Liveness;
use common::{InProcessAgent, initialize};

/// A bus that completes the WebSocket handshake and then goes silent, without
/// ever closing — the exact shape a sleeping laptop leaves behind. Sends one
/// `()` per accepted handshake so a test can count reconnects.
async fn silent_bus() -> (u16, mpsc::UnboundedReceiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let _ = tx.send(());
                // Held, never dropped: dropping sends a close frame, which is
                // precisely the signal this test exists to withhold.
                let _held = ws;
                std::future::pending::<()>().await;
            });
        }
    });
    (port, rx)
}

/// A bus that answers. Pings every 50ms and drains whatever the client sends,
/// which is also what makes tungstenite emit the automatic pongs.
async fn chatty_bus() -> (u16, mpsc::UnboundedReceiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let _ = tx.send(());
                let (mut sink, mut stream) = ws.split();
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(50)) => {
                            if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                                return;
                            }
                        }
                        msg = stream.next() => {
                            if msg.is_none() { return }
                        }
                    }
                }
            });
        }
    });
    (port, rx)
}

async fn start_agent(port: u16, name: &str) -> InProcessAgent {
    let mut a = InProcessAgent::start_isolated_with_liveness(
        format!("ws://127.0.0.1:{port}/ws"),
        name,
        Liveness {
            ping_interval: Duration::from_millis(100),
            idle_timeout: Duration::from_millis(300),
        },
    );
    initialize(&mut a).await;
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    a
}

#[tokio::test]
async fn a_silent_bus_that_never_closes_still_gets_a_reconnect() {
    let (port, mut handshakes) = silent_bus().await;
    let _agent = start_agent(port, "sleeper").await;

    timeout(Duration::from_secs(5), handshakes.recv())
        .await
        .expect("the bridge never connected at all")
        .expect("handshake channel closed");

    timeout(Duration::from_secs(5), handshakes.recv())
        .await
        .expect(
            "the bridge never reconnected: it is still waiting on a socket the bus \
             will never write to, which is the bug",
        )
        .expect("handshake channel closed");
}

#[tokio::test]
async fn a_bus_that_keeps_talking_is_not_torn_down() {
    // THE REGRESSION THAT WOULD MATTER MOST. A timer that fires on a healthy
    // connection makes every long-lived agent flap, which is worse than the
    // bug being fixed: this one is silent and rare, that one is constant.
    let (port, mut handshakes) = chatty_bus().await;
    let _agent = start_agent(port, "chatty").await;

    timeout(Duration::from_secs(5), handshakes.recv())
        .await
        .expect("the bridge never connected at all")
        .expect("handshake channel closed");

    // Five times the idle timeout, with the bus talking the whole way.
    assert!(
        timeout(Duration::from_millis(1500), handshakes.recv())
            .await
            .is_err(),
        "a connection carrying traffic was torn down and rebuilt"
    );
}
