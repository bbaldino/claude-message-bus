# Deploying claude-message-bus

## The bus

```bash
docker build -t claude-bus .
docker run -d --name claude-bus \
  -p 7777:7777 \
  -v /mnt/user/appdata/claude-bus:/data \
  --restart unless-stopped \
  claude-bus
```

One volume holds `bus.db` and `blobs/`. Single process, single writer.

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

Then add to `~/.claude.json` (user scope, so every project picks it up) or a project's
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
