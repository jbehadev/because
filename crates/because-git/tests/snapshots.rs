//! Snapshots against real temporary repositories.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use because_git::Repo;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("because-git-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "init"]);
    dir
}

fn short(s: &str) -> String {
    s[..7].to_string()
}

#[test]
fn snapshots_leave_head_the_index_and_the_working_tree_alone() {
    let dir = repo("untouched");
    std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\n").unwrap(); // unstaged
    std::fs::write(dir.join("b.txt"), "staged\n").unwrap();
    git(&dir, &["add", "b.txt"]); // staged
    std::fs::write(dir.join("c.txt"), "untracked\n").unwrap();
    let head = git(&dir, &["rev-parse", "HEAD"]);
    let status = git(&dir, &["status", "--porcelain"]);
    let staged = git(&dir, &["diff", "--cached", "--name-only"]);

    let r = Repo::discover(&dir).unwrap();
    let sha = r.snapshot(None, "before input 1", "refs/because/t/1-before").unwrap();

    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head, "HEAD moved");
    assert_eq!(git(&dir, &["status", "--porcelain"]), status, "working tree or index changed");
    assert_eq!(git(&dir, &["diff", "--cached", "--name-only"]), staged, "staging area changed");
    let files = git(&dir, &["ls-tree", "-r", "--name-only", &sha]);
    for f in ["a.txt", "b.txt", "c.txt"] {
        assert!(files.lines().any(|l| l == f), "{f} missing from snapshot: {files}");
    }
    assert_eq!(git(&dir, &["rev-parse", "refs/because/t/1-before"]).trim(), sha);
}

#[test]
fn ignored_files_are_left_out() {
    let dir = repo("ignored");
    std::fs::create_dir_all(dir.join("target")).unwrap();
    std::fs::write(dir.join("target/big.bin"), "build output").unwrap();
    let r = Repo::discover(&dir).unwrap();
    let sha = r.snapshot(None, "s", "refs/because/t/s").unwrap();
    assert!(!git(&dir, &["ls-tree", "-r", "--name-only", &sha]).contains("target/"));
}

#[test]
fn changes_between_snapshots_cover_added_and_modified_files() {
    let dir = repo("changes");
    let r = Repo::discover(&dir).unwrap();
    let before = r.snapshot(None, "before", "refs/because/t/1-before").unwrap();
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    std::fs::write(dir.join("hello.txt"), "hi\n").unwrap();
    let after = r.snapshot(Some(&before), "after", "refs/because/t/1-after").unwrap();
    let keep: HashSet<String> = [short(&after)].into();
    let changes = r.changes(&before, &after, &keep).unwrap();
    let by_path: Vec<(&str, &str, u32, u32)> =
        changes.iter().map(|c| (c.path.as_str(), c.change_type.as_str(), c.additions, c.deletions)).collect();
    assert_eq!(by_path, [("a.txt", "modify", 1, 0), ("hello.txt", "add", 1, 0)]);
    let a = &changes[0];
    assert_eq!(a.content, ["one", "two", "three", "four"]);
    assert_eq!(a.line_origins.keys().copied().collect::<Vec<_>>(), [4], "only the added line is the input's");
}

#[test]
fn edits_made_between_inputs_are_not_attributed_to_either_input() {
    let dir = repo("between");
    let r = Repo::discover(&dir).unwrap();
    let b1 = r.snapshot(None, "before 1", "refs/because/t/1-before").unwrap();
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\nagent-1\n").unwrap();
    let a1 = r.snapshot(Some(&b1), "after 1", "refs/because/t/1-after").unwrap();
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\nagent-1\nby hand\n").unwrap(); // developer, between inputs
    let b2 = r.snapshot(Some(&a1), "before 2", "refs/because/t/2-before").unwrap();
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\nagent-1\nby hand\nagent-2\n").unwrap();
    let a2 = r.snapshot(Some(&b2), "after 2", "refs/because/t/2-after").unwrap();

    let keep: HashSet<String> = [short(&a1), short(&a2)].into();
    let changes = r.changes(&b2, &a2, &keep).unwrap();
    assert_eq!(changes.len(), 1);
    let o = &changes[0].line_origins;
    assert_eq!(o.get(&4).map(|v| v.0.as_str()), Some(short(&a1).as_str()), "line 4: input 1");
    assert_eq!(o.get(&5), None, "line 5 was typed by hand between inputs");
    assert_eq!(o.get(&6).map(|v| v.0.as_str()), Some(short(&a2).as_str()), "line 6: input 2");
    assert_eq!(changes[0].additions, 1, "the diff for input 2 excludes the hand edit");
}

#[test]
fn deleted_files_have_no_content_to_show() {
    let dir = repo("delete");
    let r = Repo::discover(&dir).unwrap();
    let before = r.snapshot(None, "b", "refs/because/t/b").unwrap();
    std::fs::remove_file(dir.join("a.txt")).unwrap();
    let after = r.snapshot(Some(&before), "a", "refs/because/t/a").unwrap();
    let c = r.changes(&before, &after, &HashSet::new()).unwrap();
    assert_eq!((c[0].change_type.as_str(), c[0].deletions, c[0].content.len()), ("delete", 3, 0));
}

