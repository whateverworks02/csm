use crate::sandbox::{relative_path, run_process, Sandbox};
use anyhow::{ensure, Context, Result};
use csm::workspace::{entry_head, parse_tasks_board, Status};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

const BOARD: &str = "tasks/INDEX.md";
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
pub struct Check {
    #[serde(default = "first_turn")]
    turn: usize,
    #[serde(default)]
    baseline: Baseline,
    #[serde(flatten)]
    kind: CheckKind,
}

fn first_turn() -> usize {
    1
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Baseline {
    #[default]
    Initial,
    Previous,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CheckKind {
    RepoUnchanged,
    SessionUnchanged,
    FileUnchanged {
        path: String,
    },
    TaskInventoryUnchanged,
    BoardMove {
        entry: String,
        from: String,
        to: String,
    },
    SectionUpdated {
        path: String,
        section: String,
        contains: Vec<String>,
    },
    SectionUnchanged {
        path: String,
        section: String,
    },
    CommandOutput {
        argv: Vec<String>,
        stdout: String,
    },
    OtherSessionsUnchanged,
    HumanTrace {
        expected: String,
    },
    HumanReply,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    Human,
}

impl CheckStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Human => "HUMAN",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub status: CheckStatus,
    pub expected: String,
    pub observed: String,
    pub evidence: String,
}

impl CheckResult {
    fn checked(ok: bool, expected: String, observed: String, evidence: String) -> Self {
        Self {
            status: if ok {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            },
            expected,
            observed,
            evidence,
        }
    }
}

pub struct CheckContext<'a> {
    pub sandbox: &'a Sandbox,
    pub initial: &'a Snapshot,
    pub previous: Option<&'a Snapshot>,
    pub current: &'a Snapshot,
    pub dir: &'a Path,
}

impl Check {
    pub fn validate(&self, turn_count: usize) -> Result<()> {
        ensure!(
            (1..=turn_count).contains(&self.turn),
            "turn is out of range"
        );
        ensure!(
            !matches!(self.baseline, Baseline::Previous) || self.turn > 1,
            "turn 1 has no previous snapshot"
        );
        match &self.kind {
            CheckKind::FileUnchanged { path }
            | CheckKind::SectionUpdated { path, .. }
            | CheckKind::SectionUnchanged { path, .. } => {
                relative_path(path)?;
            }
            CheckKind::BoardMove { entry, from, to } => ensure!(
                !entry.is_empty()
                    && from != to
                    && Status::from_label(from).is_some()
                    && Status::from_label(to).is_some(),
                "board move needs an entry and two distinct board statuses"
            ),
            CheckKind::CommandOutput { argv, .. } => ensure!(
                argv.first().is_some_and(|arg| !arg.is_empty()),
                "missing command"
            ),
            _ => {}
        }
        Ok(())
    }

    pub fn applies(&self, turn: usize) -> bool {
        self.turn == turn
    }

    pub fn evaluate(&self, context: &CheckContext<'_>, index: usize) -> Result<CheckResult> {
        let baseline = match self.baseline {
            Baseline::Initial => context.initial,
            Baseline::Previous => context.previous.context("missing previous snapshot")?,
        };
        let current = context.current;
        let evidence = |path| format!("{} and {}", baseline.evidence(path), current.evidence(path));
        let result = match &self.kind {
            CheckKind::RepoUnchanged => {
                let mut changes = differences(&baseline.repo, &current.repo);
                changes.extend(
                    differences(&baseline.git, &current.git)
                        .into_iter()
                        .map(|change| format!("git {change}")),
                );
                unchanged(
                    "repo files and Git state",
                    changes,
                    format!("{} and {}", evidence("repo"), evidence("git")),
                )
            }
            CheckKind::SessionUnchanged => unchanged(
                "session files",
                differences(&baseline.session, &current.session),
                evidence("session"),
            ),
            CheckKind::FileUnchanged { path } => {
                let key = Path::new(path);
                let old = baseline
                    .session
                    .get(key)
                    .with_context(|| format!("missing baseline file {path}"))?;
                let new = current.session.get(key);
                unchanged(
                    path,
                    difference(key, Some(old), new).into_iter().collect(),
                    evidence(&format!("session/{path}")),
                )
            }
            CheckKind::TaskInventoryUnchanged => {
                let initial = task_inventory(&baseline.session);
                let current_inventory = task_inventory(&current.session);
                CheckResult::checked(
                    initial == current_inventory,
                    format!("task files {initial:?}"),
                    format!("task files {current_inventory:?}"),
                    evidence("session/tasks"),
                )
            }
            CheckKind::BoardMove { entry, from, to } => {
                let before = board_positions(baseline, entry);
                ensure!(
                    before == [from.as_str()],
                    "expected {entry} in {from} at baseline, found {before:?}"
                );
                let after = board_positions(current, entry);
                CheckResult::checked(
                    after == [to.as_str()],
                    format!("{entry} moves from {from} to {to}"),
                    format!("{entry} in {after:?}"),
                    evidence("session/tasks/INDEX.md"),
                )
            }
            CheckKind::SectionUpdated {
                path,
                section,
                contains,
            } => {
                let before = read_section(baseline, path, section);
                let after = read_section(current, path, section);
                let ok = after.as_ref().is_some_and(|body| {
                    !body.trim().is_empty() && contains.iter().all(|part| body.contains(part))
                }) && before != after;
                CheckResult::checked(
                    ok,
                    format!("{path} {section} updated with {contains:?}"),
                    format!("section before {before:?}, after {after:?}"),
                    evidence(&format!("session/{path}")),
                )
            }
            CheckKind::SectionUnchanged { path, section } => {
                let before = read_section(baseline, path, section)
                    .with_context(|| format!("missing baseline section {section} in {path}"))?;
                let after = read_section(current, path, section);
                CheckResult::checked(
                    after.as_ref() == Some(&before),
                    format!("{path} {section} stays {before:?}"),
                    format!("section is {after:?}"),
                    evidence(&format!("session/{path}")),
                )
            }
            CheckKind::CommandOutput { argv, stdout } => {
                command_check(context, index, argv, stdout)?
            }
            CheckKind::OtherSessionsUnchanged => unchanged(
                "other session files",
                differences(&baseline.other_sessions, &current.other_sessions),
                evidence("other-sessions"),
            ),
            CheckKind::HumanTrace { expected } => CheckResult {
                status: CheckStatus::Human,
                expected: expected.clone(),
                observed: "tool trace captured".into(),
                evidence: format!("turn-{:02}/events.jsonl", self.turn),
            },
            CheckKind::HumanReply => CheckResult {
                status: CheckStatus::Human,
                expected: "review the reply for relevance and accuracy".into(),
                observed: "reply captured".into(),
                evidence: format!("turn-{:02}/reply.md", self.turn),
            },
        };
        Ok(result)
    }
}

