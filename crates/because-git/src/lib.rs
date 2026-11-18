//! Git plumbing for live sessions: snapshot the working tree around each input
//! and turn the difference into `FileChange`s with per-line origins.
//!
//! Snapshots are commit objects built from a private index and kept alive by
//! refs under `refs/because/`. They never touch HEAD, the branch, the
//! user's index or the working tree. Because they are ordinary commits, the
//! same diff and blame logic as `fixtures/build_fixture.py` applies to them.
//!
//! Each input gets a snapshot before it is sent and one after it finishes, so
//! edits the developer makes between inputs land in a "before" commit and are
//! not attributed to the agent.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use because_core::{FileChange, Hunk, Risk};

/// Files up to this many lines are shown whole in the code pane; larger ones
/// are windowed around their hunks.
const WHOLE_FILE_MAX: usize = 3000;
const WINDOW: u32 = 40;

#[derive(Debug, Clone)]
pub struct Repo {
    pub root: PathBuf,
    git_dir: PathBuf,
    /// The git directory every worktree of this repository shares.
    common_dir: PathBuf,
}

/// The tree of nothing, for diffing a root commit against.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// A commit as `commits_between` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub sha: String,
    /// First parent; empty for a root commit.
    pub parent: String,
    /// Committer time, milliseconds since the epoch.
    pub time_ms: i64,
    pub author: String,
    pub subject: String,
}

impl Repo {
    pub fn discover(dir: &Path) -> Result<Self> {
        let out = run(dir, &["rev-parse", "--show-toplevel", "--absolute-git-dir", "--git-common-dir"], &[])
            .context("not inside a git repository")?;
        let mut lines = out.lines();
        let root = PathBuf::from(lines.next().unwrap_or_default());
        let git_dir = PathBuf::from(lines.next().unwrap_or_default());
        // Relative to where git ran when it is not absolute.
        let common = PathBuf::from(lines.next().unwrap_or_default());
        let common_dir = if common.is_absolute() { common } else { dir.join(common) };
        let common_dir = common_dir.canonicalize().unwrap_or(common_dir);
        Ok(Self { root, git_dir, common_dir })
    }

    /// The store's file: one per repository, shared by all its worktrees, in
    /// the git directory so it is never part of a working tree or a snapshot.
    pub fn store_path(&self) -> PathBuf {
        self.common_dir.join("because").join("slicer.db")
    }

