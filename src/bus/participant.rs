//! HTTP participant leases: a token-addressed connection whose lifetime is
//! renewed by polling, layered over `Registry` for presence/delivery.
//!
//! A WebSocket agent's presence is its socket's lifetime; an HTTP participant
//! has no socket to hold open, so a *lease* stands in — created at register,
//! renewed by every receive/send/resume, and swept when it goes stale. The
//! lease owns a `Registry` mpsc receiver, which does double duty: the bus fans
//! delivery into it (so `send_to` reports this participant `delivered`, not
//! `queued`, while the lease is live), and a parked `/receive` long-poll awaits
//! it as its wakeup.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

/// Makes each minted token distinct even within the same nanosecond.
static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Bus-side configuration for HTTP participation. Default = no authority: no
/// secret, no reserved names, so a bus nobody configured behaves as before.
#[derive(Clone, Debug)]
pub struct ParticipantConfig {
    pub relayer_secret: Option<String>,
    pub reserved_names: HashSet<String>,
    pub lease_ttl: Duration,
}

impl Default for ParticipantConfig {
    fn default() -> Self {
        Self {
            relayer_secret: None,
            reserved_names: HashSet::new(),
            lease_ttl: Duration::from_millis(120_000),
        }
    }
}

/// The outcome of resolving a presented `X-Relayer-Secret`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayerDecision {
    /// No secret presented — a plain bot participant.
    Plain,
    /// A configured secret was presented and matches.
    Relayer,
    /// A secret was presented but is wrong, or none is configured — refuse
    /// loudly rather than silently downgrade to a bot lease.
    Refused,
}

/// One live lease.
pub struct Lease {
    pub name: String,
    pub relayer: bool,
    pub last_seen: tokio::time::Instant,
    pub rx: Arc<Mutex<tokio::sync::mpsc::Receiver<crate::proto::FromBus>>>,
}

/// What a handler needs from a live lease: who it speaks as, whether it carries
/// authority, and the wakeup/delivery channel a long-poll awaits. Cheap to clone
/// out of the map so the handler can drop the map lock before it parks.
#[derive(Clone)]
pub struct LeaseHandle {
    pub name: String,
    pub relayer: bool,
    pub rx: Arc<Mutex<tokio::sync::mpsc::Receiver<crate::proto::FromBus>>>,
}

/// Token → lease. A cloneable handle over shared state, like `Registry`.
#[derive(Clone)]
pub struct Leases {
    cfg: ParticipantConfig,
    inner: Arc<Mutex<HashMap<String, Lease>>>,
}

