import styles from './Empty.module.css'

/// The one empty state that earns instruction — it is the first thing a new user
/// sees, and the answer is a command.
///
/// The command is `claude-bus init`, not the `claude-bus register` the design
/// mock showed: there is no `register` subcommand, and agents are not registered
/// directly. `init` writes the MCP config into a project and the agent registers
/// itself when a Claude Code session launches there.
///
/// The address comes from `location.host` rather than `/api/meta`'s hostname,
/// because the reader is demonstrably looking at a page served from it. The
/// server's own hostname may not resolve from wherever they are.
///
/// `--project` is not decoration. `src/main.rs` leaves the scope `None` when
/// neither `--user` nor `--project` is passed, so `init` has to ask — and
/// `prompt_scope`'s blank-input branch returns `Scope::User` while the prompt
/// text marks no option as the default. Pressing Enter therefore configures
/// EVERY project on the machine, which is the opposite of what the sentence
/// above this block promises. Naming the scope makes the screen's instruction
/// and its command agree.
///
/// It does not make the command scriptable, and this comment should not be read
/// as claiming that: `init`'s apply gate still needs a terminal or `--yes`, by
/// design. `--yes` is deliberately not rendered here — a pasted one-liner should
/// not silently apply a config change the reader never saw a plan for.
///
/// All of this came from running the rendered command rather than reading it,
/// which is the check this screen exists to pass.
export function NewBus() {
  const address = location.host
  return (
    <div className={styles.newBus}>
      <p className={styles.eyebrow}>no agents</p>
      <h2 className={styles.headline}>The bus is running. Nothing has joined it.</h2>
      <p className={styles.body}>
        Run this in any project directory, then start a Claude Code session there. The agent
        registers itself on launch and appears here within a heartbeat.
      </p>
      <pre className={styles.command} data-testid="command">
        claude-bus init --project --bus ws://{address}/ws
      </pre>
      <p className={styles.status}>
        <span className={styles.dot} />
        listening on {address} · this page updates itself
      </p>
    </div>
  )
}
