//! `claude-bus hook <event> [type]`: called by Claude Code hooks, never by people.
//!
//! The contract with Claude Code is strict. Hooks block the session until they exit,
//! and `UserPromptSubmit` stdout is injected into the model's context. So this never
//! writes to stdout, never makes a network call, always exits 0, and reads at most
//! 1 MiB of stdin. A diagnostic goes to stderr, which Claude Code shows only in
//! verbose mode.

use std::io::Read;

use crate::agent::status;

const MAX_STDIN: u64 = 1024 * 1024;

pub fn run(args: &[String]) {
    if let Err(e) = try_run(args) {
        eprintln!("claude-bus hook: {e}");
    }
}

fn try_run(args: &[String]) -> anyhow::Result<()> {
    // Read (and discard) stdin first, whatever happens next, so Claude Code's write
    // to the pipe never blocks or fails. Nothing in it is needed: the file is keyed
    // by the Claude Code pid, not the `session_id` stdin carries (see
    // `status::status_key_for_pid`).
    let mut buf = Vec::new();
    let _ = std::io::stdin().take(MAX_STDIN).read_to_end(&mut buf);
    let Some(ev) = status::HookEvent::parse(args) else {
        anyhow::bail!("unknown hook event {args:?}");
    };
    let key = status::status_key_for_pid(claude_pid());
    let Some(dir) = status::status_dir(
        std::env::var("XDG_STATE_HOME").ok(),
        std::env::var("HOME").ok(),
    ) else {
        anyhow::bail!("neither XDG_STATE_HOME nor HOME is set");
    };
    let path = dir.join(format!("{key}.json"));
    let now = crate::store::now_ms();
    if let Some(next) = status::next(status::read(&path).as_ref(), &ev, now) {
        status::write(&path, &next)?;
    }
    Ok(())
}

/// Claude Code sets `CLAUDE_PID` on its hooks. Without it (or with garbage in it),
/// the hook's parent is the Claude Code process anyway.
fn claude_pid() -> u32 {
    std::env::var("CLAUDE_PID")
        .ok()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or_else(std::os::unix::process::parent_id)
}
