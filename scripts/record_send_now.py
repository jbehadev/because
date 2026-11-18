#!/usr/bin/env python3
"""Record what the Claude Code CLI does with an input sent while it works.

Writes fixtures/claude-stream/send-now.jsonl, from one process with
`--replay-user-messages`:

1. An input that runs two Bash commands. A second input is sent at the first
   tool call. The CLI reads it at a tool boundary: its echo comes before the
   one `result`.
2. An input that only writes text. A second input is sent at the first text.
   No tool boundary remains: its echo comes after the `result`, and it gets a
   `result` of its own.

Costs about a cent (haiku).

    python3 scripts/record_send_now.py
"""

import json
import os
import subprocess
import sys
import tempfile

from scrub_fixture import scrub

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "fixtures", "claude-stream", "send-now.jsonl")
claude = sys.argv[1] if len(sys.argv) > 1 else "claude"

scratch = tempfile.mkdtemp(prefix="because-now-")
p = subprocess.Popen(
    [claude, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
     "--include-partial-messages", "--replay-user-messages", "--permission-prompt-tool", "stdio",
     "--permission-mode", "default", "--no-session-persistence", "--model", "haiku"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=scratch)


def send(text):
    line = json.dumps({"type": "user", "message": {"role": "user", "content": text}})
    out.write("# sent: " + line + "\n")
    p.stdin.write(line + "\n")
    p.stdin.flush()


CASES = [
    ("Use the Bash tool to run exactly: sleep 4; echo one. "
     "Then use the Bash tool to run exactly: sleep 4; echo two. Then reply with a short summary.",
     "Also: end your final reply with the word PINEAPPLE.",
     lambda e: e.get("type") == "assistant" and any(b.get("type") == "tool_use" for b in e["message"]["content"]),
     1),
    ("Write a 300-word paragraph about bees.",
     "Also: end your final reply with the word MANGO.",
     lambda e: e.get("type") == "stream_event" and e["event"].get("delta", {}).get("type") == "text_delta",
     2),
]

out = open(OUT, "w")
for first, late, when, results in CASES:
    send(first)
    sent = False
    seen = 0
    for line in p.stdout:
        out.write(line)
        out.flush()
        e = json.loads(line)
        if e.get("type") == "control_request" and e["request"].get("subtype") == "can_use_tool":
            reply = {"type": "control_response", "response": {"subtype": "success", "request_id": e["request_id"],
                     "response": {"behavior": "allow", "updatedInput": e["request"]["input"]}}}
            out.write("# sent: " + json.dumps(reply) + "\n")
            p.stdin.write(json.dumps(reply) + "\n")
            p.stdin.flush()
        if not sent and when(e):
            sent = True
            send(late)
        if e.get("type") == "user" and e.get("isReplay"):
            print("ECHO", json.dumps(e["message"]["content"])[:80])
        if e.get("type") == "result":
            print("RESULT", json.dumps(e.get("result", ""))[-60:])
            seen += 1
            if seen == results:
                break
p.stdin.close()
p.kill()
out.close()
scrub(OUT)
print("wrote", OUT)
