//! `claude-bus spawn` — start a new project from chat.
//!
//! Creates `<root>/<name>`, wires it to the bus, and starts an interactive
//! Claude Code session in it inside the human's usual tmux layout (session
//! `<name>`: window 0 `claude`, window 1 a shell), then waits for the new agent
//! to come online. Run by an agent already on the host (hub, on hardac) on the
//! human's say-so; a later per-host spawner daemon calls the same `run`.
//!
//! Everything that touches the outside world goes through `System`, so the
//! sequence — and its cleanup on each failure — is tested without tmux, git, or
//! a bus. See docs/superpowers/specs/2026-10-24-spawn-design.md.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::launch::{LaunchMode, build_claude_args};

/// How long to wait for the new session to register on the bus.
pub const REGISTER_TIMEOUT: Duration = Duration::from_secs(30);
/// How often to look at the claude window and the bus while waiting.
pub const POLL_EVERY: Duration = Duration::from_millis(500);
/// The pause between keypresses answering a dialog, so the TUI sees each one.
pub const KEY_GAP: Duration = Duration::from_millis(300);

pub struct SpawnRequest {
    pub name: String,
    pub prompt: Option<String>,
    pub mode: LaunchMode,
    /// `None` means `~/work`.
    pub root: Option<PathBuf>,
    pub bus: String,
    pub home: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SpawnOutcome {
    pub name: String,
    pub dir: PathBuf,
    pub tmux_session: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SpawnError(pub String);

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SpawnError {}

fn err(msg: impl Into<String>) -> SpawnError {
    SpawnError(msg.into())
}

/// Whether an agent is known to the bus, and if so whether it is online.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Unknown,
    Offline,
    Online,
}

/// The outside world, as `run` needs it.
pub trait System {
    fn path_exists(&self, p: &Path) -> bool;
    fn canonicalize(&self, p: &Path) -> std::io::Result<PathBuf>;
    fn create_dir(&self, p: &Path) -> std::io::Result<()>;
    fn remove_dir_all(&self, p: &Path) -> std::io::Result<()>;
    /// Run `program` with `args` (in `cwd` if given). `Ok(stdout)` on a zero
    /// exit, `Err(message)` otherwise.
    fn run(&self, program: &str, args: &[String], cwd: Option<&Path>) -> Result<String, String>;
    fn presence(&self, bus: &str, name: &str) -> impl Future<Output = Result<Presence, String>>;
    fn sleep(&self, d: Duration) -> impl Future<Output = ()>;
    fn log(&self, line: &str);
}

/// `^[a-z0-9-]+$`: the name becomes a directory, a tmux session and an agent
/// name, and this is safe as all three.
pub fn validate_name(name: &str) -> Result<(), SpawnError> {
    let ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(err(format!(
            "invalid name {name:?}: use lowercase letters, digits and dashes only"
        )))
    }
}

/// The directory projects are created in: `~/work`, or `requested` if it
/// resolves (symlinks followed) to `~/work` or below.
pub fn resolve_root(
    requested: Option<&Path>,
    home: &Path,
    canonicalize: impl Fn(&Path) -> std::io::Result<PathBuf>,
) -> Result<PathBuf, SpawnError> {
    let work = home.join("work");
    let work =
        canonicalize(&work).map_err(|e| err(format!("{} is not usable: {e}", work.display())))?;
    let Some(requested) = requested else {
        return Ok(work);
    };
    // Resolved, not compared as written: a symlink or `..` must not lead out.
    let root = canonicalize(requested)
        .map_err(|e| err(format!("--root {} is not usable: {e}", requested.display())))?;
    if root.starts_with(&work) {
        Ok(root)
    } else {
        Err(err(format!(
            "--root {} is outside {}; projects can only be created under it",
            requested.display(),
            work.display()
        )))
    }
}

