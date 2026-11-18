#!/usr/bin/env python3
"""Build fixtures/ripgrep.json from a ripgrep clone plus fixtures/authored.json.

Real, derived from git:   diffs, hunk ranges, post-image content, commit
                          metadata, and per-line origins (git blame).
Authored, from the JSON:  inputs, summaries, sections, narration, risks,
                          timings, costs.

Usage:
    git clone --depth 150 https://github.com/BurntSushi/ripgrep /tmp/ripgrep
    python3 fixtures/build_fixture.py --repo /tmp/ripgrep

Every authored hunk reference ("path#n") is checked against the extracted
hunks, so a change in diff context size fails here instead of silently
attaching sections to the wrong code.
"""

import argparse
import json
import pathlib
import re
import subprocess
import sys

CONTEXT = 3            # -U3; authored hunk refs depend on this
WHOLE_FILE_MAX = 400   # include whole post-image files up to this many lines
WINDOW = 25            # otherwise, this many lines either side of the hunks


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True,
                          text=True, check=True).stdout


def parse_diff(text):
    files, cur = [], None
    for line in text.split("\n"):
        m = re.match(r"^diff --git a/(.*) b/(.*)$", line)
        if m:
            cur = {"path": m.group(2), "hunks": []}
            files.append(cur)
            continue
        m = re.match(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$", line)
        if m and cur is not None:
            cur["hunks"].append({
                "old_start": int(m.group(1)), "old_lines": int(m.group(2) or 1),
                "new_start": int(m.group(3)), "new_lines": int(m.group(4) or 1),
                "header": m.group(5).strip(), "patch": [line]})
            continue
        if cur is not None and cur["hunks"] and (line[:1] in (" ", "+", "-", "\\") or line == ""):
            cur["hunks"][-1]["patch"].append(line)
    for f in files:
        for h in f["hunks"]:
            h["patch"] = "\n".join(h["patch"]).rstrip("\n")
    return files


def blame_origins(repo, sha, path, keep):
    """final line -> (short sha, line number in that commit's version).

    Only lines whose origin is one of `keep` are returned: provenance outside
    the session is not this fixture's business.
    """
    out, cur = {}, None
    for line in git(repo, "blame", "--porcelain", sha, "--", path).split("\n"):
        m = re.match(r"^([0-9a-f]{40}) (\d+) (\d+)", line)
        if m:
            cur = (m.group(1)[:7], int(m.group(2)), int(m.group(3)))
        elif line.startswith("\t") and cur:
            short, orig, final = cur
            if short in keep:
                out[final] = [short, orig]
    return out


def build(repo, authored):
    out = {"sessions": []}
    for s in authored["sessions"]:
        keep = {g["commit"] for g in s["segments"] if g["commit"]}
        segments = []
        for i, g in enumerate(s["segments"]):
            seg = {"id": f'{s["id"]}-{i + 1}', "seq": i + 1, "input": g["input"],
                   "summary": g["summary"], "elapsed": g["elapsed"],
                   "cost_usd": g["cost_usd"], "model": "claude-opus-5",
                   "commit": None, "sections": [], "changes": []}
            refs = {}
            if g["commit"]:
                sha = git(repo, "rev-parse", g["commit"]).strip()
                seg["commit"] = {"sha": sha, "short": sha[:7],
                                 "subject": git(repo, "log", "-1", "--format=%s", sha).strip(),
                                 "author": git(repo, "log", "-1", "--format=%an", sha).strip(),
                                 "committed_at": git(repo, "log", "-1", "--format=%cI", sha).strip()}
                files = parse_diff(git(repo, "show", "--format=", f"-U{CONTEXT}", sha))
                for fi, f in enumerate(files):
                    cid = f'{sha[:7]}-f{fi}'
                    blob = git(repo, "show", f"{sha}:{f['path']}").split("\n")
                    if len(blob) <= WHOLE_FILE_MAX:
                        lo, hi = 1, len(blob)
                    else:
                        lo = max(1, min(h["new_start"] for h in f["hunks"]) - WINDOW)
                        hi = min(len(blob), max(h["new_start"] + h["new_lines"] for h in f["hunks"]) + WINDOW)
                    origins = blame_origins(repo, sha, f["path"], keep)
                    origins = {str(k): v for k, v in origins.items() if lo <= k <= hi}
                    hunks = []
                    for hi_, h in enumerate(f["hunks"]):
                        refs[f'{f["path"]}#{hi_}'] = (cid, hi_)
                        hunks.append({"seq": hi_, **{k: h[k] for k in ("header", "old_start", "old_lines", "new_start", "new_lines", "patch")},
                                      "summary": g.get("hunk_summaries", {}).get(f'{f["path"]}#{hi_}', "")})
                    patch_lines = [l for h in f["hunks"] for l in h["patch"].split("\n")[1:]]
                    seg["changes"].append({
                        "id": cid, "path": f["path"], "change_type": "modify",
                        "additions": sum(1 for l in patch_lines if l.startswith("+")),
                        "deletions": sum(1 for l in patch_lines if l.startswith("-")),
                        "risk": g.get("risks", {}).get(f["path"], "none"),
                        "summary": g.get("file_summaries", {}).get(f["path"], ""),
                        "state": "ok", "content_start_line": lo, "content": blob[lo - 1:hi],
                        "hunks": hunks, "line_origins": origins})
            for si, sec in enumerate(g["sections"]):
                hunk_refs = []
                for r in sec["hunks"]:
                    if r not in refs:
                        sys.exit(f'{g["commit"]}: authored hunk ref {r!r} not found; '
                                 f'extracted: {sorted(refs)}')
                    cid, hs = refs[r]
                    hunk_refs.append({"change_id": cid, "hunk_seq": hs})
                seg["sections"].append({"seq": si, "title": sec["title"], "intent": sec["intent"],
                                        "summary": sec["summary"], "confidence": sec["confidence"],
                                        "narration": [{"kind": k, "body": b} for k, b in sec["narration"]],
                                        "hunk_refs": hunk_refs})
            segments.append(seg)
        out["sessions"].append({"id": s["id"], "title": s["title"], "repo": authored["repo"],
                                "branch": s["branch"], "segments": segments})
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True, help="path to a ripgrep clone")
    here = pathlib.Path(__file__).parent
    ap.add_argument("--authored", default=str(here / "authored.json"))
    ap.add_argument("--out", default=str(here / "ripgrep.json"))
    a = ap.parse_args()
    fx = build(a.repo, json.loads(pathlib.Path(a.authored).read_text()))
    pathlib.Path(a.out).write_text(json.dumps(fx, indent=1, ensure_ascii=False) + "\n")
    for s in fx["sessions"]:
        print(f'{s["id"]:16s}', " · ".join(
            f'{g["seq"]}:{(g["commit"] or {}).get("short", "question")}'
            f'({len(g["changes"])}f/{sum(len(c["line_origins"]) for c in g["changes"])}o)'
            for g in s["segments"]))


if __name__ == "__main__":
    main()
