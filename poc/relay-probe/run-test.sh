#!/usr/bin/env bash
# Permission-relay probe. Two runs, ~3 minutes total.
#
#   ./run-test.sh          run A — no skip flag: prompts SHOULD relay
#   ./run-test.sh skip     run B — with skip flag: prompts SHOULD NOT fire
#
# Env knobs: PROBE_VERDICT=allow|deny|none   PROBE_DELAY_MS=6000

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
LOG="$PWD/relay-probe.log"
MODE="${1:-normal}"

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
warn() { printf '\033[33m%s\033[0m\n' "$*"; }

[ -d node_modules ] || { echo "installing deps..."; npm install --silent || exit 1; }
rm -f "$LOG"

if [ "$MODE" = "skip" ]; then
  FLAGS="--dangerously-skip-permissions"
  cat <<'EOF'

════════════════════════════════════════════════════════════════════
  RUN B — with --dangerously-skip-permissions
════════════════════════════════════════════════════════════════════

The whole gating story in the design rests on this: a session started
with the skip flag should raise NO permission prompts, so relay is
inert for it. The docs only promise it bypasses "most" prompts, which
is why we are checking rather than assuming.

Ask Claude to do the SAME things as run A:

  write a file called probe-scratch.txt containing the word hello,
  then run the shell command `date`

EXPECTED: it just does them, and the probe log stays empty.
If any request appears in the log, the design's assumption is WRONG
and gated-vs-ungated is not simply a launch-flag decision.
EOF
else
  FLAGS=""
  cat <<'EOF'

════════════════════════════════════════════════════════════════════
  RUN A — no skip flag: prompts should relay
════════════════════════════════════════════════════════════════════

Ask Claude to do two things, so we see both a Write and a Bash:

  write a file called probe-scratch.txt containing the word hello,
  then run the shell command `date`

WHAT TO WATCH. Each time a permission dialog appears in your terminal,
LEAVE IT ALONE for a few seconds. The probe answers "allow" over the
channel after 6s. What we need to know:

  • does the dialog close on its own, without you touching it?
    -> that is the verdict path working end to end
  • if you answer locally first, does that win harmlessly?
    -> both answers are supposed to race, first one wins

Type /exit when done.
EOF
fi

cat <<'EOF'

You will get two dialogs first: the development-channels warning
(choose "I am using this for local development") and the MCP server
consent (choose "Use this MCP server").

EOF
read -r -p "Press Enter to launch..." _ || true

# shellcheck disable=SC2086
claude --dangerously-load-development-channels server:relay-probe $FLAGS

echo
bold "── relay-probe.log ────────────────────────────────────────────────"
if [ -s "$LOG" ]; then cat "$LOG"; else warn "(log is empty — no permission requests were relayed)"; fi
echo
COUNT=$(grep -c "=== PERMISSION REQUEST" "$LOG" 2>/dev/null || echo 0)
bold "permission requests relayed: $COUNT"

if [ "$MODE" = "skip" ]; then
  if [ "$COUNT" -eq 0 ]; then
    echo "✓ As designed: the skip flag suppressed everything, relay is inert."
  else
    warn "✗ Prompts fired DESPITE the skip flag — the design's gating assumption is wrong."
  fi
else
  if [ "$COUNT" -gt 0 ]; then
    echo "✓ Relay fires. Check above whether dialogs closed without you touching them."
  else
    warn "✗ No requests relayed. Either the capability is not honoured here, or nothing"
    warn "  you asked for actually needed permission."
  fi
fi
rm -f probe-scratch.txt
