//! `claude-bus init` — joins the current project to a bus without hand-editing
//! Claude Code's config files.
//!
//! Split deliberately down the middle:
//!
//! - The MCP server entry lives in Claude Code's own config: `~/.claude.json`
//!   for user scope, or a project's `.mcp.json`. Neither is ours to *write* —
//!   we always shell out to `claude mcp add`, the official tool for that,
//!   never hand-editing either file. Checking whether an entry already
//!   exists is a different matter, and the two scopes are deliberately
//!   treated differently:
//!   - **Project** (`.mcp.json`): read directly (see
//!     `project_mcp_json_has_msgbus`). It's small, its shape is already
//!     documented in `docs/DEPLOY.md` for hand-editing, and it's exactly the
//!     file `claude mcp add --scope project` writes — so a `msgbus` key
//!     there is an authoritative answer, not a guess from prose.
//!   - **User** (`~/.claude.json`): never opened, in either direction, full
//!     stop. On a real machine it is tens of kilobytes of caches, history,
//!     and identifiers we have no business touching — and, confirmed by
//!     inspecting this project's own `~/.claude.json` while designing this,
//!     adjacent entries can hold plaintext API keys and secrets. Detected
//!     only by parsing the single `Scope:` line out of `claude mcp get`'s
//!     text output (`parse_mcp_scope`), the same read-only probe used either
//!     way. Do not "improve" this by symmetry with the project-scope path —
//!     the asymmetry is deliberate and the file is sensitive.
//! - The permission allowlist lives in `.claude/settings.json`, which has no
//!   equivalent CLI. That one actually is ours to merge, carefully: it is
//!   small, but it can hold unrelated keys (theme, hooks, model, ...) that
//!   must survive untouched.
//!
//! The two halves are independent and each has two possible scopes, so
//! "configured or not" is a 2x2-ish matrix, not a boolean. `plan_action`
//! below is the pure function that reduces a probed state down to what to
//! do about it; `run` probes everything (both halves, and — when no scope
//! flag was given — both scopes) before asking the user anything.
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
    /// Accept the routine plan `init` shows (the allowlist merge, and an MCP
    /// add when nothing ambiguous is going on) without an interactive
    /// prompt. Deliberately *not* sufficient on its own to overwrite an MCP
    /// entry `init` can't confirm matches the target scope — that needs
    /// `force`. See `confirm`'s doc comment.
    pub yes: bool,
    /// Authorizes overwriting an MCP entry `init` found but can't confirm is
    /// the one it would write itself (`Action::Conflict`). A separate,
    /// stronger consent than `yes`: script authors who pass `--yes` for
    /// routine runs should not thereby also be opting into clobbering
    /// something unrelated that happens to share the name `msgbus`.
    pub force: bool,
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

/// How many of the target `permissions.allow` entries are already present,
/// out of how many total. The only thing `plan_action` needs to know about
/// the allowlist half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AllowlistStatus {
    present: usize,
    total: usize,
}

impl AllowlistStatus {
    fn is_complete(self) -> bool {
        self.total > 0 && self.present == self.total
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

/// A pure PATH scan — never spawns anything. Used both to give a clear,
/// specific error when `claude` is missing (instead of the confusing "No
/// such file or directory (os error 2)" a raw spawn failure would surface)
/// and to keep the very first thing `run` does free of any subprocess. This
/// is no longer the thing that makes `--dry-run` spawn nothing overall —
/// `--dry-run` does run one read-only probe now, see `claude_mcp_get` below
/// — but it's still the reason a missing `claude` fails fast and legibly
/// rather than via a raw `ENOENT` from deep inside a probe.
fn claude_on_path() -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| dir.join("claude").is_file())
}

/// The raw result of probing for an MCP entry named `msgbus`, scope-agnostic
/// (`claude mcp get <name>` doesn't take a `--scope` argument — it reports
/// whichever entry it finds).
enum McpProbe {
    NotConfigured,
    /// The raw text `claude mcp get <name>` printed. Deliberately not parsed
    /// into a structured comparison of its content (command, args, bus URL)
    /// — shown verbatim so a human can judge whether it already matches what
    /// `init` would write. The one exception is `parse_mcp_scope` below,
    /// which reads only the single documented `Scope:` line, because knowing
    /// *where* the entry lives (not what it says) is required to answer "is
    /// the target scope already configured" at all.
    Existing(String),
}

