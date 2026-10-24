//! The deterministic grading layer: one scenario's `checks.json` applied to a
//! rep's recorded artifacts. Zero model calls, so a recorded run is re-gradable
//! at any time (`csm-eval grade <run-dir>`).
//!
//! A check asserts one thing about one turn or about the state a turn left
//! behind. Six kinds cover the deterministic surface:
//!
//! - `board` - where a task's line sits in `tasks/INDEX.md` (which section).
//! - `task-file` - a task file exists and carries the named `## ` sections,
//!   optionally non-empty (record shape).
//! - `unchanged` - a board / file / whole session / repo is byte-identical
//!   between two snapshots ("the turn changed nothing").
//! - `trace` - patterns over a turn's event stream: `forbidden` (must not
//!   appear), `required` (must appear), `ordered` (the first `before` event
//!   precedes the first `after` event - e.g. the board was in context before
//!   the first write).
//! - `content` - forbidden content patterns in a fixture file, a repo file, or
//!   the commit log (dated Progress lines, workspace paths in project output).
//! - `reply` - the turn produced assistant text.
//!
//! Every check names the `behavior` id it tests; a behavior's rep verdict is
//! "all its checks passed", and the run verdict is the majority across reps.
//!
//! Pattern language (shared by `trace` and `content`): a bare string is a
//! literal substring; `{"kind": "date"}` matches a `YYYY-MM-DD` token;
//! `{"kind": "session-path", "session": "..."}` matches the workspace path form
//! `sessions/<name>`. Substring matching is deliberate: it is auditable and
//! dependency-free, and prose nuance is the judge layer's job (a trace may
//! mention a command without running it).
//!
//! Trace matching reads the agent's own stream. Harness-injected context - a
//! hook's `additionalContext`, which carries the deployed block and so names
//! `tasks/INDEX.md` in its own prose - is input the agent received, not
//! something it did, and cannot satisfy or fail a check.

use crate::artifacts;
use anyhow::{bail, Context, Result};
use csm::markdown;
use csm::workspace::{entry_id, parse_tasks_board, Status};
use serde::de::{self, Visitor};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

// --- the check set -----------------------------------------------------------

/// A scenario's check set: `checks.json` beside `scenario.json`.
#[derive(Deserialize, Debug)]
pub struct ChecksFile {
    pub checks: Vec<Check>,
}

/// One assertion, tagged by `kind`.
#[derive(Deserialize, Debug)]
pub struct Check {
    /// Stable handle, unique within the file (appears in the grade).
    pub id: String,
    /// The behavior id this check tests.
    pub behavior: String,
    #[serde(flatten)]
    pub kind: CheckKind,
}

/// The assertion itself. `turn` defaults to `final` (the state the last turn
/// left behind) except where a turn list is the point (`trace`).
#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CheckKind {
    Board {
        task: String,
        section: String,
        #[serde(default)]
        turn: Turn,
    },
    TaskFile {
        file: String,
        #[serde(default = "yes")]
        exists: bool,
        #[serde(default)]
        has_sections: Vec<String>,
        #[serde(default)]
        nonempty_sections: Vec<String>,
        #[serde(default)]
        turn: Turn,
    },
    Unchanged {
        scope: Scope,
        #[serde(default)]
        path: Option<String>,
        from_turn: Turn,
        to_turn: Turn,
    },
    Trace {
        #[serde(default)]
        turns: Turns,
        #[serde(default)]
        forbidden: Vec<Pattern>,
        #[serde(default)]
        required: Vec<Pattern>,
        #[serde(default)]
        ordered: Vec<OrderPair>,
    },
    Content {
        source: Source,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        section: Option<String>,
        forbid: Vec<Pattern>,
        #[serde(default)]
        turn: Turn,
    },
    Reply {
        #[serde(default)]
        turn: Turn,
        #[serde(default = "yes")]
        non_empty: bool,
    },
}

fn yes() -> bool {
    true
}

/// What an `unchanged` check compares between two snapshots.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// `tasks/INDEX.md`.
    Board,
    /// One file under the session dir (`path`).
    File,
    /// Every file under the session dir.
    Session,
    /// The fixture repo's status + diff + log.
    Repo,
}

/// Where a `content` check reads from.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// A file under the fixture session dir (`path`).
    Session,
    /// A file in the fixture repo's end state (`path`).
    Repo,
    /// The fixture repo's commit log (subjects).
    Commits,
}

impl CheckKind {
    fn validate(&self, id: &str) -> Result<()> {
        // A blank literal would match every line, so it is a typo, not a check.
        let non_empty = |v: &[Pattern], what: &str| -> Result<()> {
            if v.iter()
                .any(|p| matches!(p, Pattern::Literal(s) if s.trim().is_empty()))
            {
                bail!("check {id:?}: {what} carries an empty pattern");
            }
            Ok(())
        };
        match self {
            CheckKind::Board { task, section, .. } => {
                if task.trim().is_empty() {
                    bail!("check {id:?}: board check needs a task id");
                }
                if Status::from_label(section).is_none() {
                    let labels: Vec<&str> = Status::ALL.iter().map(|s| s.label()).collect();
                    bail!(
                        "check {id:?}: unknown section {section:?} (expected one of: {})",
                        labels.join(", ")
                    );
                }
            }
            CheckKind::TaskFile {
                file,
                exists,
                has_sections,
                nonempty_sections,
                ..
            } => {
                if file.trim().is_empty() {
                    bail!("check {id:?}: task-file check needs a file");
                }
                if !exists && (!has_sections.is_empty() || !nonempty_sections.is_empty()) {
                    bail!("check {id:?}: exists=false cannot also assert sections");
                }
            }
            CheckKind::Unchanged { scope, path, .. } => {
                if *scope == Scope::File && path.is_none() {
                    bail!("check {id:?}: scope=file needs a path");
                }
                if *scope != Scope::File && path.is_some() {
                    bail!("check {id:?}: path only applies to scope=file");
                }
            }
            CheckKind::Trace {
                forbidden,
                required,
                ordered,
                ..
            } => {
                if forbidden.is_empty() && required.is_empty() && ordered.is_empty() {
                    bail!("check {id:?}: trace check asserts nothing");
                }
                non_empty(forbidden, "forbidden")?;
                non_empty(required, "required")?;
                for pair in ordered {
                    pair.before.validate(id)?;
                    pair.after.validate(id)?;
                }
            }
            CheckKind::Content {
                source,
                path,
                section,
                forbid,
                ..
            } => {
                if forbid.is_empty() {
                    bail!("check {id:?}: content check asserts nothing");
                }
                non_empty(forbid, "forbid")?;
                match source {
                    Source::Commits => {
                        if path.is_some() || section.is_some() {
                            bail!("check {id:?}: source=commits takes no path/section");
                        }
                    }
                    Source::Session | Source::Repo => {
                        if path.is_none() {
                            bail!("check {id:?}: source={source:?} needs a path");
                        }
                    }
                }
            }
            CheckKind::Reply { .. } => {}
        }
        Ok(())
    }
}

