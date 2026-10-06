//! One-line rendering of an agent status, shared by the `agents` MCP tool and the
//! `claude-bus status` CLI so the two never describe the same status differently.

use crate::proto::{AgentState, StatusView};

pub fn age(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3_600 => format!("{}m", s / 60),
        3_600..86_400 if s % 3_600 / 60 == 0 => format!("{}h", s / 3_600),
        3_600..86_400 => format!("{}h{}m", s / 3_600, s % 3_600 / 60),
        _ => format!("{}d", s / 86_400),
    }
}

fn state_word(s: AgentState) -> &'static str {
    match s {
        AgentState::Working => "working",
        AgentState::Idle => "idle",
        AgentState::BlockedOnHuman => "blocked on human",
        AgentState::Unknown => "unknown",
    }
}

pub fn render_status(s: &StatusView, online: bool, now: i64) -> String {
    let since = age(now - s.since);
    // "Offline" and "quiet" are about how long since the agent was last heard from,
    // which is the later of the state change and the last heartbeat.
    let heard = age(now - s.last_heartbeat.unwrap_or(s.since).max(s.since));
    let mut out = if !online {
        format!("offline, was {} {heard} ago", state_word(s.state))
    } else if s.quiet {
        format!("working? (quiet) {heard}")
    } else if let Some(r) = s
        .reason
        .as_deref()
        .filter(|_| s.state == AgentState::BlockedOnHuman)
    {
        format!("{} ({r}) {since}", state_word(s.state))
    } else {
        format!("{} {since}", state_word(s.state))
    };
    if let Some(t) = &s.text {
        out.push_str(": ");
        out.push_str(t);
        // Text set long before the current state began is shown with its age, so it
        // never reads as current.
        if let Some(at) = s.text_at
            && s.since - at > 10 * 60_000
        {
            out.push_str(&format!(" (text from {} ago)", age(now - at)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{AgentState, StatusView};

    fn sv(state: AgentState, since: i64, text: Option<&str>, quiet: bool) -> StatusView {
        StatusView {
            state,
            since,
            last_heartbeat: None,
            reason: None,
            text: text.map(String::from),
            text_at: text.map(|_| since),
            quiet,
        }
    }

    #[test]
    fn idle_with_text_reads_naturally() {
        let s = sv(
            AgentState::Idle,
            0,
            Some("wrote anchor.verified.json"),
            false,
        );
        assert_eq!(
            render_status(&s, true, 5 * 60_000),
            "idle 5m: wrote anchor.verified.json"
        );
    }

    #[test]
    fn offline_shows_the_last_state_as_past() {
        let s = sv(AgentState::Working, 0, None, false);
        assert_eq!(
            render_status(&s, false, 40 * 60_000),
            "offline, was working 40m ago"
        );
    }

    #[test]
    fn quiet_working_says_so() {
        let s = sv(AgentState::Working, 0, None, true);
        assert_eq!(render_status(&s, true, 25 * 60_000), "working? (quiet) 25m");
    }

    #[test]
    fn offline_and_quiet_count_from_the_last_signal() {
        // Working since 0, last heartbeat at 30m. "Offline" and "quiet" say how long
        // since the agent was last heard from; "idle" stays time-in-state.
        let mut s = sv(AgentState::Working, 0, None, false);
        s.last_heartbeat = Some(30 * 60_000);
        assert_eq!(
            render_status(&s, false, 40 * 60_000),
            "offline, was working 10m ago"
        );
        let mut q = sv(AgentState::Working, 0, None, true);
        q.last_heartbeat = Some(10 * 60_000);
        assert_eq!(render_status(&q, true, 25 * 60_000), "working? (quiet) 15m");
        let mut i = sv(AgentState::Idle, 0, None, false);
        i.last_heartbeat = Some(4 * 60_000);
        assert_eq!(render_status(&i, true, 5 * 60_000), "idle 5m");
    }

    #[test]
    fn blocked_names_the_reason() {
        let mut s = sv(AgentState::BlockedOnHuman, 0, None, false);
        s.reason = Some("permission_prompt".into());
        assert_eq!(
            render_status(&s, true, 2 * 60_000),
            "blocked on human (permission_prompt) 2m"
        );
    }

    #[test]
    fn ages_scale_to_hours_and_days() {
        assert_eq!(age(90 * 60_000), "1h30m");
        assert_eq!(age(3 * 24 * 3_600_000), "3d");
        assert_eq!(age(30_000), "30s");
    }
}
