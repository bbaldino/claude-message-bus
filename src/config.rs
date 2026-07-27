//! Agent name resolution. Claude Code does not supply a name, so the agent
//! process picks one at startup.

/// Indirection over the process environment so resolution is testable without
/// mutating real env vars, which races across parallel tests.
pub trait EnvSource {
    fn var(&self, key: &str) -> Option<String>;
    fn cwd(&self) -> Option<String>;
    fn hostname(&self) -> String;
}

pub struct RealEnv;

impl EnvSource for RealEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
    fn cwd(&self) -> Option<String> {
        std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    }
    fn hostname(&self) -> String {
        // `/proc/sys/kernel/hostname` reflects the running kernel hostname —
        // correct on every Linux host, including one whose static
        // `/etc/hostname` was never set (the normal state for a systemd
        // machine using DHCP, and for many containers). `/etc/hostname` is
        // consulted only as a fallback for the rare case `/proc` isn't
        // mounted.
        let kernel = std::fs::read_to_string("/proc/sys/kernel/hostname").ok();
        let etc = std::fs::read_to_string("/etc/hostname").ok();
        choose_hostname([kernel.as_deref(), etc.as_deref()])
    }
}

/// Picks the first non-empty (after trimming) candidate, in source-priority
/// order, falling back to `"unknown"` if every candidate is absent, empty, or
/// whitespace-only. Pure and filesystem-free so the source-selection logic
/// — the part that actually shipped broken — is unit-testable without
/// touching real files.
fn choose_hostname<'a>(candidates: impl IntoIterator<Item = Option<&'a str>>) -> String {
    candidates
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

#[derive(Debug, Default, Clone)]
pub struct NameArgs {
    pub name: Option<String>,
    pub template: Option<String>,
}

/// Lowercase; every non-alphanumeric becomes `-`. Names appear inside
/// `<channel from="...">` attributes and DM room keys, so they must stay tame.
pub fn sanitize(raw: &str) -> String {
    raw.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn project_dir_basename(env: &dyn EnvSource) -> Option<String> {
    let path = env.var("CLAUDE_PROJECT_DIR").or_else(|| env.cwd())?;
    std::path::Path::new(&path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
}

/// First match wins: --name, then CLAUDE_BUS_NAME, then --name-template,
/// then the project directory basename.
pub fn resolve_name(args: &NameArgs, env: &dyn EnvSource) -> String {
    if let Some(n) = &args.name {
        return sanitize(n);
    }
    if let Some(n) = env.var("CLAUDE_BUS_NAME") {
        return sanitize(&n);
    }
    let dir = project_dir_basename(env).unwrap_or_else(|| "agent".to_string());
    if let Some(t) = &args.template {
        let expanded = t
            .replace("{dir}", &dir)
            .replace("{host}", &env.hostname())
            .replace("{user}", &env.var("USER").unwrap_or_else(|| "user".into()));
        return sanitize(&expanded);
    }
    sanitize(&dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeEnv {
        vars: HashMap<String, String>,
        cwd: Option<String>,
    }

    impl FakeEnv {
        fn new() -> Self {
            Self {
                vars: HashMap::new(),
                cwd: Some("/home/me/work/caas".into()),
            }
        }
        fn with(mut self, k: &str, v: &str) -> Self {
            self.vars.insert(k.into(), v.into());
            self
        }
    }

    impl EnvSource for FakeEnv {
        fn var(&self, key: &str) -> Option<String> {
            self.vars.get(key).cloned()
        }
        fn cwd(&self) -> Option<String> {
            self.cwd.clone()
        }
        fn hostname(&self) -> String {
            "lisa".into()
        }
    }

    fn args(name: Option<&str>, template: Option<&str>) -> NameArgs {
        NameArgs {
            name: name.map(String::from),
            template: template.map(String::from),
        }
    }

    #[test]
    fn explicit_name_wins_over_everything() {
        let env = FakeEnv::new().with("CLAUDE_BUS_NAME", "from-env");
        assert_eq!(
            resolve_name(&args(Some("explicit"), None), &env),
            "explicit"
        );
    }

    #[test]
    fn env_var_beats_template_and_dir() {
        let env = FakeEnv::new().with("CLAUDE_BUS_NAME", "from-env");
        assert_eq!(
            resolve_name(&args(None, Some("{dir}-agent")), &env),
            "from-env"
        );
    }

    #[test]
    fn template_substitutes_dir_host_and_user() {
        let env = FakeEnv::new().with("USER", "bbaldino");
        assert_eq!(
            resolve_name(&args(None, Some("{dir}-{host}-{user}")), &env),
            "caas-lisa-bbaldino"
        );
    }

    #[test]
    fn default_is_project_dir_basename() {
        assert_eq!(resolve_name(&args(None, None), &FakeEnv::new()), "caas");
    }

    #[test]
    fn claude_project_dir_is_preferred_over_cwd() {
        // Verified in POC 1: Claude Code exports CLAUDE_PROJECT_DIR to MCP
        // subprocesses. It is explicit and survives a later cd, so it wins.
        let env = FakeEnv::new().with("CLAUDE_PROJECT_DIR", "/home/me/work/dashboard");
        assert_eq!(resolve_name(&args(None, None), &env), "dashboard");
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize("My Project!"), "my-project-");
        assert_eq!(sanitize("Caas_V2"), "caas-v2");
        assert_eq!(sanitize("already-fine"), "already-fine");
    }

    #[test]
    fn falls_back_when_nothing_is_available() {
        let mut env = FakeEnv::new();
        env.cwd = None;
        assert_eq!(resolve_name(&args(None, None), &env), "agent");
    }

    #[test]
    fn choose_hostname_prefers_the_first_source() {
        assert_eq!(
            choose_hostname([Some("kernel-host"), Some("etc-host")]),
            "kernel-host"
        );
    }

    #[test]
    fn choose_hostname_falls_through_an_empty_first_source() {
        assert_eq!(choose_hostname([Some(""), Some("etc-host")]), "etc-host");
    }

    #[test]
    fn choose_hostname_falls_through_a_whitespace_only_first_source() {
        assert_eq!(
            choose_hostname([Some("   \n"), Some("etc-host")]),
            "etc-host"
        );
    }

    #[test]
    fn choose_hostname_falls_back_to_unknown_when_everything_is_empty() {
        assert_eq!(choose_hostname([Some(""), Some("  ")]), "unknown");
    }

    #[test]
    fn choose_hostname_falls_back_to_unknown_when_nothing_is_present() {
        assert_eq!(choose_hostname([None, None]), "unknown");
    }

    #[test]
    fn choose_hostname_trims_the_winning_value() {
        assert_eq!(choose_hostname([Some("  myhost\n")]), "myhost");
    }
}