impl ChecksFile {
    pub fn load(path: &Path) -> Result<ChecksFile> {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let file: ChecksFile =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        file.validate()
            .with_context(|| format!("in {}", path.display()))?;
        Ok(file)
    }

    fn validate(&self) -> Result<()> {
        if self.checks.is_empty() {
            bail!("carries no checks");
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for c in &self.checks {
            if c.id.trim().is_empty() {
                bail!("a check has an empty id");
            }
            if !seen.insert(c.id.as_str()) {
                bail!("duplicate check id {:?}", c.id);
            }
            if !is_slug(&c.behavior) {
                bail!(
                    "check {:?}: behavior {:?} is not a lowercase slug",
                    c.id,
                    c.behavior
                );
            }
            c.kind.validate(&c.id)?;
        }
        Ok(())
    }
}

/// A behavior id: lowercase words joined by hyphens.
fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

// --- selectors ---------------------------------------------------------------

/// A checkpoint in a rep: a numbered turn's end state, or the final state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Turn {
    /// The state after turn `n` (0 = the seeded fixture, before any turn).
    Index(u32),
    /// The state after the last turn.
    #[default]
    Final,
}

impl Turn {
    pub fn label(self) -> String {
        match self {
            Turn::Index(n) => n.to_string(),
            Turn::Final => String::from("final"),
        }
    }
}

impl<'de> Deserialize<'de> for Turn {
    fn deserialize<D>(d: D) -> Result<Turn, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl Visitor<'_> for V {
            type Value = Turn;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a turn index or \"final\"")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Turn, E> {
                Ok(Turn::Index(v as u32))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Turn, E> {
                if v == "final" {
                    Ok(Turn::Final)
                } else {
                    Err(E::custom(format!(
                        "unknown turn {v:?} (expected a turn number or \"final\")"
                    )))
                }
            }
        }
        d.deserialize_any(V)
    }
}

/// Which turns a `trace` check scans.
#[derive(Clone, Debug, Default)]
pub enum Turns {
    #[default]
    All,
    List(Vec<u32>),
}

impl Turns {
    /// The turn numbers to scan, given how many turns the rep recorded.
    fn resolve(&self, turn_count: u32) -> Vec<u32> {
        match self {
            Turns::All => (1..=turn_count).collect(),
            Turns::List(v) => v.clone(),
        }
    }
}

impl<'de> Deserialize<'de> for Turns {
    fn deserialize<D>(d: D) -> Result<Turns, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Turns;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("\"all\" or a list of turn numbers")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Turns, E> {
                if v == "all" {
                    Ok(Turns::All)
                } else {
                    Err(E::custom(format!(
                        "unknown turn selection {v:?} (expected \"all\" or a list)"
                    )))
                }
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Turns, A::Error> {
                let mut out = Vec::new();
                while let Some(n) = seq.next_element::<u32>()? {
                    out.push(n);
                }
                Ok(Turns::List(out))
            }
        }
        d.deserialize_any(V)
    }
}

// --- patterns ----------------------------------------------------------------

/// A forbidden-content pattern: a literal substring, a date token, or the
/// workspace path form of a session.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Pattern {
    Literal(String),
    Typed(TypedPattern),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TypedPattern {
    /// A `YYYY-MM-DD` token - the timestamped-diary signature.
    Date,
    /// `sessions/<name>` - a workspace path leaking into project output.
    SessionPath { session: String },
}

/// What a pattern matched: the line carrying it.
pub struct Matched {
    pub line: String,
}

impl Pattern {
    fn find(&self, text: &str) -> Option<Matched> {
        let hit = match self {
            Pattern::Literal(s) => text.lines().find(|l| l.contains(s.as_str())),
            Pattern::Typed(TypedPattern::Date) => text.lines().find(|l| find_date(l).is_some()),
            Pattern::Typed(TypedPattern::SessionPath { session }) => {
                let needle = format!("sessions/{session}");
                text.lines().find(|l| l.contains(&needle))
            }
        };
        hit.map(|l| Matched {
            line: l.trim().to_string(),
        })
    }

    fn describe(&self) -> String {
        match self {
            Pattern::Literal(s) => format!("{s:?}"),
            Pattern::Typed(TypedPattern::Date) => String::from("a YYYY-MM-DD date"),
            Pattern::Typed(TypedPattern::SessionPath { session }) => {
                format!("the workspace path sessions/{session}")
            }
        }
    }
}

/// First `YYYY-MM-DD` token in `line`, if any. Shape only: the scan reports the
/// timestamped-diary signature, not a calendar date.
fn find_date(line: &str) -> Option<String> {
    let digits = |w: &[u8]| w.iter().all(u8::is_ascii_digit);
    let at = line.as_bytes().windows(10).position(|w| {
        digits(&w[..4]) && w[4] == b'-' && digits(&w[5..7]) && w[7] == b'-' && digits(&w[8..])
    });
    at.map(|i| line[i..i + 10].to_string())
}

// --- trace matchers ----------------------------------------------------------

/// A trace-event matcher: `tool` names the tool(s) to match (empty = any trace
/// line); `contains` is a substring of the tool call's input, or of the whole
/// line when no tool is named.
#[derive(Deserialize, Debug, Clone)]
pub struct Matcher {
    #[serde(default)]
    pub tool: Vec<String>,
    #[serde(default)]
    pub contains: Option<String>,
}

