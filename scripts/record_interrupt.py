#!/usr/bin/env python3
"""Record what the Claude Code CLI says when a running input is interrupted.

Writes fixtures/claude-stream/interrupt.jsonl, from one process:

1. A long input, interrupted when the first text arrives.
2. An input that runs a Bash command, interrupted while the call waits for
   approval.
3. A short input, to show that the process still takes inputs.

The recording shows the answer to each interrupt and the `result` that ends
each input. Costs about a cent (haiku).

    python3 scripts/record_interrupt.py
"""

import json
import os
import subprocess
import sys
import tempfile

from scrub_fixture import scrub

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "fixtures", "claude-stream", "interrupt.jsonl")
claude = sys.argv[1] if len(sys.argv) > 1 else "claude"

scratch = tempfile.mkdtemp(prefix="because-int-")
p = subprocess.Popen(
    [claude, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
     "--include-partial-messages", "--permission-prompt-tool", "stdio", "--permission-mode", "default",
     "--no-session-persistence", "--model", "haiku"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=scratch)


def send(o):
    line = json.dumps(o)
    out.write("# sent: " + line + "\n")
    p.stdin.write(line + "\n")
    p.stdin.flush()


INPUTS = [
    "Write a 1500-word essay about the history of the printing press.",
    "Use the Bash tool to run exactly: touch waits-for-approval.txt. Then reply: done.",
    "Reply with exactly: still here.",
]

out = open(OUT, "w")
send({"type": "user", "message": {"role": "user", "content": INPUTS[0]}})
results = 0
interrupted = 0
for line in p.stdout:
    out.write(line)
    out.flush()
    e = json.loads(line)
    kind = e.get("type")
    if kind and kind.startswith("control"):
        print("CTRL", line.strip()[:800])
    first_text = kind == "stream_event" and e["event"].get("type") == "content_block_delta"
    asks = kind == "control_request" and e["request"].get("subtype") == "can_use_tool"
    if interrupted == results and ((results == 0 and first_text) or (results == 1 and asks)):
        interrupted += 1
        send({"type": "control_request", "request_id": f"because-interrupt-{interrupted}", "request": {"subtype": "interrupt"}})
    if kind == "result":
        print("RESULT", line.strip()[:300])
        results += 1
        if results == len(INPUTS):
            break
        send({"type": "user", "message": {"role": "user", "content": INPUTS[results]}})
p.stdin.close()
p.kill()
err = p.stderr.read()
if err.strip():
    print("STDERR", err[:2000])
out.close()
scrub(OUT)
print("wrote", OUT)
