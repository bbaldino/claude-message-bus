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
use std::time::Duration;

use tokio::sync::Mutex;

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

/// One live lease.
pub struct Lease {
    pub name: String,
    pub relayer: bool,
    pub last_seen: tokio::time::Instant,
    pub rx: Arc<Mutex<tokio::sync::mpsc::Receiver<crate::proto::FromBus>>>,
}

/// Token → lease. A cloneable handle over shared state, like `Registry`.
#[derive(Clone)]
pub struct Leases {
    cfg: ParticipantConfig,
    // Wired up in the lease-lifecycle task; the config-plumbing task only needs
    // the type to exist on `App`.
    #[allow(dead_code)]
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
}
