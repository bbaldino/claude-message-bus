#!/usr/bin/env bash
# POC 2 — live confirmation that the Rust probe registers as a channel.
#
# The wire format is already verified identical to the Node probe that passed
# POC 1 (see test-notify.mjs), so this is confirmation rather than discovery.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
CRATE="$PWD"
cd ../.. || exit 1
PORT=8789
LOG="$CRATE/rust-probe.log"

warn() { printf '\033[33m%s\033[0m\n' "$*"; }
ok()   { printf '\033[32m%s\033[0m\n' "$*"; }

(cd "$CRATE" && cargo build 2>&1 | grep -E "^error" -A5) && { warn "build failed"; exit 1; }

if curl -s -m 1 "localhost:$PORT" >/dev/null 2>&1; then
  warn "something already listening on $PORT — stale probe? lsof -i :$PORT"
  exit 1
fi
rm -f "$LOG"

cat <<'EOF'

════════════════════════════════════════════════════════════════════
  POC 2 — same test as POC 1, but the channel server is Rust.
════════════════════════════════════════════════════════════════════

Clear the two dialogs (development-channels warning, then MCP server
consent), confirm the banner names server:rust-probe, then SIT IDLE.

A message is pushed in ~8s after the probe binds.
Watch for:   ← rust-probe: RUST_HELLO_MARKER ...
Approve the probe_reply prompt when it appears.

Type /exit when done.
════════════════════════════════════════════════════════════════════

EOF

read -r -p "Press Enter to launch..." _ || true

(
  for _ in $(seq 1 600); do
    curl -s -m 1 "localhost:$PORT" >/dev/null 2>&1 && break
    sleep 1
  done
  sleep 8
  curl -s -X POST "localhost:$PORT" -m 5 -d \
"RUST_HELLO_MARKER — pushed from the Rust channel server while the session was idle. \
Call probe_reply with the probe_id from this tag's attributes and text=\"ack\", then \
state in plain text exactly which attributes the <channel> tag carried." \
    >/dev/null 2>&1
) &
PUSHER=$!
trap 'kill "$PUSHER" 2>/dev/null' EXIT

claude --dangerously-load-development-channels server:rust-probe

kill "$PUSHER" 2>/dev/null
echo
echo "── rust-probe.log ─────────────────────────────────────────────────"
[ -f "$LOG" ] && cat "$LOG" || warn "(no log — probe never started)"
echo
if grep -q 'probe_reply called' "$LOG" 2>/dev/null; then
  ok "✓ POC 2 PASSED — the Rust channel server delivered and round-tripped."
else
  warn "~ No probe_reply. If no '← rust-probe:' line appeared, it did not register."
fi
