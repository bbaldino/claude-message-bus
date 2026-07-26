//! `init`'s `Action::Conflict` path: an MCP entry named `msgbus` exists but
//! `init` can't confirm it's the one at the target scope. `--yes` must not
//! be sufficient to overwrite it — that flag covers the routine "apply this
//! plan" gate, not this one. Only `--force` (or a real interactive "y") may
//! authorize it.
//!
//! Safety note: these tests drive the compiled binary *without* `--dry-run`,
//! the only way to exercise the real (non-dry-run) refusal/overwrite path.
//! They never risk real `claude` state: the fake `claude` placed first on
//! `PATH` intercepts *both* `mcp get` and `mcp add`, so even if the
//! behavior under test were broken, the "add" that would follow lands on
//! the fake script (which only echoes a marker and exits), never on a real
//! `claude` binary — and `HOME` is pointed at an isolated tempdir too, the
//! same pattern `tests/init_dry_run.rs` already uses.

use std::process::{Command, Stdio};

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// A fake `claude` whose `mcp get msgbus` always reports an entry at
/// *project* scope, and whose `mcp add` (should it ever be reached) just
/// announces itself via a marker instead of doing anything. Paired with
/// `--user` as the target scope below, so the probe's "Project config"
/// answer never matches the target — `Action::Conflict`, every time.
fn conflicting_claude_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("claude");
    let contents = r#"#!/bin/sh
if [ "$1" = "mcp" ] && [ "$2" = "get" ]; then
  echo "msgbus:"
  echo "  Scope: Project config (shared via .mcp.json)"
  echo "  Status: Connected"
  exit 0
fi
if [ "$1" = "mcp" ] && [ "$2" = "add" ]; then
  echo "FAKE_CLAUDE_MCP_ADD_WAS_CALLED"
  exit 0
fi
exit 1
"#;
    std::fs::write(&script, contents).unwrap();
    make_executable(&script);
    dir
}

#[test]
fn conflict_with_yes_but_no_force_refuses_non_interactively() {
    let project_dir = tempfile::tempdir().unwrap();
    let fake_home = tempfile::tempdir().unwrap();
    let fake_claude = conflicting_claude_dir();
    let path = format!(
        "{}:{}",
        fake_claude.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
        .args(["init", "--user", "--bus", "ws://127.0.0.1:7777/ws", "--yes"])
        .current_dir(project_dir.path())
        .env("PATH", path)
        .env("HOME", fake_home.path())
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::null())
        .output()
        .expect("run claude-bus init --user --yes (no --force)");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "should refuse rather than overwrite on --yes alone; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("--force"),
        "error should name --force as the way to authorize this; stderr was:\n{stderr}"
    );
    assert!(
        stdout.contains("already exists"),
        "should show the existing entry before refusing; stdout was:\n{stdout}"
    );
    assert!(
        !stdout.contains("FAKE_CLAUDE_MCP_ADD_WAS_CALLED")
            && !stderr.contains("FAKE_CLAUDE_MCP_ADD_WAS_CALLED"),
        "must never call `claude mcp add` when refusing; stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let home_settings = fake_home.path().join(".claude").join("settings.json");
    assert!(
        !home_settings.exists(),
        "refusing must not write settings.json either"
    );
}

#[test]
fn conflict_with_force_overwrites_non_interactively() {
    // The other half of the same story: --force *is* sufficient. Without
    // this test, a sabotage that made Conflict refuse unconditionally
    // (rather than specifically gating on --force) would still pass the
    // test above.
    let project_dir = tempfile::tempdir().unwrap();
    let fake_home = tempfile::tempdir().unwrap();
    let fake_claude = conflicting_claude_dir();
    let path = format!(
        "{}:{}",
        fake_claude.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
        .args([
            "init",
            "--user",
            "--bus",
            "ws://127.0.0.1:7777/ws",
            "--yes",
            "--force",
        ])
        .current_dir(project_dir.path())
        .env("PATH", path)
        .env("HOME", fake_home.path())
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::null())
        .output()
        .expect("run claude-bus init --user --yes --force");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "--force should authorize the overwrite; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("FAKE_CLAUDE_MCP_ADD_WAS_CALLED"),
        "the fake `claude mcp add` should have been invoked; stdout was:\n{stdout}"
    );
}
