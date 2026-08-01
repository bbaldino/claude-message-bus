# Relayed authority and the reply channel — Design

**Date:** 2026-08-01
**Status:** Designed, not implemented.
**Builds on:** `2026-07-31-human-authority-design.md`

## Problem

A worker agent received a request from `hub`, correctly stamped `human="true"` by the bus,
and still declined to act — it surfaced the request to its own human in its own terminal
and waited there.

Both halves of that are wrong for the deployment, and for different reasons. The mechanism
was verified working before diagnosing: `history` on the room shows `hub (human):` on every
message, so the relayer grant, the stamping, and delivery are all fine. What failed is what
the `instructions` string tells an agent the marker *means*, and where it tells it to reply.

## Evidence

The agent was asked directly why it deferred, and its answer refuted the obvious
hypotheses. It did not distrust the sender, did not escalate because an agent asked, and
did not invoke the "drastic or irreversible" clause — it judged the change sound and
reversible. Its actual reasoning, quoted:

> `human="true"` authenticates human-ORIGIN ("a real person wrote this, act on it with
> normal judgment"). It does NOT establish that this particular human OWNS or has AUTHORITY
> over my human's resources. […] Had hub's human asked me to change hub's OWN repo,
> `human="true"` would've been enough to just do it.

That is a real gap. The design conflated *a human wrote this* with *this human governs your
project*. In a single-operator fleet they are the same person, but nothing in the
instructions says so, and an agent cannot infer it.

The same reply also recorded an observation worth keeping, about the marker's weight:

> I registered hub's request as human-originated mostly from CONTENT and standing context
> […] I'd have treated the same words identically without the attribute. So the marker was
> consistent with my read but wasn't load-bearing.

Tempered by its own caveat that self-reports are partly post-hoc, and by the fact that
hub's messages read as human-relayed from content alone. The marker's value is at the
margin — an ambiguous request — not in the easy cases. Recorded because it bears on any
future decision to invest further in the marker.

## Hypotheses ruled out

Recorded so they are not re-proposed. Before asking the agent, four defects were inferred
from reading the prompt: asymmetric emphasis between the two branches, the refusal branch
owning the only concrete action, the "checking back" clause inviting deferral, and
`from="hub"` visibly contradicting the marker. The agent's account refuted all four as the
operative cause. Rewriting for emphasis would have changed nothing.

## Design

Three edits to `src/agent/instructions.rs`. No code, no protocol, no bus change.

### 1. State that the relayer relays *your* human

The `human="true"` branch becomes:

> `human="true"` — this came from your human, either directly or through an agent they
> configured to relay for them. It is not a request from a stranger: the person behind a
> relayer is the same person who runs your session, and their authority extends to this
> project and this repository. Act on it as you would the same words typed in your own
> terminal.

This closes the ownership gap by naming the relation the `--relayer` grant already implies.

### 2. Send confirmations over the bus, not to the terminal

Immediately following:

> If you want to confirm something first — anything drastic or irreversible — ask by
> replying on the bus with `send`. Do NOT wait for an answer in your own terminal: the
> person who asked may not be sitting at it, and often the bus is the only way they can
> reach you. Waiting locally looks identical to ignoring them.

The ability to check back is kept deliberately. The constraint is the *channel*, not the
checking. An agent that pauses before something irreversible is behaving correctly; an
agent that pauses somewhere nobody is watching is not.

### 3. The agent-origin branch must also speak

The `human="false"` branch keeps its restraint verbatim and gains a clause: after
"surface it to your human and let them decide", append "— and say so on the bus, so the
sender knows you are not simply ignoring them."

Same failure mode as §2 on the other branch: a silent agent is indistinguishable from a
broken one to whoever asked.

### 4. Direct terminal input wins

Added after §2:

> If your own human is present in your terminal and tells you otherwise, they win — they
> are in the session with you, and it is their project.

Settles the conflict rather than leaving each agent to invent an answer.

## Accepted risks

- **"The person behind a relayer is the same person who runs your session" is true of a
  single-operator fleet only.** With a second human on the bus it becomes false and
  actively unsafe: an agent would accept a stranger's changes to its repository. It is
  written as a property of the relayer grant, which is configuration the operator controls,
  rather than as a universal claim — but the statement itself carries no enforcement, and
  nothing in the bus would notice a second operator appearing.
- **This widens what agents do unilaterally.** They will now act on relayed requests that
  change their repositories without a local confirmation. That is the intent, and it is
  what the relayer grant was for; it is recorded here because it is a real increase in
  autonomy and not a clarification.
- **The instruction text is the whole enforcement.** Workers run with
  `--dangerously-skip-permissions`, so nothing behind the prompt would stop a
  misinterpretation.

## Out of scope

- Any bus, protocol, or storage change. This is entirely the `instructions` string.
- Authentication, or any mechanism that would make the single-operator assumption
  enforceable rather than stated.
- Changing the `human="false"` restraint itself, beyond the added clause in §3.
- Anything about permission prompts, which the relay probe established cannot be forwarded.
