//! `claude-bus init` — joins the current project to a bus without hand-editing
//! Claude Code's config files.
//!
//! Split deliberately down the middle:
//!
//! - The MCP server entry lives in Claude Code's own config (`~/.claude.json`
//!   for user scope, or a project's `.mcp.json`). That file is not ours: on a
//!   real machine it is tens of kilobytes of caches, history, and identifiers
//!   we have no business touching by hand. We never open it — we shell out to
//!   `claude mcp add`, the official tool for exactly this.
//! - The permission allowlist lives in `.claude/settings.json`, which has no
//!   equivalent CLI. That one actually is ours to merge, carefully: it is
//!   small, but it can hold unrelated keys (theme, hooks, model, ...) that
//!   must survive untouched.
//!
//! The nine tool names come from `crate::agent::handler::BUS_TOOL_NAMES` —
//! the same const `list_tools` is checked against in `tests/agent_contract.rs`
//! — so this file can never drift from the tools it is allowlisting.

use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::agent::handler::BUS_TOOL_NAMES;
use crate::config::{self, EnvSource, RealEnv};

const DEFAULT_BUS: &str = "ws://127.0.0.1:7777/ws";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    Project,
}

impl Scope {
    fn claude_scope_flag(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project => "project",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Scope::User => "every project (user scope + ~/.claude/settings.json)",
            Scope::Project => "this project only (.mcp.json + .claude/settings.json)",
        }
    }

    fn settings_path(self, project_dir: &Path) -> PathBuf {
        match self {
            Scope::User => home_dir().join(".claude").join("settings.json"),
            Scope::Project => project_dir.join(".claude").join("settings.json"),
        }
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// CLI flags for `claude-bus init`. Parsed in `main.rs`, kept as plain data
/// here so the merge/plan logic below never has to know about `argv`.
#[derive(Debug, Clone, Default)]
pub struct InitArgs {
    pub scope: Option<Scope>,
    pub bus: Option<String>,
    pub dry_run: bool,
    pub yes: bool,
}

/// The fully-qualified permission strings `init` wants present, in
/// `BUS_TOOL_NAMES` order. Recomputed from the const every call — never
/// hand-copied — so it cannot drift from `list_tools`.
fn qualified_tool_names() -> Vec<String> {
    BUS_TOOL_NAMES
        .iter()
        .map(|t| format!("mcp__msgbus__{t}"))
        .collect()
}

/// Merge `tools` into `existing`'s `permissions.allow`, preserving every
/// other top-level key, preserving the existing order of `allow`, appending
/// only what's missing, and never duplicating an entry that's already there.
///
/// A pure function on purpose: the risky part of `init` is "does this JSON
/// surgery keep what it should and add only what it should," which does not
/// need a filesystem, a subprocess, or a TTY to test.
pub fn merge_allowlist(existing: Option<Value>, tools: &[&str]) -> Value {
    let mut root = match existing {
        Some(Value::Object(map)) => Value::Object(map),
        // No file, or a file that isn't a JSON object at the top level:
        // start from an empty object rather than propagating garbage.
        _ => json!({}),
    };
    let root_map = root.as_object_mut().expect("root was just built as Object");

    let permissions = root_map.entry("permissions").or_insert_with(|| json!({}));
    if !permissions.is_object() {
        *permissions = json!({});
    }
    let perm_map = permissions
        .as_object_mut()
        .expect("permissions was just ensured to be an Object");

    let allow = perm_map.entry("allow").or_insert_with(|| json!([]));
    if !allow.is_array() {
        *allow = json!([]);
    }
    let allow_arr = allow
        .as_array_mut()
        .expect("allow was just ensured to be an Array");

    for tool in tools {
        let already_present = allow_arr.iter().any(|v| v.as_str() == Some(*tool));
        if !already_present {
            allow_arr.push(Value::String((*tool).to_string()));
        }
    }

    root
}

/// The subset of `merge_allowlist`'s output relevant to what we print:
/// which of `tools` were newly appended, and how many top-level keys already
/// existed in the file before this merge touched anything.
struct MergePlan {
    added: Vec<String>,
    existing_top_level_keys: usize,
    merged: Value,
}

fn plan_merge(existing: Option<Value>, tools: &[String]) -> MergePlan {
    let existing_top_level_keys = match &existing {
        Some(Value::Object(map)) => map.len(),
        _ => 0,
    };
    let already: HashSet<&str> = match &existing {
        Some(Value::Object(map)) => map
            .get("permissions")
            .and_then(|p| p.get("allow"))
            .and_then(|a| a.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default(),
        _ => HashSet::new(),
    };
    let added = tools
        .iter()
        .filter(|t| !already.contains(t.as_str()))
        .cloned()
        .collect();

    let tool_refs: Vec<&str> = tools.iter().map(String::as_str).collect();
    let merged = merge_allowlist(existing, &tool_refs);

    MergePlan {
        added,
        existing_top_level_keys,
        merged,
    }
}

fn read_json_file(path: &Path) -> anyhow::Result<Option<Value>> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let v: Value = serde_json::from_str(&s)
                .map_err(|e| anyhow::anyhow!("{}: invalid JSON: {e}", path.display()))?;
            Ok(Some(v))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_json_file(path: &Path, value: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    std::fs::write(path, text)?;
    Ok(())
}

/// A pure PATH scan — never spawns anything. `--dry-run` must run zero
/// subprocesses, and even outside dry-run this gives a clear, specific error
/// instead of the confusing "No such file or directory (os error 2)" a raw
/// spawn failure would surface.
fn claude_on_path() -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| dir.join("claude").is_file())
}

