# Relayers in the web UI — Design

**Date:** 2026-08-01
**Status:** Designed, not implemented.
**Builds on:** `2026-07-31-human-authority-design.md`, `2026-07-31-agent-version-reporting-design.md`

## Problem

`claude-bus serve --relayer <name>` grants a named agent the human's authority: its sends
are stamped `human: true`, and workers act on them as they would their own human's words.
It is the most consequential piece of configuration the bus has.

It is also invisible. Nothing in the web UI says which agents hold it, or whether any do.

The failure that motivates this is not a reader's curiosity but a silent misconfiguration.
A mistyped flag — `--relayer hubb`, or the equals form `--relayer=hub`, which this binary
deliberately does not parse — yields an empty or wrong set. Nothing errors. The only
symptom is that workers go back to deferring to their own humans, which is exactly what a
regression in the origin-aware instructions would look like. The two are indistinguishable
without reading the bus's startup log.

`serve` already logs the resolved set at startup for this reason. That helps only someone
who thinks to run `make bus-logs`. The dashboard is the surface people actually look at.

## Design

### 1. A badge on the agent row

`agent_row` in `src/web/mod.rs` — the single renderer both agent tables share — gains a
parameter for whether the agent is a configured relayer, and renders a badge beside the
existing `human` one when it is. A new CSS class, styled consistently with the existing
`.off`, `.human`, and `.stale` badges rather than reusing any of them, since it means none
of those things.

The value comes from `App.relayers` at render time, not from the agent's row. This
preserves the decision made when relayers were introduced: a relayer is *not* recorded as
`is_human` in the `agents` table, because relaying is a property of a send, not of an
identity. Nothing about that changes here.

### 2. State the configured set

Both agent pages state the resolved set beside the existing "this bus is running `<version>`"
note — `relayers: hub`, or `relayers: (none)` when empty.

**This is the load-bearing half, not the badge.** With badges alone, a typo'd relayer name
badges nothing, and the page is byte-identical to a correctly-configured bus that happens to
have no relayer connected. Printing the set makes the mistake visible: the wrong name appears
in the line, and no row carries the badge. That pairing is the diagnosis.

`Relayers::names()` already exists and returns the set sorted, added so `serve` could log it
without exposing the inner field. This is a second consumer of it.

## Rejected

**A `version`-style column.** Relayer status is one bit and applies to very few rows; a
whole column would be mostly empty. The badge idiom already exists for exactly this.

**Recording relayer status on the `agents` row.** It is configuration, not agent state.
Writing it would make the table disagree with the running config the moment the flag changed,
and would reintroduce the identity-versus-send confusion the original design rejected.

**A field on `AgentInfo` / the `agents` MCP tool.** A relayer can read its own configuration;
nothing on the bus needs to ask. The tool's shape stays put.

**A distinct "configured but never connected" state.** A configured name with no matching row
is already visible — it appears in the set line while nothing is badged. Inventing a third
visual state to say the same thing costs more than it explains.

## Accepted risks

- **The badge reflects configuration, not what was stamped historically.** Removing a name
  from `--relayer` and restarting drops the badge immediately, while messages that agent sent
  under the old grant remain `human: true` in the transcript, correctly. The badge answers
  "who holds this now", not "who held it when this was sent".
- **The set line reflects the running process.** A flag changed in `compose.yaml` but not yet
  applied shows the old value until the bus restarts. That is the honest reading — the running
  grant is what matters — but it means the page and the compose file can disagree.

## Out of scope

- Any write path. The web UI stays read-only; relayers are changed by restarting the bus with
  different flags.
- Recording relayer status in the store, in `AgentInfo`, or in the event log.
- Changing how the grant itself works.
