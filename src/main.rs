use claude_bus::config;
use claude_bus::init::{self, InitArgs, Scope};

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// Every value of a repeatable flag, e.g. `--relayer hub --relayer voice`.
fn flags(args: &[String], name: &str) -> Vec<String> {
    args.iter()
        .enumerate()
        .filter(|(_, a)| a.as_str() == name)
        .filter_map(|(i, _)| args.get(i + 1).cloned())
        .collect()
}

/// True if `name` appears with no value following it — either as the last argument, or
/// immediately before another flag. `flags` above silently drops such an occurrence
/// rather than erroring, which for a repeatable flag like `--relayer` would otherwise
/// leave a mistyped invocation looking identical to "no relayers configured".
fn has_valueless_occurrence(args: &[String], name: &str) -> bool {
    args.iter().enumerate().any(|(i, a)| {
        a.as_str() == name && args.get(i + 1).is_none_or(|next| next.starts_with("--"))
    })
}

fn usage() -> ! {
    eprintln!("claude-bus — a message bus for Claude Code agents");
    eprintln!();
    eprintln!(
        "  claude-bus serve [--port 7777] [--data ./data] [--relayer <name>]... \
         [--relayer-secret <s>] [--reserve-name <n>]... [--exchange-cap 50]"
    );
    eprintln!("  claude-bus agent [--bus ws://host:7777/ws] [--name <n>] [--name-template <t>]");
    eprintln!(
        "  claude-bus launch [<claude args>...]   # start claude wired for the bus \
         (non-interactive; e.g. add --continue)"
    );
    eprintln!(
        "  claude-bus spawn <name> [--prompt <task>] [--mode auto|unattended] [--root <dir>] \
         [--bus ws://host:7777/ws]   # new project + session in tmux"
    );
    eprintln!("  claude-bus tail <room> [--bus ws://host:7777/ws]");
    eprintln!("  claude-bus status [--bus ws://host:7777/ws]   # every agent's status");
    eprintln!("  claude-bus chat (<room> | --to <agent>) [--bus ws://host:7777/ws] [--name <n>]");
    eprintln!(
        "  claude-bus init [--user | --project] [--bus ws://host:7777/ws] [--dry-run] [--yes] \
         [--force]"
    );
    std::process::exit(2);
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") => {
            if has_valueless_occurrence(&args, "--relayer") {
                eprintln!("claude-bus serve: --relayer requires a value, e.g. --relayer hub");
                std::process::exit(2);
            }
            let port: u16 = flag(&args, "--port")
                .and_then(|p| p.parse().ok())
                .unwrap_or(7777);
            let data = flag(&args, "--data").unwrap_or_else(|| "./data".to_string());
            let relayers = claude_bus::bus::Relayers::new(flags(&args, "--relayer"));
            let participants = claude_bus::bus::participant::ParticipantConfig {
                relayer_secret: flag(&args, "--relayer-secret"),
                reserved_names: flags(&args, "--reserve-name").into_iter().collect(),
                ..Default::default()
            };
            let exchange_cap = match claude_bus::bus::delivery::parse_exchange_cap(
                flag(&args, "--exchange-cap").as_deref(),
            ) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("claude-bus serve: {e}");
                    std::process::exit(2);
                }
            };
            claude_bus::bus::serve(
                port,
                std::path::PathBuf::from(data),
                relayers,
                participants,
                exchange_cap,
            )
            .await?;
            Ok(())
        }
        Some("launch") => {
            // Everything after "launch" is forwarded to `claude` verbatim.
            claude_bus::launch::run(args[2..].to_vec())?;
            Ok(())
        }
        Some("agent") => {
            let name = config::resolve_name(
                &config::NameArgs {
                    name: flag(&args, "--name"),
                    template: flag(&args, "--name-template"),
                },
                &config::RealEnv,
            );
            let bus = flag(&args, "--bus").unwrap_or_else(|| "ws://127.0.0.1:7777/ws".to_string());
            claude_bus::agent::run(bus, name).await?;
            Ok(())
        }
        Some("tail") => {
            let bus = flag(&args, "--bus").unwrap_or_else(|| "ws://127.0.0.1:7777/ws".to_string());
            // The room is the first positional argument after "tail".
            let room = args.get(2).filter(|a| !a.starts_with("--")).cloned();
            claude_bus::tail::run(bus, room).await?;
            Ok(())
        }
        Some("status") => {
            let bus = flag(&args, "--bus").unwrap_or_else(|| "ws://127.0.0.1:7777/ws".to_string());
            claude_bus::status_cli::run(bus).await?;
            Ok(())
        }
        Some("chat") => {
            let to = flag(&args, "--to");
            let positional = args.get(2).filter(|a| !a.starts_with("--")).cloned();
            let Some(target) = claude_bus::chat::chat_target(positional, to) else {
                eprintln!(
                    "usage: claude-bus chat (<room> | --to <agent>) \
                     [--bus ws://host:7777/ws] [--name <n>]"
                );
                std::process::exit(2);
            };
            let bus = flag(&args, "--bus").unwrap_or_else(|| "ws://127.0.0.1:7777/ws".to_string());
            let name = flag(&args, "--name")
                .or_else(|| std::env::var("USER").ok())
                .unwrap_or_else(|| "human".to_string());
            claude_bus::chat::run(bus, target, name).await?;
            Ok(())
        }
        Some("init") => {
            let user = has_flag(&args, "--user");
            let project = has_flag(&args, "--project");
            if user && project {
                eprintln!("claude-bus init: pass only one of --user or --project");
                std::process::exit(2);
            }
            let scope = if user {
                Some(Scope::User)
            } else if project {
                Some(Scope::Project)
            } else {
                None
            };
            let init_args = InitArgs {
                scope,
                bus: flag(&args, "--bus"),
                dry_run: has_flag(&args, "--dry-run"),
                yes: has_flag(&args, "--yes"),
                force: has_flag(&args, "--force"),
            };
            init::run(init_args)?;
            Ok(())
        }
        Some("spawn") => {
            let usage = "usage: claude-bus spawn <name> [--prompt \"<first task>\"] \
                         [--mode auto|unattended] [--root <dir>] [--bus ws://host:7777/ws]";
            let Some(name) = args.get(2).filter(|a| !a.starts_with("--")).cloned() else {
                eprintln!("{usage}");
                std::process::exit(2);
            };
            let mode = match flag(&args, "--mode").as_deref() {
                None | Some("auto") => claude_bus::launch::LaunchMode::Auto,
                Some("unattended") => claude_bus::launch::LaunchMode::Unattended,
                Some(other) => {
                    eprintln!("claude-bus spawn: --mode must be auto or unattended, got {other:?}");
                    std::process::exit(2);
                }
            };
            let Some(home) = std::env::var_os("HOME") else {
                eprintln!("claude-bus spawn: HOME is not set");
                std::process::exit(2);
            };
            let req = claude_bus::spawn::SpawnRequest {
                name,
                prompt: flag(&args, "--prompt"),
                mode,
                root: flag(&args, "--root").map(std::path::PathBuf::from),
                bus: flag(&args, "--bus").unwrap_or_else(|| "ws://127.0.0.1:7777/ws".to_string()),
                home: std::path::PathBuf::from(home),
            };
            match claude_bus::spawn::run(&req, &claude_bus::spawn::RealSystem).await {
                Ok(out) => {
                    println!("spawned {}", out.name);
                    println!("  directory     {}", out.dir.display());
                    println!(
                        "  tmux session  {}  (window 0: claude, window 1: shell)",
                        out.tmux_session
                    );
                    println!("  attach        tmux attach -t {}", out.tmux_session);
                    Ok(())
                }
                Err(e) => {
                    eprintln!("claude-bus spawn: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("hook") => {
            // Not a person-facing command: Claude Code invokes this directly, installed
            // by `claude-bus init`. Synchronous and infallible by design; see
            // `claude_bus::hook`.
            claude_bus::hook::run(&args[2..]);
            Ok(())
        }
        _ => usage(),
    }
}
