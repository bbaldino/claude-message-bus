//! `claude-bus init --dry-run` must be genuinely airtight: it must write and
//! mutate nothing, no matter what it finds. This drives the actual binary —
//! not the internal `run()` function directly — so a regression in the
//! dry-run gate inside `main.rs`/`init::run` is caught the same way a user
//! would hit it.
//!
//! `init` now probes `claude mcp get msgbus` before prompting, including
//! under `--dry-run` (see `src/init.rs` for why: it's read-only, so it
//! doesn't weaken "writes and mutates nothing," it just means dry-run is no
//! longer "spawns zero processes"). So the fake `claude` on `PATH` here must
//! actually be executable and answer that probe, unlike the old zero-byte
//! placeholder that only needed to satisfy the `is_file()` PATH check.

use std::process::{Command, Stdio};

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// A fake `claude` on PATH, ahead of the real one, whose `mcp get msgbus`
/// always reports "not configured" (exit 1) — the same thing the real CLI
/// prints for a name it doesn't know. Good enough for every dry-run test
/// below: none of them depend on an msgbus entry actually existing, and
/// `--dry-run` must never call `claude mcp add` regardless of what `mcp get`
/// says.
fn fake_claude_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("claude");
    std::fs::write(
        &script,
        "#!/bin/sh\necho 'No MCP server named \"msgbus\". Configured servers: none'\nexit 1\n",
    )
    .unwrap();
    make_executable(&script);
    dir
}