/// Runs `claude mcp get <name>`. Per the CLI's documented behavior (verified
/// by hand against a real `claude` binary): exit 0 and prints the existing
/// entry's scope when one exists, non-zero and an explanatory message on
/// stdout when it does not. Read-only — no config file is touched by this
/// call — which is why `run` now calls it unconditionally, including under
/// `--dry-run`: the guarantee `--dry-run` exists to provide is "writes and
/// mutates nothing," not "spawns nothing," and this call keeps the former
/// true while making the latter no longer true.
fn claude_mcp_get(name: &str) -> anyhow::Result<McpProbe> {
    let output = Command::new("claude").args(["mcp", "get", name]).output()?;
    if output.status.success() {
        Ok(McpProbe::Existing(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ))
    } else {
        Ok(McpProbe::NotConfigured)
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

/// Reads only the one documented `Scope:` line out of `claude mcp get`'s
/// output — never the command, args, or anything else about the entry.
/// `None` means the text didn't recognizably say either scope (an
/// unrecognized format, or the third `local` scope `init` doesn't offer),
/// and is treated the same as "differs": ambiguous enough to need a human,
/// not a value worth guessing at.
fn parse_mcp_scope(raw: &str) -> Option<Scope> {
    if raw.contains("Project config") {
        Some(Scope::Project)
    } else if raw.contains("User config") {
        Some(Scope::User)
    } else {
        None
    }
}

/// How the MCP probe relates to one specific target scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpState {
    NotConfigured,
    /// An entry exists and it's confirmed to be at the scope being asked
    /// about.
    MatchesScope,
    /// An entry exists but isn't confirmed to be at the scope being asked
    /// about — a different scope, or (for user scope) text
    /// `parse_mcp_scope` couldn't read. Treated conservatively: never
    /// assumed to satisfy this scope.
    Differs,
}

/// Reads `.mcp.json` directly rather than parsing `claude mcp get`'s prose —
/// see the module doc comment for why project scope gets this and user
/// scope deliberately does not. Only checks for the presence of a `msgbus`
/// key under `mcpServers`, the same "don't structurally compare content"
/// restraint `merge_allowlist`'s sibling functions already apply elsewhere:
/// presence is authoritative (this is the exact file `claude mcp add
/// --scope project` writes), but what the entry's command/args/URL actually
/// say is still left for a human to judge if it ever matters.
fn project_mcp_json_has_msgbus(project_dir: &Path) -> anyhow::Result<bool> {
    let path = project_dir.join(".mcp.json");
    let has_entry = read_json_file(&path)?
        .and_then(|v| v.get("mcpServers").and_then(|m| m.get("msgbus")).cloned())
        .is_some();
    Ok(has_entry)
}

/// Pure: `.mcp.json`'s content, already reduced to "does it have a `msgbus`
/// entry," decides project scope on its own — a probe result reporting an
/// entry elsewhere never overrides an authoritative "no" from the file
/// itself. If `.mcp.json` doesn't have one, whether *something* named
/// `msgbus` was found anywhere (almost certainly at user scope, or possibly
/// an unapproved/unusual state) still needs a human's eyes rather than being
/// silently treated as "safe to add here too" — untested territory for
/// `claude mcp add --scope project` when a same-named entry already exists
/// elsewhere.
fn mcp_state_for_project_scope(mcp_json_has_msgbus: bool, probe: &McpProbe) -> McpState {
    if mcp_json_has_msgbus {
        return McpState::MatchesScope;
    }
    match probe {
        McpProbe::NotConfigured => McpState::NotConfigured,
        McpProbe::Existing(_) => McpState::Differs,
    }
}

/// Pure: user scope has no direct-read equivalent (see the module doc
/// comment for why `~/.claude.json` is never opened), so this is the only
/// detection mechanism for that scope — parse the one documented `Scope:`
/// line out of `claude mcp get`'s text.
fn mcp_state_for_user_scope(probe: &McpProbe) -> McpState {
    match probe {
        McpProbe::NotConfigured => McpState::NotConfigured,
        McpProbe::Existing(raw) => match parse_mcp_scope(raw) {
            Some(Scope::User) => McpState::MatchesScope,
            _ => McpState::Differs,
        },
    }
}

/// The five things `init` can decide to do for one scope, once both halves
/// have been probed. A pure function on the same principle as
/// `merge_allowlist`: the decision of *what* to do never needs a
/// filesystem, a subprocess, or a TTY to get right, only the already-probed
/// state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    NothingToDo,
    AddMcpOnly,
    AddAllowlistOnly,
    AddBoth,
    /// The MCP entry exists but isn't confirmed to match this scope. Never
    /// resolved automatically — always surfaced to a human (see `McpState::
    /// Differs`), regardless of whether the allowlist half is already fine.
    Conflict,
}

