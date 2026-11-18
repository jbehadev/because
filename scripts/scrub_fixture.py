#!/usr/bin/env python3
"""Remove private data from a recorded Claude Code stream fixture.

The record_*.py scripts call this on each file that they write. Run it on a
fixture by hand as follows:

    python3 scripts/scrub_fixture.py fixtures/claude-stream/<name>.jsonl

The rules are the same as the rules in fixtures/README.md:

- Replace the home directory with /home/user.
- Replace the scratch directory with /repo.
- Apply these rules also to the dash-joined directory form of Claude Code.
- Cut each list in the `init` event to three entries. Plugin entries and
  namespaced entries ("plugin:skill") are removed first, because they identify
  the machine.

The script fails if a private string remains after the scrub.
"""

import json
import os
import re
import sys

HOME = os.path.expanduser("~")
DASH_HOME = HOME.replace("/", "-")
USER = os.path.basename(HOME)

# A scratch directory from tempfile.mkdtemp(): macOS /var/folders, or /tmp.
SCRATCH = re.compile(r"(?:/private)?/var/folders/[^/\"]+/[^/\"]+/T/[^/\"\\]+|/tmp/because-[^/\"\\]+")
DASH_SCRATCH = re.compile(r"-(?:private-)?var-folders-[^/\"\\]+|-tmp-because-[^/\"\\]+")
INIT_LISTS = ("tools", "slash_commands", "skills", "agents", "terminal_slash_commands", "plugins")


def scrub_text(s):
    s = SCRATCH.sub("/repo", s)
    s = DASH_SCRATCH.sub("-repo", s)
    s = s.replace(HOME, "/home/user").replace(DASH_HOME, "-home-user")
    return s


def trim(items):
    if all(isinstance(i, str) for i in items):
        kept = [i for i in items if ":" not in i]
    else:
        kept = []
    if len(kept) > 3 or len(kept) < len(items):
        return kept[:3] + ["…trimmed"]
    return kept


def scrub_line(line):
    if line.startswith("#"):
        return scrub_text(line)
    e = json.loads(line)
    if e.get("type") == "system" and e.get("subtype") == "init":
        for key in INIT_LISTS:
            if isinstance(e.get(key), list):
                e[key] = trim(e[key])
    return scrub_text(json.dumps(e, ensure_ascii=False, separators=(",", ":"))) + "\n"


def scrub(path):
    with open(path) as f:
        lines = [scrub_line(l) if l.strip() else l for l in f]
    text = "".join(lines)
    left = [p for p in (HOME, USER, "/var/folders") if p and p in text]
    if left:
        sys.exit(f"{path}: private strings remain after the scrub: {left}")
    with open(path, "w") as f:
        f.write(text)


if __name__ == "__main__":
    for p in sys.argv[1:]:
        scrub(p)
        print("scrubbed", p)
