//! Fix round 2's exact reproduction: a project directory containing only
//!
//! ```json
//! {"mcpServers":{"msgbus":{"command":"claude-bus","args":["agent","--bus","ws://OTHER:9999/ws"]}}}
//! ```
//!
//! (an untrusted directory, so `claude mcp get` doesn't see it and instead
//! honestly answers about a real ambient user-scope entry), then
//! `claude-bus init --project --bus ws://127.0.0.1:7777/ws --yes < /dev/null`.
//!
//! Two things were wrong: the status summary said "user scope" (from the
//! probe) while the plan correctly acted on the project-scope entry
//! `.mcp.json` actually has — contradicting itself — and the run ended with
//! an unqualified "Done." plus the launch reminder despite leaving an MCP
//! entry pointed at `ws://OTHER:9999/ws`, not the address that was asked
//! for.
//!
//! Safety note: same pattern as `tests/init_conflict.rs` — this drives the
//! compiled binary *without* `--dry-run` (the only way to reach the closing
//! "Done." vs. partial-outcome code path), but the fake `claude` on `PATH`
//! intercepts both `mcp get` and `mcp add` and `HOME` is an isolated
//! tempdir, so no real `claude` binary or config is ever touched.

use std::process::{Command, Stdio};

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// A fake `claude` whose `mcp get msgbus` reports an entry at *user* scope
/// (simulating the real ambient ~/.claude.json entry the reviewer's machine
/// had) regardless of what `.mcp.json` in the test's project dir says —
/// exactly the "two sources disagree" shape the finding describes. `mcp
/// add` just announces itself via a marker rather than doing anything.
fn user_scope_claude_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("claude");
    let contents = r#"#!/bin/sh
if [ "$1" = "mcp" ] && [ "$2" = "get" ]; then
  echo "msgbus:"
  echo "  Scope: User config (available in all your projects)"
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
fn summary_names_project_scope_and_ending_does_not_overclaim_success() {
    let project_dir = tempfile::tempdir().unwrap();
    let fake_home = tempfile::tempdir().unwrap();
    let fake_claude = user_scope_claude_dir();
    let path = format!(
        "{}:{}",
        fake_claude.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    std::fs::write(
        project_dir.path().join(".mcp.json"),
        r#"{"mcpServers":{"msgbus":{"command":"claude-bus","args":["agent","--bus","ws://OTHER:9999/ws"]}}}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
        .args([
            "init",
            "--project",
            "--bus",
            "ws://127.0.0.1:7777/ws",
            "--yes",
        ])
        // No --force: the existing project entry must be left untouched.
        .current_dir(project_dir.path())
        .env("PATH", path)
        .env("HOME", fake_home.path())
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::null())
        .output()
        .expect("run claude-bus init --project --yes (no --force)");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "leaving an ambiguous entry untouched while applying the allowlist is a valid, \
         intentional outcome, not a failure; stdout: {stdout}\nstderr: {stderr}"
    );

    // Finding 1: the summary must name the *project* entry that .mcp.json
    // actually has, not only the unrelated user-scope one the probe found.
    assert!(
        stdout.contains("project scope (.mcp.json)"),
        "status summary must show the project entry .mcp.json actually has; stdout was:\n{stdout}"
    );
    // ... and since a user-scope entry also genuinely exists (per the fake
    // probe), that shadowing should be visible too, not silently dropped.
    assert!(
        stdout.contains("user scope (~/.claude.json)"),
        "status summary should also disclose the shadowing user-scope entry; stdout was:\n{stdout}"
    );

    // Finding 2: the close must not claim unqualified success. No bare
    // "Done." line, no unchanged launch-reminder block, and --force must be
    // named as how to actually change the entry.
    assert!(
        !stdout.lines().any(|l| l.trim() == "Done."),
        "must not print a bare \"Done.\" when the MCP entry was left unverified; stdout was:\n{stdout}"
    );
    assert!(
        !stdout.contains("dangerously-load-development-channels"),
        "must not print the unchanged launch reminder over an unverified entry; stdout was:\n{stdout}"
    );
    assert!(
        stdout.contains("--force"),
        "closing message should name --force as the way to actually change the entry; \
         stdout was:\n{stdout}"
    );

    // The allowlist half genuinely did get applied...
    let settings =
        std::fs::read_to_string(project_dir.path().join(".claude").join("settings.json"))
            .expect("allowlist should have been written");
    assert!(settings.contains("mcp__msgbus__send"));

    // ...but the MCP entry itself must not have been touched: no `claude
    // mcp add` call (the fake script's marker would appear if it had been),
    // and .mcp.json is byte-for-byte the same file this test wrote.
    assert!(
        !stdout.contains("FAKE_CLAUDE_MCP_ADD_WAS_CALLED")
            && !stderr.contains("FAKE_CLAUDE_MCP_ADD_WAS_CALLED"),
        "must never call `claude mcp add` without --force; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let mcp_json = std::fs::read_to_string(project_dir.path().join(".mcp.json")).unwrap();
    assert!(mcp_json.contains("ws://OTHER:9999/ws"));
}