fn command_check(
    context: &CheckContext<'_>,
    index: usize,
    argv: &[String],
    expected: &str,
) -> Result<CheckResult> {
    let stdout = format!("check-{index:02}.stdout.txt");
    let stderr = format!("check-{index:02}.stderr.txt");
    let mut command = context.sandbox.read_only_command();
    // The fixture's command can execute code written by the evaluated agent.
    command.args(argv);
    let status = run_process(
        command,
        Stdio::null(),
        File::create(context.dir.join(&stdout))?,
        File::create(context.dir.join(&stderr))?,
        CHECK_TIMEOUT,
    )
    .with_context(|| format!("running check {argv:?}"))?;
    let output = fs::read(context.dir.join(&stdout))?;
    Ok(CheckResult::checked(
        status.success() && output == expected.as_bytes(),
        format!("{argv:?} exits 0 and prints {expected:?}"),
        format!("{status}, stdout {:?}", String::from_utf8_lossy(&output)),
        format!(
            "{label}/{stdout} and {label}/{stderr}",
            label = context
                .dir
                .file_name()
                .context("turn directory name")?
                .to_string_lossy()
        ),
    ))
}

fn board_positions(snapshot: &Snapshot, entry: &str) -> Vec<&'static str> {
    let board = parse_tasks_board(snapshot.text(BOARD).unwrap_or_default());
    board
        .sections()
        .into_iter()
        .flat_map(|(entries, status)| {
            entries
                .iter()
                .filter(|line| entry_head(line) == entry)
                .map(move |_| status.label())
        })
        .collect()
}

fn read_section(snapshot: &Snapshot, path: &str, title: &str) -> Option<String> {
    let text = snapshot.text(path)?;
    let mut lines = text.lines();
    lines.find(|line| {
        line.strip_prefix("## ")
            .is_some_and(|heading| heading.trim() == title)
    })?;
    Some(
        lines
            .take_while(|line| !line.starts_with("## "))
            .collect::<Vec<_>>()
            .join("\n")
            .trim_matches('\n')
            .to_owned(),
    )
}

fn task_inventory(tree: &Tree) -> BTreeSet<&Path> {
    tree.keys()
        .map(PathBuf::as_path)
        .filter(|path| {
            path.parent() == Some(Path::new("tasks"))
                && path.extension().is_some_and(|ext| ext == "md")
                && *path != Path::new(BOARD)
        })
        .collect()
}

fn unchanged(label: &str, changes: Vec<String>, evidence: String) -> CheckResult {
    let ok = changes.is_empty();
    CheckResult::checked(
        ok,
        format!("{label} unchanged"),
        if ok {
            "unchanged".into()
        } else {
            changes.join(", ")
        },
        evidence,
    )
}

fn difference<T: PartialEq>(path: &Path, old: Option<&T>, new: Option<&T>) -> Option<String> {
    if old == new {
        return None;
    }
    let change = match (old, new) {
        (None, _) => "added",
        (_, None) => "removed",
        _ => "changed",
    };
    Some(format!("{change} {}", path.display()))
}