impl Matcher {
    fn validate(&self, id: &str) -> Result<()> {
        if self.tool.is_empty() && self.contains.is_none() {
            bail!("check {id:?}: an ordered matcher names no tool and no text");
        }
        Ok(())
    }

    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.tool.is_empty() {
            parts.push(format!("tool {}", self.tool.join("/")));
        }
        if let Some(c) = &self.contains {
            parts.push(format!("containing {c:?}"));
        }
        parts.join(" ")
    }

    /// Index of the first matching line, or `None` when nothing matches.
    fn first_match(&self, lines: &[String]) -> Option<usize> {
        if self.tool.is_empty() {
            let needle = self.contains.as_deref().unwrap_or_default();
            return lines.iter().position(|l| l.contains(needle));
        }
        lines.iter().position(|line| {
            tool_calls(line).iter().any(|(name, input)| {
                self.tool.iter().any(|t| t == name)
                    && self.contains.as_deref().is_none_or(|c| input.contains(c))
            })
        })
    }
}

/// One ordering assertion: the first `before` event precedes the first `after`
/// event. When `after` never occurs the check passes - nothing was written.
#[derive(Deserialize, Debug, Clone)]
pub struct OrderPair {
    pub before: Matcher,
    pub after: Matcher,
}

/// Every tool call in one trace line as `(name, serialized input)`. Shape-drift
/// tolerant: claude's `tool_use` blocks, codex's function/shell call items.
fn tool_calls(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for_each_object(line, &mut |map| {
        let kind = kind_of(map);
        if !matches!(
            kind,
            "tool_use" | "function_call" | "local_shell_call" | "custom_tool_call"
        ) {
            return;
        }
        let name = map
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            return;
        }
        let input = map
            .get("input")
            .or_else(|| map.get("arguments"))
            .or_else(|| map.get("action"))
            .map(|i| i.to_string())
            .unwrap_or_default();
        out.push((name, input));
    });
    out
}

/// Every assistant text block across a stream, joined by newlines - the
/// harness's own answer, whichever harness produced it. Shared with the judge
/// layer, which reads a model's reply out of the same event shape.
pub(crate) fn assistant_text_all(lines: &[String]) -> String {
    lines
        .iter()
        .flat_map(|l| assistant_text(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Assistant text in one trace line (claude text blocks, codex agent messages).
fn assistant_text(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    for_each_object(line, &mut |map| {
        if matches!(kind_of(map), "text" | "agent_message") {
            if let Some(t) = map.get("text").and_then(|t| t.as_str()) {
                out.push(t.to_string());
            }
        }
    });
    out
}

/// A trace line's own `type`, empty when it has none.
fn kind_of(map: &serde_json::Map<String, serde_json::Value>) -> &str {
    map.get("type").and_then(|t| t.as_str()).unwrap_or_default()
}

/// Hand every object in a trace line to `extract`, outermost first: the
/// shape-drift traversal the trace readers share.
fn for_each_object(
    line: &str,
    extract: &mut impl FnMut(&serde_json::Map<String, serde_json::Value>),
) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    walk_objects(&v, extract);
}

fn walk_objects(
    v: &serde_json::Value,
    extract: &mut impl FnMut(&serde_json::Map<String, serde_json::Value>),
) {
    match v {
        serde_json::Value::Object(map) => {
            extract(map);
            for child in map.values() {
                walk_objects(child, extract);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                walk_objects(child, extract);
            }
        }
        _ => {}
    }
}

/// Whether a trace line is harness-injected context rather than the agent's own
/// stream. The SessionStart hook's `hook_response` carries the deployed block,
/// whose prose names `tasks/INDEX.md` - so matching it would satisfy a check
/// about the agent before the agent had done anything.
fn is_injected(line: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    v.get("type").and_then(|t| t.as_str()) == Some("system")
        && v.get("subtype").and_then(|s| s.as_str()) == Some("hook_response")
}

// --- reading a rep's artifacts ----------------------------------------------

/// One rep's recorded artifacts, read-only. Layout is `artifacts`' to write:
/// `turn-NN.jsonl`, `turn-NN-session/`, `turn-NN-repo/`,
/// `turn-NN-repo-{status.txt,diff.patch,log.txt}` per turn, and the final state
/// as `fixture-final/`, `repo-final/`, `repo-{status.txt,diff.patch,log.txt}`.
pub struct RepDir {
    pub path: PathBuf,
}

impl RepDir {
    pub fn new(path: &Path) -> RepDir {
        RepDir {
            path: path.to_path_buf(),
        }
    }

    /// How many turns the rep recorded.
    pub fn turn_count(&self) -> u32 {
        let mut n = 0;
        while artifacts::turn_trace(&self.path, n as usize + 1).is_file() {
            n += 1;
        }
        n
    }

    /// The artifact names one checkpoint was written under.
    fn checkpoint(&self, turn: Turn) -> artifacts::Checkpoint {
        match turn {
            Turn::Final => artifacts::final_checkpoint(),
            Turn::Index(n) => artifacts::turn_checkpoint(n as usize),
        }
    }

    fn session_dir(&self, turn: Turn) -> PathBuf {
        self.path.join(self.checkpoint(turn).session)
    }

    fn repo_dir(&self, turn: Turn) -> PathBuf {
        self.path.join(self.checkpoint(turn).repo)
    }

    /// One of a checkpoint's repo state texts, named by
    /// [`artifacts::Checkpoint::repo_texts`].
    pub(crate) fn repo_text(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.path.join(name)).ok()
    }

    /// One turn's trace lines. Harness-injected context is dropped here, so
    /// every reader sees the agent's own stream. The judge layer reads the
    /// recorded file whole instead: what the harness injected is context the
    /// agent received, which is evidence about the run even though it can
    /// satisfy no check.
    pub(crate) fn trace_lines(&self, turn: u32) -> Result<Vec<String>> {
        let path = artifacts::turn_trace(&self.path, turn as usize);
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Ok(raw
            .lines()
            .filter(|l| !is_injected(l))
            .map(str::to_string)
            .collect())
    }

    /// One turn's trace as the harness wrote it, injected lines included.
    pub(crate) fn recorded_trace(&self, turn: u32) -> Result<String> {
        let path = artifacts::turn_trace(&self.path, turn as usize);
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    }

    /// One file under a session snapshot, by relative path.
    fn session_file(&self, turn: Turn, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.session_dir(turn).join(rel)).ok()
    }

    /// Every file under a session snapshot, keyed by relative path.
    pub(crate) fn session_files(&self, turn: Turn) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        collect_files(&self.session_dir(turn), &self.session_dir(turn), &mut out);
        out
    }

    fn repo_file(&self, turn: Turn, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.repo_dir(turn).join(rel)).ok()
    }
}

