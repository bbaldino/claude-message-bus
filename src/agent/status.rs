//! The agent-side half of status: the state file the hook writes and the bridge reads.
//!
//! The hook (`claude-bus hook`) is a separate, short-lived process that Claude Code
//! spawns on lifecycle events. It cannot reach the bridge's memory, so the two meet
//! at a small JSON file keyed by Claude session id. The file is also what makes
//! status survive a bus restart: the last event's result is still on disk when the
//! bridge reconnects, even if no hook has fired since.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::proto::AgentState;

pub const HEARTBEAT_EVERY_MS: i64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookState {
    pub state: AgentState,
    pub changed_at_ms: i64,
    pub heartbeat_at_ms: i64,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    PromptSubmit,
    ToolUse,
    Stop,
    /// Carries the Notification matcher type that fired it, e.g. `permission_prompt`.
    Blocked(String),
}

impl HookEvent {
    pub fn parse(args: &[String]) -> Option<HookEvent> {
        match args.first().map(String::as_str)? {
            "prompt-submit" => Some(HookEvent::PromptSubmit),
            "tool-use" => Some(HookEvent::ToolUse),
            "stop" => Some(HookEvent::Stop),
            "blocked" => args.get(1).map(|t| HookEvent::Blocked(t.clone())),
            _ => None,
        }
    }
}

/// The file to write, or `None` when nothing changed and no heartbeat is due.
/// Skipping redundant writes is what keeps a `tool-use` hook cheap on every tool call.
pub fn next(prev: Option<&HookState>, ev: &HookEvent, now_ms: i64) -> Option<HookState> {
    let (state, reason) = match ev {
        HookEvent::PromptSubmit | HookEvent::ToolUse => (AgentState::Working, None),
        HookEvent::Stop => (AgentState::Idle, None),
        HookEvent::Blocked(t) => (AgentState::BlockedOnHuman, Some(t.clone())),
    };
    match prev {
        Some(p) if p.state == state && p.reason == reason => {
            (now_ms - p.heartbeat_at_ms >= HEARTBEAT_EVERY_MS).then(|| HookState {
                heartbeat_at_ms: now_ms,
                ..p.clone()
            })
        }
        _ => Some(HookState {
            state,
            changed_at_ms: now_ms,
            heartbeat_at_ms: now_ms,
            reason,
        }),
    }
}

/// The one place both sides derive the file name. The hook passes the `session_id`
/// from its stdin; the bridge passes `CLAUDE_CODE_SESSION_ID`. If Task 0's spike
/// showed those differ, this is the only function to change.
pub fn status_key(session_id: Option<&str>) -> Option<String> {
    let id = session_id?.trim();
    if id.is_empty() {
        return None;
    }
    Some(
        id.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect(),
    )
}

pub fn status_dir(xdg_state_home: Option<String>, home: Option<String>) -> Option<PathBuf> {
    match (xdg_state_home.filter(|s| !s.is_empty()), home) {
        (Some(x), _) => Some(PathBuf::from(x).join("claude-bus/status")),
        (None, Some(h)) => Some(PathBuf::from(h).join(".local/state/claude-bus/status")),
        (None, None) => None,
    }
}

pub fn read(path: &Path) -> Option<HookState> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Atomic: write a sibling temp file, then rename over the target, so the bridge's
/// poller can never read a half-written file.
pub fn write(path: &Path, s: &HookState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(s)?)?;
    std::fs::rename(&tmp, path)
}

/// Everything the bridge knows about its own status. The bridge always sends all of
/// it, never a diff (see `bus::status::Statuses::apply`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalStatus {
    pub hook: Option<HookState>,
    /// The text and when it was set, by this machine's clock.
    pub text: Option<(String, i64)>,
}

pub type StatusTx = std::sync::Arc<tokio::sync::watch::Sender<LocalStatus>>;

fn age(now: i64, then: i64) -> Option<u64> {
    u64::try_from(now - then).ok()
}

impl LocalStatus {
    /// What to send on registration: nothing when there is no status at all, so a
    /// fresh session does not announce an empty one.
    pub fn to_wire(&self, now_ms: i64) -> Option<crate::proto::ToBus> {
        if self.hook.is_none() && self.text.is_none() {
            return None;
        }
        Some(self.to_wire_always(now_ms))
    }