    /// The repository's main working tree — the one the common git directory
    /// belongs to — even when this `Repo` is a linked worktree of it.
    pub fn main_root(&self) -> PathBuf {
        match self.common_dir.file_name() {
            Some(n) if n == ".git" => self.common_dir.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone()),
            _ => self.root.clone(),
        }
    }

    /// Commits made between two times (committer time, milliseconds), on any
    /// branch, tag or remote — not `because`'s own snapshots, not stashes.
    /// Oldest first.
    pub fn commits_between(&self, since_ms: i64, until_ms: i64) -> Result<Vec<CommitInfo>> {
        let out = self.git(&[
            "log",
            "--exclude=refs/because/*",
            "--exclude=refs/stash",
            "--all",
            "--reverse",
            &format!("--since=@{}", since_ms.div_euclid(1000)),
            &format!("--until=@{}", until_ms.div_euclid(1000) + 1),
            "--format=%H%x1f%P%x1f%ct%x1f%an%x1f%s%x1e",
        ])?;
        Ok(out
            .split('\x1e')
            .filter_map(|rec| {
                let f: Vec<&str> = rec.trim().split('\x1f').collect();
                let [sha, parents, ct, author, subject] = f[..] else { return None };
                let time_ms = ct.parse::<i64>().ok()? * 1000;
                (time_ms >= since_ms && time_ms <= until_ms).then(|| CommitInfo {
                    sha: sha.to_string(),
                    parent: parents.split(' ').next().unwrap_or_default().to_string(),
                    time_ms,
                    author: author.to_string(),
                    subject: subject.to_string(),
                })
            })
            .collect())
    }

    /// Paths and line counts between two commits, without content or blame:
    /// what a list of changed files needs, and cheap enough to run over a whole
    /// history.
    pub fn change_stats(&self, before: &str, after: &str) -> Result<Vec<FileChange>> {
        let base = if before.is_empty() { EMPTY_TREE } else { before };
        let status = self.git(&["diff", "--name-status", "-z", "--no-renames", base, after])?;
        let mut kinds = std::collections::HashMap::new();
        let mut f = status.split('\0').filter(|s| !s.is_empty());
        while let (Some(st), Some(path)) = (f.next(), f.next()) {
            kinds.insert(path.to_string(), st.to_string());
        }
        let numstat = self.git(&["diff", "--numstat", "-z", "--no-renames", base, after])?;
        let short = &after[..7.min(after.len())];
        let mut out = Vec::new();
        for rec in numstat.split('\0').filter(|s| !s.is_empty()) {
            let mut parts = rec.splitn(3, '\t');
            let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
            let binary = a == "-";
            let change_type = match kinds.get(path).map(String::as_str) {
                _ if binary => "binary",
                Some("A") => "add",
                Some("D") => "delete",
                Some("T") => "type",
                _ => "modify",
            };
            out.push(FileChange {
                id: format!("{short}-f{}", out.len()),
                path: path.to_string(),
                change_type: change_type.into(),
                additions: a.parse().unwrap_or(0),
                deletions: d.parse().unwrap_or(0),
                risk: Risk::None,
                summary: String::new(),
                state: "unsummarized".into(),
                content_start_line: 1,
                content: Vec::new(),
                hunks: Vec::new(),
                line_origins: BTreeMap::new(),
            });
        }
        Ok(out)
    }

    /// `because`'s own files for this repository (private index, logs).
    /// Inside the git directory, so they are never part of the working tree.
    /// For a linked worktree this is that worktree's own git directory, which
    /// is what keeps two sessions from writing one private index.
    pub fn data_dir(&self) -> PathBuf {
        self.git_dir.join("because")
    }

    /// Where this repository's session worktrees live: one directory beside the
    /// repository holding all of them. Beside and not inside, because a
    /// worktree under the working tree would show up in the repository's own
    /// status and in every snapshot taken of it.
    pub fn worktrees_dir(&self) -> PathBuf {
        let name = self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".into());
        match self.root.parent() {
            Some(p) => p.join(format!("{name}.sessions")),
            None => std::env::temp_dir().join("because-sessions"),
        }
    }

    /// A working tree of this repository's own, for a session that has to run
    /// at the same time as another. Two agents editing one checkout would make
    /// the before/after snapshots — the whole account of who changed what —
    /// meaningless, so each session after the first gets a tree to itself.
    ///
    /// `base` is a commit the new tree starts from. Passing a snapshot of the
    /// main working tree carries edits that were never committed into it, so
    /// the session starts from what is on screen rather than from HEAD. What it
    /// does not carry is ignored files — build output, `.env` — because a
    /// snapshot leaves those out, as `git add -A` would.
    ///
    /// The tree shares the object database and the refs: snapshots taken in it
    /// are visible from the repository it came from, and `git worktree list`
    /// has it.
    pub fn worktree(&self, name: &str, base: &str, branch: &str) -> Result<Repo> {
        let dir = self.worktrees_dir().join(name);
        if dir.exists() {
            bail!("{} already exists", dir.display());
        }
        std::fs::create_dir_all(self.worktrees_dir())?;
        let dir = dir.to_string_lossy().into_owned();
        self.git(&["worktree", "add", "--quiet", "-b", branch, &dir, base])?;
        Repo::discover(Path::new(&dir))
    }

    /// Every working tree of this repository — the main one and each linked
    /// worktree — with how many files in it have changes not yet committed:
    /// modified, staged, or untracked and not ignored, as `git status` counts
    /// them. A tree git cannot read (removed by hand, say) is left out.
    pub fn uncommitted(&self) -> Result<BTreeMap<String, usize>> {
        let list = self.git(&["worktree", "list", "--porcelain"])?;
        Ok(list
            .lines()
            .filter_map(|l| l.strip_prefix("worktree "))
            .filter_map(|dir| {
                let st = run(Path::new(dir), &["status", "--porcelain", "-z"], &[]).ok()?;
                // `-z` puts a rename's old path in a field of its own, after
                // the entry; it is the same file, not another one.
                let mut n = 0;
                let mut f = st.split('\0').filter(|s| !s.is_empty());
                while let Some(e) = f.next() {
                    n += 1;
                    if e.starts_with('R') || e.starts_with('C') {
                        f.next();
                    }
                }
                Some((dir.to_string(), n))
            })
            .collect())
    }

    pub fn delete_ref(&self, refname: &str) -> Result<()> {
        self.git(&["update-ref", "-d", refname]).map(|_| ())
    }

    pub fn branch(&self) -> String {
        run(&self.root, &["rev-parse", "--abbrev-ref", "HEAD"], &[])
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "(no commits yet)".into())
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        run(&self.root, args, &[])
    }

    /// Record the working tree as a commit object and point `refname` at it.
    /// Ignored files are left out, as `git add -A` would.
    pub fn snapshot(&self, parent: Option<&str>, message: &str, refname: &str) -> Result<String> {
        let dir = self.data_dir();
        std::fs::create_dir_all(&dir)?;
        let index = dir.join("index");
        // Start from a copy of the real index so `add -A` only re-hashes files
        // whose stat data changed. The real index is read, never written.
        let real = self.git_dir.join("index");
        if real.exists() {
            std::fs::copy(&real, &index)?;
        } else {
            let _ = std::fs::remove_file(&index);
        }
        let idx = index.to_string_lossy().to_string();
        let env = [("GIT_INDEX_FILE", idx.as_str())];
        run(&self.root, &["add", "-A"], &env)?;
        let tree = run(&self.root, &["write-tree"], &env)?.trim().to_string();
        let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
        if let Some(p) = parent {
            args.extend(["-p", p]);
        }
        let ident = [
            ("GIT_AUTHOR_NAME", "because"),
            ("GIT_AUTHOR_EMAIL", "because@localhost"),
            ("GIT_COMMITTER_NAME", "because"),
            ("GIT_COMMITTER_EMAIL", "because@localhost"),
        ];
        let sha = run(&self.root, &args, &ident)?.trim().to_string();
        self.git(&["update-ref", refname, &sha])?;
        Ok(sha)
    }

    /// The files that changed between two snapshots, with hunks, post-image
    /// content and per-line origins. `keep` holds the short shas whose lines
    /// count as written by an input; everything else is not attributed.
    pub fn changes(&self, before: &str, after: &str, keep: &HashSet<String>) -> Result<Vec<FileChange>> {
        let before = if before.is_empty() { EMPTY_TREE } else { before };
        let short = &after[..7.min(after.len())];
        let listing = self.git(&["diff", "--name-status", "-z", "--no-renames", before, after])?;
        let mut fields = listing.split('\0').filter(|s| !s.is_empty());
        let mut out = Vec::new();
        while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
            let change_type = match status {
                "A" => "add",
                "D" => "delete",
                "M" => "modify",
                "T" => "type",
                _ => "modify",
            };
            let patch = self.git(&["diff", "--no-color", "--no-ext-diff", "-U3", before, after, "--", path])?;
            let binary = patch.lines().any(|l| l.starts_with("Binary files"));
            let hunks = parse_hunks(&patch);
            let (additions, deletions) = hunks.iter().fold((0, 0), |(a, d), h| {
                let lines = h.lines();
                (a + lines.added.len() as u32, d + lines.deleted_at.values().map(Vec::len).sum::<usize>() as u32)
            });
            let (start, content, line_origins) = if binary || change_type == "delete" {
                (1, Vec::new(), BTreeMap::new())
            } else {
                let text = self.git(&["show", &format!("{after}:{path}")])?;
                let all: Vec<String> = text.lines().map(String::from).collect();
                let (lo, hi) = if all.len() <= WHOLE_FILE_MAX {
                    (1, all.len() as u32)
                } else {
                    let first = hunks.iter().map(|h| h.new_start).min().unwrap_or(1);
                    let last = hunks.iter().map(|h| h.new_start + h.new_lines).max().unwrap_or(1);
                    (first.saturating_sub(WINDOW).max(1), (last + WINDOW).min(all.len() as u32))
                };
                let origins = self
                    .blame_origins(after, path, keep)?
                    .into_iter()
                    .filter(|(l, _)| *l >= lo && *l <= hi)
                    .collect();
                (lo, all[(lo as usize - 1).min(all.len())..(hi as usize).min(all.len())].to_vec(), origins)
            };
            out.push(FileChange {
                id: format!("{short}-f{}", out.len()),
                path: path.to_string(),
                change_type: if binary { "binary".into() } else { change_type.into() },
                additions,
                deletions,
                risk: Risk::None,
                summary: String::new(),
                state: "unsummarized".into(),
                content_start_line: start,
                content,
                hunks,
                line_origins,
            });
        }
        Ok(out)
    }

    /// Post-image line → (short sha, line in that commit), for lines whose
    /// origin is one of `keep`.
    pub fn blame_origins(&self, commit: &str, path: &str, keep: &HashSet<String>) -> Result<BTreeMap<u32, (String, u32)>> {
        let out = self.git(&["blame", "--porcelain", commit, "--", path])?;
        let mut map = BTreeMap::new();
        let mut cur: Option<(String, u32, u32)> = None;
        for line in out.lines() {
            if line.starts_with('\t') {
                if let Some((sha, orig, fin)) = &cur
                    && keep.contains(sha)
                {
                    map.insert(*fin, (sha.clone(), *orig));
                }
                continue;
            }
            let mut parts = line.split(' ');
            if let (Some(sha), Some(orig), Some(fin)) = (parts.next(), parts.next(), parts.next())
                && sha.len() == 40
                && sha.bytes().all(|b| b.is_ascii_hexdigit())
                && let (Ok(o), Ok(f)) = (orig.parse(), fin.parse())
            {
                cur = Some((sha[..7].to_string(), o, f));
            }
        }
        Ok(map)
    }
}