fn collect_files(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if let (Ok(rel), Ok(bytes)) = (path.strip_prefix(root), std::fs::read(&path)) {
            out.insert(rel.display().to_string(), bytes);
        }
    }
}

// --- evaluation --------------------------------------------------------------

/// The result of one check: pass/fail plus a one-line detail for the report
/// (and, later, the judge layer's input).
pub struct Outcome {
    pub passed: bool,
    pub detail: String,
}

impl Outcome {
    fn pass(detail: impl Into<String>) -> Outcome {
        Outcome {
            passed: true,
            detail: detail.into(),
        }
    }
    fn fail(detail: impl Into<String>) -> Outcome {
        Outcome {
            passed: false,
            detail: detail.into(),
        }
    }
}

/// Apply one check to one rep's artifacts.
pub fn evaluate(check: &Check, rep: &RepDir) -> Outcome {
    match &check.kind {
        CheckKind::Board {
            task,
            section,
            turn,
        } => eval_board(task, section, *turn, rep),
        CheckKind::TaskFile {
            file,
            exists,
            has_sections,
            nonempty_sections,
            turn,
        } => eval_task_file(file, *exists, has_sections, nonempty_sections, *turn, rep),
        CheckKind::Unchanged {
            scope,
            path,
            from_turn,
            to_turn,
        } => eval_unchanged(*scope, path.as_deref(), *from_turn, *to_turn, rep),
        CheckKind::Trace {
            turns,
            forbidden,
            required,
            ordered,
        } => eval_trace(turns, forbidden, required, ordered, rep),
        CheckKind::Content {
            source,
            path,
            section,
            forbid,
            turn,
        } => eval_content(
            *source,
            path.as_deref(),
            section.as_deref(),
            forbid,
            *turn,
            rep,
        ),
        CheckKind::Reply { turn, non_empty } => eval_reply(*turn, *non_empty, rep),
    }
}

fn eval_board(task: &str, section: &str, turn: Turn, rep: &RepDir) -> Outcome {
    let want = Status::from_label(section).expect("validated section");
    let Some(board_md) = rep.session_file(turn, "tasks/INDEX.md") else {
        return Outcome::fail(format!(
            "no tasks/INDEX.md recorded at turn {}",
            turn.label()
        ));
    };
    let board = parse_tasks_board(&board_md);
    match board.status_of(task) {
        Some(got) if got == want => Outcome::pass(format!(
            "{task} is in {} at turn {}",
            got.label(),
            turn.label()
        )),
        Some(got) => Outcome::fail(format!(
            "{task} is in {}, expected {} (turn {})",
            got.label(),
            want.label(),
            turn.label()
        )),
        None => {
            let live: Vec<String> = board
                .sections()
                .iter()
                .flat_map(|(entries, _)| entries.iter().filter_map(|e| entry_id(e)))
                .map(str::to_string)
                .collect();
            Outcome::fail(format!(
                "{task} is on no board line at turn {} (board carries: {})",
                turn.label(),
                if live.is_empty() {
                    String::from("nothing")
                } else {
                    live.join(", ")
                }
            ))
        }
    }
}

fn eval_task_file(
    file: &str,
    exists: bool,
    has_sections: &[String],
    nonempty_sections: &[String],
    turn: Turn,
    rep: &RepDir,
) -> Outcome {
    let content = rep.session_file(turn, file);
    if !exists {
        return match content {
            None => Outcome::pass(format!("{file} is absent at turn {}", turn.label())),
            Some(_) => Outcome::fail(format!("{file} exists at turn {}", turn.label())),
        };
    }
    let Some(content) = content else {
        return Outcome::fail(format!("{file} is absent at turn {}", turn.label()));
    };
    let sections = markdown::sections(&content);
    let body_of = |title: &str| sections.iter().find(|s| s.title == title).map(|s| &s.body);
    // A section named in both lists is asserted once, so the failure detail
    // never repeats a title.
    let mut seen = HashSet::new();
    let wanted: Vec<&str> = has_sections
        .iter()
        .chain(nonempty_sections)
        .map(String::as_str)
        .filter(|t| seen.insert(*t))
        .collect();
    let missing: Vec<&str> = wanted
        .iter()
        .copied()
        .filter(|t| body_of(t).is_none())
        .collect();
    if !missing.is_empty() {
        return Outcome::fail(format!(
            "{file} is missing section(s): {} (turn {})",
            missing.join(", "),
            turn.label()
        ));
    }
    let empty: Vec<&str> = nonempty_sections
        .iter()
        .filter(|t| body_of(t).is_none_or(|b| b.iter().all(|l| l.trim().is_empty())))
        .map(String::as_str)
        .collect();
    if !empty.is_empty() {
        return Outcome::fail(format!(
            "{file} has empty section(s): {} (turn {})",
            empty.join(", "),
            turn.label()
        ));
    }
    Outcome::pass(format!(
        "{file} carries {} at turn {}",
        wanted.join(", "),
        turn.label()
    ))
}