fn differences<T: PartialEq>(
    old: &BTreeMap<PathBuf, T>,
    new: &BTreeMap<PathBuf, T>,
) -> Vec<String> {
    let paths: BTreeSet<_> = old.keys().chain(new.keys()).collect();
    paths
        .into_iter()
        .filter_map(|path| difference(path, old.get(path), new.get(path)))
        .collect()
}

#[derive(PartialEq)]
enum Entry {
    Directory(u32),
    File { bytes: Vec<u8>, mode: u32 },
    Symlink(PathBuf),
}

type Tree = BTreeMap<PathBuf, Entry>;

pub struct Snapshot {
    root: PathBuf,
    repo: Tree,
    session: Tree,
    other_sessions: Tree,
    git: BTreeMap<PathBuf, Vec<u8>>,
}

impl Snapshot {
    pub fn capture(sandbox: &Sandbox, dest: &Path) -> Result<Self> {
        let mut repo = Tree::new();
        let mut session = Tree::new();
        snapshot_tree(&sandbox.repo, &dest.join("repo"), Path::new(""), &mut repo)?;
        snapshot_tree(
            &sandbox.session,
            &dest.join("session"),
            Path::new(""),
            &mut session,
        )?;
        let sessions = sandbox.session.parent().context("sessions directory")?;
        let mut other_sessions = Tree::new();
        let other_dest = dest.join("other-sessions");
        fs::create_dir_all(&other_dest)?;
        for entry in fs::read_dir(sessions)? {
            let entry = entry?;
            if entry.path() != sandbox.session && entry.file_type()?.is_dir() {
                let prefix = PathBuf::from(entry.file_name());
                snapshot_tree(sessions, &other_dest, &prefix, &mut other_sessions)?;
                other_sessions.insert(
                    prefix,
                    Entry::Directory(entry.metadata()?.permissions().mode() & 0o777),
                );
            }
        }
        let git_dir = dest.join("git");
        fs::create_dir(&git_dir)?;
        let mut git = BTreeMap::new();
        for (name, args) in [
            (
                "status.txt",
                &[
                    "status",
                    "--porcelain=v1",
                    "--branch",
                    "--untracked-files=all",
                ][..],
            ),
            ("refs.txt", &["show-ref", "--head"][..]),
            (
                "staged.patch",
                &[
                    "diff",
                    "--cached",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--binary",
                    "HEAD",
                ][..],
            ),
            (
                "unstaged.patch",
                &["diff", "--no-ext-diff", "--no-textconv", "--binary"][..],
            ),
            ("worktrees.txt", &["worktree", "list", "--porcelain"][..]),
        ] {
            let mut command = sandbox.read_only_command();
            command
                .args(["git", "--no-optional-locks", "-c", "core.fsmonitor=false"])
                .args(args);
            let stdout = git_dir.join(name);
            let stderr = git_dir.join(format!("{name}.stderr"));
            let status = run_process(
                command,
                Stdio::null(),
                File::create(&stdout)?,
                File::create(&stderr)?,
                CHECK_TIMEOUT,
            )
            .with_context(|| format!("capturing git {args:?}; see {}", stderr.display()))?;
            ensure!(
                status.success(),
                "capturing git {args:?} exited with {status}; see {}",
                stderr.display()
            );
            git.insert(PathBuf::from(name), fs::read(stdout)?);
        }
        Ok(Self {
            root: dest.to_owned(),
            repo,
            session,
            other_sessions,
            git,
        })
    }

    fn evidence(&self, path: &str) -> String {
        self.root.join(path).display().to_string()
    }

    fn text(&self, path: &str) -> Option<&str> {
        match self.session.get(Path::new(path))? {
            Entry::File { bytes, .. } => std::str::from_utf8(bytes).ok(),
            _ => None,
        }
    }
}

fn snapshot_tree(source: &Path, dest: &Path, prefix: &Path, entries: &mut Tree) -> Result<()> {
    fs::create_dir_all(dest.join(prefix))?;
    for child in fs::read_dir(source.join(prefix))? {
        let child = child?;
        if child.file_name() == ".git" {
            continue;
        }
        let relative = prefix.join(child.file_name());
        let kind = child.file_type()?;
        let mode = || {
            child
                .metadata()
                .map(|metadata| metadata.permissions().mode() & 0o777)
        };
        let entry = if kind.is_dir() {
            snapshot_tree(source, dest, &relative, entries)?;
            Entry::Directory(mode()?)
        } else if kind.is_file() {
            let bytes = fs::read(child.path())?;
            fs::write(dest.join(&relative), &bytes)?;
            let mode = mode()?;
            fs::set_permissions(
                dest.join(&relative),
                fs::Permissions::from_mode(mode & 0o755 | 0o600),
            )?;
            Entry::File { bytes, mode }
        } else if kind.is_symlink() {
            let target = fs::read_link(child.path())?;
            fs::write(
                dest.join(&relative),
                format!("symlink to {}\n", target.display()),
            )?;
            Entry::Symlink(target)
        } else {
            anyhow::bail!("unsupported fixture entry {}", child.path().display());
        };
        entries.insert(relative, entry);
    }
    Ok(())
}