fn plan_action(mcp: McpState, allowlist: AllowlistStatus) -> Action {
    match (mcp, allowlist.is_complete()) {
        (McpState::Differs, _) => Action::Conflict,
        (McpState::MatchesScope, true) => Action::NothingToDo,
        (McpState::MatchesScope, false) => Action::AddAllowlistOnly,
        (McpState::NotConfigured, true) => Action::AddMcpOnly,
        (McpState::NotConfigured, false) => Action::AddBoth,
    }
}

/// Everything probed and decided for one scope: read-only, computed once,
/// reused for both the status display and (if that scope ends up chosen)
/// the actual apply.
struct ScopePlan {
    scope: Scope,
    settings_path: PathBuf,
    allowlist: AllowlistStatus,
    merge: MergePlan,
    action: Action,
}

fn build_scope_plan(
    scope: Scope,
    project_dir: &Path,
    tools: &[String],
    mcp_probe: &McpProbe,
) -> anyhow::Result<ScopePlan> {
    let settings_path = scope.settings_path(project_dir);
    let existing_settings = read_json_file(&settings_path)?;
    let merge = plan_merge(existing_settings, tools);
    let allowlist = AllowlistStatus {
        present: tools.len() - merge.added.len(),
        total: tools.len(),
    };
    let mcp_state = match scope {
        Scope::Project => {
            mcp_state_for_project_scope(project_mcp_json_has_msgbus(project_dir)?, mcp_probe)
        }
        Scope::User => mcp_state_for_user_scope(mcp_probe),
    };
    let action = plan_action(mcp_state, allowlist);
    Ok(ScopePlan {
        scope,
        settings_path,
        allowlist,
        merge,
        action,
    })
}

fn mcp_status_line(probe: &McpProbe) -> String {
    match probe {
        McpProbe::NotConfigured => "not configured".to_string(),
        McpProbe::Existing(raw) => match parse_mcp_scope(raw) {
            Some(Scope::Project) => "project scope (.mcp.json)".to_string(),
            Some(Scope::User) => "user scope (~/.claude.json)".to_string(),
            None => "existing, scope unclear — see `claude mcp get msgbus`".to_string(),
        },
    }
}

fn allowlist_fragment(scope: Scope, status: AllowlistStatus) -> String {
    if status.present == 0 {
        format!("{} not configured", scope.claude_scope_flag())
    } else {
        format!(
            "{} {}/{}",
            scope.claude_scope_flag(),
            status.present,
            status.total
        )
    }
}

/// Prints the compact status block — the whole point of probing first: the
/// user sees the real situation before answering anything.
fn print_status(probe: &McpProbe, plans: &[&ScopePlan]) {
    println!("  mcp entry   {}", mcp_status_line(probe));
    let fragments: Vec<String> = plans
        .iter()
        .map(|p| allowlist_fragment(p.scope, p.allowlist))
        .collect();
    println!("  allowlist   {}", fragments.join(" · "));
}