enum McpState {
    NotConfigured,
    /// The raw text `claude mcp get <name>` printed. Deliberately not parsed
    /// into a structured comparison — shown verbatim so a human can judge
    /// whether it already matches what `init` would write.
    Existing(String),
}

/// Runs `claude mcp get <name>`. Per the CLI's documented behavior (verified
/// by hand against a real `claude` binary): exit 0 and prints the existing
/// entry's scope when one exists, non-zero and an explanatory message on
/// stdout when it does not.
fn claude_mcp_get(name: &str) -> anyhow::Result<McpState> {
    let output = Command::new("claude").args(["mcp", "get", name]).output()?;
    if output.status.success() {
        Ok(McpState::Existing(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ))
    } else {
        Ok(McpState::NotConfigured)
    }
}

fn claude_mcp_add(scope: Scope, bus_url: &str) -> anyhow::Result<()> {
    let status = Command::new("claude")
        .args([
            "mcp",
            "add",
            "--scope",
            scope.claude_scope_flag(),
            "msgbus",
            "--",
            "claude-bus",
            "agent",
            "--bus",
            bus_url,
        ])
        .status()?;
    if !status.success() {
        anyhow::bail!("`claude mcp add` exited with {status}");
    }
    Ok(())
}

/// Ask a yes/no question. `--yes` shortcuts to true without printing
/// anything. When stdin is not a terminal, this returns false immediately
/// without printing or blocking — a script that re-runs `init` unattended
/// should never hang on a prompt nobody is there to answer, and declining is
/// the safe default matching the `[y/N]` convention used throughout.
fn confirm(prompt: &str, assume_yes: bool, interactive: bool) -> bool {
    if assume_yes {
        return true;
    }
    if !interactive {
        return false;
    }
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

fn prompt_line(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return None;
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn prompt_scope() -> Scope {
    loop {
        println!("Scope?");
        println!("  1) this project only    .mcp.json (committable) + .claude/settings.json");
        println!("  2) every project         user scope + ~/.claude/settings.json");
        match prompt_line("> ") {
            Some(s) if s == "1" => return Scope::Project,
            Some(s) if s == "2" => return Scope::User,
            None => return Scope::User, // blank input accepts the shown default
            Some(_) => println!("please enter 1 or 2"),
        }
    }
}

fn prompt_bus() -> String {
    let prompt = format!("Bus address [{DEFAULT_BUS}]: ");
    prompt_line(&prompt).unwrap_or_else(|| DEFAULT_BUS.to_string())
}

fn launch_reminder() {
    println!();
    println!("Sessions need to opt in to channels explicitly:");
    println!();
    println!("  claude --dangerously-load-development-channels server:msgbus");
    println!();
    println!(
        "Confirm the startup banner names server:msgbus. If it does not, messages are \
         dropped silently — no error to the sender, no error anywhere."
    );
}

/// Project display info: the raw (unsanitized) directory Claude Code will
/// treat as the project, and its basename, for the informational header.
fn project_dir() -> PathBuf {
    let env = RealEnv;
    let raw = env
        .var("CLAUDE_PROJECT_DIR")
        .or_else(|| env.cwd())
        .unwrap_or_else(|| ".".to_string());
    PathBuf::from(raw)
}

pub fn run(args: InitArgs) -> anyhow::Result<()> {
    if !claude_on_path() {
        anyhow::bail!(
            "claude-bus init needs the Claude Code CLI (the `claude` command) on PATH — it \
             is what owns ~/.claude.json and .mcp.json, and init shells out to it rather \
             than hand-editing them. Install Claude Code, or add it to PATH, and try again."
        );
    }

    let is_tty = std::io::stdin().is_terminal();

    let project_dir = project_dir();
    let project_name = project_dir
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "agent".to_string());
    let agent_name = config::resolve_name(&config::NameArgs::default(), &RealEnv);

    println!();
    println!("  project     {project_name}  ({})", project_dir.display());
    println!("  agent name  {agent_name}");

    let scope = match args.scope {
        Some(s) => s,
        None if is_tty => {
            println!("  msgbus      not configured (checked after scope is chosen)");
            println!();
            prompt_scope()
        }
        None => Scope::User,
    };

    let bus = match args.bus.clone() {
        Some(b) => b,
        None if is_tty => {
            println!();
            prompt_bus()
        }
        None => DEFAULT_BUS.to_string(),
    };

    println!();
    println!("Scope: {}", scope.describe());
    println!("Bus address: {bus}");

    let settings_path = scope.settings_path(&project_dir);
    let existing_settings = read_json_file(&settings_path)?;
    let tools = qualified_tool_names();
    let plan = plan_merge(existing_settings, &tools);

    if args.dry_run {
        println!();
        println!(
            "msgbus      dry run — not checked (run without --dry-run, or `claude mcp get \
             msgbus`, to see the current entry)"
        );
        println!();
        println!("Would run:");
        println!(
            "  claude mcp add --scope {} msgbus -- claude-bus agent --bus {bus}",
            scope.claude_scope_flag()
        );
        println!();
        println!("Would merge into {}:", settings_path.display());
        if plan.added.is_empty() {
            println!("  permissions.allow already has all 9 entries; no changes needed");
        } else {
            println!(
                "  + permissions.allow    {} entries ({} … {})",
                plan.added.len(),
                plan.added.first().unwrap(),
                plan.added.last().unwrap()
            );
        }
        println!(
            "  {} existing top-level key(s) preserved.",
            plan.existing_top_level_keys
        );
        println!();
        println!("Dry run: nothing was written, nothing was run.");
        return Ok(());
    }

    let mcp_state = claude_mcp_get("msgbus")?;

    let will_add_mcp = match &mcp_state {
        McpState::NotConfigured => true,
        McpState::Existing(raw) => {
            println!();
            println!("An MCP entry named \"msgbus\" already exists:");
            println!();
            for line in raw.lines() {
                println!("  {line}");
            }
            println!();
            confirm(
                "Overwrite it by re-running `claude mcp add`? [y/N] ",
                args.yes,
                is_tty,
            )
        }
    };

    let will_merge_settings = !plan.added.is_empty();

    if !will_add_mcp && !will_merge_settings {
        println!();
        println!(
            "Already configured for {} scope. Nothing to do.",
            scope.claude_scope_flag()
        );
        launch_reminder();
        return Ok(());
    }

    println!();
    if will_add_mcp {
        println!("Will run:");
        println!(
            "  claude mcp add --scope {} msgbus -- claude-bus agent --bus {bus}",
            scope.claude_scope_flag()
        );
    } else {
        println!("Will leave the existing msgbus MCP entry untouched.");
    }
    println!();
    if will_merge_settings {
        println!("Will merge into {}:", settings_path.display());
        println!(
            "  + permissions.allow    {} entries ({} … {})",
            plan.added.len(),
            plan.added.first().unwrap(),
            plan.added.last().unwrap()
        );
        println!(
            "  {} existing top-level key(s) preserved.",
            plan.existing_top_level_keys
        );
    } else {
        println!(
            "permissions.allow already has all 9 entries; {} left untouched.",
            settings_path.display()
        );
    }

    println!();
    if !confirm("Apply? [y/N] ", args.yes, is_tty) {
        println!("Aborted; nothing changed.");
        return Ok(());
    }

    if will_add_mcp {
        claude_mcp_add(scope, &bus)?;
    }
    if will_merge_settings {
        write_json_file(&settings_path, &plan.merged)?;
    }

    println!();
    println!("Done.");
    launch_reminder();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_existing_file_creates_permissions_allow() {
        let result = merge_allowlist(None, &["a", "b"]);
        assert_eq!(result, json!({ "permissions": { "allow": ["a", "b"] } }));
    }

    #[test]
    fn file_with_no_permissions_key_gets_one_added_and_other_keys_kept() {
        let existing = json!({ "theme": "dark", "model": "sonnet" });
        let result = merge_allowlist(Some(existing), &["a"]);
        assert_eq!(
            result,
            json!({
                "theme": "dark",
                "model": "sonnet",
                "permissions": { "allow": ["a"] }
            })
        );
    }

    #[test]
    fn permissions_present_but_no_allow_key_gets_allow_added_and_deny_kept() {
        let existing = json!({ "permissions": { "deny": ["Bash(rm -rf /)"] } });
        let result = merge_allowlist(Some(existing), &["a"]);
        assert_eq!(
            result,
            json!({
                "permissions": {
                    "deny": ["Bash(rm -rf /)"],
                    "allow": ["a"]
                }
            })
        );
    }

    #[test]
    fn unrelated_existing_allow_entries_are_preserved_in_order() {
        let existing = json!({ "permissions": { "allow": ["Bash(ls)", "Read(**)"] } });
        let result = merge_allowlist(Some(existing), &["a"]);
        assert_eq!(
            result["permissions"]["allow"],
            json!(["Bash(ls)", "Read(**)", "a"])
        );
    }

    #[test]
    fn already_present_entries_are_not_duplicated() {
        let existing = json!({ "permissions": { "allow": ["a", "b"] } });
        let result = merge_allowlist(Some(existing), &["a", "b"]);
        assert_eq!(result["permissions"]["allow"], json!(["a", "b"]));
    }

    #[test]
    fn partial_overlap_appends_only_the_missing_entries_in_order() {
        let existing = json!({ "permissions": { "allow": ["a"] } });
        let result = merge_allowlist(Some(existing), &["a", "b", "c"]);
        assert_eq!(result["permissions"]["allow"], json!(["a", "b", "c"]));
    }

    #[test]
    fn unrelated_top_level_keys_survive_a_merge_that_adds_allow_entries() {
        let existing = json!({
            "theme": "dark",
            "hooks": { "UserPromptSubmit": [] },
            "permissions": { "allow": [] }
        });
        let result = merge_allowlist(Some(existing), &["a"]);
        assert_eq!(result["theme"], json!("dark"));
        assert_eq!(result["hooks"], json!({ "UserPromptSubmit": [] }));
        assert_eq!(result["permissions"]["allow"], json!(["a"]));
    }

    #[test]
    fn qualified_names_are_prefixed_and_match_bus_tool_names_order() {
        let names = qualified_tool_names();
        assert_eq!(names.len(), BUS_TOOL_NAMES.len());
        for (q, raw) in names.iter().zip(BUS_TOOL_NAMES.iter()) {
            assert_eq!(q, &format!("mcp__msgbus__{raw}"));
        }
    }
}