    /// What to send on a change: always a frame, because the bus applies each one as
    /// a full replace. Clearing the text of a session with no hooks leaves nothing,
    /// and only an explicit "unknown, no text" frame tells the bus the old text is gone.
    pub fn to_wire_always(&self, now_ms: i64) -> crate::proto::ToBus {
        let h = self.hook.as_ref();
        crate::proto::ToBus::Status {
            state: h.map_or(AgentState::Unknown, |h| h.state),
            state_age_ms: h.and_then(|h| age(now_ms, h.changed_at_ms)),
            heartbeat_age_ms: h.and_then(|h| age(now_ms, h.heartbeat_at_ms)),
            reason: h.and_then(|h| h.reason.clone()),
            text: self.text.as_ref().map(|(t, _)| t.clone()),
            text_age_ms: self.text.as_ref().and_then(|(_, at)| age(now_ms, *at)),
            waiting_on: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::AgentState::*;

    fn st(state: crate::proto::AgentState, changed: i64, hb: i64) -> HookState {
        HookState {
            state,
            changed_at_ms: changed,
            heartbeat_at_ms: hb,
            reason: None,
        }
    }

    #[test]
    fn events_map_to_states() {
        assert_eq!(
            next(None, &HookEvent::PromptSubmit, 5).unwrap().state,
            Working
        );
        assert_eq!(next(None, &HookEvent::ToolUse, 5).unwrap().state, Working);
        assert_eq!(next(None, &HookEvent::Stop, 5).unwrap().state, Idle);
        let b = next(None, &HookEvent::Blocked("permission_prompt".into()), 5).unwrap();
        assert_eq!(
            (b.state, b.reason.as_deref()),
            (BlockedOnHuman, Some("permission_prompt"))
        );
    }

    #[test]
    fn a_change_resets_since_and_heartbeat() {
        let n = next(Some(&st(Working, 0, 0)), &HookEvent::Stop, 100).unwrap();
        assert_eq!((n.changed_at_ms, n.heartbeat_at_ms), (100, 100));
    }

    #[test]
    fn same_state_only_rewrites_for_a_due_heartbeat() {
        let prev = st(Working, 0, 0);
        assert_eq!(
            next(Some(&prev), &HookEvent::ToolUse, HEARTBEAT_EVERY_MS - 1),
            None
        );
        let n = next(Some(&prev), &HookEvent::ToolUse, HEARTBEAT_EVERY_MS).unwrap();
        assert_eq!(
            (n.changed_at_ms, n.heartbeat_at_ms),
            (0, HEARTBEAT_EVERY_MS)
        );
    }

    #[test]
    fn a_tool_use_after_a_prompt_unblocks() {
        let blocked = HookState {
            reason: Some("permission_prompt".into()),
            ..st(BlockedOnHuman, 0, 0)
        };
        let n = next(Some(&blocked), &HookEvent::ToolUse, 10).unwrap();
        assert_eq!((n.state, n.reason), (Working, None));
    }

    #[test]
    fn args_parse() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            HookEvent::parse(&a(&["prompt-submit"])),
            Some(HookEvent::PromptSubmit)
        );
        assert_eq!(
            HookEvent::parse(&a(&["tool-use"])),
            Some(HookEvent::ToolUse)
        );
        assert_eq!(HookEvent::parse(&a(&["stop"])), Some(HookEvent::Stop));
        assert_eq!(
            HookEvent::parse(&a(&["blocked", "elicitation_dialog"])),
            Some(HookEvent::Blocked("elicitation_dialog".into()))
        );
        assert_eq!(HookEvent::parse(&a(&["blocked"])), None);
        assert_eq!(HookEvent::parse(&a(&["teleport"])), None);
    }

    #[test]
    fn keys_are_filesystem_safe_and_never_empty() {
        assert_eq!(
            status_key(Some("0f9c1d2e-3a4b")).as_deref(),
            Some("0f9c1d2e-3a4b")
        );
        assert_eq!(
            status_key(Some("../../etc/passwd")).as_deref(),
            Some("______etc_passwd")
        );
        assert_eq!(status_key(Some("")), None);
        assert_eq!(status_key(None), None);
    }

    #[test]
    fn the_dir_prefers_xdg_state_home() {
        assert_eq!(
            status_dir(Some("/x".into()), Some("/home/b".into())),
            Some(PathBuf::from("/x/claude-bus/status"))
        );
        assert_eq!(
            status_dir(None, Some("/home/b".into())),
            Some(PathBuf::from("/home/b/.local/state/claude-bus/status"))
        );
        assert_eq!(status_dir(None, None), None);
    }

    #[test]
    fn to_wire_sends_ages_and_whole_status() {
        let ls = LocalStatus {
            hook: Some(st(Idle, 1_000, 9_000)),
            text: Some(("done".into(), 7_000)),
        };
        match ls.to_wire(10_000) {
            Some(crate::proto::ToBus::Status {
                state,
                state_age_ms,
                heartbeat_age_ms,
                text,
                text_age_ms,
                ..
            }) => {
                assert_eq!(state, Idle);
                assert_eq!(state_age_ms, Some(9_000));
                assert_eq!(heartbeat_age_ms, Some(1_000));
                assert_eq!(text.as_deref(), Some("done"));
                assert_eq!(text_age_ms, Some(3_000));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn text_without_hooks_reports_unknown_and_nothing_reports_nothing() {
        assert_eq!(LocalStatus::default().to_wire(0), None);
        let ls = LocalStatus {
            hook: None,
            text: Some(("x".into(), 0)),
        };
        assert!(matches!(
            ls.to_wire(0),
            Some(crate::proto::ToBus::Status { state: Unknown, .. })
        ));
    }

    #[test]
    fn an_emptied_status_still_has_a_frame_for_a_change() {
        match LocalStatus::default().to_wire_always(0) {
            crate::proto::ToBus::Status {
                state: Unknown,
                text: None,
                reason: None,
                ..
            } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn write_then_read_round_trips_and_garbage_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/s.json");
        let s = st(Idle, 1, 2);
        write(&p, &s).unwrap();
        assert_eq!(read(&p), Some(s));
        std::fs::write(&p, "not json").unwrap();
        assert_eq!(read(&p), None);
    }
}
