#!/usr/bin/env python3
"""Record what the Claude Code CLI says when a tool call needs approval.

Writes fixtures/claude-stream/permission.jsonl: one input that runs two Bash
commands in a scratch directory, with `--permission-prompt-tool stdio`. The
first request is allowed, the second denied, so the recording holds both
answers and what the agent does with each. Costs about a cent (haiku).

    python3 scripts/record_permission.py
"""

import json
import os
import subprocess
import sys
import tempfile

from scrub_fixture import scrub

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "fixtures", "claude-stream", "permission.jsonl")
claude = sys.argv[1] if len(sys.argv) > 1 else "claude"

scratch = tempfile.mkdtemp(prefix="because-perm-")
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


out = open(OUT, "w")
send({"type": "user", "message": {"role": "user", "content":
      "Use the Bash tool to run exactly: echo hello-approval. "
      "Then use the Bash tool to run exactly: echo second-one. Then reply: done."}})
asked = 0
for line in p.stdout:
    out.write(line)
    out.flush()
    e = json.loads(line)
    kind = e.get("type")
    if kind and kind.startswith("control"):
        print("CTRL", line.strip()[:1500])
    if kind == "control_request" and e["request"].get("subtype") == "can_use_tool":
        asked += 1
        if asked == 1:
            answer = {"behavior": "allow", "updatedInput": e["request"]["input"]}
        else:
            answer = {"behavior": "deny", "message": "Denied in `because`."}
        send({"type": "control_response",
              "response": {"subtype": "success", "request_id": e["request_id"], "response": answer}})
    if kind == "user":
        print("USER", line.strip()[:600])
    if kind == "result":
        print("RESULT", line.strip()[:800])
        break
p.stdin.close()
p.kill()
err = p.stderr.read()
if err.strip():
    print("STDERR", err[:2000])
out.close()
scrub(OUT)
print("wrote", OUT)