/// Ask a yes/no question, or skip asking if `assume_yes` is set. This is a
/// generic primitive with no opinion on *which* flag `assume_yes` should be
/// — that is the caller's responsibility, and it matters: `init` has two
/// confirmation points that need different consent. The final "apply the
/// plan just shown" gate is routine and `--yes` covers it. Overwriting an
/// MCP entry that `init` found but couldn't confirm matches the target scope
/// (`Action::Conflict`) is not routine, and `--yes` must never be passed as
/// `assume_yes` for that call — only `--force` (or a real interactive "y")
/// may authorize it. See the `Action::Conflict` arm in `run` for how that
/// split is enforced, including refusing outright rather than silently
/// declining when neither is available non-interactively.
///
/// When stdin is not a terminal and `assume_yes` is false, this returns
/// false immediately without printing or blocking — a script that re-runs
/// `init` unattended should never hang on a prompt nobody is there to
/// answer, and declining is the safe default matching the `[y/N]`
/// convention used throughout.
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

    // Fail closed rather than guess: an interactive user picking "every
    // project" off the menu is an affirmative, visible choice. A script that
    // never mentioned scope at all is not — and user scope is the more
    // consequential of the two, since ~/.claude/settings.json affects every
    // project on the machine, not just the one the script happens to be
    // sitting in. Silently defaulting there would turn a copy-pasted
    // `claude-bus init --bus ... --yes` meant for one project into a
    // machine-wide change with no error and no prompt. This check runs
    // before any probing, so a scope-less non-interactive call bails
    // immediately regardless of what's already configured — with no flag,
    // there's no well-defined "target scope" for a short-circuit to report
    // against, so there's nothing to check yet.
    if args.scope.is_none() && !is_tty {
        anyhow::bail!(
            "claude-bus init: scope must be given explicitly in non-interactive use (stdin \
             is not a terminal) — pass --user (every project, ~/.claude/settings.json) or \
             --project (this project only, .claude/settings.json here)."
        );
    }

    let project_dir = project_dir();
    let project_name = project_dir
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "agent".to_string());
    let agent_name = config::resolve_name(&config::NameArgs::default(), &RealEnv);

    println!();
    println!("  project     {project_name}  ({})", project_dir.display());
    println!("  agent name  {agent_name}");
    println!();

    let tools = qualified_tool_names();

    // Probe everything up front, before any prompt — including the scope
    // and bus-address prompts below. `claude mcp get` is read-only; reading
    // `.claude/settings.json` is read-only. Neither writes or mutates
    // anything, so doing this before asking the user anything (and even
    // under `--dry-run`) doesn't weaken the dry-run guarantee that matters.
    let mcp_probe = claude_mcp_get("msgbus")?;

    let (scope, chosen_plan) = match args.scope {
        Some(target) => {
            let plan = build_scope_plan(target, &project_dir, &tools, &mcp_probe)?;
            print_status(&mcp_probe, &[&plan]);
            if plan.action == Action::NothingToDo {
                println!();
                println!(
                    "Already fully configured for {} scope. Nothing to do.",
                    target.claude_scope_flag()
                );
                launch_reminder();
                return Ok(());
            }
            (target, plan)
        }
        None => {
            // is_tty is guaranteed true here — the bail above already
            // handled the non-interactive, no-flag case. Probe both scopes:
            // without a flag, "the target scope" isn't decided yet, and the
            // whole point is to let the user skip the question if one scope
            // turns out to already be fully configured.
            let plan_project = build_scope_plan(Scope::Project, &project_dir, &tools, &mcp_probe)?;
            let plan_user = build_scope_plan(Scope::User, &project_dir, &tools, &mcp_probe)?;
            print_status(&mcp_probe, &[&plan_project, &plan_user]);

            let already_done = [&plan_project, &plan_user]
                .into_iter()
                .find(|p| p.action == Action::NothingToDo);
            if let Some(done) = already_done {
                let other = if done.scope == Scope::Project {
                    Scope::User
                } else {
                    Scope::Project
                };
                println!();
                println!(
                    "Already fully configured for {} scope. Nothing to do.",
                    done.scope.claude_scope_flag()
                );
                println!(
                    "Run again with --{} if you also want {} scope configured.",
                    other.claude_scope_flag(),
                    other.claude_scope_flag()
                );
                launch_reminder();
                return Ok(());
            }

            println!();
            println!("Neither scope is fully configured yet.");
            println!();
            let target = prompt_scope();
            let plan = if target == Scope::Project {
                plan_project
            } else {
                plan_user
            };
            (target, plan)
        }
    };

    // The bus address is only relevant if we might call `claude mcp add` —
    // skip asking for it otherwise (e.g. an allowlist-only fix-up), the same
    // "don't ask questions whose answers don't matter" principle that
    // motivated probing before prompting in the first place.
    let needs_bus = matches!(
        chosen_plan.action,
        Action::AddMcpOnly | Action::AddBoth | Action::Conflict
    );

    let bus = match args.bus.clone() {
        Some(b) => b,
        None if needs_bus && is_tty => {
            println!();
            prompt_bus()
        }
        None => DEFAULT_BUS.to_string(),
    };

    println!();
    println!("Scope: {}", scope.describe());
    if needs_bus {
        println!("Bus address: {bus}");
    }

    if args.dry_run {
        print_dry_run_preview(&chosen_plan, &bus, &mcp_probe);
        return Ok(());
    }

    let will_add_mcp = match chosen_plan.action {
        Action::AddMcpOnly | Action::AddBoth => true,
        Action::AddAllowlistOnly => false,
        Action::Conflict => {
            if let McpProbe::Existing(raw) = &mcp_probe {
                println!();
                println!("An MCP entry named \"msgbus\" already exists:");
                println!();
                for line in raw.lines() {
                    println!("  {line}");
                }
                println!();
            }
            // `--yes` covers the routine "apply this plan" gate below, not
            // this one: overwriting an entry `init` can't confirm matches
            // the target scope needs its own, stronger consent. A
            // non-interactive run without `--force` refuses outright rather
            // than falling back to "declined, nothing changed" — silently
            // treating an unauthorized overwrite as a no-op would hide a
            // real problem (the entry that's there might not be what the
            // caller thinks it is) behind a successful-looking exit.
            if !is_tty && !args.force {
                anyhow::bail!(
                    "claude-bus init: an existing \"msgbus\" MCP entry was found that init \
                     can't confirm matches {} scope (shown above) — refusing to overwrite it \
                     non-interactively without --force. Pass --force to overwrite, or run \
                     interactively to decide.",
                    scope.claude_scope_flag()
                );
            }
            confirm(
                "Overwrite it by re-running `claude mcp add`? [y/N] ",
                args.force,
                is_tty,
            )
        }
        Action::NothingToDo => unreachable!("NothingToDo already returned above"),
    };

    let will_merge_settings = !chosen_plan.merge.added.is_empty();

    if !will_add_mcp && !will_merge_settings {
        println!();
        println!("Nothing to do for {} scope.", scope.claude_scope_flag());
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
        println!("Will merge into {}:", chosen_plan.settings_path.display());
        println!(
            "  + permissions.allow    {} entries ({} … {})",
            chosen_plan.merge.added.len(),
            chosen_plan.merge.added.first().unwrap(),
            chosen_plan.merge.added.last().unwrap()
        );
        println!(
            "  {} existing top-level key(s) preserved.",
            chosen_plan.merge.existing_top_level_keys
        );
    } else {
        println!(
            "permissions.allow already has all {} entries; {} left untouched.",
            chosen_plan.allowlist.total,
            chosen_plan.settings_path.display()
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
        write_json_file(&chosen_plan.settings_path, &chosen_plan.merge.merged)?;
    }

    println!();
    println!("Done.");
    launch_reminder();
    Ok(())
}

