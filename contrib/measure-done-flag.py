#!/usr/bin/env python3
"""How reliably do agents' `done` flags predict a reply?

Read-only: pulls every room's transcript from a running bus over its HTTP API and
reports, for messages sent by agents (not humans):

  - how often the next message in the room is a reply from someone else within
    the window, split by done=false vs done=true;
  - of the done=false messages with no quick reply, how many contain a question
    and how many read like a closer or report.

This is the evidence for whether `done=false` can stand in for "waiting on an
answer". On 2026-10-06 (before `done` became required) it was 66% vs 35%, with
roughly 93 of 236 unanswered done=false messages reading like closers: too noisy
to infer waits from. Re-run it after the flag has been required for a while.

Usage: contrib/measure-done-flag.py [http://host:7777] [--window-hours 2] [--since YYYY-MM-DD]
"""

import argparse
import datetime
import json
import re
import urllib.parse
import urllib.request

PAGE = 1000
CLOSER = re.compile(
    r"\b(thanks|thank you|confirmed|done|landed|published|accepted|verified|complete|green|ack)\b",
    re.I,
)


def get(base, path):
    with urllib.request.urlopen(base + path, timeout=20) as r:
        return json.load(r)


def transcript(base, room):
    msgs, before = [], None
    while True:
        q = {"limit": PAGE}
        if before is not None:
            q["before"] = before
        page = get(base, f"/api/rooms/{urllib.parse.quote(room, safe='')}/messages?" + urllib.parse.urlencode(q))
        if not page:
            break
        msgs = page + msgs
        before = min(m["id"] for m in page)
        if len(page) < PAGE:
            break
    return sorted(msgs, key=lambda m: m["id"])


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("bus", nargs="?", default="http://127.0.0.1:7777")
    ap.add_argument("--window-hours", type=float, default=2.0)
    ap.add_argument("--since", help="only count messages sent on or after this date (YYYY-MM-DD)")
    args = ap.parse_args()
    base = args.bus.rstrip("/")
    window = args.window_hours * 3600 * 1000
    since = 0
    if args.since:
        since = int(datetime.datetime.fromisoformat(args.since).timestamp() * 1000)

    humans = {a["name"] for a in get(base, "/api/agents") if a.get("isHuman")}
    counts = {(done, quick): 0 for done in (False, True) for quick in (False, True)}
    unanswered = []
    for room in get(base, "/api/rail")["rooms"]:
        ms = transcript(base, room["name"])
        for i, m in enumerate(ms):
            if m["from"] in humans or m["createdAt"] < since:
                continue
            nxt = ms[i + 1] if i + 1 < len(ms) else None
            quick = nxt is not None and nxt["from"] != m["from"] and nxt["createdAt"] - m["createdAt"] <= window
            counts[(m["done"], quick)] += 1
            if not m["done"] and not quick:
                unanswered.append(m)

    for done in (False, True):
        total = counts[(done, True)] + counts[(done, False)]
        pct = 100 * counts[(done, True)] / total if total else 0
        print(f"done={str(done).lower():5}: {total:5} msgs; next message is a reply within "
              f"{args.window_hours:g}h: {counts[(done, True)]} ({pct:.0f}%)")
    agent_total = sum(counts.values())
    if agent_total:
        print(f"done=true share of agent messages: "
              f"{100 * (counts[(True, True)] + counts[(True, False)]) / agent_total:.0f}%")
    questions = sum(1 for m in unanswered if "?" in m["body"])
    closers = sum(1 for m in unanswered if CLOSER.search(m["body"][:200]) and "?" not in m["body"][:400])
    print(f"done=false with no quick reply: {len(unanswered)}; contain a '?': {questions}; "
          f"read like a closer/report: {closers}")


if __name__ == "__main__":
    main()
