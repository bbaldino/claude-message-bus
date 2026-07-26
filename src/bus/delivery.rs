//! Runaway guards. Two agents replying to each other will volley indefinitely,
//! each reply triggering the other's channel. Overnight that is real money.
//!
//! The cap default of 20 comes from POC 3, where a real negotiation converged
//! in eight messages. It is a backstop at ~2.5x observed length, not a working
//! limit — the models already self-terminate when instructed to.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

pub const DEFAULT_CAP: u32 = 20;
pub const DEFAULT_MIN_INTERVAL_MS: i64 = 2000;

#[derive(Debug, PartialEq, Eq)]
pub enum GuardVerdict {
    Allow,
    RateLimited { retry_in_ms: i64 },
    Paused { count: u32 },
}

#[derive(Default)]
struct RoomState {
    /// Messages in this room since the last human input.
    exchanges: u32,
    /// Last send time per agent, for the rate limit.
    last_send: HashMap<String, i64>,
}

#[derive(Clone)]
pub struct Guards {
    cap: u32,
    min_interval_ms: i64,
    rooms: Arc<Mutex<HashMap<String, RoomState>>>,
}

impl Default for Guards {
    fn default() -> Self {
        Self::new(DEFAULT_CAP, DEFAULT_MIN_INTERVAL_MS)
    }
}

impl Guards {
    pub fn new(cap: u32, min_interval_ms: i64) -> Self {
        Self {
            cap,
            min_interval_ms,
            rooms: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// `now_ms` is passed in rather than read here so the rate limit is
    /// testable without sleeping.
    pub async fn check(&self, room: &str, agent: &str, now_ms: i64) -> GuardVerdict {
        let mut rooms = self.rooms.lock().await;
        let state = rooms.entry(room.to_string()).or_default();

        if state.exchanges >= self.cap {
            return GuardVerdict::Paused {
                count: state.exchanges,
            };
        }

        if self.min_interval_ms > 0
            && let Some(last) = state.last_send.get(agent)
        {
            let elapsed = now_ms - last;
            if elapsed < self.min_interval_ms {
                return GuardVerdict::RateLimited {
                    retry_in_ms: self.min_interval_ms - elapsed,
                };
            }
        }

        state.exchanges += 1;
        state.last_send.insert(agent.to_string(), now_ms);
        GuardVerdict::Allow
    }

    pub async fn reset(&self, room: &str) {
        if let Some(state) = self.rooms.lock().await.get_mut(room) {
            state.exchanges = 0;
        }
    }

    pub async fn reset_all_for(&self, rooms: &[String]) {
        let mut guard = self.rooms.lock().await;
        for r in rooms {
            if let Some(state) = guard.get_mut(r) {
                state.exchanges = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allows_normal_traffic() {
        let g = Guards::new(20, 0);
        assert!(matches!(
            g.check("r", "caas", 1000).await,
            GuardVerdict::Allow
        ));
    }

    #[tokio::test]
    async fn rate_limits_a_too_fast_second_message() {
        let g = Guards::new(20, 2000);
        assert!(matches!(
            g.check("r", "caas", 1000).await,
            GuardVerdict::Allow
        ));
        match g.check("r", "caas", 1500).await {
            GuardVerdict::RateLimited { retry_in_ms } => assert_eq!(retry_in_ms, 1500),
            other => panic!("expected rate limit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rate_limit_is_per_agent_per_room() {
        let g = Guards::new(20, 2000);
        g.check("r", "caas", 1000).await;
        // A different agent in the same room is unaffected.
        assert!(matches!(
            g.check("r", "dashboard", 1000).await,
            GuardVerdict::Allow
        ));
        // The same agent in a different room is unaffected.
        assert!(matches!(
            g.check("other", "caas", 1000).await,
            GuardVerdict::Allow
        ));
    }

    #[tokio::test]
    async fn pauses_after_the_cap_is_reached() {
        let g = Guards::new(3, 0);
        for i in 0..3 {
            assert!(
                matches!(g.check("r", "caas", i).await, GuardVerdict::Allow),
                "message {i} should pass"
            );
        }
        match g.check("r", "caas", 99).await {
            GuardVerdict::Paused { count } => assert_eq!(count, 3),
            other => panic!("expected pause, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_cap_counts_the_room_not_the_individual_agent() {
        let g = Guards::new(2, 0);
        assert!(matches!(g.check("r", "caas", 0).await, GuardVerdict::Allow));
        assert!(matches!(
            g.check("r", "dashboard", 1).await,
            GuardVerdict::Allow
        ));
        assert!(matches!(
            g.check("r", "caas", 2).await,
            GuardVerdict::Paused { .. }
        ));
    }

    #[tokio::test]
    async fn reset_clears_a_pause() {
        let g = Guards::new(1, 0);
        g.check("r", "caas", 0).await;
        assert!(matches!(
            g.check("r", "caas", 1).await,
            GuardVerdict::Paused { .. }
        ));
        g.reset("r").await;
        assert!(matches!(g.check("r", "caas", 2).await, GuardVerdict::Allow));
    }

    #[tokio::test]
    async fn default_cap_matches_the_spec() {
        let g = Guards::default();
        for i in 0..20 {
            assert!(matches!(
                g.check("r", "a", i * 10_000).await,
                GuardVerdict::Allow
            ));
        }
        assert!(matches!(
            g.check("r", "a", 999_999).await,
            GuardVerdict::Paused { .. }
        ));
    }
}