/// `--dry-run`'s preview: same probed state as a real run, same decision
/// (`chosen_plan.action`), just print-and-stop instead of confirm-and-apply.
/// Never calls `claude_mcp_add` or `write_json_file` — that's what makes the
/// "writes and mutates nothing" guarantee structural rather than "trust me."
fn print_dry_run_preview(plan: &ScopePlan, bus: &str, mcp_probe: &McpProbe) {
    println!();
    match plan.action {
        Action::NothingToDo => unreachable!("NothingToDo already returned before dry-run preview"),
        Action::AddMcpOnly => {
            println!("Would run:");
            println!(
                "  claude mcp add --scope {} msgbus -- claude-bus agent --bus {bus}",
                plan.scope.claude_scope_flag()
            );
            println!();
            println!(
                "permissions.allow already has all {} entries; no changes needed.",
                plan.allowlist.total
            );
        }
        Action::AddAllowlistOnly => {
            println!(
                "MCP entry already present ({} scope); would leave it untouched.",
                plan.scope.claude_scope_flag()
            );
            println!();
            print_merge_preview(plan);
        }
        Action::AddBoth => {
            println!("Would run:");
            println!(
                "  claude mcp add --scope {} msgbus -- claude-bus agent --bus {bus}",
                plan.scope.claude_scope_flag()
            );
            println!();
            print_merge_preview(plan);
        }
        Action::Conflict => {
            if let McpProbe::Existing(raw) = mcp_probe {
                println!(
                    "An MCP entry named \"msgbus\" already exists (not confirmed to be at {} \
                     scope):",
                    plan.scope.claude_scope_flag()
                );
                println!();
                for line in raw.lines() {
                    println!("  {line}");
                }
                println!();
                println!(
                    "Would require an interactive confirmation, or --force, to overwrite it \
                     (run without --dry-run to decide; --yes alone would not be enough)."
                );
            }
            if !plan.merge.added.is_empty() {
                println!();
                print_merge_preview(plan);
            }
        }
    }
    println!();
    println!(
        "Dry run: wrote nothing. The only thing run was the read-only `claude mcp get msgbus` \
         check above."
    );
}