/// The command tmux runs in window 0. Clears the spawning session's own Claude
/// Code variables, so the child cannot inherit its identity, and pins the agent
/// name.
pub fn claude_command(name: &str, mode: LaunchMode, prompt: Option<&str>) -> Vec<String> {
    let mut cmd: Vec<String> = vec!["env".into()];
    for var in [
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_PROJECT_DIR",
        "CLAUDE_CODE_ENTRYPOINT",
    ] {
        cmd.push("-u".into());
        cmd.push(var.into());
    }
    cmd.push(format!("CLAUDE_BUS_NAME={name}"));
    cmd.push("claude".into());
    let extra: Vec<String> = prompt.map(|p| vec![p.to_string()]).unwrap_or_default();
    cmd.extend(build_claude_args(mode, &extra));
    cmd
}

/// The two tmux invocations (arguments after `tmux`): the detached session with
/// window 0 `claude`, then window 1, a shell. Both start in `dir`.
pub fn tmux_commands(name: &str, dir: &Path, claude: &[String]) -> Vec<Vec<String>> {
    let dir = dir.display().to_string();
    let mut session: Vec<String> = [
        "new-session",
        "-d",
        "-s",
        name,
        "-n",
        "claude",
        "-c",
        &dir,
        "--",
    ]
    .iter()
    .map(|a| a.to_string())
    .collect();
    session.extend(claude.iter().cloned());
    let shell: Vec<String> = ["new-window", "-t", &format!("{name}:"), "-c", &dir]
        .iter()
        .map(|a| a.to_string())
        .collect();
    vec![session, shell]
}

/// A start-up dialog that blocks a session nobody is attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    /// "Is this a project you created or one you trust?" — shown for every new
    /// project root. Default selection: "No, exit".
    Trust,
    /// "WARNING: Loading development channels" — shown on every launch with the
    /// msgbus channel. Default selection: "I am using this for local development".
    DevChannels,
}

impl Dialog {
    /// The keys that pick the "proceed" option from the dialog's default selection.
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            Dialog::Trust => &["Down", "Enter"],
            Dialog::DevChannels => &["Enter"],
        }
    }
}

/// Which dialog, if any, the claude window shows. Matched on exact text, so a
/// reworded dialog is not recognized — and so never answered: spawn falls
/// through to its registration timeout instead of guessing. The trust prompt
/// is recognized only when it names `dir`, the directory this run created.
pub fn dialog_in(pane: &str, dir: &Path) -> Option<Dialog> {
    dialog_in_home(pane, dir, Path::new("/nonexistent-home"))
}

/// `dialog_in`, also accepting the trust prompt's `~/…` spelling of `dir` when
/// `dir` is under `home` (Claude Code may abbreviate the home directory).
pub fn dialog_in_home(pane: &str, dir: &Path, home: &Path) -> Option<Dialog> {
    let names_dir = pane.contains(&dir.display().to_string())
        || dir
            .strip_prefix(home)
            .is_ok_and(|rel| pane.contains(&format!("~/{}", rel.display())));
    if pane.contains("Is this a project you created or one you trust?")
        && pane.contains("Yes, I trust this folder")
        && names_dir
    {
        Some(Dialog::Trust)
    } else if pane.contains("WARNING: Loading development channels")
        && pane.contains("I am using this for local development")
    {
        Some(Dialog::DevChannels)
    } else {
        None
    }
}

fn mode_name(mode: LaunchMode) -> &'static str {
    match mode {
        LaunchMode::Auto => "auto",
        LaunchMode::Unattended => "unattended",
    }
}

fn log_line(
    req: &SpawnRequest,
    dir: Option<&Path>,
    result: &Result<SpawnOutcome, SpawnError>,
) -> String {
    serde_json::json!({
        "ts": crate::store::now_ms(),
        "name": req.name,
        "dir": dir.map(|d| d.display().to_string()),
        "mode": mode_name(req.mode),
        "result": if result.is_ok() { "ok" } else { "error" },
        "error": result.as_ref().err().map(|e| e.0.clone()),
    })
    .to_string()
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|a| a.to_string()).collect()
}

