//! The local store: SQLite, one file per repository.
//!
//! It keeps what cannot be recovered from anywhere else — what was typed, what
//! the agent said and did, what it cost, what was generated about it — and
//! points at git for the code. A file change is a row of paths, counts and
//! summaries; its hunks' text and the code around them are re-derived from the
//! commits the row names (`docs/design/01-data-model.md` §4: point at git,
//! don't copy it).
//!
//! It lives in the repository's git directory (`.git/because/slicer.db`),
//! not the working tree: a database in the working tree would be swept into
//! every snapshot, and one in the git directory is shared by all of the
//! repository's worktrees, which is where `cuz live` runs its extra sessions.
//!
//! The schema is a subset of `01-data-model.md` §5 — the tables `cuz live` and
//! the transcript importer write today — with text stored inline rather than as
//! blobs. Tables for sections, jobs and search arrive with the code that fills
//! them, as later versions in the ladder below.

use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use because_core::{
    Commit, FileChange, Hunk, NarrationStep, Origin, Risk, SegStatus, Section, Segment, Session, StepKind, time,
};

/// Each entry takes the schema from the previous version to the next. Never
/// edit one after it has shipped; add another.
const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE sessions (
        id            TEXT PRIMARY KEY,      -- Claude Code's session id
        origin        TEXT NOT NULL,         -- hosted | imported
        workdir       TEXT NOT NULL,         -- where the agent worked: the repository or a session worktree
        branch        TEXT,
        title         TEXT,                  -- a title it came with (imported: Claude Code's own)
        started_at    TEXT NOT NULL,
        updated_at    TEXT NOT NULL,
        recorded_cost REAL,                  -- the session total, when only that was recorded
        source_path   TEXT,                  -- the transcript an imported session was read from
        source_size   INTEGER,               -- and its size and mtime then, to re-read only what changed
        source_mtime  INTEGER
    );

    CREATE TABLE segments (
        id               TEXT PRIMARY KEY,   -- <session id>:<seq>
        session_id       TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        seq              INTEGER NOT NULL,
        input            TEXT NOT NULL,
        reply            TEXT NOT NULL DEFAULT '',
        model            TEXT NOT NULL,
        status           TEXT NOT NULL,      -- running | done | failed
        started_at       TEXT NOT NULL,
        duration_ms      INTEGER NOT NULL DEFAULT 0,
        cost_usd         REAL,               -- NULL when only a total covering several inputs was recorded
        summary          TEXT NOT NULL DEFAULT '',
        summary_by       TEXT NOT NULL DEFAULT '',
        summary_cost_usd REAL NOT NULL DEFAULT 0,
        commit_sha       TEXT,               -- after-snapshot, or the last commit matched to the input
        link             TEXT NOT NULL DEFAULT '',  -- '' (snapshots) | commit_time
        UNIQUE (session_id, seq)
    );

    -- What the agent said and did, in order: the raw material sections are cut from.
    CREATE TABLE steps (
        segment_id  TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
        seq         INTEGER NOT NULL,
        kind        TEXT NOT NULL,           -- text | thinking | tool_call
        body        TEXT NOT NULL,
        PRIMARY KEY (segment_id, seq)
    );

    CREATE TABLE commits (
        sha           TEXT PRIMARY KEY,
        parent_sha    TEXT,                  -- what the input's changes are measured against
        author        TEXT,
        committed_at  TEXT,
        subject       TEXT,
        segment_id    TEXT REFERENCES segments(id) ON DELETE SET NULL
    );

    CREATE TABLE file_changes (
        id             TEXT PRIMARY KEY,
        segment_id     TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
        seq            INTEGER NOT NULL,
        commit_sha     TEXT,
        path           TEXT NOT NULL,
        change_type    TEXT NOT NULL,
        additions      INTEGER NOT NULL DEFAULT 0,
        deletions      INTEGER NOT NULL DEFAULT 0,
        patch_ref      TEXT,                 -- git:<base>..<sha>:<path>
        summary        TEXT NOT NULL DEFAULT '',
        risk           TEXT NOT NULL DEFAULT 'none',
        summary_state  TEXT NOT NULL DEFAULT 'unsummarized',
        UNIQUE (segment_id, seq)
    );
    CREATE INDEX file_changes_path ON file_changes(path);

    CREATE TABLE hunks (
        file_change_id  TEXT NOT NULL REFERENCES file_changes(id) ON DELETE CASCADE,
        seq             INTEGER NOT NULL,
        old_start       INTEGER NOT NULL,
        old_lines       INTEGER NOT NULL,
        new_start       INTEGER NOT NULL,
        new_lines       INTEGER NOT NULL,
        header          TEXT NOT NULL DEFAULT '',
        summary         TEXT NOT NULL DEFAULT '',
        PRIMARY KEY (file_change_id, seq)
    );
    "#,
];