fn print_merge_preview(plan: &ScopePlan) {
    println!("Would merge into {}:", plan.settings_path.display());
    println!(
        "  + permissions.allow    {} entries ({} … {})",
        plan.merge.added.len(),
        plan.merge.added.first().unwrap(),
        plan.merge.added.last().unwrap()
    );
    println!(
        "  {} existing top-level key(s) preserved.",
        plan.merge.existing_top_level_keys
    );
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

    // --- parse_mcp_scope -----------------------------------------------

    #[test]
    fn parse_mcp_scope_recognizes_project_config() {
        let raw = "msgbus:\n  Scope: Project config (shared via .mcp.json)\n  Status: ✔ Connected";
        assert_eq!(parse_mcp_scope(raw), Some(Scope::Project));
    }

    #[test]
    fn parse_mcp_scope_recognizes_user_config() {
        let raw =
            "msgbus:\n  Scope: User config (available in all your projects)\n  Status: ✔ Connected";
        assert_eq!(parse_mcp_scope(raw), Some(Scope::User));
    }

    #[test]
    fn parse_mcp_scope_returns_none_for_unrecognized_text() {
        let raw = "msgbus:\n  Scope: Local config (private to you in this project)\n";
        assert_eq!(parse_mcp_scope(raw), None);
    }

    // --- mcp_state_for_user_scope: parses `claude mcp get`'s prose --------

    #[test]
    fn user_scope_not_configured_when_probe_finds_nothing() {
        assert_eq!(
            mcp_state_for_user_scope(&McpProbe::NotConfigured),
            McpState::NotConfigured
        );
    }

    #[test]
    fn user_scope_matches_when_probe_says_user_config() {
        let probe =
            McpProbe::Existing("Scope: User config (available in all your projects)".into());
        assert_eq!(mcp_state_for_user_scope(&probe), McpState::MatchesScope);
    }

    #[test]
    fn user_scope_differs_when_probe_says_project_config() {
        let probe = McpProbe::Existing("Scope: Project config (shared via .mcp.json)".into());
        assert_eq!(mcp_state_for_user_scope(&probe), McpState::Differs);
    }

    #[test]
    fn user_scope_differs_when_scope_text_is_unrecognized() {
        let probe = McpProbe::Existing("Scope: Local config (private to you)".into());
        assert_eq!(mcp_state_for_user_scope(&probe), McpState::Differs);
    }

    // --- mcp_state_for_project_scope: authoritative from .mcp.json's own
    // content, never overridden by what the (unrelated) probe found ------

    #[test]
    fn project_scope_matches_when_mcp_json_has_the_entry() {
        // Even if the probe found nothing at all — .mcp.json's own content
        // is authoritative for project scope, per the brief: it's the exact
        // file `claude mcp add --scope project` writes.
        assert_eq!(
            mcp_state_for_project_scope(true, &McpProbe::NotConfigured),
            McpState::MatchesScope
        );
    }

    #[test]
    fn project_scope_matches_regardless_of_what_the_probe_says_elsewhere() {
        // The probe here describes a *user*-scope entry, which does not
        // change the answer: .mcp.json having the key is what decides
        // project scope, full stop — this is the whole point of Finding 2,
        // removing the dependency on `claude mcp get`'s wording.
        let probe =
            McpProbe::Existing("Scope: User config (available in all your projects)".into());
        assert_eq!(
            mcp_state_for_project_scope(true, &probe),
            McpState::MatchesScope
        );
    }

    #[test]
    fn project_scope_not_configured_when_mcp_json_lacks_it_and_probe_finds_nothing() {
        assert_eq!(
            mcp_state_for_project_scope(false, &McpProbe::NotConfigured),
            McpState::NotConfigured
        );
    }

    #[test]
    fn project_scope_differs_when_mcp_json_lacks_it_but_something_named_msgbus_exists() {
        // .mcp.json has no entry, but the probe found *something* (almost
        // certainly at user scope) — never silently treated as "safe to add
        // here too."
        let probe =
            McpProbe::Existing("Scope: User config (available in all your projects)".into());
        assert_eq!(
            mcp_state_for_project_scope(false, &probe),
            McpState::Differs
        );
    }

    // --- project_mcp_json_has_msgbus: the direct-read half of Finding 2 --

    #[test]
    fn project_mcp_json_has_msgbus_true_when_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"msgbus":{"command":"claude-bus","args":["agent"]}}}"#,
        )
        .unwrap();
        assert!(project_mcp_json_has_msgbus(dir.path()).unwrap());
    }

    #[test]
    fn project_mcp_json_has_msgbus_false_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!project_mcp_json_has_msgbus(dir.path()).unwrap());
    }

    #[test]
    fn project_mcp_json_has_msgbus_false_when_other_servers_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"other-server":{"command":"foo"}}}"#,
        )
        .unwrap();
        assert!(!project_mcp_json_has_msgbus(dir.path()).unwrap());
    }

    // --- plan_action: the decision matrix -------------------------------
    //
    // Each case pins one dimension and varies the other, so that breaking
    // either half's handling in isolation fails a case here. See the
    // sabotage evidence in the report for confirmation these actually catch
    // the breakage they name.

    fn complete() -> AllowlistStatus {
        AllowlistStatus {
            present: 9,
            total: 9,
        }
    }

    fn missing() -> AllowlistStatus {
        AllowlistStatus {
            present: 0,
            total: 9,
        }
    }

    fn partial() -> AllowlistStatus {
        AllowlistStatus {
            present: 4,
            total: 9,
        }
    }

    #[test]
    fn fully_configured_is_nothing_to_do() {
        assert_eq!(
            plan_action(McpState::MatchesScope, complete()),
            Action::NothingToDo
        );
    }

    #[test]
    fn allowlist_present_mcp_missing_is_mcp_only() {
        assert_eq!(
            plan_action(McpState::NotConfigured, complete()),
            Action::AddMcpOnly
        );
    }

    #[test]
    fn mcp_present_allowlist_missing_is_allowlist_only() {
        assert_eq!(
            plan_action(McpState::MatchesScope, missing()),
            Action::AddAllowlistOnly
        );
    }

    #[test]
    fn mcp_present_allowlist_partial_is_allowlist_only() {
        assert_eq!(
            plan_action(McpState::MatchesScope, partial()),
            Action::AddAllowlistOnly
        );
    }

    #[test]
    fn neither_configured_is_add_both() {
        assert_eq!(
            plan_action(McpState::NotConfigured, missing()),
            Action::AddBoth
        );
    }

    #[test]
    fn differing_mcp_entry_is_conflict_even_with_complete_allowlist() {
        assert_eq!(plan_action(McpState::Differs, complete()), Action::Conflict);
    }

    #[test]
    fn differing_mcp_entry_is_conflict_even_with_missing_allowlist() {
        assert_eq!(plan_action(McpState::Differs, missing()), Action::Conflict);
    }

    #[test]
    fn allowlist_status_is_complete_requires_nonzero_total() {
        assert!(
            !AllowlistStatus {
                present: 0,
                total: 0
            }
            .is_complete()
        );
        assert!(complete().is_complete());
        assert!(!partial().is_complete());
        assert!(!missing().is_complete());
    }
}
