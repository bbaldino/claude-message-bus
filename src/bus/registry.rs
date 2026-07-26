//! Who is connected right now. Presence is connection lifetime: an agent is
//! online exactly as long as its WebSocket is open.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::proto::FromBus;

/// Per-connection outbound queue depth.
///
/// A bounded channel is what turns "the peer is not keeping up" into a
/// correct, observable `queued` verdict instead of unbounded memory growth.
/// The cap must clear ordinary bursts without ever reporting `queued` for a
/// peer that is simply alive and briefly busy:
///
/// - A room fan-out enqueues exactly one event per *other* member per `send`,
///   not one per member — so a single `send` never contributes more than one
///   entry to any one peer's queue.
/// - The exchange guard's default cap (`DEFAULT_CAP` = 20, see
///   `bus::delivery`) bounds how many rapid-fire messages one room can
///   produce before the room pauses, so a single room's worst case is ~20
///   queued events for a peer that hasn't drained yet.
/// - `send_unread_summaries` adds at most one `Unread` event per room the
///   agent belongs to on reconnect.
///
/// 64 comfortably clears the worst plausible combination of those (one
/// maxed-out room plus a generous number of unread-summary rooms plus the
/// connection's own Registered/Reply traffic) while still bounding memory to
/// a handful of KB per connection if a peer truly vanishes.
pub const CHANNEL_CAPACITY: usize = 64;

pub type Sender = tokio::sync::mpsc::Sender<FromBus>;

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
        let qualified = format!("{name}@{host}");

        // Decide which *root* address this (name, host) pair belongs under,
        // before worrying about numeric suffixes.
        //
        // Checking only `contains_key(name)` is not enough: the bare name
        // can be unclaimed at this instant merely because the connection
        // that held it detached, while *this host* still has a live
        // connection under the qualified form from an earlier cross-host
        // collision (e.g. `dashboard@nas`, maybe itself already suffixed as
        // `dashboard@nas#2`). Handing the bare name back in that situation
        // would give this host two live, differently-shaped identities for
        // the same base name — and `hosts_for` always renders the bare-name
        // holder as `name@host`, so the two would print as the *same*
        // string in the ambiguity list, making them indistinguishable. This
        // is the invariant boundary: a root may only be the bare `name` if
        // no other live connection anywhere (bare-holder-on-this-host
        // aside) already established a qualified family for this host.
        let root = match conns.get(name) {
            // This host already holds (or is re-registering into) the bare
            // name: same-host family, stays on the bare root and gets the
            // next `#N` suffix if occupied.
            Some(c) if c.host == host => name.to_string(),
            // A different host holds the bare name: must qualify.
            Some(_) => qualified.clone(),
            // Bare name currently unclaimed by anyone. Only grant it fresh
            // if this host has no existing qualified family (exact or
            // suffixed) already in play; otherwise keep using that family.
            None => {
                let qualified_prefix = format!("{qualified}#");
                if conns.contains_key(&qualified)
                    || conns.keys().any(|k| k.starts_with(&qualified_prefix))
                {
                    qualified.clone()
                } else {
                    name.to_string()
                }
            }
        };

        let effective = if !conns.contains_key(&root) {
            root
        } else {
            let mut n = 2;
            loop {
                let c = format!("{root}#{n}");
                if !conns.contains_key(&c) {
                    break c;
                }
                n += 1;
            }
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

    /// `true` means the event was actually queued onto the peer's connection
    /// — the sole basis for the `delivered_to` / `queued_for` split callers
    /// report back to the model. `false` covers both "no such peer" and "that
    /// peer's queue is full" (i.e. it is not draining, whether because it is
    /// gone or just badly behind): either way the honest answer is `queued`,
    /// never `delivered`. `try_send` is synchronous, so this never awaits
    /// while holding the registry lock.
    pub async fn send_to(&self, name: &str, msg: FromBus) -> bool {
        let conns = self.conns.lock().await;
        match conns.get(name) {
            Some(c) => c.tx.try_send(msg).is_ok(),
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

    fn channel() -> (Sender, tokio::sync::mpsc::Receiver<FromBus>) {
        tokio::sync::mpsc::channel(CHANNEL_CAPACITY)
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

    /// Regression for a defect found in fix round 1: `attach` decided the
    /// bare-name fast path with a single `contains_key(name)` check, which
    /// only sees whether the exact bare key is free — it misses that this
    /// host already has a *live* qualified connection for the same base
    /// name (from an earlier cross-host collision). Freeing the bare key by
    /// detaching one connection must not let a second, still-live connection
    /// on the same host claim it: that would give the host two apparently
    /// distinct identities, and `hosts_for` would render both as the same
    /// `name@host` string, making them indistinguishable.
    #[tokio::test]
    async fn a_second_live_connection_on_a_host_does_not_reclaim_a_freed_bare_name() {
        let reg = Registry::new();
        let (tx1, _r1) = channel();
        let (tx2, _r2) = channel();
        let (tx3, _r3) = channel();

        assert_eq!(reg.attach("dashboard", "lisa", tx1).await, "dashboard");
        assert_eq!(reg.attach("dashboard", "nas", tx2).await, "dashboard@nas");
        reg.detach("dashboard").await;
        let third = reg.attach("dashboard", "nas", tx3).await;

        assert_ne!(
            third, "dashboard",
            "nas already has a live connection (dashboard@nas); it must not \
             also be handed the freed bare name: got {third:?}"
        );

        // Every live connection must render as a distinct string from
        // hosts_for, or the ambiguity list can't actually disambiguate them.
        let hosts = reg.hosts_for("dashboard").await;
        let mut deduped = hosts.clone();
        deduped.dedup();
        assert_eq!(
            hosts.len(),
            deduped.len(),
            "hosts_for must not render two live connections identically: {hosts:?}"
        );
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

    /// This is the second half of the defect fix: an unbounded channel let a
    /// wedged peer's queue grow forever while `send_to` kept returning `true`
    /// (delivered). With a bounded channel, a full queue is itself a correct
    /// `queued` signal. Sabotage check: swapping `try_send` back for `send`
    /// on an unbounded channel (or just not draining `rx` before asserting)
    /// makes this pass for the wrong reason, so the receiver here is
    /// deliberately never drained — the capacity is the only thing standing
    /// between "queued" and "delivered".
    #[tokio::test]
    async fn send_to_a_full_channel_reports_failure_not_delivery() {
        let reg = Registry::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        reg.attach("caas", "lisa", tx).await;

        // Fill the one slot. Nobody ever calls `rx.recv()`, so this is the
        // only message the channel will ever hold.
        assert!(
            reg.send_to(
                "caas",
                FromBus::Registered {
                    name: "caas".into()
                }
            )
            .await,
            "the first message has room and must be reported delivered"
        );

        // The channel is now full: the peer is not keeping up (or is gone),
        // and that must read as queued, not delivered.
        assert!(
            !reg.send_to(
                "caas",
                FromBus::Registered {
                    name: "caas".into()
                }
            )
            .await,
            "a full queue must never be reported as delivered"
        );
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
