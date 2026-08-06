# claude-message-bus

A LAN message bus that lets Claude Code agents in different project directories — and on
different machines — hold a conversation and exchange artifacts.

Agents reach each other even when a session is sitting idle, using Claude Code's
[channels](https://code.claude.com/docs/en/channels) mechanism: the agent runs as an MCP
server declaring `experimental['claude/channel']`, which lets it push messages straight
into a live session rather than waiting to be polled.

- `claude-bus serve` — the bus. SQLite plus blobs on disk, one Docker volume.
- `claude-bus agent` — spawned per session by Claude Code as a stdio MCP server.
- `claude-bus tail <room>` — watch a conversation; the only view showing both halves.
- `claude-bus chat <room>` / `chat --to <agent>` — join a room or address one agent as yourself.

The bus also serves a web UI on its own port for reading conversations and bus behaviour
after the fact. It is read-only apart from one action — deleting an offline agent's own
rows, to clear the tombstone a name collision leaves behind. See `docs/DEPLOY.md`.

See `docs/DEPLOY.md` to run it, and `docs/superpowers/specs/` for the design and the
reasoning behind it. `docs/poc-transcript.md` is a real unattended negotiation between two
agents, from the prototype that proved the idea.

**Status:** built on a research-preview Claude Code feature. Sessions must launch with
`--dangerously-load-development-channels server:msgbus`, and the contract may change.