#[test]
fn dry_run_writes_nothing_and_prints_the_plan() {
    let project_dir = tempfile::tempdir().unwrap();
    let fake_claude = fake_claude_dir();
    let path = format!(
        "{}:{}",
        fake_claude.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
        .args([
            "init",
            "--dry-run",
            "--project",
            "--bus",
            "ws://127.0.0.1:7777/ws",
        ])
        .current_dir(project_dir.path())
        .env("PATH", path)
        .env("HOME", project_dir.path()) // isolate user-scope paths too, just in case
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("run claude-bus init --dry-run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "dry run should exit 0; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("Dry run: wrote nothing."),
        "missing the dry-run footer; stdout was:\n{stdout}"
    );
    assert!(
        stdout.contains("claude mcp add --scope project msgbus"),
        "should show the plan it would run; stdout was:\n{stdout}"
    );
    assert!(
        stdout.contains("mcp entry   not configured"),
        "should show the probed mcp state up front; stdout was:\n{stdout}"
    );

    // The whole point: nothing on disk changed. A fresh tempdir should still
    // be completely empty afterward — no .claude/settings.json, no
    // .claude/ directory at all, no .mcp.json.
    let entries: Vec<_> = std::fs::read_dir(project_dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(
        entries.is_empty(),
        "dry run must write nothing, but the project dir now contains: {entries:?}"
    );
}

#[test]
fn dry_run_reports_pending_changes_even_when_a_settings_file_already_exists() {
    // Same guarantee, but starting from a settings.json that already has
    // unrelated content — dry run must still read (fine) without writing
    // (not fine to skip verifying).
    let project_dir = tempfile::tempdir().unwrap();
    let fake_claude = fake_claude_dir();
    let path = format!(
        "{}:{}",
        fake_claude.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let claude_dir = project_dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let settings_path = claude_dir.join("settings.json");
    let original = "{\n  \"theme\": \"dark\"\n}\n";
    std::fs::write(&settings_path, original).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
        .args([
            "init",
            "--dry-run",
            "--project",
            "--bus",
            "ws://127.0.0.1:7777/ws",
        ])
        .current_dir(project_dir.path())
        .env("PATH", path)
        .env("HOME", project_dir.path())
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("run claude-bus init --dry-run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        stdout.contains("permissions.allow") && stdout.contains("9 entries"),
        "should describe the 9-entry permissions merge it would make; stdout was:\n{stdout}"
    );

    let after = std::fs::read_to_string(&settings_path).unwrap();
    assert_eq!(
        after, original,
        "dry run must not modify an existing settings.json"
    );
}

#[test]
fn dry_run_partial_configuration_offers_only_the_missing_half() {
    // Someone followed half of DEPLOY.md by hand: the allowlist is already
    // fully populated (via .claude/settings.json), but there's no msgbus MCP
    // entry (the fake `claude` here always reports "not configured"). `init`
    // should recognize this as "MCP only" — not re-describe the allowlist
    // merge, since there's nothing left to merge.
    let project_dir = tempfile::tempdir().unwrap();
    let fake_claude = fake_claude_dir();
    let path = format!(
        "{}:{}",
        fake_claude.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let claude_dir = project_dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let settings_path = claude_dir.join("settings.json");
    let full_allowlist = serde_json::json!({
        "permissions": {
            "allow": [
                "mcp__msgbus__send",
                "mcp__msgbus__history",
                "mcp__msgbus__rooms",
                "mcp__msgbus__agents",
                "mcp__msgbus__join",
                "mcp__msgbus__put_file",
                "mcp__msgbus__get_file",
                "mcp__msgbus__list_files",
                "mcp__msgbus__resume"
            ]
        }
    });
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&full_allowlist).unwrap(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
        .args([
            "init",
            "--dry-run",
            "--project",
            "--bus",
            "ws://127.0.0.1:7777/ws",
        ])
        .current_dir(project_dir.path())
        .env("PATH", path)
        .env("HOME", project_dir.path())
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("run claude-bus init --dry-run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        stdout.contains("allowlist   project 9/9"),
        "should show the allowlist as already complete; stdout was:\n{stdout}"
    );
    assert!(
        stdout.contains("claude mcp add --scope project msgbus"),
        "should still show the MCP entry it would add; stdout was:\n{stdout}"
    );
    assert!(
        stdout.contains("already has all 9 entries; no changes needed"),
        "should say the allowlist needs no changes rather than re-describing a merge; \
         stdout was:\n{stdout}"
    );
    assert!(
        !stdout.contains("Would merge into"),
        "should not show a merge plan when there is nothing left to merge; stdout was:\n{stdout}"
    );

    let after = std::fs::read_to_string(&settings_path).unwrap();
    assert_eq!(
        after,
        serde_json::to_string_pretty(&full_allowlist).unwrap(),
        "dry run must not modify settings.json"
    );
}

#[test]
fn project_scope_is_detected_from_mcp_json_even_when_the_probe_disagrees() {
    // Finding 2 (fix round 1): project-scope detection reads .mcp.json
    // directly rather than depending on `claude mcp get`'s wording. Proven
    // here with a fake `claude` whose `mcp get` always reports "not
    // configured" — as if the CLI's prose changed, or the entry is
    // otherwise invisible to the probe — while `.mcp.json` genuinely has
    // the `msgbus` key. `init` must still recognize project scope as
    // already configured, entirely from the file.
    let project_dir = tempfile::tempdir().unwrap();
    let fake_claude = fake_claude_dir(); // always reports NotConfigured
    let path = format!(
        "{}:{}",
        fake_claude.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    std::fs::write(
        project_dir.path().join(".mcp.json"),
        r#"{"mcpServers":{"msgbus":{"command":"claude-bus","args":["agent","--bus","ws://127.0.0.1:7777/ws"]}}}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
        .args([
            "init",
            "--dry-run",
            "--project",
            "--bus",
            "ws://127.0.0.1:7777/ws",
        ])
        .current_dir(project_dir.path())
        .env("PATH", path)
        .env("HOME", project_dir.path())
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("run claude-bus init --dry-run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        !stdout.contains("claude mcp add"),
        "should not offer to add an MCP entry .mcp.json already has; stdout was:\n{stdout}"
    );
    assert!(
        stdout.contains("MCP entry already present"),
        "should recognize the entry from .mcp.json directly; stdout was:\n{stdout}"
    );

    // Nothing written, as with every other dry-run scenario.
    assert!(!project_dir.path().join(".claude").exists());
}

#[test]
fn non_interactive_with_no_scope_flag_fails_closed_instead_of_guessing() {
    // A script that never mentioned scope at all must not have that decision
    // made for it — silently defaulting to user scope would turn a
    // copy-pasted `claude-bus init --bus ... --yes` meant for one project
    // into a machine-wide settings.json write. `--dry-run` is used here only
    // to keep the test itself inert if this regresses; the assertion is
    // about the exit code and message, not about what a real run would do.
    let project_dir = tempfile::tempdir().unwrap();
    let fake_claude = fake_claude_dir();
    let path = format!(
        "{}:{}",
        fake_claude.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
        .args(["init", "--dry-run", "--bus", "ws://127.0.0.1:7777/ws"])
        // No --user / --project.
        .current_dir(project_dir.path())
        .env("PATH", path)
        .env("HOME", project_dir.path())
        .env_remove("CLAUDE_PROJECT_DIR")
        // Explicit, rather than relying on however `cargo test` happens to
        // wire up the harness's stdin: this test is specifically about the
        // non-TTY path, so make stdin unambiguously not a terminal.
        .stdin(Stdio::null())
        .output()
        .expect("run claude-bus init --dry-run with no scope flag");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "should exit non-zero rather than guess a scope; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("--user") && stderr.contains("--project"),
        "error should name both flags so the fix is obvious; stderr was:\n{stderr}"
    );
    assert!(
        stderr.to_lowercase().contains("non-interactive"),
        "error should explain why it refused; stderr was:\n{stderr}"
    );

    // Belt and suspenders: confirm it really did nothing, not just that it
    // printed an error and proceeded anyway.
    let entries: Vec<_> = std::fs::read_dir(project_dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(
        entries.is_empty(),
        "failing closed must still write nothing, but the project dir now contains: {entries:?}"
    );
}