/// Parse the `@@` hunks of a single-file unified diff.
pub fn parse_hunks(patch: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut body: Vec<&str> = Vec::new();
    let flush = |hunks: &mut Vec<Hunk>, body: &mut Vec<&str>| {
        if let Some(h) = hunks.last_mut() {
            h.patch = body.join("\n");
        }
        body.clear();
    };
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("@@ -") {
            flush(&mut hunks, &mut body);
            let (ranges, header) = rest.split_once(" @@").unwrap_or((rest, ""));
            let (old, new) = ranges.split_once(" +").unwrap_or((ranges, "0"));
            let range = |r: &str| -> (u32, u32) {
                let (a, b) = r.split_once(',').unwrap_or((r, "1"));
                (a.parse().unwrap_or(0), b.parse().unwrap_or(1))
            };
            let ((old_start, old_lines), (new_start, new_lines)) = (range(old), range(new));
            hunks.push(Hunk {
                seq: hunks.len() as u32,
                header: header.trim().to_string(),
                old_start,
                old_lines,
                new_start,
                new_lines,
                summary: String::new(),
                patch: String::new(),
            });
            body.push(line);
        } else if !hunks.is_empty() && (line.starts_with([' ', '+', '-', '\\']) || line.is_empty()) {
            body.push(line);
        }
    }
    flush(&mut hunks, &mut body);
    hunks
}

fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .envs(env.iter().copied())
        .output()
        .context("could not run git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
