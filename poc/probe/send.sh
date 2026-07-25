#!/usr/bin/env bash
# Push an ad-hoc message into the running probe session.
#   ./send.sh "some text"
# Useful for testing a second exchange, or for pushing while the session is
# mid-turn rather than idle (events should batch and land on the next turn).
set -uo pipefail
MSG="${*:-PROBE_HELLO_MARKER — ad-hoc push, please acknowledge with probe_reply}"
curl -sS -X POST localhost:8788 -m 5 -d "$MSG" && echo
