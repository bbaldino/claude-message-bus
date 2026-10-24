# `claude-bus spawn`: start a new project from chat

Status: v1, approved by the human via hub (2026-10-24). The dialog handling (§ Dialogs) awaits the human's choice.

## Goal

From a chat (Tars, raven, hub), the human asks for a new project. Hub runs one command on hardac, which:
- creates the project directory
- starts an interactive Claude Code session in it, in the human's usual tmux layout
- confirms the session has come online on the bus

The human can then message the new agent from the phone, or attach to its tmux session and type.

## Where it lives

In the `claude-bus` CLI, run by an agent that is already on the target host (hub, on hardac). It does **not** live in the bus server:
- The bus runs in a container on another machine and cannot reach the hosts. Agents connect out to it.
- Giving the bus the power to start processes on dev machines would turn a message relay into a way to run code remotely.

The core is a library function (`claude_bus::spawn::run`), so a later per-host spawner daemon can call the same code path.

## Usage

```
claude-bus spawn <name> [--prompt "<first task>"] [--mode auto|unattended]
                        [--root <dir>] [--bus ws://host:7777/ws]
```

- `<name>` must match `^[a-z0-9-]+$`. It becomes the directory, the tmux session and the agent name.
- `--prompt`: the session's first message. Optional.
- `--mode`: `auto` (the default) or `unattended`. Both forbid the tools that would wait on a terminal nobody is attached to (`AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`).
  - `auto` adds `--permission-mode auto`. Claude Code settles most permission prompts itself, but risky actions still pause and show as "blocked on human" in agent status.
  - `unattended` adds `--dangerously-skip-permissions`, exactly like `claude-bus launch` (hub's workers).
- `--root`: where projects are created. The default is `~/work`, and any value must be `~/work` or inside it.
- `--bus`: used to check that the name is free, to wait for registration, and for `init` where it runs. The default is `ws://127.0.0.1:7777/ws`, like the other commands.

On success, it prints the agent name, the directory, the tmux session, and `tmux attach -t <name>`, and exits 0. On any failure, it prints a one-line reason to stderr and exits non-zero.

## Steps

1. **Validate.** The name matches the pattern. `<root>/<name>` does not exist. The root resolves (symlinks followed) to `~/work` or below. No agent with that name is registered on the bus (checked with the observer `list_agents`, the same call `claude-bus status` uses). A tmux session of that name does not already exist.
2. **Create** `<root>/<name>` and run `git init` in it.
3. **Wire to the bus.** If `claude mcp get msgbus` reports a **user-scope** entry, skip this step: user scope already connects every project (hardac today). Otherwise run `claude-bus init --project --yes --bus <bus>` in the new directory.
   - `init --project` refuses when a user-scope entry exists, which is why the check comes first.
4. **Start tmux** in the human's default server (no `-L` socket), matching the convention for every project on hardac:
   - `tmux new-session -d -s <name> -n claude -c <dir> -- <claude command>`: window 0, an interactive Claude Code TUI.
   - `tmux new-window -t <name>: -c <dir>`: window 1, the default shell (fish), in the project directory.
   - The claude command is run as
     `env -u CLAUDECODE -u CLAUDE_CODE_SESSION_ID -u CLAUDE_PROJECT_DIR -u CLAUDE_CODE_ENTRYPOINT CLAUDE_BUS_NAME=<name> claude <mode args> [<prompt>]`.
   - That clears the spawning session's own Claude Code variables, so the child can't inherit hub's identity, and pins the agent name.
5. **Dialogs** (see below).
6. **Wait** up to 30s for `<name>` to show as online on the bus.
7. **Log** one JSON line per attempt to `$XDG_STATE_HOME/claude-bus/spawn.log` (default `~/.local/state/…`): time, name, directory, mode, result, and the error if any. The same line also goes to stderr.

### Failure handling

- **Before step 2:** nothing has been created.
- **Failure in steps 2–4:** spawn removes the directory it created (it holds only what spawn put there) and kills the tmux session if it started one.
- **Failure in steps 5–6** (a dialog not recognized, or registration timed out): the session is **left running**, so the human can attach and finish by hand. The error includes the last lines of the claude window.

## Dialogs

A brand-new project hits two interactive dialogs before the session reaches the bus. This was verified live on 2026-10-24:
1. **Folder trust**: "Is this a project you created or one you trust?" It appears for every new git repository, even inside a trusted folder, because trust is per project root.
2. **Development channels warning**: "I am using this for local development". It appears on every launch with `--dangerously-load-development-channels server:msgbus`.

**Recommended (pending the human's choice):** spawn answers both by keypress through tmux.
- It reads the claude window with `capture-pane`, and acts only when a dialog's identifying text matches exactly.
- It accepts trust only for the directory it created in this run.
- Unrecognized text is never guessed at: the run falls through to the 30s registration timeout with the window contents in the error.

The alternative is to stop after starting the session and have the human attach to answer the dialogs. Editing `~/.claude.json` to pre-trust the folder is ruled out: this repo never writes that file.

## Security

- **Who triggers it:** only the human's own words (directly, or through a relayer). This is hub's policy under the bus's existing `human="true"` rule; an agent's request is never enough.
- **What it can create:** only a new directory under `~/work`, with a validated name. It never overwrites an existing directory, tmux session or agent name.
- **What the new session can touch:** anything the user can. The directory restriction controls where a project is created, not what the session may do. Real confinement would need a container or a separate user, and is out of scope.
- **Permission mode:** `auto` by default (see Usage). `unattended` must be asked for explicitly.

## Not in v1

- A per-host spawner daemon for machines without a resident agent.
- Remote Control attach.
- Templates or scaffolding beyond `git init`.
- Deleting or archiving projects.

## Testing

- **Unit tests:**
  - name validation, and root confinement (including `..` and symlinks)
  - the command lines built for each mode
  - the tmux commands
  - the dialog matcher on captured pane text
  - the log line
- **Library tests through a fake `System`:** commands, tmux and the bus are injected, which covers:
  - step order
  - skipping `init` with a user-scope entry, running it without one
  - cleanup on each failure
  - leaving the session up on timeout
- **A live check on hardac:** spawn a throwaway name against a scratch bus, verify window 0 and window 1, and that the agent registers. Then delete it.
