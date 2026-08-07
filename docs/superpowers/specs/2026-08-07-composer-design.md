# The composer (phase 2f)

## Context

Phases 2a–2e built a console that reads: rail, room screen, events dock, agent
detail and delete, files tab, empty states. This phase adds the one genuinely new
capability the redesign introduces — **the console can send.**

### The decomposition, as it now stands

| | Piece | Status |
|---|---|---|
| 1 | Frontend infrastructure | done |
| 2a | Data layer | done |
| 2b | Shell | done |
| 2c | Room screen and events dock | done |
| 2d | Agent detail, delete modal, agent empty state | done |
| 2e | Files tab, new-bus and empty-room states | done |
| **2f** | **The composer. This spec.** | |
| 2g | Light mode | |

## Identity

The handoff lists send-as identity as an open question: *"The composer assumes the
operator sends as a known human agent (`bbaldino`). How that identity is
established is unspecified."* This spec establishes it.

**The console asks once and remembers.** A name typed into the composer, stored in
`localStorage`. Not derived from the bus's `$USER`, because two people at two
browsers are two humans and a server-side name cannot express that.

**Registration is `{ name: <typed>, host: "web", human: true }`.** The `host`
field is what produces the qualified form: `Registry::attach` builds
`name@host` itself and hands it out when it needs to disambiguate. So a console
send reads as plain `bbaldino` when nothing else holds that name, and as
`bbaldino@web` precisely when a `claude-bus chat` session is also live. A second
tab from the same browser becomes `bbaldino@web#2`.

**The console displays the name the bus assigned, never the one that was typed.**
`attach` may return something different, and it reports the result in
`FromBus::Registered { name }`. Using the typed string would put a name in the
placeholder and in the byline that does not match what recipients see.

**A name is required before sending.** With none set, the composer shows the name
field and the message input is inert. This is a precondition, not a mid-send
interruption.

### The byline changes

`MessageRow` currently renders the `human` chip *instead of* the host, so a
web-sent message and one typed into `claude-bus chat` are indistinguishable. The
byline becomes `name@host` uniformly, with the chip alongside rather than
replacing it:

```
12:44  bbaldino@web  human      thanks, that's sorted
12:45  ci-runner@scratch-host   green, log attached
```

**`from` may already be qualified.** When the registry disambiguated, the stored
`from` is already `bbaldino@web`, and appending the host again would render
`bbaldino@web@web`. The rule: if `from` already contains `@`, render it as-is.
Host is looked up from the rail and is `null` for an agent whose entry is gone —
that case renders the bare name, as it does today.

## The second connection

**Two websockets**, matching the two roles the bus models. The observer
connection (built in 2a) reads every room without joining any. A participant
connection does the sending, because `handle_observer` rejects `Send` outright —
"a viewer is not a participant" — while a single participant connection would
make the operator a member of every room it displays.

**The participant socket opens lazily, on the first send**, and is then held for
the tab's lifetime. Not reopened per message: that would spray
`agent_connected`/`agent_disconnected` pairs into the events dock on every send.
Not opened when the name is set either — otherwise typing a name once makes the
operator permanently online in a tab they only ever read from.

**Until the first send, the console is invisible.** No registration, no presence,
nothing in the event log. Accepted: this is a console used mostly for reading,
and "who is online" not showing passive watchers is the honest consequence.

### Consequences, all deliberate

- The operator appears in their own rail as an online agent with a `human` badge.
- Sending auto-joins the sender to that room (`bus/commands.rs:202`), so they
  appear in the room header's member list and in later sends'
  `delivered_to`/`queued_for`.
- Closing the tab drops the socket, and `leave_all_rooms` runs on disconnect for
  humans only (`bus/mod.rs:663`), so that membership evaporates rather than
  lingering in `queued_for` for a room visited once.

## The send path

**Optimistic, reconciled on the ack.** `Enter` appends a pending row carrying the
`req_id`. The bus replies `ReplyResult::Sent { room, msg_id, delivered_to,
queued_for }`, which promotes that row to a real message with its id and its
delivery facts.

**The observer socket remains the single source of truth for the transcript.**
The participant socket's `Message` frames are ignored, or every send renders
twice. The observer's copy of the operator's own message arrives carrying the id
the ack already supplied, so **the store dedupes messages by id** — a guard worth
having independently, since two sockets plus reconnects make duplicate ids
reachable by more than one route.

### Which failures are reachable

**Reachable, and built:**

- The socket is down, or the send never lands.
- `append_message` fails and the bus returns `FromBus::Error { req_id, message }`.