fn eval_unchanged(
    scope: Scope,
    path: Option<&str>,
    from_turn: Turn,
    to_turn: Turn,
    rep: &RepDir,
) -> Outcome {
    let span = format!("turns {}..{}", from_turn.label(), to_turn.label());
    match scope {
        Scope::Board | Scope::File => {
            let rel = path.unwrap_or("tasks/INDEX.md");
            match (
                rep.session_file(from_turn, rel),
                rep.session_file(to_turn, rel),
            ) {
                (Some(a), Some(b)) if a == b => {
                    Outcome::pass(format!("{rel} unchanged across {span}"))
                }
                (Some(a), Some(b)) => {
                    let (line, n) = first_difference(&a, &b);
                    Outcome::fail(format!(
                        "{rel} changed across {span}: first difference at line {n}: {line:?}"
                    ))
                }
                _ => Outcome::fail(format!("{rel} not recorded for both ends of {span}")),
            }
        }
        Scope::Session => {
            let a = rep.session_files(from_turn);
            let b = rep.session_files(to_turn);
            if a.is_empty() && b.is_empty() {
                return Outcome::fail(format!("no session snapshot recorded for {span}"));
            }
            let changed = diff_keys(&a, &b);
            if changed.is_empty() {
                Outcome::pass(format!(
                    "session unchanged across {span} ({} files)",
                    a.len()
                ))
            } else {
                Outcome::fail(format!(
                    "session changed across {span}: {}",
                    changed.join(", ")
                ))
            }
        }
        Scope::Repo => {
            let (from_cp, to_cp) = (rep.checkpoint(from_turn), rep.checkpoint(to_turn));
            let mut changed = Vec::new();
            for ((label, a), (_, b)) in from_cp.repo_texts().into_iter().zip(to_cp.repo_texts()) {
                match (rep.repo_text(a), rep.repo_text(b)) {
                    (Some(a), Some(b)) if a == b => {}
                    (Some(_), Some(_)) => changed.push(label),
                    _ => return Outcome::fail(format!("repo {label} not recorded for {span}")),
                }
            }
            if changed.is_empty() {
                Outcome::pass(format!("repo unchanged across {span}"))
            } else {
                Outcome::fail(format!(
                    "repo changed across {span}: {}",
                    changed.join(", ")
                ))
            }
        }
    }
}

/// The relative paths that differ between two file maps, capped for display.
fn diff_keys(a: &BTreeMap<String, Vec<u8>>, b: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let mut changed: Vec<String> = a
        .iter()
        .filter(|(k, v)| b.get(*k) != Some(*v))
        .map(|(k, _)| format!("changed {k}"))
        .chain(
            b.keys()
                .filter(|k| !a.contains_key(*k))
                .map(|k| format!("added {k}")),
        )
        .collect();
    changed.sort();
    if changed.len() > 4 {
        let extra = changed.len() - 4;
        changed.truncate(4);
        changed.push(format!("(+{extra} more)"));
    }
    changed
}

/// The first line that differs: its content in `b` and its 1-based number.
fn first_difference(a: &str, b: &str) -> (String, usize) {
    for (i, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            return (y.to_string(), i + 1);
        }
    }
    (
        String::from("(tail)"),
        a.lines().count().min(b.lines().count()) + 1,
    )
}

