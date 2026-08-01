# Agents ordered by last seen — Design

**Date:** 2026-08-01
**Status:** Designed, not implemented.
**Builds on:** `2026-07-27-observability-design.md`, `2026-07-31-agent-version-reporting-design.md`

## Problem

The agents list has accumulated participants that are never coming back — `relay-probe`
(a throwaway Node probe from the permission-relay experiment), `samsung-tablet`,
`wordle-helper`, `webapp`, `frigate-debug`. Ordered alphabetically, they sit interleaved
with the agents actually in use, and the list is now fifteen rows.

That directly degrades the version-reporting feature added the same week: its whole job is
spotting the agents whose version differs from the bus's, and it is harder to scan a
column when a third of the rows are for sessions that no longer exist.

## What was considered and cut

An earlier draft of this design had a dormancy filter (hide agents not seen for N days,
with an `?all=1` toggle) and a remove button. Both were cut as more than the problem needs.

Removal in particular is worth recording as rejected rather than merely skipped, because
it looks cheaper than it is:

- The web UI performs no writes by design (`DEPLOY.md`), because the bus has no
  authentication and anything the UI can do is available to anything that can reach the
  port. A remove button makes that claim false.
- Deleting the `agents` row would not remove the agent. `room_members.agent_name` and
  `cursors.agent_name` are plain `TEXT` with no foreign key, so the agent would vanish
  from the list while remaining a room member with a live delivery cursor — still counted
  in `queued_for` on every send to that room, forever. Removal is a three-table operation.

Neither cost is justified by what is actually wrong, which is ordering. Checking the
deployed bus settled it: every dormant agent is a member of exactly one dead DM room
(`dm:hub|relay-probe`, `dm:hub|samsung-tablet`, `dm:frigate-debug|hub`), each with two
members and no remaining traffic, and two of them belong to no room at all. Their durable
memberships cost nothing in practice. The problem is that they are in the way visually,
and sorting moves them out of the way.

## Design

### 1. Order by recency

`Store::agents()` changes its ordering:

```sql
SELECT name, host, cwd, session_id, online, is_human, version, last_seen
FROM agents ORDER BY last_seen DESC, name
```

The `name` tiebreaker is not cosmetic. `last_seen` is millisecond-granularity and several
agents registering within the same millisecond is routine — especially in tests, where a
bare `ORDER BY last_seen DESC` would give nondeterministic order on ties and produce
flaky assertions.

`last_seen` is already written on every registration and on every online/offline
transition. Nothing new is recorded; it has simply never been read back.

This ordering applies to every consumer of `Store::agents()` — both web tables and the
`agents` MCP tool — because they share the query. That is intended: most-recently-active
first is a better default than alphabetical everywhere, and having the tool and the page
disagree about order would be worse than either choice.

### 2. Surface it

`AgentRow` gains `last_seen: i64`, and both agent tables (`/` and `/agents`) gain a
`last seen` column rendered with the existing `fmt_time` helper — the same one already
used for message and event timestamps, which renders today's times as `HH:MM:SS.mmm`, older
ones with a date, and degrades to `t=<ms>` rather than panicking a page over one bad row.

The column is not decoration. Without it the list is in an order the reader cannot account
for: not alphabetical, with no visible reason. It also closes a gap `DEPLOY.md` already
admits to — the observability design promised a "last seen" column and it was never built.

`AgentInfo` and the `agents` MCP tool gain nothing. The tool's ordering changes with the
shared query, but its fields stay as they are; adding `last_seen` there is a separate
question nobody has asked.

## Accepted risks

- **`last_seen` reflects the last connect or disconnect, not the last message.** An agent
  holding a long idle connection sorts by when it connected. That is the honest meaning of
  the stored value and matches what the column will say; it is not a proxy for "most
  recently talkative".
- **Dormant agents remain room members with live cursors.** Unchanged by this design, and
  currently harmless — see *What was considered and cut*. If a dormant agent ever holds a
  membership in a room still in use, its `queued_for` entries will be visible again, and
  the three-table removal question returns.

## Out of scope

- Hiding, filtering, or paginating agents.
- Removing agents, by any route.
- Adding `last_seen` to the `agents` MCP tool or to `AgentInfo`.
- Any write path in the web UI.
