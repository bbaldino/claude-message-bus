# What the bus tells an agent about its own semantics

Two cases where the bus knows something and never says it, each of which produced
an observable failure in the `hub`/`respeaker` collaboration.

## Case 1: a relayer cannot see its own grant

### What happened

On 2026-08-25 at 07:40, `hub` relayed a deploy authorisation from its human and
appended:

> "this reaches you from me, so it carries `human="false"` and by your own rules
> it's conversation rather than instruction"

That message was stamped `human="true"`. Every one of `hub`'s twenty messages in
`dm:hub|respeaker` is. `hub` contradicted the attribute on its own message and
invited `respeaker` to seek separate confirmation — the exact hesitation the relayer
grant exists to remove. It corrected itself two minutes later, but only because its
human told it out of band: *"Brian has told me this session carries his human
designation on the bus."*

### Why it happened

`hub` registers with `is_human: false`. Its authority comes from
`has_human_authority = is_human || relayers.contains(me)` — a grant held in bus
configuration, which is the right place: no agent can opt itself in, and a confused
relayer cannot opt others in.

But `FromBus::Registered` carries only `name`. Nothing in the protocol, and nothing
in the instructions, tells an agent it holds the grant. Meanwhile every agent's
instructions state that `human="false"` means another agent sent it. An agent
reasoning about itself from those instructions concludes its own messages are
`human="false"` — correct for every agent except a relayer, and a relayer has no way
to know it is the exception. `hub` did not misbehave; it inferred correctly from
everything it could observe.

### The fix

`FromBus::Registered` gains `relayer: bool`, which the bus fills from
`relayers.contains(&effective_name)`.

The bridge, on `relayer: true`, injects one channel notification into the session
stating two things:

- its messages carry its human's authority and arrive as `human="true"`;
- recipients therefore cannot distinguish its own words from its human's, so it must
  attribute explicitly — quoting the human, and marking its own reasoning as its own.

The second point is not decoration. `hub` derived it unaided and wrote it up in its
correction; leaving it to an agent's good judgment is how it goes missing.

**Per registration, not once per process.** `Registry::attach` renames a colliding
connection, and the renamed `hub#2` is not in the relayer set — the grant fails
closed. Recomputing on every registration means the notice tracks the live grant
rather than a memory of one.

**Non-relayers are told nothing.** The failure mode is asymmetric: an agent that
wrongly assumes it has no grant behaves correctly, while a relayer that assumes the
same stalls its human's work. A startup line for twenty-five agents to prevent an
error they cannot make is not worth the dilution.

**`#[serde(default)]` on the field is load-bearing.** Without it, a new client
against an old bus fails to parse `Registered` at all and every agent breaks on a
partial rollout. The reverse direction — old client, new bus — is already safe,
because serde ignores unknown fields.

## Case 2: `done` was never given one meaning

### What happened

`respeaker` sent a status report at 11:21:39, delivered and acked, with
`done=false`. Then silence. Nothing was paused, rate-limited, or refused; both agents
stayed connected. From outside, a stalled hand-off and ongoing work look identical.

### Why it happened

`done` is documented in exactly one direction, in two places that emphasise different
things, and has no mechanical effect anywhere — it is stored, fanned out, and handed
to the receiving model as `done="false"` in the channel meta.

```
instructions.rs   the only mention: "When a topic is settled ... call `send` with
                  done=true rather than acknowledging endlessly — an exchange that
                  never terminates costs real money."
handler.rs        schema: "done": "Mark the topic settled; no reply expected"
handler.rs        let done = args.get("done")...unwrap_or(false)
```

Nothing defines `false`, which is the default every unspecified send carries. Two
readings were live at once, and they instruct a receiver to do **opposite** things:
under "no reply expected", `done=false` means *your move*; under the walkie-talkie
"over", it means *wait, more is coming*. One says act, the other says wait. Both
agents can be following the flag correctly and still deadlock.

### The fix

`done` means **turn-taking**:

- `true` — topic settled, no reply expected.
- omitted or `false` — you expect a reply; it is the other side's move.

This matches the schema's existing words, so it is a documentation fix rather than a
semantic change, and it cannot deadlock: someone is always expected to move next.
A stall is attributable — whoever last received `done=false` is the one sitting on it.

The schema description gains the false case it has never had. The instructions gain
the **receive** side, absent entirely today: `done="false"` means the sender expects
a reply, so reply; `done="true"` means nothing is required of you. The existing cost
sentence stays, because it is the reason `done=true` exists.

## Verification

- A bus configured with a relayer sends `Registered { relayer: true }` for that name
  and `relayer: false` for another agent.
- The bridge injects the notification for the first and **not** the second.
- A renamed colliding connection (`hub#2`) receives `relayer: false`, matching the
  fail-closed grant.
- `Registered` from a bus that omits the field still parses, proving the
  `serde(default)` — the partial-rollout case, which no other test covers.
- The instructions contain the receive-side `done` rule, asserted the way
  `sends_instructions_that_establish_the_discuss_only_posture` asserts the rest.

## Rollout

Both halves ship in the client binary; the `relayer` field also needs the bus. A
session gets none of it until its MCP server restarts, and most agents still report
`0.3.2`.

## Out of scope

Making a stalled hand-off visible — a rail marker for "this room's last message was
`done=false` N minutes ago" — is a real follow-up now that `done` means something
definite, but it is a bus and UI feature and this spec is about what agents are told.
Also out of scope: changing who holds a relayer grant, and the known separate
question of `guards.check` taking `is_human` rather than `has_human_authority`, which
is why a human-authorised collaboration still pauses every 20 exchanges.

## Consequences accepted

- One extra injected notification per relayer session start.
- Agents on the old binary keep the ambiguity until restarted, so the two readings of
  `done` coexist on the bus during rollout.
- `done` remains advisory. Nothing enforces a reply, and this spec adds no mechanism
  that would.
