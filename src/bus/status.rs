//! Agent status, kept in memory beside the registry.
//!
//! Never persisted, on purpose: a persisted `online` column once showed agents
//! from a dead bus as online. After a bus restart, agents resend their status
//! on registration (see `agent::bridge`), so nothing is lost by keeping this in
//! memory. Staleness (`quiet`) is computed on every read, never stored.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::proto::{AgentState, StatusView};

pub(crate) const TEXT_CAP: usize = 200;
pub(crate) const QUIET_AFTER_MS: i64 = 10 * 60 * 1000;

pub(crate) struct Update {
    pub state: AgentState,
    pub state_age_ms: Option<u64>,
    pub heartbeat_age_ms: Option<u64>,
    pub reason: Option<String>,
    pub text: Option<String>,
    pub text_age_ms: Option<u64>,
}

pub(crate) struct Transition {
    pub before: Option<AgentState>,
    pub after: AgentState,
    pub reason: Option<String>,
}

#[derive(Clone)]
struct Entry {
    state: AgentState,
    since: i64,
    last_heartbeat: Option<i64>,
    reason: Option<String>,
    text: Option<String>,
    text_at: Option<i64>,
}

#[derive(Clone, Default)]
pub(crate) struct Statuses(Arc<Mutex<HashMap<String, Entry>>>);

/// One line, at most `TEXT_CAP` characters. Line breaks become spaces, so every
/// renderer (tool text, CLI table, console row) can treat the text as a single line.
pub(crate) fn truncate_text(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(TEXT_CAP)
        .collect()
}

fn at(now: i64, age_ms: Option<u64>) -> Option<i64> {
    age_ms.map(|a| now - i64::try_from(a).unwrap_or(i64::MAX).min(now))
}

impl Statuses {
    pub(crate) async fn apply(&self, name: &str, u: Update, now: i64) -> Transition {
        let mut map = self.0.lock().await;
        let before = map.get(name).map(|e| e.state);
        let text = u.text.map(|t| truncate_text(&t)).filter(|t| !t.is_empty());
        let text_at = text.as_ref().and(at(now, u.text_age_ms).or(Some(now)));
        let entry = Entry {
            state: u.state,
            since: at(now, u.state_age_ms).unwrap_or(now),
            last_heartbeat: at(now, u.heartbeat_age_ms),
            reason: u.reason.clone(),
            text,
            text_at,
        };
        map.insert(name.to_string(), entry);
        Transition {
            before,
            after: u.state,
            reason: u.reason,
        }
    }

    pub(crate) async fn view(&self, name: &str, now: i64) -> Option<StatusView> {
        self.0.lock().await.get(name).map(|e| view_of(e, now))
    }

    pub(crate) async fn all(&self, now: i64) -> HashMap<String, StatusView> {
        self.0
            .lock()
            .await
            .iter()
            .map(|(k, e)| (k.clone(), view_of(e, now)))
            .collect()
    }

    pub(crate) async fn state_of(&self, name: &str) -> Option<AgentState> {
        self.0.lock().await.get(name).map(|e| e.state)
    }

    pub(crate) async fn remove(&self, name: &str) -> Option<AgentState> {
        self.0.lock().await.remove(name).map(|e| e.state)
    }
}

fn view_of(e: &Entry, now: i64) -> StatusView {
    let last_signal = e.last_heartbeat.unwrap_or(e.since).max(e.since);
    StatusView {
        state: e.state,
        since: e.since,
        last_heartbeat: e.last_heartbeat,
        reason: e.reason.clone(),
        text: e.text.clone(),
        text_at: e.text_at,
        quiet: e.state == AgentState::Working && now - last_signal > QUIET_AFTER_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::AgentState;

    fn upd(state: AgentState) -> Update {
        Update {
            state,
            state_age_ms: Some(0),
            heartbeat_age_ms: Some(0),
            reason: None,
            text: None,
            text_age_ms: None,
        }
    }

    #[tokio::test]
    async fn ages_become_bus_times() {
        let s = Statuses::default();
        let mut u = upd(AgentState::Idle);
        u.state_age_ms = Some(60_000);
        u.text = Some("wrote anchor.verified.json".into());
        u.text_age_ms = Some(5_000);
        s.apply("caas", u, 1_000_000).await;
        let v = s.view("caas", 1_000_000).await.unwrap();
        assert_eq!(v.since, 940_000);
        assert_eq!(v.text_at, Some(995_000));
        assert_eq!(v.text.as_deref(), Some("wrote anchor.verified.json"));
    }

    #[tokio::test]
    async fn quiet_is_derived_at_read_time_and_only_while_working() {
        let s = Statuses::default();
        s.apply("w", upd(AgentState::Working), 0).await;
        assert!(!s.view("w", QUIET_AFTER_MS).await.unwrap().quiet);
        assert!(s.view("w", QUIET_AFTER_MS + 1).await.unwrap().quiet);
        s.apply("i", upd(AgentState::Idle), 0).await;
        assert!(!s.view("i", QUIET_AFTER_MS * 10).await.unwrap().quiet);
    }

    #[tokio::test]
    async fn text_is_capped_on_a_char_boundary_and_flattened() {
        let s = Statuses::default();
        let mut u = upd(AgentState::Working);
        u.text = Some(format!("é{}\nsecond line", "x".repeat(5_000)));
        s.apply("a", u, 0).await;
        let t = s.view("a", 0).await.unwrap().text.unwrap();
        assert_eq!(t.chars().count(), TEXT_CAP);
        assert!(!t.contains('\n'), "status text renders on one line: {t:?}");
    }

    #[tokio::test]
    async fn empty_text_clears_it() {
        let s = Statuses::default();
        let mut u = upd(AgentState::Working);
        u.text = Some("busy".into());
        s.apply("a", u, 0).await;
        let mut u = upd(AgentState::Working);
        u.text = Some(String::new());
        s.apply("a", u, 0).await;
        assert_eq!(s.view("a", 0).await.unwrap().text, None);
    }

    #[tokio::test]
    async fn apply_reports_the_transition() {
        let s = Statuses::default();
        let t = s.apply("a", upd(AgentState::Working), 0).await;
        assert_eq!((t.before, t.after), (None, AgentState::Working));
        let mut b = upd(AgentState::BlockedOnHuman);
        b.reason = Some("permission_prompt".into());
        let t = s.apply("a", b, 1).await;
        assert_eq!(
            (t.before, t.after),
            (Some(AgentState::Working), AgentState::BlockedOnHuman)
        );
        assert_eq!(t.reason.as_deref(), Some("permission_prompt"));
    }

    #[tokio::test]
    async fn remove_forgets_the_agent() {
        let s = Statuses::default();
        s.apply("a", upd(AgentState::Idle), 0).await;
        assert_eq!(s.remove("a").await, Some(AgentState::Idle));
        assert!(s.view("a", 0).await.is_none());
    }
}
