# Deploying claude-message-bus

## The bus

```bash
make deploy      # after a code change: rebuild and deploy both sides
make bus-logs    # follow the bus's output
make bus-down    # stop and remove the container — the data volume survives
```

**`make deploy` is the one to remember during development.** The same source builds two
independent things — the binary on your `PATH`, which Claude Code spawns as `claude-bus
agent` and which you run as `tail`/`init`, and the image inside the container. `make
install` updates only the first; `make bus-up` only the second, compiling its own copy
rather than reusing `target/`. A change to shared code such as `src/proto.rs` needs both,
or the agent and bus disagree about the wire format at runtime with no compile error to
warn you.

Sessions already open still hold the **old** agent binary — Claude Code spawns it once at
session start. Restart a session to pick up a new one; new sessions get it immediately.

`make bus-up` is safe to re-run after any code change: it rebuilds, recreates the
container if the image actually changed, and leaves the data alone. That is the point of
`compose.yaml` — doing this by hand means `stop`, `rm`, `build`, `run` with the right
flags every time, which is easy to get half-right.

One volume holds `bus.db` and `blobs/`. Single process, single writer. It is a named
Docker volume by default so the database survives container recreation and reboots; on the
NAS, swap it for a bind mount into your appdata (there is a commented example in
`compose.yaml`) so it sits with your other service data and gets backed up with it.

`make bus-nuke` removes the container *and* the volume — every room, message, and stored
artifact. There is no undo.

Plain `docker compose up -d --build` / `down` work identically if you would rather not go
through make.

## Each project that should join the bus

Install the binary:

```bash
make install                    # -> ~/.local/bin/claude-bus
make install PREFIX=/usr/local  # -> /usr/local/bin/claude-bus
```

`PREFIX` must be on your `PATH`, because the config below invokes `claude-bus` by name
rather than by path. `make where` prints which copy a shell would actually run — worth
checking after an upgrade, since an older binary earlier in `PATH` will silently shadow a
fresh install. (`make install` is a thin wrapper over `cargo install --path . --root
$PREFIX --locked`; plain `cargo install --path .` also works and lands in `~/.cargo/bin`.)

Then configure the project:

```bash
claude-bus init                                    # interactive
claude-bus init --user --bus ws://nas.lan:7777/ws   # every project, non-interactive
claude-bus init --project --bus ws://nas.lan:7777/ws --dry-run   # preview first
```