pub struct Store {
    conn: Connection,
}

/// What the store knows about an imported transcript, to skip it when it has
/// not changed since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
}

impl Store {
    /// Open (creating if needed) and bring the schema up to date.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut s = Self { conn };
        s.migrate()?;
        Ok(s)
    }

    pub fn schema_version(&self) -> Result<usize> {
        Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize)
    }

    fn migrate(&mut self) -> Result<()> {
        let have = self.schema_version()?;
        if have > MIGRATIONS.len() {
            bail!(
                "this store is schema version {have}, newer than this cuz understands ({}). \
                 It is a cache: delete it to start over, or use a newer cuz.",
                MIGRATIONS.len()
            );
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(have) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql).with_context(|| format!("schema migration to v{}", i + 1))?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Write a session's own row. Its inputs are written by `save_segment`.
    pub fn save_session(&self, s: &Session, source: Option<&Source>) -> Result<()> {
        let origin = if s.origin == Origin::Imported { "imported" } else { "hosted" };
        let started = if s.started_at.is_empty() { time::now_rfc3339() } else { s.started_at.clone() };
        let updated = s.segments.iter().map(|x| x.started_at.as_str()).filter(|t| !t.is_empty()).max().unwrap_or(&started);
        self.conn.execute(
            "INSERT INTO sessions (id, origin, workdir, branch, title, started_at, updated_at, recorded_cost,
                                   source_path, source_size, source_mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                workdir = excluded.workdir, branch = excluded.branch, title = excluded.title,
                updated_at = excluded.updated_at, recorded_cost = excluded.recorded_cost,
                source_path = excluded.source_path, source_size = excluded.source_size,
                source_mtime = excluded.source_mtime",
            params![
                s.id,
                origin,
                s.repo,
                s.branch,
                s.title,
                started,
                updated,
                s.recorded_cost,
                source.map(|x| x.path.clone()),
                source.map(|x| x.size as i64),
                source.map(|x| x.mtime),
            ],
        )?;
        Ok(())
    }

    /// Write one input and everything under it, replacing what was there. An
    /// input is small and is rewritten whole at each point it changes —
    /// sent, finished, its changes computed, its summary written.
    pub fn save_segment(&mut self, session_id: &str, seg: &Segment) -> Result<()> {
        let id = segment_id(session_id, seg.seq);
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM file_changes WHERE segment_id = ?1", [&id])?;
        tx.execute("DELETE FROM steps WHERE segment_id = ?1", [&id])?;
        tx.execute("UPDATE commits SET segment_id = NULL WHERE segment_id = ?1", [&id])?;
        tx.execute(
            "INSERT INTO segments (id, session_id, seq, input, reply, model, status, started_at, duration_ms,
                                   cost_usd, summary, summary_by, summary_cost_usd, commit_sha, link)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO UPDATE SET
                input = excluded.input, reply = excluded.reply, model = excluded.model,
                status = excluded.status, started_at = excluded.started_at,
                duration_ms = excluded.duration_ms, cost_usd = excluded.cost_usd,
                summary = excluded.summary, summary_by = excluded.summary_by,
                summary_cost_usd = excluded.summary_cost_usd, commit_sha = excluded.commit_sha,
                link = excluded.link",
            params![
                id,
                session_id,
                seg.seq,
                seg.input,
                seg.reply,
                seg.model,
                status_str(seg.status),
                if seg.started_at.is_empty() { time::now_rfc3339() } else { seg.started_at.clone() },
                seg.duration_ms as i64,
                (!seg.cost_unrecorded).then_some(seg.cost_usd),
                seg.summary,
                seg.summary_by,
                seg.summary_cost_usd,
                seg.commit.as_ref().map(|c| c.sha.clone()),
                seg.link,
            ],
        )?;
        let steps = seg.sections.iter().flat_map(|s| &s.narration);
        for (i, step) in steps.enumerate() {
            tx.execute(
                "INSERT INTO steps (segment_id, seq, kind, body) VALUES (?1, ?2, ?3, ?4)",
                params![id, i as i64, kind_str(step.kind), step.body],
            )?;
        }
        if let Some(c) = &seg.commit {
            tx.execute(
                "INSERT INTO commits (sha, parent_sha, author, committed_at, subject, segment_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(sha) DO UPDATE SET parent_sha = excluded.parent_sha, segment_id = excluded.segment_id",
                params![c.sha, c.parent, c.author, c.committed_at, c.subject, id],
            )?;
        }
        for (i, fc) in seg.changes.iter().enumerate() {
            let fc_id = format!("{id}:f{i}");
            let patch_ref = seg.commit.as_ref().map(|c| format!("git:{}..{}:{}", c.parent, c.sha, fc.path));
            tx.execute(
                "INSERT INTO file_changes (id, segment_id, seq, commit_sha, path, change_type, additions,
                                           deletions, patch_ref, summary, risk, summary_state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    fc_id,
                    id,
                    i as i64,
                    seg.commit.as_ref().map(|c| c.sha.clone()),
                    fc.path,
                    fc.change_type,
                    fc.additions,
                    fc.deletions,
                    patch_ref,
                    fc.summary,
                    risk_str(fc.risk),
                    fc.state,
                ],
            )?;
            for h in &fc.hunks {
                tx.execute(
                    "INSERT INTO hunks (file_change_id, seq, old_start, old_lines, new_start, new_lines, header, summary)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![fc_id, h.seq, h.old_start, h.old_lines, h.new_start, h.new_lines, h.header, h.summary],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Replace an imported session wholesale: its transcript is the truth, and
    /// re-reading it gives every input again.
    pub fn replace_imported(&mut self, s: &Session, source: &Source) -> Result<()> {
        self.conn.execute("DELETE FROM segments WHERE session_id = ?1", [&s.id])?;
        self.save_session(s, Some(source))?;
        for seg in &s.segments {
            self.save_segment(&s.id, seg)?;
        }
        Ok(())
    }

    /// How a session got here, if it is here: `hosted` or `imported`.
    pub fn origin_of(&self, id: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT origin FROM sessions WHERE id = ?1", [id], |r| r.get(0)).optional()?)
    }

    /// The transcript an imported session was last read from.
    pub fn source_of(&self, id: &str) -> Result<Option<Source>> {
        Ok(self
            .conn
            .query_row(
                "SELECT source_path, source_size, source_mtime FROM sessions WHERE id = ?1 AND source_path IS NOT NULL",
                [id],
                |r| Ok(Source { path: r.get(0)?, size: r.get::<_, i64>(1)? as u64, mtime: r.get(2)? }),
            )
            .optional()?)
    }

    /// Every stored session with at least one input, most recently active
    /// first. File changes come without their code (`code_unloaded`), and an
    /// input that was still running when its run of `cuz live` ended is marked
    /// as failed, since nothing will finish it now.
    pub fn load_sessions(&self) -> Result<Vec<Session>> {
        let mut q = self.conn.prepare(
            "SELECT id, origin, workdir, branch, title, started_at, recorded_cost FROM sessions
             WHERE EXISTS (SELECT 1 FROM segments WHERE session_id = sessions.id)
             ORDER BY updated_at DESC",
        )?;
        let rows = q.query_map([], |r| {
            let origin: String = r.get(1)?;
            Ok(Session {
                id: r.get(0)?,
                origin: if origin == "imported" { Origin::Imported } else { Origin::Stored },
                repo: r.get(2)?,
                branch: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                title: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                started_at: r.get(5)?,
                recorded_cost: r.get(6)?,
                segments: Vec::new(),
            })
        })?;
        let mut out = Vec::new();
        for s in rows {
            let mut s = s?;
            s.segments = self.load_segments(&s.id)?;
            out.push(s);
        }
        Ok(out)
    }

    fn load_segments(&self, session_id: &str) -> Result<Vec<Segment>> {
        let mut q = self.conn.prepare(
            "SELECT s.id, s.seq, s.input, s.reply, s.model, s.status, s.started_at, s.duration_ms, s.cost_usd,
                    s.summary, s.summary_by, s.summary_cost_usd, s.link,
                    c.sha, c.parent_sha, c.author, c.committed_at, c.subject
             FROM segments s LEFT JOIN commits c ON c.sha = s.commit_sha
             WHERE s.session_id = ?1 ORDER BY s.seq",
        )?;
        let rows = q.query_map([session_id], |r| {
            let status: String = r.get(5)?;
            let cost: Option<f64> = r.get(8)?;
            let duration_ms = r.get::<_, i64>(7)? as u64;
            let sha: Option<String> = r.get(13)?;
            let commit = sha.map(|sha| -> rusqlite::Result<Commit> {
                Ok(Commit {
                    short: sha[..7.min(sha.len())].to_string(),
                    sha,
                    parent: r.get::<_, Option<String>>(14)?.unwrap_or_default(),
                    author: r.get::<_, Option<String>>(15)?.unwrap_or_default(),
                    committed_at: r.get::<_, Option<String>>(16)?.unwrap_or_default(),
                    subject: r.get::<_, Option<String>>(17)?.unwrap_or_default(),
                })
            });
            Ok((
                r.get::<_, String>(0)?,
                Segment {
                    seq: r.get(1)?,
                    input: r.get(2)?,
                    reply: r.get(3)?,
                    model: r.get(4)?,
                    status: match status.as_str() {
                        "done" => SegStatus::Done,
                        _ => SegStatus::Failed,
                    },
                    started_at: r.get(6)?,
                    duration_ms,
                    elapsed: if duration_ms > 0 { time::fmt_duration(duration_ms) } else { String::new() },
                    cost_usd: cost.unwrap_or(0.0),
                    cost_unrecorded: cost.is_none(),
                    summary: r.get(9)?,
                    summary_by: r.get(10)?,
                    summary_cost_usd: r.get(11)?,
                    link: r.get(12)?,
                    commit: commit.transpose()?,
                    ..Default::default()
                },
                status == "running",
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, mut seg, interrupted) = row?;
            seg.id = id.clone();
            if interrupted && seg.reply.is_empty() {
                seg.reply = "ts live ended while this input was running; it did not finish here.".into();
            }
            seg.sections = vec![Section::whole_response(self.load_steps(&id)?)];
            seg.changes = self.load_changes(&id)?;
            seg.code_unloaded = !seg.changes.is_empty();
            out.push(seg);
        }
        Ok(out)
    }

    fn load_steps(&self, segment_id: &str) -> Result<Vec<NarrationStep>> {
        let mut q = self.conn.prepare("SELECT kind, body FROM steps WHERE segment_id = ?1 ORDER BY seq")?;
        let rows = q.query_map([segment_id], |r| {
            let kind: String = r.get(0)?;
            Ok(NarrationStep { kind: kind_from(&kind), body: r.get(1)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn load_changes(&self, segment_id: &str) -> Result<Vec<FileChange>> {
        let mut q = self.conn.prepare(
            "SELECT id, path, change_type, additions, deletions, summary, risk, summary_state
             FROM file_changes WHERE segment_id = ?1 ORDER BY seq",
        )?;
        let rows = q.query_map([segment_id], |r| {
            let risk: String = r.get(6)?;
            Ok(FileChange {
                id: r.get(0)?,
                path: r.get(1)?,
                change_type: r.get(2)?,
                additions: r.get(3)?,
                deletions: r.get(4)?,
                summary: r.get(5)?,
                risk: risk_from(&risk),
                state: r.get(7)?,
                content_start_line: 1,
                content: Vec::new(),
                hunks: Vec::new(),
                line_origins: Default::default(),
            })
        })?;
        let mut out: Vec<FileChange> = rows.collect::<rusqlite::Result<_>>()?;
        let mut q = self.conn.prepare(
            "SELECT seq, old_start, old_lines, new_start, new_lines, header, summary
             FROM hunks WHERE file_change_id = ?1 ORDER BY seq",
        )?;
        for fc in &mut out {
            fc.hunks = q
                .query_map([&fc.id], |r| {
                    Ok(Hunk {
                        seq: r.get(0)?,
                        old_start: r.get(1)?,
                        old_lines: r.get(2)?,
                        new_start: r.get(3)?,
                        new_lines: r.get(4)?,
                        header: r.get(5)?,
                        summary: r.get(6)?,
                        patch: String::new(),
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(out)
    }

    /// Short shas of every commit tied to one of a session's inputs, each with
    /// the input's own commit. Blame is restricted to these so that a line
    /// counts as the session's only when one of its inputs wrote it; an input
    /// matched to several commits answers for all of them.
    pub fn session_commits(&self, session_id: &str) -> Result<Vec<(String, String)>> {
        let mut q = self.conn.prepare(
            "SELECT c.sha, s.commit_sha FROM commits c JOIN segments s ON s.id = c.segment_id
             WHERE s.session_id = ?1",
        )?;
        let rows = q.query_map([session_id], |r| {
            let sha: String = r.get(0)?;
            let own: Option<String> = r.get(1)?;
            Ok((sha[..7.min(sha.len())].to_string(), own.map(|o| o[..7.min(o.len())].to_string()).unwrap_or_default()))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Record the further commits matched to an imported input, beyond the one
    /// on the segment row itself.
    pub fn add_commit(&self, session_id: &str, seq: u32, c: &Commit) -> Result<()> {
        self.conn.execute(
            "INSERT INTO commits (sha, parent_sha, author, committed_at, subject, segment_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(sha) DO UPDATE SET segment_id = excluded.segment_id",
            params![c.sha, c.parent, c.author, c.committed_at, c.subject, segment_id(session_id, seq)],
        )?;
        Ok(())
    }
}

pub fn segment_id(session_id: &str, seq: u32) -> String {
    format!("{session_id}:{seq}")
}

fn status_str(s: SegStatus) -> &'static str {
    match s {
        SegStatus::Running => "running",
        SegStatus::Done => "done",
        SegStatus::Failed => "failed",
    }
}

fn kind_str(k: StepKind) -> &'static str {
    match k {
        StepKind::Text => "text",
        StepKind::Thinking => "thinking",
        StepKind::ToolCall => "tool_call",
    }
}

fn kind_from(s: &str) -> StepKind {
    match s {
        "thinking" => StepKind::Thinking,
        "tool_call" => StepKind::ToolCall,
        _ => StepKind::Text,
    }
}

fn risk_str(r: Risk) -> &'static str {
    match r {
        Risk::None => "none",
        Risk::Behavioral => "behavioral",
        Risk::Api => "api",
        Risk::Security => "security",
        Risk::Perf => "perf",
        Risk::Data => "data",
    }
}

fn risk_from(s: &str) -> Risk {
    match s {
        "behavioral" => Risk::Behavioral,
        "api" => Risk::Api,
        "security" => Risk::Security,
        "perf" => Risk::Perf,
        "data" => Risk::Data,
        _ => Risk::None,
    }
}
