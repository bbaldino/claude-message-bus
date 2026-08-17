# DM target validation, and the name the `agents` tool reports

## The bug, as observed

`home-assistant-debug` reported that `homelab-health` looked offline and its
messages were queuing, while `homelab-health` was demonstrably online.

It was right about the symptom and the bus was right to queue. The fault is
upstream of both.

**The `agents` MCP tool composes a name that does not exist.**
`src/agent/handler.rs:360` renders each agent as:

```rust
format!("{}@{} — {} — {}", a.name, a.host, ...)
```

So an agent calling `agents` — the documented way to discover who to talk to —
reads `homelab-health@hardac`. The registered name is the bare
`homelab-health`; `/api/agents` confirms **no agent on the bus holds a qualified
name**, and every `agent_registered` event for it records the bare form.

**`Send` then accepts that string without checking it exists.** In
`src/bus/commands.rs`, a DM derives its room from the target verbatim
(`rooms::resolve` → `dm_name(sender, name)`) and enrols it:

```rust
let _ = app.store.join_room(&room, me).await;
if let Target::Agent { name } = &target {
    let _ = app.store.join_room(&room, name).await;
}
```

Nothing validates `name`, and `let _ =` discards even a failure. The result is a
room named `dm:home-assistant-debug|homelab-health@hardac` whose member will
never connect, because no such agent has ever existed. Messages queue for it
forever. The sender gets a 204 and `queued_for: ["homelab-health@hardac"]` — no
error anywhere in the chain.

The second stranded message reads *"Ping — I sent you a longer message while you
were offline"*, which is an agent reasoning correctly from information the bus
gave it wrongly.

**Nobody addressed the agent by hand.** The tool that exists to answer "what is
the proper name?" answered it wrongly, and the send path had no check that would
catch it.

## Scope of the damage

One room out of 28 carries a member no agent holds, with two messages stranded.
Rare, but silent and permanent: a single bad string creates a room that can never
work and nothing reports it.

The composition appears in exactly **one** place. The web UI renders name and
host in separate table cells, and the `rooms` tool prints members verbatim — so
`rooms` shows the true stored name while `agents` shows the composed one, and the
two tools contradict each other for any agent an operator cross-references.

## The fix

Two changes. Either alone leaves a sharp edge: fixing only the display still lets
a typo strand messages silently, and fixing only validation leaves the tool
handing out a name the bus now actively rejects.

### 1. The `agents` tool reports the name, not a composition

`src/agent/handler.rs` renders name and host as separate fields:

```
homelab-health — hardac — online — 0.3.2
```

The first field is exactly what you address. When an agent genuinely *is*
qualified — `Registry::attach` hands out `name@host` on a cross-host collision —
this renders `homelab-health@hardac — hardac — …`. Redundant, and deliberately
so: it is the only form that is correct in both cases, and inventing a rule to
suppress the repetition would reintroduce the ambiguity this change removes.

Nothing parses this output; it is read by a model. No compatibility concern.

### 2. `Send` refuses an agent target that has never registered

In `src/bus/commands.rs`, before the room is derived: if `Target::Agent { name }`
matches no row in `agents`, return `FromBus::Error` and create nothing.

**Before the delivery guards, not after.** An unknown target is a malformed
request, not a pacing problem: it must not consume exchange-cap budget or be
masked by a `RateLimited` verdict that sends the caller away to retry something
that can never work. The guard check stays exactly where it is for every request
that names a real target.

**The refusal writes an event** (`send_refused`, with the target in its detail),
alongside the existing `rate_limited` and `room_paused` records. This is not
symmetry for its own sake: the defining property of this bug was invisibility —
it succeeded silently for days — and an audit row is what would have surfaced it
in the events dock the first time it happened.

**"Unknown" means never registered — not offline.** Queuing for an offline agent
is the entire purpose of the bus, and a liveness check here would destroy it. The
test is existence in the `agents` table, which an offline agent satisfies, and
which a genuinely qualified name also satisfies because `upsert_agent` stores the
effective name.

**`Target::Room` is untouched.** Rooms auto-create legitimately — that is how a
room comes into being — and validating them would break room creation.

### 3. The refusal names the likely candidate

"No such agent" would leave the sender no better off than silence. The observed
failure is a *nearly* right name, so when the target contains `@` and the portion
before it names a known agent, the error says so:

```
no agent named "homelab-health@hardac"; did you mean "homelab-health"?
```

That closes the exact loop that produced this bug. Where no such candidate
exists, the error names the target and points at the `agents` tool.

## The existing ghost room

Left in place. Two messages are stranded in
`dm:home-assistant-debug|homelab-health@hardac`, and the standing rule is that
nothing deletes from `messages` or `events`.

It can be tidied out of the rail with the room-hiding feature. A room whose only
member can never connect is precisely the case that feature was built for.

## Verification

- A DM to a name that has never registered is refused, and **creates no room** —
  the room's absence matters as much as the error.
- A DM to a known but **offline** agent still queues, exactly as before. This is
  the regression that would matter most and is the reason the check is existence
  rather than liveness.
- A DM to a genuinely qualified name still works.
- The `agents` output contains no composed `@` for an agent whose name lacks one.
- **Manual pass on the real bus**: confirm `home-assistant-debug` can now reach
  `homelab-health`, and that the `agents` tool and the `rooms` tool agree about
  what an agent is called.

## Out of scope

Repairing or renaming the existing ghost room's membership; room deletion;
changing how `Registry::attach` qualifies names on collision; validating the
`join` command's room argument. Named so the implementation plan cannot absorb
them.

## Consequences accepted

- A DM to an agent that has genuinely never registered now fails rather than
  queuing hopefully. That is the intended behaviour, but it is a behaviour
  change: anything relying on messaging an agent into existence will break.
- The `agents` listing is one field wider.
- The ghost room stays until someone hides it.
