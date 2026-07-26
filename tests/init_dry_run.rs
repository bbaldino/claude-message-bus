//! `claude-bus init --dry-run` must be genuinely airtight: it is the thing a
//! cautious user reaches for first, so it must write nothing and run nothing
//! that could mutate real state. This drives the actual binary — not the
//! internal `run()` function directly — so a regression in the dry-run gate
//! inside `main.rs`/`init::run` is caught the same way a user would hit it.

use std::process::Command;

/// A fake `claude` on PATH, ahead of the real one. `claude_on_path()` only
/// checks for a file named `claude`, so this satisfies the PATH check
/// without needing (or risking invoking) a real Claude Code install — and
/// dry-run must never actually spawn it anyway.
fn fake_claude_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("claude"), b"").unwrap();
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
        stdout.contains("Dry run: nothing was written, nothing was run."),
        "missing the dry-run footer; stdout was:\n{stdout}"
    );
    assert!(
        stdout.contains("claude mcp add --scope project msgbus"),
        "should show the plan it would run; stdout was:\n{stdout}"
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
