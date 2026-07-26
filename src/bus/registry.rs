//! Who is connected right now. Presence is connection lifetime: an agent is
//! online exactly as long as its WebSocket is open.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::proto::FromBus;

pub type Sender = tokio::sync::mpsc::UnboundedSender<FromBus>;

struct Conn {
    host: String,
    tx: Sender,
}

#[derive(Clone)]
pub struct Registry {
    conns: Arc<Mutex<HashMap<String, Conn>>>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            conns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a connection, returning the *effective* name. A collision on a
    /// different host qualifies to `name@host`; on the same host it suffixes
    /// `#2`, `#3`, … Nothing is ever silently renamed out from under a caller
    /// that already holds a name.
    pub async fn attach(&self, name: &str, host: &str, tx: Sender) -> String {
        let mut conns = self.conns.lock().await;
        if !conns.contains_key(name) {
            conns.insert(
                name.to_string(),
                Conn {
                    host: host.to_string(),
                    tx,
                },
            );
            return name.to_string();
        }
        let existing_host = conns.get(name).map(|c| c.host.clone()).unwrap_or_default();
        let candidate = if existing_host != host {
            format!("{name}@{host}")
        } else {
            let mut n = 2;
            loop {
                let c = format!("{name}#{n}");
                if !conns.contains_key(&c) {
                    break c;
                }
                n += 1;
            }
        };
        // The qualified form can itself collide if two same-named agents share a
        // host *and* an earlier qualified name; fall through to numeric suffixes.
        let effective = if conns.contains_key(&candidate) {
            let mut n = 2;
            loop {
                let c = format!("{candidate}#{n}");
                if !conns.contains_key(&c) {
                    break c;
                }
                n += 1;
            }
        } else {
            candidate
        };
        conns.insert(
            effective.clone(),
            Conn {
                host: host.to_string(),
                tx,
            },
        );
        effective
    }

    pub async fn detach(&self, name: &str) {
        self.conns.lock().await.remove(name);
    }

    pub async fn send_to(&self, name: &str, msg: FromBus) -> bool {
        let conns = self.conns.lock().await;
        match conns.get(name) {
            Some(c) => c.tx.send(msg).is_ok(),
            None => false,
        }
    }

    pub async fn online(&self) -> Vec<String> {
        let mut names: Vec<String> = self.conns.lock().await.keys().cloned().collect();
        names.sort();
        names
    }

    /// Every effective name whose base matches `base`, for building the
    /// "ambiguous: dashboard@lisa, dashboard@nas" error.
    pub async fn hosts_for(&self, base: &str) -> Vec<String> {
        let conns = self.conns.lock().await;
        let mut out: Vec<String> = conns
            .iter()
            .filter(|(name, _)| {
                name.as_str() == base
                    || name.starts_with(&format!("{base}@"))
                    || name.starts_with(&format!("{base}#"))
            })
            .map(|(name, c)| {
                if name == base {
                    format!("{name}@{}", c.host)
                } else {
                    name.clone()
                }
            })
            .collect();
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::FromBus;

    fn channel() -> (Sender, tokio::sync::mpsc::UnboundedReceiver<FromBus>) {
        tokio::sync::mpsc::unbounded_channel()
    }

    #[tokio::test]
    async fn first_registration_keeps_its_name() {
        let reg = Registry::new();
        let (tx, _rx) = channel();
        assert_eq!(reg.attach("caas", "lisa", tx).await, "caas");
    }

    #[tokio::test]
    async fn same_name_on_a_different_host_keeps_the_name() {
        // Both stay addressable; disambiguation happens at send time via name@host.
        let reg = Registry::new();
        let (tx1, _r1) = channel();
        let (tx2, _r2) = channel();
        assert_eq!(reg.attach("dashboard", "lisa", tx1).await, "dashboard");
        assert_eq!(reg.attach("dashboard", "nas", tx2).await, "dashboard@nas");
        let hosts = reg.hosts_for("dashboard").await;
        assert_eq!(hosts.len(), 2, "both hosts registered: {hosts:?}");
    }

    /// `hosts_for` is what builds the "ambiguous: dashboard@lisa, dashboard@nas"
    /// error text, so it must qualify *every* match — including the connection
    /// that is holding the bare, unqualified name — not just count them.
    #[tokio::test]
    async fn hosts_for_qualifies_the_bare_name_holder_too() {
        let reg = Registry::new();
        let (tx1, _r1) = channel();
        let (tx2, _r2) = channel();
        reg.attach("dashboard", "lisa", tx1).await;
        reg.attach("dashboard", "nas", tx2).await;
        let hosts = reg.hosts_for("dashboard").await;
        assert_eq!(hosts, vec!["dashboard@lisa", "dashboard@nas"]);
    }

    #[tokio::test]
    async fn second_session_on_the_same_host_gets_a_suffix() {
        let reg = Registry::new();
        let (tx1, _r1) = channel();
        let (tx2, _r2) = channel();
        assert_eq!(reg.attach("caas", "lisa", tx1).await, "caas");
        assert_eq!(reg.attach("caas", "lisa", tx2).await, "caas#2");
    }

    #[tokio::test]
    async fn detach_frees_the_name_for_reuse() {
        let reg = Registry::new();
        let (tx1, _r1) = channel();
        reg.attach("caas", "lisa", tx1).await;
        reg.detach("caas").await;
        let (tx2, _r2) = channel();
        assert_eq!(reg.attach("caas", "lisa", tx2).await, "caas");
    }

    #[tokio::test]
    async fn send_to_a_connected_agent_delivers() {
        let reg = Registry::new();
        let (tx, mut rx) = channel();
        reg.attach("caas", "lisa", tx).await;
        assert!(
            reg.send_to(
                "caas",
                FromBus::Registered {
                    name: "caas".into()
                }
            )
            .await
        );
        assert!(rx.recv().await.is_some());
    }

    #[tokio::test]
    async fn send_to_an_absent_agent_reports_failure() {
        let reg = Registry::new();
        assert!(
            !reg.send_to(
                "ghost",
                FromBus::Registered {
                    name: "ghost".into()
                }
            )
            .await
        );
    }

    #[tokio::test]
    async fn online_lists_effective_names_sorted() {
        let reg = Registry::new();
        let (tx1, _r1) = channel();
        let (tx2, _r2) = channel();
        reg.attach("dashboard", "lisa", tx1).await;
        reg.attach("caas", "lisa", tx2).await;
        assert_eq!(reg.online().await, vec!["caas", "dashboard"]);
    }
}