/// Validation that must pass before anything is created.
async fn preflight<S: System>(req: &SpawnRequest, sys: &S) -> Result<PathBuf, SpawnError> {
    validate_name(&req.name)?;
    let root = resolve_root(req.root.as_deref(), &req.home, |p| sys.canonicalize(p))?;
    let dir = root.join(&req.name);
    if sys.path_exists(&dir) {
        return Err(err(format!("{} already exists", dir.display())));
    }
    match sys.presence(&req.bus, &req.name).await {
        Ok(Presence::Unknown) => {}
        Ok(_) => {
            return Err(err(format!(
                "an agent named {:?} is already registered on the bus",
                req.name
            )));
        }
        Err(e) => return Err(err(format!("could not check the bus at {}: {e}", req.bus))),
    }
    if sys
        .run(
            "tmux",
            &args(&["has-session", "-t", &format!("={}", req.name)]),
            None,
        )
        .is_ok()
    {
        return Err(err(format!(
            "a tmux session named {:?} already exists",
            req.name
        )));
    }
    Ok(dir)
}

/// Steps 2-4. On failure, says whether the tmux session had been started, so the
/// caller knows what to clean up.
fn setup<S: System>(req: &SpawnRequest, sys: &S, dir: &Path) -> Result<(), (SpawnError, bool)> {
    let fail = |e: String, started| (err(e), started);
    // User scope already connects every project to the bus (and `init --project`
    // refuses when it exists), so only a host without it needs the project wired.
    let user_scope = sys
        .run("claude", &args(&["mcp", "get", "msgbus"]), Some(dir))
        .ok()
        .and_then(|out| crate::init::parse_mcp_scope(&out))
        == Some(crate::init::Scope::User);
    if !user_scope {
        sys.run(
            "claude-bus",
            &args(&["init", "--project", "--yes", "--bus", &req.bus]),
            Some(dir),
        )
        .map_err(|e| fail(format!("claude-bus init failed: {e}"), false))?;
    }
    let claude = claude_command(&req.name, req.mode, req.prompt.as_deref());
    let [session, shell]: [Vec<String>; 2] = tmux_commands(&req.name, dir, &claude)
        .try_into()
        .expect("two tmux commands");
    sys.run("tmux", &session, None)
        .map_err(|e| fail(format!("tmux new-session failed: {e}"), false))?;
    sys.run("tmux", &shell, None)
        .map_err(|e| fail(format!("tmux new-window failed: {e}"), true))?;
    Ok(())
}

/// The last few lines of the claude window, for an error the human can act on.
fn pane_tail<S: System>(sys: &S, name: &str) -> String {
    let pane = sys
        .run(
            "tmux",
            &args(&["capture-pane", "-p", "-t", &format!("={name}:claude")]),
            None,
        )
        .unwrap_or_default();
    let lines: Vec<&str> = pane.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(8)..].join("\n")
}

/// Start a new project. See the module doc.
pub async fn run<S: System>(req: &SpawnRequest, sys: &S) -> Result<SpawnOutcome, SpawnError> {
    let (dir, result) = match preflight(req, sys).await {
        Err(e) => (None, Err(e)),
        Ok(dir) => {
            let result = create_and_start(req, sys, &dir).await;
            (Some(dir), result)
        }
    };
    sys.log(&log_line(req, dir.as_deref(), &result));
    result
}

