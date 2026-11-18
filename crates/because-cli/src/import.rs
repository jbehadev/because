//! Reading Claude Code's transcripts for this repository into the store, so
//! sessions run in Claude Code itself show up as history.
//!
//! Each transcript is one session. It is read again only when it has changed
//! since the last import (size or modification time), and a session `cuz live`
//! ran itself is skipped: the store already has it, from the inside.
//!
//! A transcript does not say which files an input changed — a shell command's
//! edits leave no trace in it — so each input is tied to the commits made while
//! it ran, on any branch. That is a guess when a person commits by hand
//! mid-input, and the UI says so (`Segment::link = "commit_time"`).

use std::path::{Path, PathBuf};

use because_core::Commit;
use because_git::Repo;
use because_store::{Source, Store};

/// A commit made this long after an input's last recorded activity still
/// counts as that input's: the record of a tool's result can be written a
/// moment before the commit it reports is timestamped.
const SLACK_MS: i64 = 2_000;

#[derive(Debug, Default)]
pub struct Report {
    pub dir: Option<PathBuf>,
    pub imported: usize,
    pub inputs: usize,
    pub linked: usize,
    pub unchanged: usize,
    pub hosted: usize,
    pub empty: usize,
    pub failed: Vec<String>,
}

impl Report {
    /// One line for the status bar, when there is something to say.
    pub fn notice(&self) -> Option<String> {
        match (self.imported, self.failed.len()) {
            (0, 0) => None,
            (n, 0) => Some(format!(
                "Read {n} Claude Code session{} into history — {} input{}.",
                if n == 1 { "" } else { "s" },
                self.inputs,
                if self.inputs == 1 { "" } else { "s" }
            )),
            (_, f) => Some(format!("{f} Claude Code transcript(s) could not be read; `cuz import-claude` says why.")),
        }
    }
}

use because_agent::claude::transcript::{projects_dir, slug};

/// Import what is new or changed. `say` hears about progress worth printing.
pub fn run(repo: &Repo, store: &mut Store, say: impl FnMut(&str)) -> Report {
    match projects_dir() {
        Some(projects) => run_in(&projects, repo, store, say),
        None => Report::default(),
    }
}

