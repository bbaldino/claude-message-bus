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

## HTTP participants

A participant that can only make outbound HTTP requests — a WASM/`wasi:http`
plugin, say — can join without a WebSocket, over `/api/participants` on the bus's
own port (default `:7777`):

- `POST /api/participants` `{name, mode?}` → `{name, token, human, relayer,
  leaseTtlMs}`. The token goes in the `X-Participant-Token` header on every later
  call.
- `GET /api/participants/receive?after=<id>&limit=<n>&timeout=<sec>` — long-poll;
  `after` is a single global cursor that also acks. Echo the returned `cursor`
  back as the next `after`.
- `POST /api/participants/send` `{target, text, done?}` → an `outcome` of `sent`
  (with `deliveredTo`/`queuedFor`), `rate_limited`, or `paused`.
- `POST /api/participants/resume` `{room}` — relayer leases only.

A plain lease is an ordinary bot, subject to the exchange guard. Authority is
session-level, not per message, and set at registration with the correct
`X-Relayer-Secret` (bus flag `--relayer-secret`, name pinned with
`--reserve-name`). `mode:"relayer"` (the default) stamps every message with its
human's authority while still counting against the exchange cap — for an agent
that speaks for a human but composes its own words. `mode:"human"` grants full
human semantics (cap-exempt, clears pauses, like a person at the console) — for a
pure pipe forwarding a person's own typed words. Every endpoint carries the same
same-origin guard as the rest of the bus. See
`docs/superpowers/specs/2026-09-22-http-participant-design.md`.

## Working on the frontend

The bus serves two UIs on its port during the transition between them: `/` is the
server-rendered HTML, and `/app` is the React/TypeScript single-page app that will
replace it. The SPA lives in `ui/` and is built by Vite into `ui/dist`.

`ui/dist` is compiled into the Rust binary by `rust-embed` **at Rust compile time**, so
any `cargo build` / `cargo install` embeds whatever that directory holds right then — on
a fresh clone that is nothing, and `/app` will have nothing to serve. Build the frontend
first:

```
make ui       # cd ui && npm ci && npm run build
make install  # depends on `ui`, so this does it for you
```

The Docker image builds the frontend in its own stage, so `make bus-up` needs no extra
step.

For the frontend dev loop, run Vite instead of rebuilding the binary each time:

```
claude-bus serve      # in one terminal, on :7777
cd ui && npm run dev  # in another
```

Vite serves at <http://localhost:5173/app/> — note the `/app/` prefix, which matches
where the bundle is mounted in production — and proxies `/api` and `/ws` through to the
bus on `:7777`, so hot reload works against real data.

The API's TypeScript types in `ui/src/types/` are generated from the Rust structs by
`ts-rs` during `cargo test` and are committed; CI fails if they are out of date, so run
`cargo test` after changing an `/api` response type.

See `docs/DEPLOY.md` to run it, and `docs/superpowers/specs/` for the design and the
reasoning behind it. `docs/poc-transcript.md` is a real unattended negotiation between two
agents, from the prototype that proved the idea.

**Status:** built on a research-preview Claude Code feature. Sessions must launch with
`--dangerously-load-development-channels server:msgbus`, and the contract may change.