async fn create_and_start<S: System>(
    req: &SpawnRequest,
    sys: &S,
    dir: &Path,
) -> Result<SpawnOutcome, SpawnError> {
    sys.create_dir(dir)
        .map_err(|e| err(format!("could not create {}: {e}", dir.display())))?;
    if let Err((e, started)) = setup(req, sys, dir) {
        // Everything here was created by this run, so it is safe to undo.
        if started {
            let _ = sys.run(
                "tmux",
                &args(&["kill-session", "-t", &format!("={}", req.name)]),
                None,
            );
        }
        let _ = sys.remove_dir_all(dir);
        return Err(e);
    }

    let polls = (REGISTER_TIMEOUT.as_millis() / POLL_EVERY.as_millis()).max(1);
    let target = format!("={}:claude", req.name);
    let mut answered: Vec<Dialog> = Vec::new();
    for _ in 0..polls {
        // `-J` joins wrapped lines, so a long directory path is matched whole.
        let pane = sys
            .run(
                "tmux",
                &args(&["capture-pane", "-p", "-J", "-t", &target]),
                None,
            )
            .unwrap_or_default();
        if let Some(dialog) = dialog_in_home(&pane, dir, &req.home)
            && !answered.contains(&dialog)
        {
            // Each dialog once: if it is still on screen next poll, the TUI has
            // not redrawn yet, and a second answer could land on the next one.
            answered.push(dialog);
            for key in dialog.keys() {
                let _ = sys.run("tmux", &args(&["send-keys", "-t", &target, key]), None);
                sys.sleep(KEY_GAP).await;
            }
        }
        if let Ok(Presence::Online) = sys.presence(&req.bus, &req.name).await {
            // Only now: a git repository nested in ~/work is not covered by its
            // trust, so initializing before the session started would have raised
            // the trust dialog. The running session is unaffected.
            sys.run("git", &args(&["init", "-q"]), Some(dir))
                .map_err(|e| {
                    err(format!(
                        "{name:?} is online (`tmux attach -t {name}`), but git init failed: {e}. \
                     Run `git init` in {dir} yourself.",
                        name = req.name,
                        dir = dir.display()
                    ))
                })?;
            return Ok(SpawnOutcome {
                name: req.name.clone(),
                dir: dir.to_path_buf(),
                tmux_session: req.name.clone(),
            });
        }
        sys.sleep(POLL_EVERY).await;
    }
    // Left running on purpose: the human can attach and finish by hand.
    Err(err(format!(
        "{name:?} did not come online within {secs}s. The session is still running: \
         `tmux attach -t {name}` to see why. Last lines of its claude window:\n{tail}",
        name = req.name,
        secs = REGISTER_TIMEOUT.as_secs(),
        tail = pane_tail(sys, &req.name),
    )))
}

/// The real world: the filesystem, child processes, the bus (as an observer),
/// and an append-only log at `$XDG_STATE_HOME/claude-bus/spawn.log`.
pub struct RealSystem;

