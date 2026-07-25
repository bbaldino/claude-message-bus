# POC 1 — manual interactive test

Headless (`-p`) testing is exhausted: the channel machinery never engages in that mode.
This is the authoritative test. Takes about two minutes.

## Run it

**Terminal 1** — start a session in this repo with the probe armed as a development
channel:

```bash
cd /home/bbaldino/work/claude-message-bus
claude --dangerously-load-development-channels server:probe
```

Expect two dialogs:

1. A full-screen warning listing development channels → choose
   **"I am using this for local development"**.
2. *"New MCP server found in this project: probe"* → choose **"Use this MCP server"**.

**This is the result that matters.** Immediately under the startup banner, look for:

```
Channels (experimental) messages from server:probe inject directly in this session
  · restart without --dangerously-load-development-channels to stop
```

- **Notice present** → channels work on this account. Continue.
- **"blocked by org policy"** → your claude.ai org needs an Owner to enable
  `channelsEnabled`. Design needs rethinking if that's not available.
- **No notice at all** → the feature isn't live for this account/build. Same conclusion.

Then let the session go **idle at the prompt**. Don't type anything.

**Terminal 2** — push a message into that idle session:

```bash
curl -X POST localhost:8788 -d "PROBE_HELLO_MARKER: reply via probe_reply with probe_id"
```

## What to look for in Terminal 1

| Check | Meaning |
| --- | --- |
| A line like `← probe: PROBE_HELLO_MARKER...` appears **while idle** | **The core premise holds.** Unknown #1 resolved. |
| Claude wakes and acts without you typing | Idle delivery confirmed |
| Claude calls `probe_reply` (approve the permission prompt) | Two-way path works |
| After approval, `SENT_ECHO_MARKER probe_id=1 text="..."` visible in the transcript | **Unknown #4 resolved** — tool results render, so the real `send` tool can echo outbound text back to your terminal |

That last one is the whole reason you'd be able to watch both halves of an
agent-to-agent conversation locally. If the marker does *not* appear, we fall back to
`claude-bus tail` as the only full view.

## Afterwards

```bash
cat poc/probe/probe.log        # every emission, timestamped
rm /home/bbaldino/work/claude-message-bus/.mcp.json   # stops other sessions here prompting
```

Paste the startup notice and whether the markers appeared, and I'll fold the answers
into the plan.
