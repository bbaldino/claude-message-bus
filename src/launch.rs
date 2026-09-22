//! `claude-bus launch` — start a Claude Code session wired for the bus.
//!
//! A shortcut for the flags a hub-driven, non-interactive bus agent is always
//! launched with: the development-channel flag that turns on the `msgbus`
//! channel, plus the non-interactive posture (skip permission prompts, and
//! disallow the tools that would block on local terminal input). Anything the
//! caller appends — `--continue`, a `-p` prompt, whatever — is forwarded to
//! `claude` verbatim, so the caller opts into those themselves.

/// Build the argument vector passed to `claude`: the fixed flags first, then the
/// caller's own args appended verbatim.
///
/// `--dangerously-skip-permissions` is placed *after* the `--disallowedTools`
/// list so the variadic list is bounded by it — a caller's passthrough arg can
/// never be swallowed as another disallowed-tool name.
pub fn build_launch_args(extra: &[String]) -> Vec<String> {
    let mut args: Vec<String> = [
        "--dangerously-load-development-channels",
        "server:msgbus",
        "--disallowedTools",
        "AskUserQuestion",
        "EnterPlanMode",
        "ExitPlanMode",
        "--dangerously-skip-permissions",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(extra.iter().cloned());
    args
}

/// Replace this process with `claude`, launched with the bus flags plus `extra`.
///
/// `exec` hands the terminal straight to `claude`, so the interactive session
/// (raw TTY, ctrl-C, resize) behaves exactly as if `claude` had been run
/// directly. On success it never returns; it returns only if `claude` could not
/// be started at all.
pub fn run(extra: Vec<String>) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    let args = build_launch_args(&extra);
    let err = std::process::Command::new("claude").args(&args).exec();
    Err(anyhow::anyhow!(
        "could not launch `claude` ({err}). Is Claude Code installed and on PATH?"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_the_fixed_flags_then_passthrough() {
        let args = build_launch_args(&["--continue".to_string()]);
        assert_eq!(
            args,
            vec![
                "--dangerously-load-development-channels",
                "server:msgbus",
                "--disallowedTools",
                "AskUserQuestion",
                "EnterPlanMode",
                "ExitPlanMode",
                "--dangerously-skip-permissions",
                "--continue",
            ]
        );
    }

    #[test]
    fn no_extra_args_is_just_the_fixed_flags() {
        let args = build_launch_args(&[]);
        assert_eq!(args.len(), 7);
        assert_eq!(args.last().unwrap(), "--dangerously-skip-permissions");
    }

    #[test]
    fn skip_permissions_bounds_the_disallowed_list_before_any_passthrough() {
        // The whole reason for the ordering: a passthrough arg must land after
        // a non-variadic flag, so it can't be read as another disallowed tool.
        let args = build_launch_args(&["--continue".to_string()]);
        let disallow = args.iter().position(|a| a == "--disallowedTools").unwrap();
        let skip = args
            .iter()
            .position(|a| a == "--dangerously-skip-permissions")
            .unwrap();
        let cont = args.iter().position(|a| a == "--continue").unwrap();
        assert!(
            disallow < skip && skip < cont,
            "the disallowed-tools list must be capped before passthrough: {args:?}"
        );
    }
}
