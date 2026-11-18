#!/usr/bin/env python3
"""Record what the Claude Code CLI says when the agent asks the developer a question.

Writes fixtures/claude-stream/question.jsonl: one input that makes the agent
call AskUserQuestion with two questions, the second a multi-select, with
`--permission-prompt-tool stdio`. The answer fills `updatedInput.answers`, as
`because` does. The agent then repeats the answers it got, so the
recording shows that they arrived. Costs about a cent (haiku).

    python3 scripts/record_question.py
"""

import json
import os
import subprocess
import sys
import tempfile

from scrub_fixture import scrub

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "fixtures", "claude-stream", "question.jsonl")
claude = sys.argv[1] if len(sys.argv) > 1 else "claude"

scratch = tempfile.mkdtemp(prefix="because-question-")
p = subprocess.Popen(
    [claude, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
     "--include-partial-messages", "--permission-prompt-tool", "stdio", "--permission-mode", "acceptEdits",
     "--no-session-persistence", "--model", "haiku"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=scratch)


def send(o):
    line = json.dumps(o)
    out.write("# sent: " + line + "\n")
    p.stdin.write(line + "\n")
    p.stdin.flush()


out = open(OUT, "w")
send({"type": "user", "message": {"role": "user", "content":
      "Use the AskUserQuestion tool once, with two questions. "
      "Question 1: \"Which color?\", header \"Color\", options \"Red\" and \"Blue\", single select. "
      "Question 2: \"Which sizes?\", header \"Sizes\", options \"Small\", \"Medium\" and \"Large\", multi select. "
      "Then reply with the answers you got, exactly as the tool gave them."}})
for line in p.stdout:
    out.write(line)
    out.flush()
    e = json.loads(line)
    kind = e.get("type")
    if kind and kind.startswith("control"):
        print("CTRL", line.strip()[:1500])
    if kind == "control_request" and e["request"].get("subtype") == "can_use_tool":
        inp = e["request"]["input"]
        answers = {}
        for q in inp.get("questions", []):
            if q.get("multiSelect"):
                answers[q["question"]] = ", ".join(o["label"] for o in q["options"][:2])
            else:
                answers[q["question"]] = q["options"][-1]["label"]
        answer = {"behavior": "allow", "updatedInput": dict(inp, answers=answers)}
        send({"type": "control_response",
              "response": {"subtype": "success", "request_id": e["request_id"], "response": answer}})
    if kind == "user":
        print("USER", line.strip()[:800])
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