fn run_in(projects: &Path, repo: &Repo, store: &mut Store, mut say: impl FnMut(&str)) -> Report {
    let mut report = Report::default();
    let dir = projects.join(slug(&repo.main_root()));
    report.dir = Some(dir.clone());
    let Ok(entries) = std::fs::read_dir(&dir) else { return report };
    let mut files: Vec<(PathBuf, Source)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| {
            let meta = std::fs::metadata(&p).ok()?;
            let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
            let source = Source { path: p.display().to_string(), size: meta.len(), mtime };
            Some((p, source))
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut todo = Vec::new();
    for (path, source) in files {
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match store.origin_of(&id) {
            Ok(Some(o)) if o == "hosted" => report.hosted += 1,
            _ if store.source_of(&id).ok().flatten().is_some_and(|s| s.size == source.size && s.mtime == source.mtime) => {
                report.unchanged += 1
            }
            _ => todo.push((path, source)),
        }
    }
    if !todo.is_empty() {
        say(&format!(
            "Reading {} Claude Code transcript{} from {} …",
            todo.len(),
            if todo.len() == 1 { "" } else { "s" },
            dir.display()
        ));
    }
    for (path, source) in todo {
        match import_one(repo, store, &path, &source) {
            Ok(None) => report.empty += 1,
            Ok(Some((inputs, linked))) => {
                report.imported += 1;
                report.inputs += inputs;
                report.linked += linked;
            }
            Err(e) => report.failed.push(format!("{}: {e}", path.display())),
        }
    }
    report
}

/// `Ok(None)` for a transcript with no typed input in it.
fn import_one(repo: &Repo, store: &mut Store, path: &Path, source: &Source) -> anyhow::Result<Option<(usize, usize)>> {
    let text = std::fs::read_to_string(path)?;
    let Some(t) = because_agent::claude::transcript::parse(&text) else { return Ok(None) };
    let mut session = t.session;
    if session.id.is_empty() {
        session.id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    }
    // Commits made while each input ran, from one listing of the whole span.
    let span = (t.windows.first().map(|w| w.0).unwrap_or(0), t.windows.iter().map(|w| w.1).max().unwrap_or(0));
    let commits = repo.commits_between(span.0, span.1 + SLACK_MS).unwrap_or_default();
    let mut extra: Vec<(u32, Commit)> = Vec::new();
    let mut linked = 0;
    for (seg, (start, end)) in session.segments.iter_mut().zip(&t.windows) {
        let mine: Vec<&because_git::CommitInfo> =
            commits.iter().filter(|c| c.time_ms >= *start && c.time_ms <= end + SLACK_MS).collect();
        let (Some(first), Some(last)) = (mine.first(), mine.last()) else { continue };
        let commit = |c: &because_git::CommitInfo, parent: &str| Commit {
            sha: c.sha.clone(),
            short: c.sha[..7.min(c.sha.len())].to_string(),
            parent: parent.to_string(),
            subject: c.subject.clone(),
            author: c.author.clone(),
            committed_at: because_core::time::format_ms(c.time_ms),
        };
        seg.changes = repo.change_stats(&first.parent, &last.sha).unwrap_or_default();
        seg.commit = Some(commit(last, &first.parent));
        seg.link = "commit_time".into();
        seg.code_unloaded = !seg.changes.is_empty();
        for c in &mine[..mine.len() - 1] {
            extra.push((seg.seq, commit(c, &c.parent)));
        }
        linked += 1;
    }
    let inputs = session.segments.len();
    store.replace_imported(&session, source)?;
    for (seq, c) in extra {
        store.add_commit(&session.id, seq, &c)?;
    }
    Ok(Some((inputs, linked)))
}

/// `cuz import-claude`: the same import `cuz live` does at start, with a report.
pub fn command() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = Repo::discover(&cwd).map_err(|e| anyhow::anyhow!("`cuz import-claude` runs inside a git repository: {e}"))?;
    let mut store = Store::open(&repo.store_path())?;
    let r = run(&repo, &mut store, |l| println!("{l}"));
    let Some(dir) = &r.dir else {
        anyhow::bail!("neither CLAUDE_CONFIG_DIR nor HOME is set, so there is nowhere to look for transcripts");
    };
    if !dir.is_dir() {
        println!("No Claude Code transcripts for this repository: {} does not exist.", dir.display());
        println!("Claude Code names that directory after the path it was started in; sessions started in a");
        println!("subdirectory of the repository are kept under that subdirectory's name and are not read.");
        return Ok(());
    }
    println!("Claude Code history for {}", repo.main_root().display());
    println!("  from        {}", dir.display());
    println!("  into        {}", repo.store_path().display());
    println!("  imported    {} session(s), {} input(s); {} input(s) tied to commits made while they ran", r.imported, r.inputs, r.linked);
    println!("  unchanged   {} (read before, not modified since)", r.unchanged);
    println!("  skipped     {} run by cuz live (already stored), {} with no typed input", r.hosted, r.empty);
    for f in &r.failed {
        println!("  FAILED      {f}");
    }
    println!();
    println!("Claude Code deletes transcripts after 30 days unless `cleanupPeriodDays` is raised in its");
    println!("settings; what has been imported stays in the store regardless.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str], epoch: Option<i64>) -> String {
        let mut c = Command::new("git");
        c.arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args);
        if let Some(e) = epoch {
            c.env("GIT_COMMITTER_DATE", format!("@{e} +0000")).env("GIT_AUTHOR_DATE", format!("@{e} +0000"));
        }
        let out = c.output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// The fixture's two inputs ran 17:02:54–17:02:57.5 and 17:02:58.9–17:03:02.5
    /// on 2026-10-02 (UTC). Commits are made at chosen times around them.
    #[test]
    fn inputs_are_tied_to_the_commits_made_while_they_ran() {
        let root = std::env::temp_dir().join(format!("because-import-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo_dir = root.join("repo");
        std::fs::create_dir_all(&repo_dir).unwrap();
        let at = |hms: &str| because_core::time::parse_ms(&format!("2026-10-02T{hms}Z")).unwrap() / 1000;
        git(&repo_dir, &["init", "-q"], None);
        std::fs::write(repo_dir.join("README.md"), "x\n").unwrap();
        git(&repo_dir, &["add", "-A"], None);
        git(&repo_dir, &["commit", "-qm", "before the session"], Some(at("16:00:00")));
        std::fs::write(repo_dir.join("notes.txt"), "alpha\n").unwrap();
        git(&repo_dir, &["add", "-A"], None);
        git(&repo_dir, &["commit", "-qm", "add notes"], Some(at("17:02:56")));
        std::fs::write(repo_dir.join("notes.txt"), "alpha\nbeta\n").unwrap();
        git(&repo_dir, &["commit", "-qam", "append beta"], Some(at("17:03:01")));
        std::fs::write(repo_dir.join("notes.txt"), "alpha\nbeta\ngamma\n").unwrap();
        git(&repo_dir, &["commit", "-qam", "by hand, later"], Some(at("17:30:00")));

        let repo = Repo::discover(&repo_dir).unwrap();
        let projects = root.join("projects");
        let dir = projects.join(slug(&repo.main_root()));
        std::fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("8062b589-4cab-4e08-86e4-484ac35ec56e.jsonl");
        std::fs::copy("../../fixtures/claude-transcript/two-inputs.jsonl", &transcript).unwrap();

        let mut store = Store::open_in_memory().unwrap();
        let r = run_in(&projects, &repo, &mut store, |_| {});
        assert_eq!((r.imported, r.inputs, r.linked), (1, 2, 2), "{r:?}");
        let s = &store.load_sessions().unwrap()[0];
        let (a, b) = (&s.segments[0], &s.segments[1]);
        assert_eq!(a.commit.as_ref().unwrap().subject, "add notes");
        assert_eq!(b.commit.as_ref().unwrap().subject, "append beta", "not the commit made by hand later");
        assert_eq!((a.link.as_str(), a.changes.len(), a.changes[0].path.as_str()), ("commit_time", 1, "notes.txt"));
        assert_eq!((b.changes[0].additions, b.changes[0].deletions), (1, 0));
        // The code view's base: input 2 is measured from input 1's commit.
        assert_eq!(b.commit.as_ref().unwrap().parent, a.commit.as_ref().unwrap().sha);

        // Unchanged: not read again. Grown: read again, replacing.
        let r = run_in(&projects, &repo, &mut store, |_| {});
        assert_eq!((r.imported, r.unchanged), (0, 1));
        let mut text = std::fs::read_to_string("../../fixtures/claude-transcript/two-inputs.jsonl").unwrap();
        text.push_str(r#"{"type":"ai-title","aiTitle":"Notes","sessionId":"8062b589-4cab-4e08-86e4-484ac35ec56e"}"#);
        text.push('\n');
        std::fs::write(&transcript, text).unwrap();
        let r = run_in(&projects, &repo, &mut store, |_| {});
        assert_eq!(r.imported, 1);
        let all = store.load_sessions().unwrap();
        assert_eq!((all.len(), all[0].segments.len(), all[0].title.as_str()), (1, 2, "Notes"));

        // A session cuz live ran itself is already stored, from the inside.
        let hosted = because_core::Session { id: "hosted-1".into(), repo: "/r".into(), ..Default::default() };
        store.save_session(&hosted, None).unwrap();
        std::fs::copy(&transcript, dir.join("hosted-1.jsonl")).unwrap();
        let r = run_in(&projects, &repo, &mut store, |_| {});
        assert_eq!((r.hosted, r.imported), (1, 0));
        let _ = std::fs::remove_dir_all(&root);
    }
}
