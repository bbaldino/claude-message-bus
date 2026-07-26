#!/usr/bin/env bash
# UserPromptSubmit hook: tells the bus the human is active in this project, which
# resets the exchange-cap counter for that agent's rooms.
#
# Optional. Without it, a paused room is cleared with the `resume` tool instead.
#
# This must never block or fail a user's prompt: it always exits 0. If the bus is
# unreachable (e.g. CLAUDE_BUS_HTTP is not set to match the bus's actual address),
# it prints a one-line warning to stderr and moves on rather than failing silently.
#
# Install in .claude/settings.json:
#   {
#     "hooks": {
#       "UserPromptSubmit": [
#         { "hooks": [ { "type": "command",
#                        "command": "/path/to/human-active-hook.sh",
#                        "timeout": 5 } ] }
#       ]
#     }
#   }
BUS_HTTP="${CLAUDE_BUS_HTTP:-http://127.0.0.1:7777}"
NAME="${CLAUDE_BUS_NAME:-$(basename "${CLAUDE_PROJECT_DIR:-$PWD}")}"
curl -sf -m 2 -X POST "$BUS_HTTP/human-active?agent=$NAME" >/dev/null 2>&1 \
  || echo "human-active-hook: could not reach bus at $BUS_HTTP (set CLAUDE_BUS_HTTP?)" >&2
exit 0