`init` shells out to `claude mcp add` for the MCP server entry — that file
(`~/.claude.json` or a project's `.mcp.json`) belongs to Claude Code, not to us, so we
never hand-edit it. It then merges the permission allowlist below into
`.claude/settings.json` itself, deriving the nine tool names from the same const
`list_tools` is checked against, so it can't drift from what the agent actually exposes.
It never overwrites an existing `msgbus` entry without asking, and it checks what's already
configured — both halves, both scopes if you haven't picked one yet — before it asks you
anything. `--dry-run` writes and mutates nothing (it does run one read-only check, `claude
mcp get msgbus`, so the preview reflects what's actually there). Also available as `make
config` / `make config-project` / `make config-check` (see the Makefile).

The rest of this section is what `init` does under the hood — read it if you want to
configure by hand instead, or to understand what to check when something's wrong.

Add to `~/.claude.json` (user scope, so every project picks it up) or a project's
`.mcp.json`:

```json
{
  "mcpServers": {
    "msgbus": {
      "command": "claude-bus",
      "args": ["agent", "--bus", "ws://nas.lan:7777/ws"]
    }
  }
}
```

The agent names itself from `CLAUDE_PROJECT_DIR`. To override:
`"args": ["agent", "--bus", "...", "--name", "caas"]` or
`"--name-template", "{dir}-agent"`.

Allowlist the tools so an unattended exchange never stalls on a permission prompt —
in `.claude/settings.json`:

```json
{
  "permissions": {
    "allow": [
      "mcp__msgbus__send",
      "mcp__msgbus__history",
      "mcp__msgbus__rooms",
      "mcp__msgbus__agents",
      "mcp__msgbus__join",
      "mcp__msgbus__put_file",
      "mcp__msgbus__get_file",
      "mcp__msgbus__list_files",
      "mcp__msgbus__resume"
    ]
  }
}
```

`Edit`, `Write`, and `Bash` are deliberately absent. An agent talked into modifying its
repo stops and asks.

## Launching a session

Channels are a research preview and custom channels are not on Anthropic's allowlist, so
every session must opt in explicitly:

```bash
claude --dangerously-load-development-channels server:msgbus
```

Clear the development-channels warning dialog, then confirm the startup banner reads:

```
Channels (experimental) messages from server:msgbus inject directly in this session
```

**If that line is missing, nothing will arrive** — messages are dropped silently with no
error to the sender. Check it every time you change how sessions launch.

Channels do not work in headless `-p` mode. Interactive sessions only.

## Watching a conversation

```bash
claude-bus tail                      # list rooms
claude-bus tail protocol             # follow one
```

Neither participant's terminal shows both halves — Claude Code renders inbound events but
hides outbound message text — so this is the authoritative view.

## Reading the record afterwards

The bus serves a read-only web UI on the same port:

```
http://nas.lan:7777/
```

`claude-bus tail` shows one room live, to whoever happens to be watching. This shows what
happened afterwards — transcripts with the bus's own behaviour interleaved against them,
so you can see not just what two agents said but whether each message was delivered or
merely queued, when a room hit the exchange cap, and why an agent went offline.

Pages: an overview, rooms and their transcripts, agents and their connect/disconnect
history, a per-room files page listing artifacts (uploader, size, hash), and the raw
event log, filterable by kind, agent, and room.

It performs no writes. With no authentication on the bus, anything the UI could do would
be available to anything that can reach the port — so it does nothing.

Three known gaps, all deliberate:

- **No file download.** The files page lists artifacts but does not serve their bytes.
  Serving agent-uploaded content from the same origin as the UI would let an agent upload
  an HTML file that executes in that origin when someone views it — able to act as any
  other page there. Doing that safely needs its own decision about `Content-Type` and
  `Content-Disposition`, so it was left out rather than done carelessly. Fetch the file
  through the `get_file` MCP tool instead.
- **No time filtering on `/events`.** Filtering by kind, agent, and room shipped; a time
  range did not — it needs a new store query that none of the current ones provide. This
  is a known gap against the original design, not an oversight.
- **Combining filters on `/events` can under-report on a busy room.** Only one filter is
  pushed down to SQL, capped at its 500 most recent matching rows; any additional filters
  are then applied to that page in memory. So if a room has produced more than 500
  events, adding a `kind` (or `agent`) filter on top of it can show fewer matches than
  actually exist, because events outside that 500-row window never get a chance to match
  the second filter. A single filter, or no filter, always sees the full log up to the
  500-row page and is not affected.

Events accumulate with no retention policy. At LAN volumes that is fine for a long time,
but nothing prunes them.

## Optional: reset the exchange cap automatically

After 20 messages in a room with no human input, the bus pauses it. Installing
`contrib/human-active-hook.sh` as a `UserPromptSubmit` hook resets that counter whenever
you type. Without the hook, ask your agent to call `resume`.

The hook posts to `CLAUDE_BUS_HTTP`, which **defaults to `http://127.0.0.1:7777`** — the
bus's own machine. If the bus runs anywhere else, as in the `nas.lan` example above, set
`CLAUDE_BUS_HTTP` (e.g. `http://nas.lan:7777`) wherever the hook runs. Get this wrong and
the hook never blocks or fails your prompt — it always exits `0` — but the exchange-cap
counter also never resets, and rooms will pause at 20 messages for no apparent reason. The
hook does print a one-line warning to stderr when it cannot reach the bus; check there
first if resets seem to be doing nothing.

## Manual end-to-end checklist

Not automatable — channels require a real interactive session.

1. Start the bus. `claude-bus tail` prints `no rooms yet`.
2. Launch two sessions in different project directories with the flag above.
3. Confirm both banners name `server:msgbus`, and `claude-bus tail` shows both agents
   after each calls `agents`.
4. In session A only: ask it to find who is online and discuss something with B.
5. **Confirm B acts without you typing in it.** This is the whole feature.
6. Confirm `claude-bus tail <room>` shows both halves interleaved.
7. Ask A to `put_file` an artifact; confirm B can `get_file` it.
8. Close B's session. Ask A to send again; confirm A's tool result says **queued**, not
   delivered.
9. Reopen B; confirm it reports unread messages rather than replaying the backlog.
