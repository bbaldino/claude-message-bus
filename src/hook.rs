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
    let Some(ev) = status::HookEvent::parse(args) else {
        anyhow::bail!("unknown hook event {args:?}");
    };
    let mut buf = Vec::new();
    std::io::stdin().take(MAX_STDIN).read_to_end(&mut buf)?;
    let input: serde_json::Value = serde_json::from_slice(&buf)?;
    let Some(key) = status::status_key(input["session_id"].as_str()) else {
        anyhow::bail!("no session_id on stdin");
    };
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
