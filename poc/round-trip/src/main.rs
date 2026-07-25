//! POC 3 — two-session round trip.
//!
//! The walking skeleton of the real design: one binary, two subcommands.
//!   round-trip serve [--port 7777]
//!   round-trip agent [--bus ws://host:7777/ws] [--name <n>]
//!
//! In-memory only. No SQLite, no rooms, no file store — those come with the real
//! implementation. This exists to prove the full loop: A sends while B is idle,
//! B receives and replies, A receives the reply while *it* is idle.

mod agent;
mod bus;
mod proto;

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Mirrors the spec's naming rules: explicit flag, then env, then project dir.
fn default_name() -> String {
    std::env::var("CLAUDE_PROJECT_DIR")
        .ok()
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .and_then(|p| {
            std::path::Path::new(&p)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "agent".to_string())
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") => {
            let port = arg(&args, "--port")
                .and_then(|p| p.parse().ok())
                .unwrap_or(7777);
            bus::serve(port).await
        }
        Some("agent") => {
            let bus_url =
                arg(&args, "--bus").unwrap_or_else(|| "ws://127.0.0.1:7777/ws".to_string());
            let name = arg(&args, "--name")
                .or_else(|| std::env::var("CLAUDE_BUS_NAME").ok())
                .unwrap_or_else(default_name);
            agent::run(bus_url, name).await
        }
        _ => {
            eprintln!("usage:");
            eprintln!("  round-trip serve [--port 7777]");
            eprintln!("  round-trip agent [--bus ws://127.0.0.1:7777/ws] [--name <n>]");
            std::process::exit(2);
        }
    }
}
