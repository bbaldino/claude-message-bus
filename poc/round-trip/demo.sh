#!/usr/bin/env bash
# POC 3 — live two-session demo.
#
# Starts the bus, then you launch two Claude Code sessions in two other terminals.
# Each session auto-names itself from its project directory, so they come up as
# "project-alpha" and "project-beta".

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
HERE="$PWD"
PORT=7777
BUSLOG="$HERE/bus.log"

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
warn() { printf '\033[33m%s\033[0m\n' "$*"; }

cargo build 2>&1 | grep -E "^error" -A5 && { warn "build failed"; exit 1; }

if ss -ltn 2>/dev/null | grep -q ":$PORT "; then
  warn "port $PORT is already in use — another bus running? kill it first."
  exit 1
fi

rm -f "$BUSLOG"
./target/debug/round-trip serve --port "$PORT" > "$BUSLOG" 2>&1 &
BUS=$!
trap 'kill "$BUS" 2>/dev/null' EXIT
sleep 1

kill -0 "$BUS" 2>/dev/null || { warn "bus failed to start:"; cat "$BUSLOG"; exit 1; }

cat <<EOF

════════════════════════════════════════════════════════════════════
  POC 3 — two agents, two projects, one conversation
════════════════════════════════════════════════════════════════════

Bus is up on port $PORT (pid $BUS). Leave this terminal running.

Open TWO more terminals and run one of these in each:

EOF
bold "  cd $HERE/demo/project-alpha && claude --dangerously-load-development-channels server:msgbus"
bold "  cd $HERE/demo/project-beta  && claude --dangerously-load-development-channels server:msgbus"
cat <<'EOF'

Clear the dialogs in both. Each names itself from its directory, so they
register as "project-alpha" and "project-beta". The bus tools are already
allowlisted in each project's .claude/settings.json, so no approval prompts
should interrupt the exchange.

Then, in the ALPHA session, type something like:

  Use the agents tool to see who is online, then send project-beta a message
  proposing a wire format for a small RPC protocol, and discuss it with them.

Now watch the BETA session — you never typed in it. It should wake up on its
own, and the two should negotiate back and forth.

Things worth watching for:
  • does beta act while idle, with no input from you?
  • do they converge and stop, or ping-pong forever? (this sets the
    exchange-cap default in the spec)
  • does either try to edit files despite the instructions saying not to?
    (that would stall on a permission prompt, which is the intended fence)

This window tails the bus, so you can watch both halves in one place.
Ctrl-C to stop.
════════════════════════════════════════════════════════════════════

EOF

tail -f "$BUSLOG"