#[test]
fn a_session_worktree_starts_from_the_tree_as_it_is_now_and_snapshots_on_its_own() {
    let dir = repo("worktree");
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\nnot committed\n").unwrap(); // work in progress
    let main = Repo::discover(&dir).unwrap();
    // A worktree left by an earlier run of this test is not a failure of this one.
    let _ = std::fs::remove_dir_all(main.worktrees_dir());
    git(&dir, &["worktree", "prune"]);
    let base = main.snapshot(None, "session 2 starts here", "refs/because/w/base").unwrap();
    let wt = main.worktree("2-abcdef", &base, "because/session-2-abcdef").unwrap();

    assert_ne!(wt.root, main.root, "a tree of its own");
    assert_eq!(std::fs::read_to_string(wt.root.join("a.txt")).unwrap(), "one\ntwo\nthree\nnot committed\n");
    assert_eq!(wt.branch(), "because/session-2-abcdef");
    assert_ne!(wt.data_dir(), main.data_dir(), "and a private index of its own, so two sessions never share one");

    // What each session's agent does is only in that session's snapshots.
    std::fs::write(wt.root.join("a.txt"), "one\ntwo\nthree\nnot committed\nsession 2\n").unwrap();
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\nnot committed\nsession 1\n").unwrap();
    let after = wt.snapshot(Some(&base), "after input 1", "refs/because/w/1-after").unwrap();
    let keep: HashSet<String> = [short(&after)].into();
    let changes = wt.changes(&base, &after, &keep).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].content, ["one", "two", "three", "not committed", "session 2"]);
    assert_eq!(changes[0].additions, 1, "the other session's edit is not in this one's diff");

    git(&dir, &["worktree", "remove", "--force", &wt.root.to_string_lossy()]);
}

#[test]
fn a_repo_with_no_commits_can_be_snapshotted() {
    let dir = std::env::temp_dir().join(format!("because-git-test-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("x.txt"), "x\n").unwrap();
    let r = Repo::discover(&dir).unwrap();
    assert!(r.snapshot(None, "s", "refs/because/t/s").is_ok());
}

fn commit_at(dir: &Path, msg: &str, epoch: i64) -> String {
    let date = format!("@{epoch} +0000");
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", msg])
        .env("GIT_COMMITTER_DATE", &date)
        .env("GIT_AUTHOR_DATE", &date)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

#[test]
fn commits_are_listed_by_time_and_snapshots_are_not_among_them() {
    let dir = repo("commit-times");
    let r = Repo::discover(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n2\nthree\n").unwrap();
    let c1 = commit_at(&dir, "first", 2_000_000_000);
    std::fs::write(dir.join("a.txt"), "one\n2\n3\n").unwrap();
    let c2 = commit_at(&dir, "second", 2_000_000_600);
    r.snapshot(Some(&c2), "a snapshot", "refs/because/x/1-after").unwrap();
    let all = r.commits_between(1_999_999_000_000, 2_000_001_000_000).unwrap();
    let shas: Vec<&str> = all.iter().map(|c| c.sha.as_str()).collect();
    assert_eq!(shas, [c1.as_str(), c2.as_str()], "oldest first, and no snapshot");
    assert_eq!(all[1].parent, c1);
    assert_eq!((all[0].subject.as_str(), all[0].time_ms), ("first", 2_000_000_000_000));
    let first_only = r.commits_between(2_000_000_000_000, 2_000_000_300_000).unwrap();
    assert_eq!(first_only.len(), 1);
}

#[test]
fn change_stats_lists_files_without_content() {
    let dir = repo("stats");
    let r = Repo::discover(&dir).unwrap();
    let base = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    std::fs::write(dir.join("b.txt"), "new\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "two files"]);
    let head = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    let got = r.change_stats(&base, &head).unwrap();
    let rows: Vec<(&str, &str, u32, u32)> =
        got.iter().map(|c| (c.path.as_str(), c.change_type.as_str(), c.additions, c.deletions)).collect();
    assert_eq!(rows, [("a.txt", "modify", 2, 1), ("b.txt", "add", 1, 0)]);
    assert!(got.iter().all(|c| c.content.is_empty() && c.hunks.is_empty()));
    // A root commit is measured against the empty tree.
    let root = git(&dir, &["rev-list", "--max-parents=0", "HEAD"]).trim().to_string();
    assert_eq!(r.change_stats("", &root).unwrap().len(), 2);
    assert_eq!(r.changes("", &root, &HashSet::new()).unwrap().len(), 2);
}

#[test]
fn every_worktree_shares_one_store() {
    let dir = repo("store-path");
    let r = Repo::discover(&dir).unwrap();
    let head = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    let wt = r.worktree("w1", &head, "because-test-w1").unwrap();
    assert_eq!(wt.store_path(), r.store_path());
    assert!(r.store_path().starts_with(dir.canonicalize().unwrap().join(".git")), "{}", r.store_path().display());
    assert_eq!(wt.main_root().canonicalize().unwrap(), dir.canonicalize().unwrap());
    assert_ne!(wt.data_dir(), r.data_dir(), "each tree keeps its own private index");
}

#[test]
fn uncommitted_counts_files_in_every_tree_and_ignores_what_git_ignores() {
    let dir = repo("uncommitted");
    let main = Repo::discover(&dir).unwrap();
    let _ = std::fs::remove_dir_all(main.worktrees_dir());
    git(&dir, &["worktree", "prune"]);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    let wt = main.worktree("u1", head.trim(), "because-test-u1").unwrap();

    std::fs::write(dir.join("a.txt"), "changed\n").unwrap(); // modified
    std::fs::write(dir.join("new.txt"), "new\n").unwrap(); // untracked
    std::fs::create_dir_all(dir.join("target")).unwrap();
    std::fs::write(dir.join("target/out"), "build\n").unwrap(); // ignored
    git(&wt.root, &["mv", "a.txt", "renamed.txt"]); // one file, two paths

    let n = main.uncommitted().unwrap();
    assert_eq!(n.get(&main.root.display().to_string()), Some(&2), "{n:?}");
    assert_eq!(n.get(&wt.root.display().to_string()), Some(&1), "{n:?}");

    git(&dir, &["worktree", "remove", "--force", &wt.root.to_string_lossy()]);
}
