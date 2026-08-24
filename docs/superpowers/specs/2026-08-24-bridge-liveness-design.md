# The agent bridge detects a dead connection itself

## The bug, as observed

`respeaker@bbaldino-thinkpad` registered on 2026-08-18 at 13:39:45 and was dropped
at 18:35:45 with `agent_disconnected / keepalive_timeout`. It never came back. Six
days later the bus still showed it offline, while on the laptop the `claude-bus`
process was alive and `ss -tnop` reported the socket to the bus as **ESTAB**.

Both halves of that are true at once, and that is the whole bug: the client holds a
connection the bus stopped having.

## The mechanism

1. The laptop slept. Pongs stopped reaching the bus.
2. The bus hit its 90s pong timeout and closed its side
   (`src/bus/mod.rs:654-663`). The FIN went to a sleeping host and was lost.
3. The bus has sent nothing to that socket since — it has no connection to send on.
   Queuing a message for the agent does not change this: the queue is drained on
   *reconnect*, so nothing about it reaches a client that never reconnects.
4. The client sits in `connect_once`'s `select!` (`src/agent/bridge.rs:79-96`),
   which wakes on exactly two things: the model wanting to send, or bytes arriving.
   A passive agent never writes, and the only peer that would write is gone.
5. `connect_once` never returns, so the reconnect loop at `src/agent/bridge.rs:41-54`
   never gets a turn. The backoff and the retry are unreachable in this state.

The bridge has no liveness check of its own. It trusts the transport to report the
death, and a lost FIN means the transport never does.

## Why this looked like it worked

Of 16 `keepalive_timeout` disconnects in the bus's event log, 11 were followed by a
registration under the **same session id**, which reads like the bridge healing
itself. Those are all `hardac` agents from one mass event on 2026-08-05 — a bus
restart. An awake host gets an RST immediately, the read fails properly, and the
existing loop does its job.

Every `respeaker` recovery in the log came back with a **different** session id: a
human restarting it. It has never once self-recovered. Sleep is simply the reliable
way to lose the return path silently; an expiring NAT mapping, a VPN drop, or a wifi
handoff produce the identical permanent hang.

## The fix

`connect_once` gains a ping ticker and an idle deadline in its `select!`.

- Every **30s**, send `Message::Ping` — the bus's `ping_interval`.
- Track the instant of the last inbound frame: *any* frame, whether a message, a
  pong, or the bus's own ping. If **90s** passes with nothing, return an error so
  the reconnect loop that already exists runs.

30/90 mirrors `Keepalive`'s defaults (`src/bus/mod.rs:62-63`) — three missed
intervals before declaring death — so detection lands within ~90–120s depending on
tick alignment and a woken laptop is back inside about two minutes.

Three properties worth stating, because each rules out a plausible alternative:

- **The client pings rather than only listening.** Relying on the bus's pings alone
  would need no ticker at all, but it couples the client's timeout to the peer's
  configured cadence, and that cadence is configurable (`Keepalive::new`). Anyone
  lengthening the bus's ping interval past the client's timeout would turn every
  idle connection into a reconnect loop, with nothing in either file to warn them.
- **The ping is also a write.** A write on a dead socket eventually fails on its
  own, which gives detection a second path that does not depend on our timer.
- **Any inbound frame resets the deadline**, not specifically a pong. It is strictly
  more information, and a busy connection cannot trip the timer just because a pong
  queued behind a burst of messages.

`STABLE_CONNECTION_THRESHOLD` already handles the recovery: a connection up for
hours resets the backoff to its 1s floor, so this is one quick retry rather than a
slow ramp.

## Logging

The give-up path gets its own line, distinct from the existing
`[agent] bus connection closed`:

```
[agent] no traffic from the bus in 90s, assuming the connection is dead
```

Today "the peer hung up" and "we gave up waiting" would be indistinguishable in the
log. If this ever misfires, that difference is the first thing worth knowing.

## Verification

The bug is that nothing observes the silence, so the test has to manufacture
silence. A plain `TcpListener` accepts the websocket handshake, holds the socket
open, and sends nothing — no messages, no pings, no close.

- The bridge reconnects anyway: assert a **second** handshake arrives within the
  deadline. Against the current code this hangs until the harness kills it, which is
  the failure to watch happen before the change exists.
- A connection carrying traffic is **not** torn down: a server that sends something
  every interval must not see a reconnect. This is the regression that would matter
  most — a timer that fires on a healthy connection would make every long-lived
  agent flap.

## Rollout

This ships in the client binary, so a session picks it up only when its MCP server
restarts. Most agents on the bus report `0.3.2` and need restarting regardless. The
currently wedged `respeaker` cannot be repaired by any release — it needs the manual
reconnect.

## Out of scope

`tail` and `chat` (foreground CLI tools; a human sees them stop), the console's
browser socket (watched by a person, and it refetches over HTTP every 25s — and
JavaScript cannot send websocket pings, so it needs a different mechanism), and any
change to the bus. Named so the implementation plan cannot absorb them.

## Consequences accepted

- Two frames every 30s per agent, in each direction, on an otherwise idle bus.
- A connection genuinely silent for 90s is now torn down and rebuilt. On a link that
  stalls that long the reconnect is the correct outcome, but it is a behaviour
  change: today such a connection is kept.
- The bridge stays silent to the model about reconnecting. The unread summary
  already reports what was missed; a "connection restored" notice would be noise.