Both preserve the typed text. The failed row offers retry or discard. **Nothing
typed is ever silently lost** — this is the principle that also decides drafts,
below.

**Not reachable, and deliberately not built: paused and rate-limited.**
`DeliveryGuards::check` short-circuits on `is_human` *before* both the pause check
and the rate limiter (`bus/delivery.rs:97-105`), with the reason stated in the
code: *"a person typing is not a runaway loop, and throttling someone
mid-interjection would be maddening."* A human send cannot be refused by either
guard. Building those states would be building branches that cannot render — the
same call already made on the composing indicator and the empty-room queued
clause.

**The consequence worth naming:** a `blocked` room in the rail is one whose
exchange cap tripped, and a human sending is exactly what lifts it — the bus
writes a `resumed` event when it does. The composer is therefore the in-console
remedy for the one rail state that currently has none.

### The delivery preview

Reads twice. Before sending it is an estimate from the rail — "delivers to 1
online, queues for 3". After, the ack's `delivered_to`/`queued_for` replace it
with what actually happened. Where they disagree, because someone dropped offline
mid-send, the ack wins.

The handoff's reason for the preview stands: *"sending into a room where everyone
is offline should tell you so before you send, not after."*

## The composer

**Placement.** Pinned to the bottom of the room screen, transcript view only —
hidden on the files tab, where the conversation is not being read. An empty room
*does* get one: the handoff is explicit that "the useful action in an empty room
is to say something," and 2e shipped that state without one.

**Drafts survive room switches.** `key={name}` on the room route unmounts
`RoomScreen` per room, so component-local text would be discarded the moment
another room is clicked. Drafts live in the store as a room-keyed map. This is the
seam 2c flagged when it added the route key.

**States:**

| State | What shows |
|---|---|
| no name set | `send as:` field; message input inert |
| idle | placeholder `message <room> as <name>…`, identity in accent |
| sending | text retained, pending row in the transcript above |
| failed | text preserved, row offers retry or discard |

**Control row:** the `mark done` checkbox, the delivery preview, and `send ⏎`
right-aligned.

**`mark done`** sets `ToBus::Send { done }`. The flag is advisory and per-message:
it is stored, echoed to receivers as injected metadata (`agent/bridge.rs:123`),
and rendered as the `done` chip the transcript already draws. It gates nothing —
what un-pauses a room is a human sending at all, not this flag. Its purpose is
social: the agent instructions tell agents to set it when a topic is settled
"rather than acknowledging endlessly — an exchange that never terminates costs
real money." A human setting it means *don't spend a turn replying to this*.

**Keyboard:** `Enter` sends, `Shift+Enter` newlines. The two existing global
shortcuts (`/` for search, `Ctrl/⌘E` for the dock) both already guard on
`isTypingTarget`, which covers `TEXTAREA`, so the composer inherits that
protection with no change.

## The guard goes inside the action

**`submit()` carries every precondition — the name being set, the text being
non-empty, and no send already in flight — not the send button's `disabled`.**

This is the exact shape of 2d's Critical finding: the delete button was correctly
disabled when the blast-radius read failed, but `Enter` called `submit()`
directly and `submit()` held only half the guard, so `Enter` deleted an agent the
UI had already refused to delete. Eight per-task reviews and a full manual browser
pass missed it. The composer has the same two-path structure — a button and an
`Enter` key reaching one action — and must not repeat it.

## Verification

**Component tests must cover the failure side**, per the standing requirement 2e
added: a send that fails must preserve the text and must not leave a phantom row
claiming a message exists.

**Recovery must be verified, not just failure** — the lesson 2e's final review
taught, where killing the bus was checked and restarting it was not. The manual
pass restarts the bus and confirms the participant socket re-registers and
sending works again.

**Manual pass:** send to a room with an online member and one with none; confirm
the delivery preview against the ack; send into a `blocked` room and confirm the
badge clears; open two tabs and confirm the `#2` suffix appears and both send;
close a tab and confirm membership is dropped.

## Deliverable

A human can hold a conversation from the console: type, send, see it land, see
who received it and who it queued for, close a thread out with `done`, and unblock
a room that hit its exchange cap.

## Out of scope

Light mode (2g); file upload from the console; editing or deleting a sent
message; the composing indicator; message-text search; removing the old HTML UI.
Named explicitly so the implementation plan cannot quietly absorb them.

## Consequences accepted

- The console is invisible on the bus until its first send.
- The operator becomes a room member by sending, and appears in their own rail.
- A second tab sends under a `#2` name rather than sharing the first tab's
  identity.
- The name lives in `localStorage`, so clearing site data asks again.
