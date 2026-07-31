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

fn usage() -> ! {
    eprintln!("claude-bus — a message bus for Claude Code agents");
    eprintln!();
    eprintln!("  claude-bus serve [--port 7777] [--data ./data]");
    eprintln!("  claude-bus agent [--bus ws://host:7777/ws] [--name <n>] [--name-template <t>]");
    eprintln!("  claude-bus tail <room> [--bus ws://host:7777/ws]");
    eprintln!("  claude-bus chat <room> [--bus ws://host:7777/ws] [--name <n>]");
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
            let port: u16 = flag(&args, "--port")
                .and_then(|p| p.parse().ok())
                .unwrap_or(7777);
            let data = flag(&args, "--data").unwrap_or_else(|| "./data".to_string());
            claude_bus::bus::serve(port, std::path::PathBuf::from(data)).await?;
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
        Some("chat") => {
            let room = args
                .get(2)
                .filter(|a| !a.starts_with("--"))
                .cloned()
                .unwrap_or_else(|| {
                    eprintln!(
                        "usage: claude-bus chat <room> [--bus ws://host:7777/ws] [--name <n>]"
                    );
                    std::process::exit(2);
                });
            let bus = flag(&args, "--bus").unwrap_or_else(|| "ws://127.0.0.1:7777/ws".to_string());
            let name = flag(&args, "--name")
                .or_else(|| std::env::var("USER").ok())
                .unwrap_or_else(|| "human".to_string());
            claude_bus::chat::run(bus, room, name).await?;
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
        _ => usage(),
    }
}