impl Leases {
    pub fn new(cfg: ParticipantConfig) -> Self {
        Self {
            cfg,
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn cfg(&self) -> &ParticipantConfig {
        &self.cfg
    }

    /// Whether a name is reserved to the relayer secret.
    pub fn is_reserved(&self, name: &str) -> bool {
        self.cfg.reserved_names.contains(name)
    }

    /// Resolve a presented `X-Relayer-Secret` into relayer status.
    ///
    /// A wrong or pointless secret is `Refused` rather than silently downgraded
    /// to a bot lease, so a misconfigured relayer fails closed instead of
    /// quietly losing authority.
    pub fn decide_relayer(&self, presented: Option<&str>) -> RelayerDecision {
        match (self.cfg.relayer_secret.as_deref(), presented) {
            (_, None) => RelayerDecision::Plain,
            (Some(want), Some(got)) if want == got => RelayerDecision::Relayer,
            (_, Some(_)) => RelayerDecision::Refused,
        }
    }

    /// Mint a token and store the lease. Returns the token.
    pub async fn open(
        &self,
        name: String,
        relayer: bool,
        rx: tokio::sync::mpsc::Receiver<crate::proto::FromBus>,
    ) -> String {
        let token = mint_token(&name);
        self.inner.lock().await.insert(
            token.clone(),
            Lease {
                name,
                relayer,
                last_seen: tokio::time::Instant::now(),
                rx: Arc::new(Mutex::new(rx)),
            },
        );
        token
    }

    /// Renew a lease's activity clock and hand back a handle, or `None` if the
    /// token is unknown or already swept.
    pub async fn touch(&self, token: &str) -> Option<LeaseHandle> {
        let mut map = self.inner.lock().await;
        let lease = map.get_mut(token)?;
        lease.last_seen = tokio::time::Instant::now();
        Some(LeaseHandle {
            name: lease.name.clone(),
            relayer: lease.relayer,
            rx: lease.rx.clone(),
        })
    }

    /// Whether a lease with this effective name is currently held.
    pub async fn name_online(&self, name: &str) -> bool {
        self.inner.lock().await.values().any(|l| l.name == name)
    }

    /// Remove and return every lease past its TTL, as `(token, name)` pairs, so
    /// the sweeper can detach each from the registry and record the disconnect.
    pub async fn expired(&self) -> Vec<(String, String)> {
        let now = tokio::time::Instant::now();
        let ttl = self.cfg.lease_ttl;
        let mut map = self.inner.lock().await;
        let stale: Vec<String> = map
            .iter()
            .filter(|(_, l)| now.duration_since(l.last_seen) >= ttl)
            .map(|(t, _)| t.clone())
            .collect();
        stale
            .into_iter()
            .map(|t| {
                let name = map.remove(&t).expect("just listed").name;
                (t, name)
            })
            .collect()
    }
}

/// An unguessable-enough session handle for a LAN-trust bus: sha256 over the
/// wall clock, a per-process counter, and the name. Not a CSPRNG — a
/// `rand`-based token is a hardening follow-up — and not the credential either:
/// the relayer secret is. It only needs to be impractical to guess blindly.
fn mint_token(name: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let digest = Sha256::digest(format!("{nanos}:{n}:{name}").as_bytes());
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()[..32]
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_grants_no_authority() {
        let c = ParticipantConfig::default();
        assert!(c.relayer_secret.is_none());
        assert!(c.reserved_names.is_empty());
        assert_eq!(c.lease_ttl, std::time::Duration::from_millis(120_000));
    }

    #[tokio::test]
    async fn touch_renews_and_unknown_token_is_none() {
        let leases = Leases::new(ParticipantConfig::default());
        let (_tx, rx) = tokio::sync::mpsc::channel(8);
        let token = leases.open("raven".into(), true, rx).await;
        let h = leases.touch(&token).await.expect("known token");
        assert_eq!(h.name, "raven");
        assert!(h.relayer);
        assert!(leases.touch("bogus").await.is_none());
    }

    #[tokio::test]
    async fn expired_returns_and_removes_stale_leases() {
        let cfg = ParticipantConfig {
            lease_ttl: std::time::Duration::from_millis(0),
            ..Default::default()
        };
        let leases = Leases::new(cfg);
        let (_tx, rx) = tokio::sync::mpsc::channel(8);
        let token = leases.open("raven".into(), false, rx).await;
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        let gone = leases.expired().await;
        assert_eq!(gone, vec![(token, "raven".to_string())]);
        assert!(
            leases.expired().await.is_empty(),
            "a stale lease is removed on the first sweep"
        );
    }

    #[tokio::test]
    async fn name_online_tracks_the_lease() {
        let leases = Leases::new(ParticipantConfig::default());
        assert!(!leases.name_online("raven").await);
        let (_tx, rx) = tokio::sync::mpsc::channel(8);
        leases.open("raven".into(), false, rx).await;
        assert!(leases.name_online("raven").await);
    }

    #[test]
    fn relayer_decision() {
        let leases = Leases::new(ParticipantConfig {
            relayer_secret: Some("s3cret".into()),
            ..Default::default()
        });
        assert_eq!(
            leases.decide_relayer(Some("s3cret")),
            RelayerDecision::Relayer
        );
        assert_eq!(leases.decide_relayer(None), RelayerDecision::Plain);
        assert_eq!(
            leases.decide_relayer(Some("wrong")),
            RelayerDecision::Refused
        );

        let none = Leases::new(ParticipantConfig::default());
        assert_eq!(
            none.decide_relayer(Some("anything")),
            RelayerDecision::Refused
        );
        assert_eq!(none.decide_relayer(None), RelayerDecision::Plain);
    }

    #[test]
    fn reserved_names_are_recognised() {
        let leases = Leases::new(ParticipantConfig {
            reserved_names: ["raven".to_string()].into_iter().collect(),
            ..Default::default()
        });
        assert!(leases.is_reserved("raven"));
        assert!(!leases.is_reserved("caas"));
    }
}