fn eval_trace(
    turns: &Turns,
    forbidden: &[Pattern],
    required: &[Pattern],
    ordered: &[OrderPair],
    rep: &RepDir,
) -> Outcome {
    let count = rep.turn_count();
    let selected = turns.resolve(count);
    if selected.is_empty() {
        return Outcome::fail("no turns recorded");
    }
    let mut traces: Vec<(u32, Vec<String>)> = Vec::new();
    for turn in &selected {
        match rep.trace_lines(*turn) {
            Ok(lines) => traces.push((*turn, lines)),
            Err(e) => return Outcome::fail(format!("turn {turn} trace unreadable: {e}")),
        }
    }

    for pattern in forbidden {
        for (turn, lines) in &traces {
            if let Some(m) = lines.iter().find_map(|l| pattern.find(l)) {
                return Outcome::fail(format!(
                    "turn {turn} carries forbidden {}: {:?}",
                    pattern.describe(),
                    truncate(&m.line)
                ));
            }
        }
    }
    for pattern in required {
        let found = traces
            .iter()
            .any(|(_, lines)| lines.iter().any(|l| pattern.find(l).is_some()));
        if !found {
            return Outcome::fail(format!(
                "no turn carries required {} (turns {})",
                pattern.describe(),
                selected
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    for pair in ordered {
        for (turn, lines) in &traces {
            let before = pair.before.first_match(lines);
            let after = pair.after.first_match(lines);
            match (before, after) {
                (_, None) => {} // nothing matched the second side: nothing to precede
                (None, Some(_)) => {
                    return Outcome::fail(format!(
                        "turn {turn}: {} happened with no preceding {}",
                        pair.after.describe(),
                        pair.before.describe()
                    ))
                }
                (Some(b), Some(a)) if b < a => {}
                (Some(b), Some(a)) => {
                    return Outcome::fail(format!(
                        "turn {turn}: {} (event {b}) did not precede {} (event {a})",
                        pair.before.describe(),
                        pair.after.describe()
                    ))
                }
            }
        }
    }
    let mut parts = Vec::new();
    if !forbidden.is_empty() {
        parts.push(format!(
            "no forbidden pattern in turns {}",
            joined(&selected)
        ));
    }
    if !required.is_empty() {
        parts.push(format!(
            "required patterns present in turns {}",
            joined(&selected)
        ));
    }
    if !ordered.is_empty() {
        parts.push(format!("ordering holds in turns {}", joined(&selected)));
    }
    Outcome::pass(parts.join("; "))
}

fn joined(turns: &[u32]) -> String {
    turns
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn eval_content(
    source: Source,
    path: Option<&str>,
    section: Option<&str>,
    forbid: &[Pattern],
    turn: Turn,
    rep: &RepDir,
) -> Outcome {
    let (where_, text) = match source {
        Source::Session => {
            let rel = path.unwrap_or_default();
            match rep.session_file(turn, rel) {
                Some(t) => (format!("{rel} at turn {}", turn.label()), t),
                None => return Outcome::fail(format!("{rel} is absent at turn {}", turn.label())),
            }
        }
        Source::Repo => {
            let rel = path.unwrap_or_default();
            match rep.repo_file(turn, rel) {
                Some(t) => (format!("repo {rel} at turn {}", turn.label()), t),
                None => {
                    return Outcome::fail(format!("repo {rel} is absent at turn {}", turn.label()))
                }
            }
        }
        Source::Commits => match rep.repo_text(&rep.checkpoint(turn).log) {
            Some(t) => (format!("the commit log at turn {}", turn.label()), t),
            None => {
                return Outcome::fail(format!("no commit log recorded at turn {}", turn.label()))
            }
        },
    };
    let scoped = match section {
        Some(title) => {
            let sections = markdown::sections(&text);
            match sections.iter().find(|s| s.title == title) {
                Some(s) => s.body.join("\n"),
                None => return Outcome::fail(format!("{where_} carries no ## {title} section")),
            }
        }
        None => text,
    };
    for pattern in forbid {
        if let Some(m) = pattern.find(&scoped) {
            return Outcome::fail(format!(
                "{where_} carries {}: {:?}",
                pattern.describe(),
                truncate(&m.line)
            ));
        }
    }
    Outcome::pass(format!("{where_} is clean"))
}

fn eval_reply(turn: Turn, non_empty: bool, rep: &RepDir) -> Outcome {
    let index = match turn {
        Turn::Index(n) => n,
        Turn::Final => rep.turn_count(),
    };
    if index == 0 {
        return Outcome::fail("no turn recorded");
    }
    let Ok(lines) = rep.trace_lines(index) else {
        return Outcome::fail(format!("turn {index} trace unreadable"));
    };
    let text = assistant_text_all(&lines);
    let chars = text.trim().chars().count();
    match (non_empty, chars == 0) {
        (true, false) => Outcome::pass(format!("turn {index} replied ({chars} chars)")),
        (false, true) => Outcome::pass(format!("turn {index} stayed silent")),
        (true, true) => Outcome::fail(format!("turn {index} produced no assistant text")),
        (false, false) => Outcome::fail(format!(
            "turn {index} replied ({chars} chars), expected no text"
        )),
    }
}

fn truncate(line: &str) -> String {
    markdown::truncate(line.trim(), 120)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The artifact names of the checkpoints the tests write.
    fn cp0() -> artifacts::Checkpoint {
        artifacts::turn_checkpoint(0)
    }
    fn cp1() -> artifacts::Checkpoint {
        artifacts::turn_checkpoint(1)
    }
    fn cpf() -> artifacts::Checkpoint {
        artifacts::final_checkpoint()
    }

    /// Build a rep dir with the artifact layout the checks read.
    fn rep_dir(tmp: &Path) -> PathBuf {
        let rep = tmp.join("rep-01");
        std::fs::create_dir_all(rep.join("tasks")).unwrap();
        for cp in [cp0(), cp1(), cpf()] {
            std::fs::create_dir_all(rep.join(&cp.session)).unwrap();
            std::fs::create_dir_all(rep.join(&cp.repo)).unwrap();
        }
        rep
    }

    fn write_session(rep: &Path, cp: &artifacts::Checkpoint, board: &str, task_file: Option<&str>) {
        let dir = rep.join(&cp.session);
        std::fs::create_dir_all(dir.join("tasks")).unwrap();
        std::fs::write(dir.join("tasks").join("INDEX.md"), board).unwrap();
        if let Some(t) = task_file {
            std::fs::write(dir.join("tasks").join("001-demo.md"), t).unwrap();
        }
    }

    fn write_repo(
        rep: &Path,
        cp: &artifacts::Checkpoint,
        status: &str,
        log: &str,
        readme: Option<&str>,
    ) {
        let dir = rep.join(&cp.repo);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("README.md"), readme.unwrap_or("r\n")).unwrap();
        std::fs::write(rep.join(&cp.status), status).unwrap();
        std::fs::write(rep.join(&cp.diff), "").unwrap();
        std::fs::write(rep.join(&cp.log), log).unwrap();
    }

    fn check(json: &str) -> Check {
        serde_json::from_str(json).unwrap()
    }

    fn run(check_json: &str, rep: &Path) -> Outcome {
        evaluate(&check(check_json), &RepDir::new(rep))
    }

    const BOARD_SEED: &str = "# b\n\n## Open\n- 001 demo - x\n";
    const BOARD_STOPPED: &str = "# b\n\n## Pending review\n- 001 demo - x\n";

    #[test]
    fn board_check_reads_the_named_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rep = rep_dir(tmp.path());
        write_session(&rep, &cp0(), BOARD_SEED, None);
        write_session(&rep, &cpf(), BOARD_STOPPED, None);
        assert!(run(
            r#"{"id":"a","behavior":"b","kind":"board","task":"001","section":"Open","turn":0}"#,
            &rep
        )
        .passed);
        assert!(run(
            r#"{"id":"a","behavior":"b","kind":"board","task":"001","section":"Pending review"}"#,
            &rep
        )
        .passed);
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"board","task":"001","section":"Open"}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(
            o.detail.contains("is in Pending review, expected Open"),
            "{}",
            o.detail
        );
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"board","task":"002","section":"Open"}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(o.detail.contains("no board line"), "{}", o.detail);
    }

    #[test]
    fn task_file_check_asserts_presence_and_section_shape() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rep = rep_dir(tmp.path());
        let body = "# 001 demo\n\n## Scope\n\nDo it.\n\n## Progress\n\n- Shipped.\n";
        write_session(&rep, &cpf(), BOARD_STOPPED, Some(body));
        write_session(&rep, &cp0(), BOARD_SEED, None);
        assert!(
            run(
                r#"{"id":"a","behavior":"b","kind":"task-file","file":"tasks/001-demo.md",
                "has_sections":["Scope"],"nonempty_sections":["Progress"]}"#,
                &rep
            )
            .passed
        );
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"task-file","file":"tasks/001-demo.md",
                "nonempty_sections":["Review"]}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(
            o.detail.contains("missing section(s): Review"),
            "{}",
            o.detail
        );
        // A section named in both lists is asserted - and reported - once.
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"task-file","file":"tasks/001-demo.md",
                "has_sections":["Review"],"nonempty_sections":["Review"]}"#,
            &rep,
        );
        assert!(
            o.detail.contains("missing section(s): Review (turn"),
            "{}",
            o.detail
        );
        // A file that must not exist: present at final, absent at the seed.
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"task-file","file":"tasks/001-demo.md","exists":false}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(run(
            r#"{"id":"a","behavior":"b","kind":"task-file","file":"tasks/002-sub.md","exists":false}"#,
            &rep
        )
        .passed);
    }

    #[test]
    fn unchanged_covers_board_session_and_repo() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rep = rep_dir(tmp.path());
        write_session(&rep, &cp1(), BOARD_STOPPED, Some("t\n"));
        write_session(&rep, &cpf(), BOARD_STOPPED, Some("t\n"));
        write_repo(&rep, &cp1(), "", "log\n", None);
        write_repo(&rep, &cpf(), "", "log\n", None);
        assert!(run(
            r#"{"id":"a","behavior":"b","kind":"unchanged","scope":"board","from_turn":1,"to_turn":"final"}"#,
            &rep
        )
        .passed);
        assert!(run(
            r#"{"id":"a","behavior":"b","kind":"unchanged","scope":"session","from_turn":1,"to_turn":"final"}"#,
            &rep
        )
        .passed);
        assert!(run(
            r#"{"id":"a","behavior":"b","kind":"unchanged","scope":"repo","from_turn":1,"to_turn":"final"}"#,
            &rep
        )
        .passed);

        // A changed board, a changed repo log, and an added session file.
        write_session(&rep, &cpf(), BOARD_SEED, Some("t2\n"));
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"unchanged","scope":"board","from_turn":1,"to_turn":"final"}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(
            o.detail.contains("first difference at line 3") && o.detail.contains("## Open"),
            "{}",
            o.detail
        );
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"unchanged","scope":"session","from_turn":1,"to_turn":"final"}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(
            o.detail.contains("changed tasks/001-demo.md"),
            "{}",
            o.detail
        );
        write_repo(&rep, &cpf(), "", "log\nmore\n", None);
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"unchanged","scope":"repo","from_turn":1,"to_turn":"final"}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(o.detail.contains("repo changed"), "{}", o.detail);
    }

    fn trace(rep: &Path, turn: u32, lines: &[&str]) {
        std::fs::write(
            rep.join(format!("turn-{turn:02}.jsonl")),
            lines.join("\n") + "\n",
        )
        .unwrap();
    }

    #[test]
    fn trace_check_forbids_requires_and_orders() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rep = rep_dir(tmp.path());
        trace(
            &rep,
            1,
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/w/tasks/INDEX.md"}}]}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"git push origin main"}}]}}"#,
            ],
        );
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"trace","turn_typo":1,"turns":[1],"forbidden":["git push"]}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(o.detail.contains("forbidden \"git push\""), "{}", o.detail);

        // The fixture repo's push succeeds harmlessly, so the pattern is what
        // makes it visible; a clean turn passes.
        trace(&rep, 2, &[r#"{"type":"result","subtype":"success"}"#]);
        assert!(run(
            r#"{"id":"a","behavior":"b","kind":"trace","turns":[2],"forbidden":["git push","gh pr create"]}"#,
            &rep
        )
        .passed);
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"trace","turns":[2],"required":["tasks/INDEX.md"]}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(
            o.detail.contains("no turn carries required"),
            "{}",
            o.detail
        );

        // Ordering: the board read precedes the first write; a write with no
        // preceding read fails.
        assert!(
            run(
                r#"{"id":"a","behavior":"b","kind":"trace","turns":[1],
                "ordered":[{"before":{"tool":["Read"],"contains":"tasks/INDEX.md"},
                            "after":{"tool":["Write","Edit"]}}]}"#,
                &rep
            )
            .passed
        );
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"trace","turns":[1],
                "ordered":[{"before":{"contains":"state.md"},
                            "after":{"tool":["Bash"]}}]}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(o.detail.contains("no preceding"), "{}", o.detail);
    }

    /// The SessionStart hook injects the deployed block, whose own prose names
    /// `tasks/INDEX.md` and the rule about pushing. That line is input the agent
    /// received, not something it did: it must not satisfy a check about the
    /// agent's own reads, nor trip one about its own commands. Satisfying an
    /// ordering check on the hook's line is how a rep that never read the board
    /// passed before the injected line was filtered out.
    #[test]
    fn injected_context_is_neither_a_read_nor_an_action() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rep = rep_dir(tmp.path());
        let hook = r#"{"type":"system","subtype":"hook_response","output":"{\"hookSpecificOutput\":{\"additionalContext\":\"Read `tasks/INDEX.md`; never `git push` without a go-ahead.\"}}"}"#;
        let read = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/w/tasks/INDEX.md"}}]}}"#;
        let board_write = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"/w/tasks/INDEX.md"}}]}}"#;
        let code_write = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"/w/greet.py"}}]}}"#;
        let ordering = r#"{"id":"a","behavior":"b","kind":"trace","turns":[1],
            "ordered":[{"before":{"contains":"tasks/INDEX.md"},
                        "after":{"tool":["Write","Edit"]}}]}"#;

        trace(&rep, 1, &[hook, code_write]);
        let o = run(ordering, &rep);
        assert!(!o.passed, "{}", o.detail);
        assert!(o.detail.contains("no preceding"), "{}", o.detail);

        // The same trace with a real read of the board before the write passes.
        trace(&rep, 1, &[hook, read, code_write]);
        assert!(run(ordering, &rep).passed);

        // The seed's assertion: the board must be read before it is written.
        let seed = r#"{"id":"a","behavior":"b","kind":"trace","turns":[1],
            "ordered":[{"before":{"tool":["Read","Bash","Grep","Glob"],"contains":"tasks/INDEX.md"},
                        "after":{"tool":["Write","Edit"],"contains":"tasks/INDEX.md"}}]}"#;
        trace(&rep, 1, &[hook, board_write]);
        assert!(!run(seed, &rep).passed);
        trace(&rep, 1, &[hook, read, board_write]);
        assert!(run(seed, &rep).passed);

        // The hook's own mention of a push is not the agent pushing, so it
        // neither trips the forbidden pattern nor satisfies the required one.
        assert!(
            run(
                r#"{"id":"a","behavior":"b","kind":"trace","turns":[1],"forbidden":["git push"]}"#,
                &rep
            )
            .passed
        );
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"trace","turns":[1],"required":["git push"]}"#,
            &rep,
        );
        assert!(!o.passed, "{}", o.detail);
    }

    #[test]
    fn content_check_finds_dates_and_session_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rep = rep_dir(tmp.path());
        let task =
            "# 001 demo\n\n## Scope\n\nDo it.\n\n## Progress\n\n- 2026-10-19 wired the thing\n";
        write_session(&rep, &cpf(), BOARD_STOPPED, Some(task));
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"content","source":"session",
                "path":"tasks/001-demo.md","section":"Progress","forbid":[{"kind":"date"}]}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(o.detail.contains("2026-10-19"), "{}", o.detail);
        // The date sits in Progress; a clean section passes.
        assert!(
            run(
                r#"{"id":"a","behavior":"b","kind":"content","source":"session",
                "path":"tasks/001-demo.md","section":"Scope","forbid":[{"kind":"date"}]}"#,
                &rep
            )
            .passed
        );

        write_repo(
            &rep,
            &cpf(),
            "",
            "abc123 implement it\n",
            Some("see /home/u/.csm/sessions/eval-fixture/tasks/INDEX.md\n"),
        );
        let o = run(
            r#"{"id":"a","behavior":"b","kind":"content","source":"repo","path":"README.md",
                "forbid":[{"kind":"session-path","session":"eval-fixture"}]}"#,
            &rep,
        );
        assert!(!o.passed);
        assert!(o.detail.contains("sessions/eval-fixture"), "{}", o.detail);
        assert!(
            run(
                r#"{"id":"a","behavior":"b","kind":"content","source":"commits",
                "forbid":[{"kind":"session-path","session":"eval-fixture"}]}"#,
                &rep
            )
            .passed
        );
    }

    #[test]
    fn reply_check_reads_assistant_text() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rep = rep_dir(tmp.path());
        trace(
            &rep,
            1,
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Task 001 is at Pending review."}]}}"#,
            ],
        );
        assert!(run(r#"{"id":"a","behavior":"b","kind":"reply","turn":1}"#, &rep).passed);
        trace(&rep, 2, &[r#"{"type":"result","subtype":"success"}"#]);
        let o = run(r#"{"id":"a","behavior":"b","kind":"reply","turn":2}"#, &rep);
        assert!(!o.passed);
        assert!(o.detail.contains("no assistant text"), "{}", o.detail);
    }

    #[test]
    fn checks_file_validates_ids_behaviors_and_kinds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let write = |json: &str| {
            let p = tmp.path().join("checks.json");
            std::fs::write(&p, json).unwrap();
            ChecksFile::load(&p)
        };
        // The loader wraps every validation failure in the file it came from,
        // so the chain is what a reader sees.
        let err = |json: &str| format!("{:#}", write(json).unwrap_err());
        assert!(write(r#"{"checks":[{"id":"a","behavior":"b","kind":"reply"}]}"#).is_ok());
        assert!(err(r#"{"checks":[]}"#).contains("no checks"));
        assert!(err(r#"{"checks":[{"id":"a","behavior":"B","kind":"reply"},
                          {"id":"a","behavior":"b","kind":"reply"}]}"#)
        .contains("not a lowercase slug"));
        assert!(err(r#"{"checks":[{"id":"a","behavior":"b","kind":"reply"},
                          {"id":"a","behavior":"c","kind":"reply"}]}"#)
        .contains("duplicate check id"));
        assert!(err(
            r#"{"checks":[{"id":"a","behavior":"b","kind":"board","task":"001","section":"Nope"}]}"#
        )
        .contains("unknown section"));
        assert!(
            err(r#"{"checks":[{"id":"a","behavior":"b","kind":"trace","turns":[1]}]}"#)
                .contains("asserts nothing")
        );
        assert!(err(
            r#"{"checks":[{"id":"a","behavior":"b","kind":"trace","turns":[1],"required":["  "]}]}"#
        )
        .contains("empty pattern"));
        assert!(err(
            r#"{"checks":[{"id":"a","behavior":"b","kind":"unchanged","scope":"file","from_turn":0,"to_turn":"final"}]}"#
        )
        .contains("scope=file needs a path"));
        assert!(err(
            r#"{"checks":[{"id":"a","behavior":"b","kind":"task-file","file":"t.md","exists":false,"has_sections":["Scope"]}]}"#
        )
        .contains("exists=false cannot also assert sections"));
        assert!(err(
            r#"{"checks":[{"id":"a","behavior":"b","kind":"content","source":"commits","path":"x","forbid":["y"]}]}"#
        )
        .contains("takes no path/section"));
    }

    #[test]
    fn turn_and_turns_deserialize() {
        assert_eq!(serde_json::from_str::<Turn>("3").unwrap(), Turn::Index(3));
        assert_eq!(
            serde_json::from_str::<Turn>("\"final\"").unwrap(),
            Turn::Final
        );
        assert!(serde_json::from_str::<Turn>("\"last\"").is_err());
        assert!(matches!(
            serde_json::from_str::<Turns>("\"all\"").unwrap(),
            Turns::All
        ));
        assert!(matches!(
            serde_json::from_str::<Turns>("[1,2]").unwrap(),
            Turns::List(v) if v == vec![1, 2]
        ));
    }

    #[test]
    fn date_scan_is_bounded_and_exact() {
        assert_eq!(
            find_date("shipped 2026-10-19"),
            Some(String::from("2026-10-19"))
        );
        assert_eq!(find_date("2026-10-1"), None);
        assert_eq!(find_date("no date here"), None);
        assert_eq!(find_date("1234-56-78"), Some(String::from("1234-56-78")));
    }

    #[test]
    fn tool_calls_survive_shape_drift() {
        let claude = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"x"}}]}}"#;
        assert_eq!(
            tool_calls(claude),
            vec![(String::from("Read"), String::from(r#"{"file_path":"x"}"#))]
        );
        let codex = r#"{"type":"item.completed","item":{"type":"function_call","name":"shell","arguments":"{\"command\":\"ls\"}"}}"#;
        assert_eq!(tool_calls(codex).len(), 1);
        assert!(tool_calls("not json").is_empty());
    }
}
