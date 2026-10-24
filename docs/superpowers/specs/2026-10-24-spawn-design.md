# `claude-bus spawn`: start a new project from chat

Status: v1, approved by the human via hub (2026-10-24). Dialog handling decided: an official mechanism where one exists, keypresses otherwise (§ Dialogs). Verified live on hardac.

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
claude-bus spawn <name> [--prompt "<first task>"] [--mode unattended|auto]
                        [--root <dir>] [--bus ws://host:7777/ws]
```

- `<name>` must match `^[a-z0-9-]+$`. It becomes the directory, the tmux session and the agent name.
- `--prompt`: the session's first message. Optional.
- `--mode`: `unattended` (the default) or `auto`. Both forbid the tools that would wait on a terminal nobody is attached to (`AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`).
  - `unattended` adds `--dangerously-skip-permissions`, so plain `spawn` starts the session exactly as `claude-bus launch` does (hub's workers). The human chose this default on 2026-10-24: a spawned session should be no different from a manually launched one.
  - `auto` (opt-in) adds `--permission-mode auto`. Claude Code settles most permission prompts itself, but risky actions still pause and show as "blocked on human" in agent status.
- `--root`: where projects are created. The default is `~/work`, and any value must be `~/work` or inside it.
- `--bus`: used to check that the name is free, to wait for registration, and for `init` where it runs. The default is `ws://127.0.0.1:7777/ws`, like the other commands.

On success, it prints the agent name, the directory, the tmux session, and `tmux attach -t <name>`, and exits 0. On any failure, it prints a one-line reason to stderr and exits non-zero.

## Steps

1. **Validate.** The name matches the pattern. `<root>/<name>` does not exist. The root resolves (symlinks followed) to `~/work` or below. No agent with that name is registered on the bus (checked with the observer `list_agents`, the same call `claude-bus status` uses). A tmux session of that name does not already exist.
2. **Create** `<root>/<name>`. `git init` comes later (step 7) — see Dialogs.
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
7. **`git init`** in the project, now that the session is running.
8. **Log** one JSON line per attempt to `$XDG_STATE_HOME/claude-bus/spawn.log` (default `~/.local/state/…`): time, name, directory, mode, result, and the error if any. The same line also goes to stderr.

### Failure handling

- **Before step 2:** nothing has been created.
- **Failure in steps 2–4:** spawn removes the directory it created (it holds only what spawn put there) and kills the tmux session if it started one.
- **Failure in steps 5–6** (a dialog not recognized, or registration timed out): the session is **left running**, so the human can attach and finish by hand. The error includes the last lines of the claude window.
- **Failure in step 7:** the agent is already online. The error says so and leaves everything in place.

## Dialogs

A brand-new project can hit two interactive dialogs before the session reaches the bus, and nobody is attached to answer them (verified live on 2026-10-24). The human chose: use a supported flag or setting where one exists, and otherwise answer by keypress, matched on exact text and failing safe. Claude Code's docs were checked for each.

| Dialog | Supported way to avoid it? | What spawn does |
|---|---|---|
| **Folder trust**: "Is this a project you created or one you trust?" | No flag, env var or settings key. The only documented pre-trust is editing `~/.claude.json`, which this repo never does. **But trust is inherited:** trusting a folder "covers any subdirectory of that directory apart from a git repository nested inside it". | **Official route.** `~/work` is already trusted on hardac, so a new folder inside it is covered, *as long as it isn't yet a git repository*. So spawn starts the session first and runs `git init` only after the agent is online. Verified live: no trust dialog. |
| | | **Fallback:** if a trust prompt does appear (an untrusted root, another host), spawn answers it by keypress only when it names the exact directory this run created, either as written or in its `~/…` form. |
| **Development channels**: "WARNING: Loading development channels … I am using this for local development" | Only for a channel plugin on Anthropic's allowlist, or on an organization's `allowedChannelPlugins` (managed settings, Team/Enterprise). A local `server:msgbus` channel has no supported non-interactive acceptance. | **Keypress fallback:** Enter, which confirms the default "I am using this for local development". This is the same confirmation the human gives by hand when launching any bus worker. |

Rules for the keypress fallback:
- The claude window is read with `tmux capture-pane -p -J`. The `-J` flag joins wrapped lines, so a long path matches whole.
- A dialog is answered only when its identifying text matches exactly.
- Each dialog is answered at most once. If it is still on screen at the next poll, the TUI hasn't redrawn yet.
- Text that isn't recognized, such as a reworded dialog after a Claude Code update, is never guessed at. The run falls through to the 30s timeout, with the window contents in the error and the session left up.

**Known limit:** on a host *without* a user-scope `msgbus` entry, spawn runs `init --project`. That writes a project `.mcp.json`, which raises a third dialog ("New MCP server found in this project"). Spawn doesn't answer it, so the run times out with the attach hint. hardac uses user scope, so v1 isn't affected.

## Security

- **Who triggers it:** only the human's own words (directly, or through a relayer). This is hub's policy under the bus's existing `human="true"` rule; an agent's request is never enough.
- **What it can create:** only a new directory under `~/work`, with a validated name. It never overwrites an existing directory, tmux session or agent name.
- **What the new session can touch:** anything the user can. The directory restriction controls where a project is created, not what the session may do. Real confinement would need a container or a separate user, and is out of scope.
- **Permission mode:** the same as `claude-bus launch` by default (prompts skipped). `auto` must be asked for explicitly.

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
