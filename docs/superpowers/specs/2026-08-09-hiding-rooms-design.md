# Hiding rooms from the rail

## Context

A DM room between two agents survives the deletion of one of them. The console's
agent delete is deliberately narrow — `forget_agent` removes the agent's
`room_members` rows, its `cursors`, and the `agents` row, and nothing else. The
delete modal says so: *"messages and files are kept; they belong to the room."*

That leaves a DM that can never be useful again sitting in the rail. Its `blocked`
badge correctly disappeared — that flag counts members who are offline with unread
messages, and deleting the membership removed the only one — but the room itself
remains, because **there is no room deletion anywhere in the codebase**. No
`DELETE FROM rooms`, no `forget_room`. Rooms are insert-only: a DM auto-creates on
first send and then exists forever.

Nothing is broken. But agent lifecycle was designed and room lifecycle never was,
and a DM to a deleted agent is the case that makes the gap visible.

## What this is not

**Not a delete.** The standing rule in this project is that nothing deletes from
`messages` or `events` — the audit log is what makes the bus trustworthy about what
happened. This adds a display flag and removes nothing.

Room *deletion* remains unbuilt and unspecified. If it is ever wanted it is a
separate design, and it collides with that rule head-on.

## Where "hidden" lives

**A `hidden` column on `rooms`**, not browser storage.

`INTEGER NOT NULL DEFAULT 0`, added with the existing
`Store::add_column_if_missing`, which already added `agents.is_human`,
`messages.human`, and `agents.version`. No new table and no migration framework.

Server-side rather than `localStorage` because the console is reachable from any
browser and any machine, and a room tidied away on one should stay tidied on the
next. It also leaves the door open for `claude-bus rooms` to respect the flag
later, which a browser-local list could never do.

**Not scoped per operator.** That would be correct if the bus had more than one
human, but there is no operator identity to scope it to — the composer's send-as
name is typed per-browser and stored in `localStorage`, which is precisely the
thing being rejected here. Adding a real operator identity is its own piece of
work, and this feature does not justify it.

## The endpoint

**`POST /api/rooms/{name}/hidden`**, body `{ "hidden": true | false }`.

It carries the same `Origin`-vs-`Host` CSRF check the HTML delete form uses
(`web::origin_matches_host`). It is a state-changing POST reachable from a browser
against a bus that binds `0.0.0.0` with no authentication, which puts it in exactly
that class. A request with no `Origin` is allowed, for the same reason stated
there: those callers could already reach the port directly.

A hide request naming a room that does not exist returns **404** and creates
nothing. This is the same reasoning the delete path documents for its unknown-name
guard — looking the name up is *"what stops any name at all from forging"* a row
and an event. A typo must not conjure a hidden room that then shows up in the
rail's count.

Note this is deliberately the opposite of `GET /api/rooms/{name}/files`, which
returns an empty list rather than a 404 for an unknown room. A read of "what is in
this room" is answerable for a room with nothing in it; a write to a room that does
not exist is not a request that can be honoured.

## The rail returns every room

`RailRoom` gains `hidden: bool`. The rail endpoint does **not** filter hidden rooms
out of its response.

This follows the rule `queued_message_count`'s own doc comment states: *"The server
ships data rather than sentences precisely so the client can write that
sentence."* The client needs the whole list regardless, to render the count —
filtering server-side would mean a second call to learn how many were filtered.

## The console

**The count lives in the ROOMS header.** That header already carries a
right-aligned note (`last 60 min`); when any room is hidden it reads `2 hidden ▾`
instead. Clicking expands the hidden rooms into the list below the visible ones,
dimmed. Collapsed on load, and the expansion is not persisted — it is a momentary
"let me look", not a preference like the theme or the dock.

When no room is hidden there is no affordance at all. The console does not
advertise a state that does not exist.

**The control lives in the room screen's tab bar**, right-aligned in the space
currently empty beside `transcript` / `files · N`. It reads `hide`, or `unhide`
when the room is already hidden — one control, one boolean.

This mirrors agent delete: you go to the thing's own screen to act on it. It also
means the way back is complete without a second control — expand the header, click
a dimmed row, and the same tab bar now offers `unhide`.

**A known friction, recorded rather than designed around:** hiding takes one click
from where you already are, while unhiding takes a round trip into the room. That
asymmetry surfaced during design and is left in deliberately, because the fix (an
`unhide` affordance on each expanded row) adds a second control for an operation
that should be rare. The manual pass looks at whether it actually grates.

## What brings a room back

**A message, and nothing else.**

In `append_message`: `UPDATE rooms SET hidden = 0 WHERE name = ?1 AND hidden = 1`.
Zero rows affected in the normal case, so it costs nothing on the send path.

Deliberately not triggered by events, file uploads, or presence changes. A room
whose only activity is a `room_joined` is not a conversation being missed, and
unhiding on every event would make the feature useless for any room with members.
A message is what the rail exists to surface.

This rule is also what makes the motivating case work: a DM to a deleted agent can
never receive a message, so it stays hidden permanently.

## Events

Manual hide and unhide append `room_hidden` / `room_unhidden`, consistent with
`room_paused` / `resumed` already being in the log.

The automatic unhide does **not** write an event. The message that caused it is
already the record, and writing both would double-write on the one path where
latency matters.

## Verification

**Every behavioural test must be confirmed to fail before the change exists** —
this project has repeatedly shipped tests that passed for reasons unrelated to
their subject.

- A hidden room reappears when a message lands. Confirm the test fails without the
  `UPDATE`, rather than asserting it does.
- Hiding removes a room from the rail's *visible* set while it remains in the
  payload with `hidden: true`. Those are two different claims and both matter.
- The failure path: hiding a room that does not exist is refused and creates
  nothing.
- Events are written for manual hide/unhide and **not** for the automatic one.

**Manual pass:** hide a room and watch it leave the rail; expand the header, click
through, unhide; hide one and post to it from another session and watch it return
unaided. Then judge whether the unhide round trip grates.

## Deliverable

A room can be tidied out of the rail and got back again, the rail says how many are
hidden, and a room that starts talking again comes back on its own.

## Out of scope

Hiding agents; bulk hide; per-operator scoping; a CLI flag. `claude-bus rooms`
keeps listing everything — the column is there if it should respect the flag later,
but that is its own change. Room deletion remains unbuilt.

## Consequences accepted

- A hidden room still appears in `claude-bus rooms` and in the events dock.
- Hiding is global: anyone using the console sees the same hidden set.
- Unhiding costs a round trip into the room.
- A hidden room that receives a message comes back whether or not that was wanted.
