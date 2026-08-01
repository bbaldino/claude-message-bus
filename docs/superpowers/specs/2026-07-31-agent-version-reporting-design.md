# Agent version reporting — Design

**Date:** 2026-07-31
**Status:** Designed, not implemented.
**Builds on:** `2026-07-25-claude-message-bus-design.md`, `2026-07-31-human-authority-design.md`,
`2026-07-31-release-tagging-standard.md`

## Problem

Claude Code spawns a stdio MCP server once at session start and never respawns it. So
upgrading `claude-bus` does not upgrade any agent already running — those sessions keep
their old binary until a human restarts them, and nothing on the bus says which ones
those are.

This is not hypothetical. Deploying the human-authority feature left seven agents
(`proxmox`, `dashboard`, `homelab-diagram`, `tars`, `caas`, `homelab-health`, `raven`)
on the pre-upgrade binary. They kept working — that is what the wire-compatibility work
bought — but they silently ignored the new origin rule, and the only way to find them was
`readlink /proc/<pid>/exe` looking for `(deleted)`. That trick works on one host, detects
only "the file was replaced since spawn", and says nothing about *which* version is
running.

Every future upgrade has the same problem.

## Why this is only now worth building

An earlier attempt at this design stalled on the version string. `Cargo.toml` had said
`0.1.0` since the first commit and the repo had no tags, so `env!("CARGO_PKG_VERSION")`
would have reported the same string for every build — including the stale ones. The
workaround under consideration was semver plus build metadata (`0.1.0+g2625483`).

Adopting the fleet release-tagging standard removed the need. release-plz now bumps
`Cargo.toml` on every release automatically, so the crate version is meaningful without
anyone remembering to bump it. Bare `CARGO_PKG_VERSION` is the right answer, and the
build-metadata workaround is unnecessary. Sequencing these two the other way round would
have baked in a workaround the project does not need.

## What this is, and is not

**Descriptive, not a control.** Nothing branches on the reported version. The bus does not
refuse an old agent, negotiate capabilities, or gate features on it. Wire compatibility is
already handled by every new protocol field being `#[serde(default)]`; this design only
makes the resulting version spread *visible*.

The value is answering one operational question — "which sessions still need restarting?"
— without walking a process table on one specific host.

## Design

### 1. The wire

`ToBus::Register` gains:

```rust
    /// The agent binary's crate version. `None` means a binary that predates this
    /// field — which is exactly the signal worth surfacing, so absence is preserved
    /// rather than defaulted to a string.
    #[serde(default)]
    version: Option<String>,
```

The agent sends `env!("CARGO_PKG_VERSION")`. That value is already computed in
`src/agent/handler.rs` for the MCP `server_info.version` reported to Claude Code at
`initialize`; this is a second consumer of the same constant, not a new source of truth.

`Option<String>` rather than `String` because absence is meaningful and must stay
distinguishable from a binary that reported an empty string. An agent that predates this
change sends no field, deserialises to `None`, and displays as `unknown` — which is the
"restart this one" signal. That makes the feature useful from its first deploy rather than
only for upgrades after it.

`Observe` gains nothing. Observers (`tail`) create no `agents` row, so there is nowhere to
record a version and nothing that would read it.

### 2. Storage

`agents` gains a nullable `version TEXT` column, added by a third
`add_column_if_missing("agents", "version", "TEXT")` call in `Store::migrate` — the same
idempotent `PRAGMA table_info` pattern already used for `agents.is_human` and
`messages.human`. No default value: `NULL` is the honest representation of "did not say".

`AgentRow` gains `version: Option<String>`, and `upsert_agent` takes it. Because
`upsert_agent` writes on every registration, a session restarted onto a new binary updates
its row immediately.

### 3. Display

Both agent tables (`/` and `/agents`) gain a `version` column showing the reported string,
or `unknown` where `NULL`.

The bus renders its own `env!("CARGO_PKG_VERSION")` on both of those pages, adjacent to
the table, so a reader can compare without leaving the page they are on.

An agent whose version differs from the bus's own — including `unknown` — is marked with a
badge. This is a new CSS class styled consistently with the existing `.off` and `.human`
badges rather than a reuse of either, since it means neither "offline" nor "human"; the
point is that the page keeps one visual vocabulary instead of gaining a third. Fifteen
agents is already more than is comfortable to diff by eye, and spotting the odd ones out
is the entire job this feature exists to do.

The marker asserts "differs from the bus", not "is broken". An agent built from a branch
would be flagged and would not be stale; that is an acceptable false positive for a
homelab tool, and the version string next to the badge tells the reader which case they
are looking at.

### 4. The `agents` tool

`AgentInfo` gains `version: Option<String>`, so the existing `agents` MCP tool returns it
alongside `name`, `host`, and `online`.

This lets a switchboard agent answer "which sessions need restarting?" over the bus rather
than a human reading the web page — which is the job that hub already exists to do. It is
one field on a response that is already being built.

## Rejected alternatives

**Semver plus build metadata** (`0.1.0+g2625483`, set from `option_env!("GIT_SHA")` at
build time). Rejected as unnecessary: it existed only to distinguish two builds reporting
the same static version, and release-plz now bumps the version on every release. It also
required threading a build-time environment variable through both the `Makefile` and the
`Dockerfile`, which is real complexity for a problem that no longer exists.

**Reporting a git SHA instead of a version.** Rejected: precise but unreadable, and it
cannot be compared against the bus's own version at a glance, which is the whole point of
putting both on one page.

**Version gating** — refusing or warning on agents below some minimum. Rejected as
premature. Nothing today needs it, wire compatibility already keeps old agents working,
and a gate would turn a visibility feature into a control with its own failure modes.

## Accepted risks

- **The reported version is self-asserted**, like everything else on this bus. An agent
  could claim any string. Consistent with the existing no-authentication posture, and
  irrelevant to the failure this addresses: a stale binary reports its real version because
  it has no reason not to.
- **A version differing from the bus is not proof of staleness.** See §3. The badge is a
  prompt to look, not a verdict.

## Out of scope

- Authentication, per the existing posture.
- Any behavior that branches on version.
- Reporting versions for observers (`tail`) or for the `chat` client.
- Prompting or automating agent restarts. The bus reports; restarting a session is the
  human's action, and nothing here changes that.
