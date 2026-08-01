# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1](https://github.com/bbaldino/claude-message-bus/compare/v0.3.0...v0.3.1) - 2026-08-01

### Added

- show which agents hold the relayer grant

### Other

- implementation plan for surfacing relayers in the web UI
- design for surfacing relayers in the web UI

## [0.3.0](https://github.com/bbaldino/claude-message-bus/compare/v0.2.0...v0.3.0) - 2026-08-01

### Added

- order agents by last seen and show the timestamp

### Fixed

- address code-review findings on agents-by-last-seen

### Other

- implementation plan for ordering agents by last seen
- design for ordering agents by last seen

## [0.2.0](https://github.com/bbaldino/claude-message-bus/compare/v0.1.2...v0.2.0) - 2026-08-01

### Added

- the agents tool reports each agent's version
- show agent versions and flag the ones that differ from the bus
- agents report their version when they register

### Fixed

- address code-review findings on agent version reporting

### Other

- implementation plan for agent version reporting
- design for agent version reporting
- only release on feat and fix commits

## [0.1.2](https://github.com/bbaldino/claude-message-bus/compare/v0.1.1...v0.1.2) - 2026-08-01

### Added

- mark human senders in tail output

## [0.1.1](https://github.com/bbaldino/claude-message-bus/compare/v0.1.0...v0.1.1) - 2026-07-31

### Other

- release v0.1.0 ([#1](https://github.com/bbaldino/claude-message-bus/pull/1))

## [0.1.0](https://github.com/bbaldino/claude-message-bus/releases/tag/v0.1.0) - 2026-07-31

### Added

- claude-bus chat --to addresses a single agent
- act on a human's request, deliberate on an agent's
- tell the model whether a message came from a human
- configured relayers speak with the human's authority
- carry a message's origin to the receiving agent
- record whether a human sent a message
- mark humans in the agent list and document claude-bus chat
- claude-bus chat, an interactive room client for a human
- a human's send resets the cap, un-pauses the room, and skips the rate limit
- a human's room membership lasts only as long as their connection
- record a human registration in the store and event log
- let a registration declare itself human
- record whether an agent is a human
- show recent message text on the overview
- label table columns, add timestamps and event detail, lighten the UI
- agent, files, and event log pages
- room transcript interleaving messages and bus events
- web scaffolding, HTML escaping, and the overview page
- record what the bus does to the event log
- event log storage
- add `claude-bus init` to configure the MCP entry and permission allowlist
- docker deployment, client config, and human-active hook
- tail viewer for watching both halves of a conversation
- agent tools with delivery-confirming send
- agent bridge injecting bus messages as channel events
- agent MCP handler declaring the channel capability
- bus server with delivery acks, rooms, files, and guards
- exchange cap and per-agent rate limiting
- connection registry with presence and name collision rules
- room and DM name resolution
- agent/bus wire protocol with request correlation
- room-scoped file store with content-addressed blobs
- message log with per-agent delivery cursors
- sqlite store for agents, rooms, and membership
- crate skeleton with agent name resolution

### Fixed

- address code-review findings on human authority
- read a room's history by whether it exists, not by who is in it
- stop reporting agents from a dead bus as still online
- show milliseconds and label each table's sort direction
- advance the delivery cursor when history hands over messages
- fall back to /proc/sys/kernel/hostname before /etc/hostname
- address final review findings
- relay probe must actually be two-way
- reconcile init's status summary with its plan, and stop claiming Done. on a partial outcome
- init requires --force to overwrite an ambiguous MCP entry; project scope reads .mcp.json directly
- claude-bus init checks before it prompts
- claude-bus init fails closed when scope is unspecified and non-interactive
- tail watches rooms as an observer instead of joining them
- ping ticker can't be starved by biased select; coalesce reconnect Unread summary into one event
- split control and routing channels so inbound fan-out can't drop a connection's own replies
- keepalive ping/pong and bounded delivery channel so a vanished peer stops reading as delivered
- point .mcp.json at claude-bus on PATH, as DEPLOY.md specifies
- dogfood the bus's own MCP config; clippy warning; dupe gitignore entry
- resolve paused sends with a truthful error, not a false 10s timeout
- agent bridge acks delivered messages; reset backoff after stable uptime
- document CLAUDE_BUS_HTTP and surface human-active hook failures
- pin injection test's meta values, dedupe tokio-tungstenite dep
- restore uppercase discuss-only emphasis; loosen test to case-insensitive
- reject duplicate Register on a connection; test GetFile not-found
- document check() mutation contract, test reset does not bypass rate limit
- registry attach must not hand a freed bare name to a host that
- derive oversize-blob message from MAX_BLOB_BYTES, tighten test
- *(plan)* remove dead protocol variant, test backdoor, and lint conflict

### Other

- add release-plz for automated version bumps and tagging
- publish container image to GHCR on push and release tags
- update release-tagging plan to the revised Rust test gate
- adopt the fleet release-tagging standard and its plan for this repo
- name both routes to forging, not just the raw socket
- configure the hub as a relayer and document message origin
- reword test comment away from security-control framing
- implementation plan for human authority
- design for human authority on the bus
- implementation plan for the human participant
- design for a human participant on the bus
- run the agent MCP contract tests in-process instead of subprocess
- poll for real conditions instead of fixed sleeps in the integration suite
- describe the observability UI
- move command handling into src/bus/commands.rs
- Add observability implementation plan
- Add observability design spec
- Record milestone 0: permission relay does not fire for development channels
- Add permission-relay probe (milestone 0 for the hub design)
- Add hub and permission relay design spec
- add make deploy for the dev loop
- compose file and make targets for running the bus
- note that make install is a release build, and why it is not stripped
- make install, defaulting to ~/.local/bin
- Merge claude-message-bus implementation
- describe the read-path surface accurately in the design spec
- retire POC crates, keep the transcript as evidence
- cover partial-delivery room sends and both-supplied rejections
- cover self-exclusion in unread_count and undelivered
- base worktrees on local HEAD (repo has no remote)
- Add implementation plan and fix two spec inconsistencies
- Record POC 3 live run: two agents converged unattended
- Add POC 3: two-session round trip walking skeleton
- Record POC 2 results: rmcp handles the channel contract natively
- Add POC 2: Rust channel probe on rmcp
- Record POC 1 results: channels verified working
- Add single-terminal driver for POC 1 interactive test
- Add POC 1: channel connectivity probe
- Add claude-message-bus design spec
