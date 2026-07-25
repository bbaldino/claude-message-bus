#!/usr/bin/env bash
# POC 1 — interactive channel test, single terminal.
#
# Launches a Claude Code session with the probe armed as a development channel,
# and schedules a background push that fires once the session is up and idle.
# You just clear the dialogs and watch.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.." || exit 1
REPO="$PWD"
PORT=8788
LOG="$REPO/poc/probe/probe.log"

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
warn() { printf '\033[33m%s\033[0m\n' "$*"; }
ok()   { printf '\033[32m%s\033[0m\n' "$*"; }

# --- preflight --------------------------------------------------------------
[ -f "$REPO/.mcp.json" ] || { warn "missing .mcp.json at repo root"; exit 1; }

if [ ! -d "$REPO/poc/probe/node_modules" ]; then
  echo "installing probe deps..."
  (cd "$REPO/poc/probe" && npm install --silent) || exit 1
fi

if curl -s -m 1 "localhost:$PORT" >/dev/null 2>&1; then
  warn "something is already listening on $PORT — a stale probe from an earlier run?"
  warn "find it with:  lsof -i :$PORT    then kill it and re-run."
  exit 1
fi

rm -f "$LOG" "$REPO/poc/probe/diagnostics.json"

cat <<'EOF'

════════════════════════════════════════════════════════════════════
  POC 1 — can a channel push into an IDLE Claude Code session?
════════════════════════════════════════════════════════════════════

You'll get two dialogs. Clear them:

  1. Full-screen development-channels warning
       → "I am using this for local development"
  2. "New MCP server found in this project: probe"
       → "Use this MCP server"

THEN LOOK AT THE STARTUP BANNER. This line is the whole experiment:

  Channels (experimental) messages from server:probe inject directly
  in this session · restart without ... to stop

  • present            → channels work on this account.
  • "blocked by org policy" → needs an Owner to enable channelsEnabled
  • nothing at all     → feature not live for this account/build

After that: SIT IDLE. Do not type. About 8 seconds after the session
comes up, this script pushes a message in from the background.

Watch for a line like:      ← probe: PROBE_HELLO_MARKER ...
Approve the probe_reply permission prompt when it appears.
Then check whether this shows up in the transcript:

                            SENT_ECHO_MARKER probe_id=1 text="..."

(that one decides whether you can watch both halves of an agent
conversation in your own terminal, or only via a separate viewer)

Type /exit when done.
════════════════════════════════════════════════════════════════════

EOF

read -r -p "Press Enter to launch..." _ || true

# --- background pusher: waits for the probe to bind, then for idle ----------
(
  for _ in $(seq 1 600); do
    curl -s -m 1 "localhost:$PORT" >/dev/null 2>&1 && break
    sleep 1
  done
  sleep 8
  curl -s -X POST "localhost:$PORT" -m 5 -d \
"PROBE_HELLO_MARKER — this arrived over the channel while the session was idle. \
Call probe_reply with the probe_id from this tag's attributes and text=\"ack\", \
then state in plain text exactly which attributes the <channel> tag carried." \
    >/dev/null 2>&1
) &
PUSHER=$!
trap 'kill "$PUSHER" 2>/dev/null' EXIT

# --- the session ------------------------------------------------------------
claude --dangerously-load-development-channels server:probe

# --- evidence ---------------------------------------------------------------
kill "$PUSHER" 2>/dev/null
echo
bold "── probe.log ──────────────────────────────────────────────────────"
if [ -f "$LOG" ]; then cat "$LOG"; else warn "(no log — probe never started)"; fi
echo
if grep -q 'probe_reply called' "$LOG" 2>/dev/null; then
  ok "✓ Claude called probe_reply — the channel delivered and the two-way path works."
elif grep -q 'notification written to transport' "$LOG" 2>/dev/null; then
  warn "~ Message was pushed, but Claude never replied."
  warn "  If no '← probe:' line appeared, the channel did not register."
else
  warn "~ No notification was ever pushed. Did the session reach idle?"
fi
echo
echo "Tell Claude what the startup banner said and whether the markers appeared."
