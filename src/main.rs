use claude_bus::config;

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn usage() -> ! {
    eprintln!("claude-bus — a message bus for Claude Code agents");
    eprintln!();
    eprintln!("  claude-bus serve [--port 7777] [--data ./data]");
    eprintln!("  claude-bus agent [--bus ws://host:7777/ws] [--name <n>] [--name-template <t>]");
    eprintln!("  claude-bus tail <room> [--bus ws://host:7777/ws]");
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
            println!("serve on {port}, data at {data} — not yet implemented");
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
            // stdout is the JSON-RPC transport in agent mode: stderr only.
            eprintln!("agent name resolved to {name} — not yet implemented");
            Ok(())
        }
        Some("tail") => {
            eprintln!("tail — not yet implemented");
            Ok(())
        }
        _ => usage(),
    }
}