impl System for RealSystem {
    fn path_exists(&self, p: &Path) -> bool {
        // symlink_metadata, so even a dangling symlink at the path counts as taken.
        std::fs::symlink_metadata(p).is_ok()
    }
    fn canonicalize(&self, p: &Path) -> std::io::Result<PathBuf> {
        std::fs::canonicalize(p)
    }
    fn create_dir(&self, p: &Path) -> std::io::Result<()> {
        // Not create_dir_all: a directory that appeared since the check is refused.
        std::fs::create_dir(p)
    }
    fn remove_dir_all(&self, p: &Path) -> std::io::Result<()> {
        std::fs::remove_dir_all(p)
    }
    fn run(&self, program: &str, args: &[String], cwd: Option<&Path>) -> Result<String, String> {
        // `claude-bus` means this binary, not whatever an older PATH entry finds.
        let program = if program == "claude-bus" {
            std::env::current_exe()
                .map_err(|e| format!("cannot find this claude-bus binary: {e}"))?
        } else {
            PathBuf::from(program)
        };
        let mut cmd = std::process::Command::new(&program);
        cmd.args(args);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let out = cmd
            .output()
            .map_err(|e| format!("could not run {}: {e}", program.display()))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("{} exited with {}", program.display(), out.status)
            } else {
                stderr
            })
        }
    }
    async fn presence(&self, bus: &str, name: &str) -> Result<Presence, String> {
        let agents = crate::status_cli::fetch_agents(bus)
            .await
            .map_err(|e| e.to_string())?;
        Ok(match agents.iter().find(|a| a.name == name) {
            None => Presence::Unknown,
            Some(a) if a.online => Presence::Online,
            Some(_) => Presence::Offline,
        })
    }
    async fn sleep(&self, d: Duration) {
        tokio::time::sleep(d).await
    }
    fn log(&self, line: &str) {
        eprintln!("spawn: {line}");
        let Some(dir) = crate::agent::status::status_dir(
            std::env::var("XDG_STATE_HOME").ok(),
            std::env::var("HOME").ok(),
        )
        .and_then(|status| status.parent().map(Path::to_path_buf)) else {
            return;
        };
        let _ = std::fs::create_dir_all(&dir);
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("spawn.log"))
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashSet;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn names_are_lowercase_letters_digits_and_dashes() {
        for ok in ["foo", "my-project-2", "a"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", "Foo", "foo bar", "../x", "a/b", "x.y", "dm:a|b", "naïve",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn the_root_defaults_to_work_and_must_stay_under_it() {
        let home = Path::new("/home/b");
        let canon = |p: &Path| -> std::io::Result<PathBuf> {
            // `/home/b/link` is a symlink out to `/tmp/elsewhere`.
            Ok(match p.to_str().unwrap() {
                "/home/b/link" => PathBuf::from("/tmp/elsewhere"),
                "/home/b/work/../etc" => PathBuf::from("/home/b/etc"),
                other => PathBuf::from(other),
            })
        };
        assert_eq!(
            resolve_root(None, home, canon).unwrap(),
            PathBuf::from("/home/b/work")
        );
        assert_eq!(
            resolve_root(Some(Path::new("/home/b/work/clients")), home, canon).unwrap(),
            PathBuf::from("/home/b/work/clients")
        );
        for bad in [
            "/home/b/link",
            "/home/b/work/../etc",
            "/tmp",
            "/home/b/workshop",
        ] {
            assert!(
                resolve_root(Some(Path::new(bad)), home, canon).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn the_claude_command_clears_the_spawners_identity_and_pins_the_name() {
        let cmd = claude_command("foo", LaunchMode::Auto, Some("build it"));
        assert_eq!(
            &cmd[..10],
            &s(&[
                "env",
                "-u",
                "CLAUDECODE",
                "-u",
                "CLAUDE_CODE_SESSION_ID",
                "-u",
                "CLAUDE_PROJECT_DIR",
                "-u",
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_BUS_NAME=foo",
            ])[..]
        );
        assert_eq!(cmd[10], "claude");
        assert_eq!(
            cmd[11..],
            build_claude_args(LaunchMode::Auto, &s(&["build it"]))[..]
        );
        let bare = claude_command("foo", LaunchMode::Unattended, None);
        assert_eq!(bare.last().unwrap(), "--dangerously-skip-permissions");
    }

    #[test]
    fn tmux_makes_the_humans_layout_in_the_default_server() {
        let cmds = tmux_commands("foo", Path::new("/home/b/work/foo"), &s(&["claude"]));
        assert_eq!(
            cmds,
            vec![
                s(&[
                    "new-session",
                    "-d",
                    "-s",
                    "foo",
                    "-n",
                    "claude",
                    "-c",
                    "/home/b/work/foo",
                    "--",
                    "claude"
                ]),
                s(&["new-window", "-t", "foo:", "-c", "/home/b/work/foo"]),
            ]
        );
        assert!(
            !cmds.iter().flatten().any(|a| a == "-L" || a == "-S"),
            "no private socket: the session must show up in the human's own tmux"
        );
    }

    /// A scripted world: records every command, and answers them from rules.
    struct Fake {
        existing: RefCell<HashSet<PathBuf>>,
        calls: RefCell<Vec<String>>,
        user_scope_mcp: bool,
        fail_on: Option<&'static str>,
        presence_before: Presence,
        /// The agent comes online after this many `presence` checks during the wait.
        online_after: Option<usize>,
        checks: RefCell<usize>,
        logs: RefCell<Vec<String>>,
        removed: RefCell<Vec<PathBuf>>,
        /// What successive `capture-pane` calls return; empty once exhausted.
        panes: RefCell<std::collections::VecDeque<String>>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                existing: RefCell::new(HashSet::new()),
                calls: RefCell::new(vec![]),
                user_scope_mcp: true,
                fail_on: None,
                presence_before: Presence::Unknown,
                online_after: Some(2),
                checks: RefCell::new(0),
                logs: RefCell::new(vec![]),
                removed: RefCell::new(vec![]),
                panes: RefCell::new(Default::default()),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
        fn called(&self, prefix: &str) -> bool {
            self.calls().iter().any(|c| c.starts_with(prefix))
        }
    }

    impl System for Fake {
        fn path_exists(&self, p: &Path) -> bool {
            self.existing.borrow().contains(p)
        }
        fn canonicalize(&self, p: &Path) -> std::io::Result<PathBuf> {
            Ok(p.to_path_buf())
        }
        fn create_dir(&self, p: &Path) -> std::io::Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("mkdir {}", p.display()));
            self.existing.borrow_mut().insert(p.to_path_buf());
            Ok(())
        }
        fn remove_dir_all(&self, p: &Path) -> std::io::Result<()> {
            self.removed.borrow_mut().push(p.to_path_buf());
            self.existing.borrow_mut().remove(p);
            Ok(())
        }
        fn run(
            &self,
            program: &str,
            args: &[String],
            _cwd: Option<&Path>,
        ) -> Result<String, String> {
            let line = format!("{program} {}", args.join(" "));
            self.calls.borrow_mut().push(line.clone());
            if let Some(f) = self.fail_on
                && line.starts_with(f)
            {
                return Err(format!("{f} failed"));
            }
            if line.starts_with("tmux has-session") {
                return Err("no such session".into());
            }
            if line.starts_with("claude mcp get msgbus") {
                return if self.user_scope_mcp {
                    Ok("msgbus:\n  Scope: User config (available in all your projects)".into())
                } else {
                    Err("No MCP server found".into())
                };
            }
            if line.starts_with("tmux capture-pane") {
                return Ok(self.panes.borrow_mut().pop_front().unwrap_or_default());
            }
            Ok(String::new())
        }
        async fn presence(&self, _bus: &str, _name: &str) -> Result<Presence, String> {
            let mut n = self.checks.borrow_mut();
            *n += 1;
            // The first check is the "is the name free" one, before anything is created.
            if *n == 1 {
                return Ok(self.presence_before);
            }
            Ok(match self.online_after {
                Some(k) if *n > k => Presence::Online,
                _ => Presence::Unknown,
            })
        }
        async fn sleep(&self, _d: Duration) {}
        fn log(&self, line: &str) {
            self.logs.borrow_mut().push(line.to_string());
        }
    }

    fn req(name: &str) -> SpawnRequest {
        SpawnRequest {
            name: name.into(),
            prompt: None,
            mode: LaunchMode::Auto,
            root: None,
            bus: "ws://bus/ws".into(),
            home: PathBuf::from("/home/b"),
        }
    }

    #[tokio::test]
    async fn a_spawn_creates_the_project_starts_tmux_and_waits_for_the_agent() {
        let sys = Fake::new();
        let out = run(&req("foo"), &sys).await.unwrap();
        assert_eq!(
            out,
            SpawnOutcome {
                name: "foo".into(),
                dir: PathBuf::from("/home/b/work/foo"),
                tmux_session: "foo".into(),
            }
        );
        let calls = sys.calls();
        let pos = |p: &str| {
            calls
                .iter()
                .position(|c| c.starts_with(p))
                .unwrap_or_else(|| panic!("{p} never ran: {calls:#?}"))
        };
        assert!(pos("mkdir /home/b/work/foo") < pos("tmux new-session"));
        assert!(pos("tmux new-session") < pos("tmux new-window"));
        // git init comes only once the agent is online: a nested git repository is
        // not covered by ~/work's trust, so initializing first would raise the
        // trust dialog (verified live 2026-10-24).
        assert!(pos("tmux new-window") < pos("git init"));
        assert!(
            *sys.checks.borrow() >= 3,
            "online was checked before git init"
        );
        assert!(
            !sys.called("claude-bus init"),
            "user scope already wires it"
        );
        assert_eq!(sys.logs.borrow().len(), 1, "one log line per spawn");
        assert!(
            sys.logs.borrow()[0].contains("\"result\":\"ok\""),
            "{:?}",
            sys.logs.borrow()
        );
    }

    #[tokio::test]
    async fn without_a_user_scope_entry_the_project_is_wired_with_init() {
        let mut sys = Fake::new();
        sys.user_scope_mcp = false;
        run(&req("foo"), &sys).await.unwrap();
        assert!(
            sys.called("claude-bus init --project --yes --bus ws://bus/ws"),
            "{:#?}",
            sys.calls()
        );
    }

    #[tokio::test]
    async fn an_existing_directory_or_registered_name_is_refused_before_anything_is_created() {
        let sys = Fake::new();
        sys.existing
            .borrow_mut()
            .insert(PathBuf::from("/home/b/work/foo"));
        assert!(run(&req("foo"), &sys).await.is_err());
        assert!(!sys.called("mkdir") && !sys.called("tmux new-session"));

        let mut sys = Fake::new();
        sys.presence_before = Presence::Offline;
        let e = run(&req("foo"), &sys).await.unwrap_err();
        assert!(e.0.contains("already"), "{e}");
        assert!(!sys.called("mkdir"));
    }

    #[tokio::test]
    async fn a_failure_before_tmux_starts_removes_the_directory_it_created() {
        let mut sys = Fake::new();
        sys.fail_on = Some("tmux new-session");
        assert!(run(&req("foo"), &sys).await.is_err());
        assert_eq!(
            *sys.removed.borrow(),
            vec![PathBuf::from("/home/b/work/foo")]
        );
        assert_eq!(sys.logs.borrow().len(), 1);
        assert!(sys.logs.borrow()[0].contains("\"result\":\"error\""));
    }

    #[tokio::test]
    async fn a_failure_opening_the_shell_window_kills_the_session_and_removes_the_directory() {
        let mut sys = Fake::new();
        sys.fail_on = Some("tmux new-window");
        assert!(run(&req("foo"), &sys).await.is_err());
        assert!(
            sys.called("tmux kill-session -t =foo"),
            "{:#?}",
            sys.calls()
        );
        assert_eq!(
            *sys.removed.borrow(),
            vec![PathBuf::from("/home/b/work/foo")]
        );
    }

    #[tokio::test]
    async fn a_registration_timeout_leaves_the_session_up_for_the_human() {
        let mut sys = Fake::new();
        sys.online_after = None;
        let e = run(&req("foo"), &sys).await.unwrap_err();
        assert!(
            e.0.contains("tmux attach -t foo"),
            "the error must say how to finish by hand: {e}"
        );
        assert!(
            !sys.called("tmux kill-session"),
            "the session must be left running"
        );
        assert!(sys.removed.borrow().is_empty(), "and the project kept");
    }

    const TRUST_PANE: &str = " Accessing workspace:\n /home/b/work/foo\n Quick safety check: Is this a project you created or one you trust? (Like your own code...)\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel";
    const CHANNELS_PANE: &str = "  WARNING: Loading development channels\n  --dangerously-load-development-channels is for local channel development only.\n  Channels: server:msgbus\n  ❯ 1. I am using this for local development\n    2. Exit";

    #[test]
    fn the_trust_dialog_is_recognized_only_for_the_directory_spawn_created() {
        let dir = Path::new("/home/b/work/foo");
        assert_eq!(dialog_in(TRUST_PANE, dir), Some(Dialog::Trust));
        assert_eq!(
            dialog_in(TRUST_PANE, Path::new("/home/b/work/other")),
            None,
            "a trust prompt for any other directory is never answered"
        );
        assert_eq!(dialog_in(CHANNELS_PANE, dir), Some(Dialog::DevChannels));
        assert_eq!(dialog_in("❯ Reply with ready\n● ready", dir), None);
        // Reworded by a Claude Code update: not recognized, so not answered.
        assert_eq!(
            dialog_in(
                " Do you trust the files in this folder?\n /home/b/work/foo",
                dir
            ),
            None
        );
    }

    #[test]
    fn each_dialog_is_answered_with_the_keys_for_its_default_selection() {
        assert_eq!(Dialog::Trust.keys(), &["Down", "Enter"]);
        assert_eq!(Dialog::DevChannels.keys(), &["Enter"]);
    }

    #[tokio::test]
    async fn the_start_up_dialogs_are_answered_once_each_before_the_agent_comes_online() {
        let mut sys = Fake::new();
        sys.online_after = Some(5);
        // The trust prompt lingers for a poll (a slow redraw), then the channels warning.
        sys.panes = RefCell::new(
            [TRUST_PANE, TRUST_PANE, CHANNELS_PANE, ""]
                .iter()
                .map(|p| p.to_string())
                .collect(),
        );
        run(&req("foo"), &sys).await.unwrap();
        let keys: Vec<String> = sys
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("tmux send-keys"))
            .collect();
        assert_eq!(
            keys,
            vec![
                "tmux send-keys -t =foo:claude Down".to_string(),
                "tmux send-keys -t =foo:claude Enter".to_string(),
                "tmux send-keys -t =foo:claude Enter".to_string(),
            ],
            "trust once (Down, Enter), then the channels warning once (Enter)"
        );
    }

    #[tokio::test]
    async fn a_trust_prompt_for_another_directory_is_never_answered() {
        let mut sys = Fake::new();
        sys.online_after = None;
        sys.panes = RefCell::new(
            std::iter::repeat_n(
                TRUST_PANE.replace("/home/b/work/foo", "/home/b/work/other"),
                3,
            )
            .collect(),
        );
        assert!(run(&req("foo"), &sys).await.is_err());
        assert!(!sys.called("tmux send-keys"), "{:#?}", sys.calls());
    }

    #[test]
    fn the_trust_prompt_may_show_the_directory_with_a_tilde() {
        let tilde = TRUST_PANE.replace("/home/b/work/foo", "~/work/foo");
        assert_eq!(
            dialog_in_home(&tilde, Path::new("/home/b/work/foo"), Path::new("/home/b")),
            Some(Dialog::Trust)
        );
        assert_eq!(
            dialog_in_home(&tilde, Path::new("/home/b/work/foo"), Path::new("/home/c")),
            None,
            "the tilde form only counts for the right home"
        );
    }

    #[tokio::test]
    async fn a_git_init_failure_after_the_agent_is_online_says_so_and_leaves_it_running() {
        let mut sys = Fake::new();
        sys.fail_on = Some("git init");
        let e = run(&req("foo"), &sys).await.unwrap_err();
        assert!(
            e.0.contains("online"),
            "the session is up and that must be clear: {e}"
        );
        assert!(e.0.contains("git init"), "{e}");
        assert!(!sys.called("tmux kill-session"));
        assert!(sys.removed.borrow().is_empty());
    }
}
