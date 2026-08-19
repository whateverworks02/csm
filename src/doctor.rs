//! Health check + guided repair for csm sessions and agent wiring.
//!
//! Modelled on `flutter doctor` / `brew doctor` / `expo doctor` / `npm doctor`:
//! a triage command that says whether things are OK, pinpoints what's wrong,
//! and tells you what to do (or does it via `--fix`).
//!
//! - Read-only by default; `--fix` repairs fixable issues (per-finding confirm,
//!   `--fix --yes` skips prompts for CI).
//! - Each check is one row with a status: `ok` / `warn` / `error`. All checks
//!   are shown (grouped by category) so the checked surface area is visible.
//! - Severity is meaningful: self-healing gaps (ghosts, incomplete scaffolds)
//!   are `warn` - they resolve on the next `csm <name>` start because
//!   [`workspace::ensure_workspace`] is idempotent. Missing wiring, malformed
//!   settings, an unwritable `~/.csm`, and orphan dirs are `error`.
//! - Wiring is never mutated here (only diagnosed); repair is `csm init`'s job.

use crate::inject;
use crate::skills;
use crate::store;
use crate::ui;
use crate::workspace;
use anyhow::Result;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn run(fix: bool, yes: bool) -> Result<()> {
    let checks = collect_checks();
    let has_finding = checks.iter().any(|c| !matches!(c.status, Status::Ok));

    // Always print the full report (every check, grouped) - like flutter/brew
    // doctor, the checked surface area should be visible, not hidden behind a
    // healthy one-liner.
    report(&checks, false);

    if !has_finding {
        println!();
        println!("{}", ui::paint(ui::GREEN_BOLD, "csm is healthy."));
        return Ok(());
    }

    if fix {
        let fixed = apply_fixable(&checks, yes)?;
        if fixed {
            eprintln!();
            eprintln!("{}", ui::epaint(ui::DIM, "re-running checks..."));
            let remaining = collect_checks();
            // Re-run prints only remaining findings - wiring is unchanged by
            // `--fix` and was already shown above.
            report(&remaining, true);
            if remaining.iter().all(|c| matches!(c.status, Status::Ok)) {
                println!();
                println!("{}", ui::paint(ui::GREEN_BOLD, "csm is healthy."));
                return Ok(());
            }
            summarize_and_exit(&remaining, false);
        } else {
            summarize_and_exit(&checks, false);
        }
    } else {
        summarize_and_exit(&checks, true);
    }
}

fn collect_checks() -> Vec<Check> {
    let mut out = Vec::new();
    consistency_checks(&mut out);
    wiring_checks(&mut out);
    out
}

// --- report ----------------------------------------------------------------

/// Print checks grouped by category. When `only_findings` is true, omit ok rows
/// (used for the concise re-run after `--fix`, so unchanged wiring isn't reprinted).
fn report(checks: &[Check], only_findings: bool) {
    for cat in [Category::Consistency, Category::Wiring] {
        let in_cat: Vec<&Check> = checks
            .iter()
            .filter(|c| c.category == cat && (!only_findings || !matches!(c.status, Status::Ok)))
            .collect();
        print_group(cat, &in_cat);
    }
}

fn print_group(cat: Category, checks: &[&Check]) {
    if checks.is_empty() {
        return;
    }
    println!();
    println!("{}", ui::paint(ui::BOLD, cat.label()));
    let lw = checks.iter().map(|c| c.label.len()).max().unwrap_or(0);
    for c in checks {
        let label = ui::paint(ui::CYAN_BOLD, &format!("{:<lw$}", c.label, lw = lw));
        println!(
            "  {}  {}  {}",
            format_status(c.status),
            label,
            ui::paint(ui::DIM, &c.detail)
        );
    }
}

fn format_status(status: Status) -> String {
    let (word, style) = match status {
        Status::Ok => ("ok", ui::GREEN),
        Status::Warn => ("warn", ui::YELLOW),
        Status::Error => ("error", ui::RED_BOLD),
    };
    ui::paint(style, &format!("{:<5}", word))
}

fn summarize_and_exit(checks: &[Check], hint_fix: bool) -> ! {
    let errors = checks
        .iter()
        .filter(|c| matches!(c.status, Status::Error))
        .count();
    let warns = checks
        .iter()
        .filter(|c| matches!(c.status, Status::Warn))
        .count();
    println!();
    println!(
        "{}",
        ui::paint(ui::DIM, &format!("{errors} error(s), {warns} warning(s)."))
    );
    if hint_fix && checks.iter().any(|c| c.fix.is_some()) {
        println!(
            "{}",
            ui::paint(
                ui::DIM,
                "Run `csm doctor --fix` to repair (`--fix --yes` skips prompts)."
            ),
        );
    }
    std::process::exit(1);
}

// --- consistency -----------------------------------------------------------

fn consistency_checks(out: &mut Vec<Check>) {
    let idx = store::load_index().unwrap_or_default();

    for (name, meta) in &idx.sessions {
        let dir = match store::session_dir(name) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if !dir.exists() {
            out.push(Check {
                category: Category::Consistency,
                label: name.clone(),
                status: Status::Warn,
                detail: "ghost - self-heals on next start (no directory)".into(),
                fix: Some(Fix::Ghost {
                    name: name.clone(),
                    meta: meta.clone(),
                }),
            });
            continue;
        }
        let missing: Vec<&str> = workspace::expected_files()
            .filter(|f| !dir.join(f).exists())
            .collect();
        if !missing.is_empty() {
            out.push(Check {
                category: Category::Consistency,
                label: name.clone(),
                status: Status::Warn,
                detail: format!("self-heals on next start (missing {})", missing.join(", ")),
                fix: Some(Fix::Incomplete {
                    name: name.clone(),
                    meta: meta.clone(),
                }),
            });
        }
    }

    // Orphans: dirs under ~/.csm/sessions/ with no index entry. These don't
    // self-heal (you'd never start an untracked session), so they're an error
    // with a manual remedy - not fixable by `--fix` (auto-removing user dirs
    // is too destructive).
    for (fname, path) in orphan_dirs(&idx) {
        out.push(Check {
            category: Category::Consistency,
            label: fname,
            status: Status::Error,
            detail: format!(
                "orphan - not tracked; inspect, then `rm -rf {}` if unwanted",
                ui::abbrev_path(&path)
            ),
            fix: None,
        });
    }

    // If everything's fine, add one ok row so the report shows the checked surface.
    if !out.iter().any(|c| c.category == Category::Consistency) {
        let n = idx.sessions.len();
        out.push(Check {
            category: Category::Consistency,
            label: "sessions".into(),
            status: Status::Ok,
            detail: if n == 0 {
                "none".into()
            } else {
                format!("{n} total, all fine")
            },
            fix: None,
        });
    }
}

// --- wiring (report-only; repair is `csm init`) ----------------------------

/// Dirs under ~/.csm/sessions/ with no index entry, as (dir_name, path).
fn orphan_dirs(idx: &store::Index) -> Vec<(String, PathBuf)> {
    let Ok(sessions_dir) = store::sessions_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&sessions_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                return None;
            }
            let fname = e.file_name().to_string_lossy().to_string();
            if idx.sessions.contains_key(&fname) {
                return None;
            }
            Some((fname, e.path()))
        })
        .collect()
}

const INIT_HINT: &str = "run `csm init`";

/// Construct a wiring check - all wiring checks are report-only (`fix: None`).
fn wiring(label: &str, status: Status, detail: String) -> Check {
    Check {
        category: Category::Wiring,
        label: label.into(),
        status,
        detail,
        fix: None,
    }
}

/// Wiring check: does the SessionStart hook file at `path` (Claude
/// `settings.json` or codex `hooks.json` - same hooks shape) contain the csm
/// hook entry? `file_desc` labels the malformed-JSON error (e.g.
/// "settings.json"). Report-only.
fn check_hook(label: &str, path: Option<PathBuf>, file_desc: &str) -> Check {
    let (status, detail) = match path {
        None => (Status::Error, INIT_HINT.to_string()),
        Some(p) => match fs::read_to_string(&p) {
            Ok(data) => match serde_json::from_str::<serde_json::Value>(&data) {
                Ok(v) if inject::sessionstart_hook_present(&v) => (Status::Ok, ui::abbrev_path(&p)),
                Ok(_) => (Status::Error, INIT_HINT.into()),
                Err(e) => (Status::Error, format!("{file_desc} malformed: {e}")),
            },
            Err(_) => (Status::Error, INIT_HINT.to_string()),
        },
    };
    wiring(label, status, detail)
}

/// Wiring check: does the agent context file at `path` contain the csm prompt
/// block? Report-only. (pi uses a `Warn`-on-miss variant with a different
/// detail format, kept inline in [`wiring_checks`].)
fn check_prompt(label: &str, path: Option<PathBuf>) -> Check {
    let (ok, detail) = match path {
        Some(p) if inject::prompt_block_present(&p) => (true, ui::abbrev_path(&p)),
        _ => (false, INIT_HINT.into()),
    };
    wiring(label, if ok { Status::Ok } else { Status::Error }, detail)
}

/// Wiring check: is the skill deployed at the Claude surface and current
/// (`skills::skill_current` - the predicate `skills::deploy` writes against, so
/// install and diagnose cannot drift)? A stale copy after an upgrade is a
/// finding, not just a missing one. The vendor-neutral `~/.csm/skills/<file>`
/// deploys through the same code path, so the Claude surface is the canary.
/// Report-only.
fn check_skill(skill: &skills::SkillSpec, path: Option<PathBuf>) -> Check {
    let (ok, detail) = match path {
        Some(p) if skills::skill_current(&p, skill.md) => (true, ui::abbrev_path(&p)),
        // Present but not the const: stale (post-upgrade, user-edited) or
        // unreadable (permissions) - either way `csm init` is the remedy.
        Some(p) if p.exists() => (false, format!("stale or unreadable; {INIT_HINT}")),
        _ => (false, INIT_HINT.into()),
    };
    wiring(
        &format!("{} skill", skill.id),
        if ok { Status::Ok } else { Status::Error },
        detail,
    )
}

fn wiring_checks(out: &mut Vec<Check>) {
    // SessionStart hook in ~/.claude/settings.json.
    out.push(check_hook(
        "SessionStart hook",
        inject::claude_dir().ok().map(|d| d.join("settings.json")),
        "settings.json",
    ));

    // csm prompt block in ~/.claude/CLAUDE.md.
    out.push(check_prompt("claude prompt", inject::claude_md_path().ok()));

    // csm skills at the Claude surface (missing or stale -> `csm init`).
    for skill in skills::SKILLS {
        out.push(check_skill(skill, skills::claude_skill_path(skill.id).ok()));
    }

    // pi prompt block - only if pi is installed (~/.pi/agent exists).
    if inject::pi_dir().ok().is_some_and(|d| d.exists()) {
        let (pi_ok, detail) = match inject::pi_context_target() {
            Ok(p) if inject::prompt_block_present(&p) => (true, ui::abbrev_path(&p)),
            Ok(p) => (false, format!("{INIT_HINT} ({})", ui::abbrev_path(&p))),
            Err(_) => (false, INIT_HINT.into()),
        };
        out.push(wiring(
            "pi prompt",
            if pi_ok { Status::Ok } else { Status::Warn },
            detail,
        ));
    }

    // codex SessionStart hook + prompt - only if codex is installed
    // (~/.codex exists). The hook file shares Claude's JSON shape, so
    // check_hook / check_prompt are reused.
    if inject::codex_dir().ok().is_some_and(|d| d.exists()) {
        out.push(check_hook(
            "codex hook",
            inject::codex_hooks_path().ok(),
            "hooks.json",
        ));
        out.push(check_prompt(
            "codex prompt",
            inject::codex_agents_target().ok(),
        ));
    }

    // csm on PATH so the hook command `csm hook` resolves.
    let csm_path = inject::which_csm();
    out.push(wiring(
        "csm on PATH",
        if csm_path.is_some() {
            Status::Ok
        } else {
            Status::Error
        },
        match &csm_path {
            Some(p) => ui::abbrev_path(p),
            None => "install csm on PATH".into(),
        },
    ));

    // Smoke-test: run the `csm` the hook actually invokes (PATH-resolved, not
    // `current_exe()`) with no active session; ok iff it exits 0. Stronger than
    // string-matching settings.json - catches a renamed/broken subcommand or a
    // stale binary on PATH. Skipped when csm isn't on PATH (the `csm on PATH`
    // check above already flags that). Runs with CSM_SESSION unset so it takes
    // the no-op path and mutates nothing.
    if let Some(exe) = &csm_path {
        let smoke_ok = smoke_test_hook(exe);
        out.push(wiring(
            "csm hook runs",
            if smoke_ok { Status::Ok } else { Status::Error },
            if smoke_ok {
                "exit 0".into()
            } else {
                "subprocess failed".into()
            },
        ));
    }

    // ~/.csm writable (csm must be able to store sessions).
    let (writable, detail) = writable_check();
    out.push(wiring(
        "~/.csm writable",
        if writable { Status::Ok } else { Status::Error },
        detail,
    ));
}

/// Run `<exe> hook` with no active session; true iff it exits 0. `exe` is the
/// PATH-resolved csm (what the SessionStart hook invokes), not `current_exe()` -
/// so a stale/broken csm on PATH is caught even when doctor is run from a
/// different binary.
fn smoke_test_hook(exe: &Path) -> bool {
    Command::new(exe)
        .arg("hook")
        .env_remove("CSM_SESSION")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Can csm write to `~/.csm`? Returns (ok, detail).
fn writable_check() -> (bool, String) {
    let home = match store::csm_home() {
        Ok(h) => h,
        Err(_) => return (false, "HOME not set".into()),
    };
    // If ~/.csm doesn't exist yet, that's fine - it's created on the first
    // session (touch_session -> save_index create_dir_all). Only probe when it
    // already exists, to keep doctor side-effect-free.
    if !home.exists() {
        return (
            true,
            format!("{} (created on first session)", ui::abbrev_path(&home)),
        );
    }
    let probe = home.join(format!(".doctor-probe-{}", std::process::id()));
    let ok = fs::write(&probe, "").is_ok();
    if ok {
        let _ = fs::remove_file(&probe);
        (true, ui::abbrev_path(&home))
    } else {
        (false, format!("can't write to {}", ui::abbrev_path(&home)))
    }
}

// --- fix flow --------------------------------------------------------------

fn apply_fixable(checks: &[Check], yes: bool) -> Result<bool> {
    let mut any = false;
    for c in checks {
        if let Some(f) = c.fix.as_ref() {
            any |= apply_fix(f, yes)?;
        }
    }
    Ok(any)
}

fn fix_header(kind: &str, name: &str) {
    eprintln!();
    eprintln!("{}", ui::epaint(ui::BOLD, &format!("{kind} `{name}`:")));
}

fn apply_fix(fix: &Fix, yes: bool) -> Result<bool> {
    match fix {
        Fix::Ghost { name, meta } => {
            fix_header("ghost", name);
            let action = if yes {
                's'
            } else {
                eprint!(
                    "{} ",
                    ui::epaint(ui::DIM, "[s]caffold / [r]emove / [S]kip?")
                );
                std::io::stderr().flush()?;
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                line.trim().chars().next().unwrap_or(' ')
            };
            match action {
                's' => {
                    workspace::ensure_workspace(name, meta)?;
                    ui::done("scaffolded", name);
                    Ok(true)
                }
                'r' => {
                    store::delete_session(name)?;
                    ui::done("removed", name);
                    Ok(true)
                }
                _ => {
                    eprintln!("{}", ui::epaint(ui::DIM, "skipped"));
                    Ok(false)
                }
            }
        }
        Fix::Incomplete { name, meta } => {
            fix_header("incomplete", name);
            let do_fix = yes || ui::confirm(&format!("fill missing files for `{name}`?"))?;
            if do_fix {
                workspace::ensure_workspace(name, meta)?;
                ui::done("filled", name);
                Ok(true)
            } else {
                eprintln!("{}", ui::epaint(ui::DIM, "skipped"));
                Ok(false)
            }
        }
    }
}

// --- types -----------------------------------------------------------------

#[derive(PartialEq)]
enum Category {
    Consistency,
    Wiring,
}

impl Category {
    fn label(&self) -> &'static str {
        match self {
            Category::Consistency => "consistency",
            Category::Wiring => "wiring",
        }
    }
}

#[derive(Clone, Copy)]
enum Status {
    Ok,
    Warn,
    Error,
}

enum Fix {
    Ghost {
        name: String,
        meta: store::SessionMeta,
    },
    Incomplete {
        name: String,
        meta: store::SessionMeta,
    },
}

struct Check {
    category: Category,
    label: String,
    status: Status,
    detail: String,
    fix: Option<Fix>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scaffold_session, with_csm_home};
    use serial_test::serial;
    use std::fs;

    /// Run consistency checks against the isolated `$CSM_HOME`.
    /// (`doctor::run` itself `process::exit(1)`s on findings and runs wiring
    /// checks that hit `~/.claude`/PATH/`which csm`/a `csm hook` subprocess -
    /// not testable in-process. The consistency + fix logic is exercised here
    /// via the private fns, which `use super::*` reaches.)
    fn consistency() -> Vec<Check> {
        let mut out = Vec::new();
        consistency_checks(&mut out);
        out
    }

    #[test]
    fn skill_check_flags_missing_stale_and_current() {
        // check_skill is pure over a spec + path, so no env isolation needed.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("SKILL.md");
        let scout = &skills::SKILLS[1];
        // Missing.
        assert!(matches!(
            check_skill(scout, Some(path.clone())).status,
            Status::Error
        ));
        // Stale (post-upgrade or user-edited copy).
        fs::write(&path, "old version").unwrap();
        let stale = check_skill(scout, Some(path.clone()));
        assert!(matches!(stale.status, Status::Error));
        assert!(stale.detail.contains("stale"));
        // Current.
        fs::write(&path, skills::SCOUT_SKILL_MD).unwrap();
        assert!(matches!(
            check_skill(scout, Some(path.clone())).status,
            Status::Ok
        ));
        // Unresolvable path.
        assert!(matches!(check_skill(scout, None).status, Status::Error));
    }

    #[test]
    #[serial]
    fn healthy_session_is_all_ok() {
        with_csm_home(|_dir| {
            scaffold_session("ws");
            let checks = consistency();
            assert!(
                checks.iter().all(|c| matches!(c.status, Status::Ok)),
                "healthy session should yield only ok checks"
            );
        });
    }

    #[test]
    #[serial]
    fn detects_ghost_session() {
        with_csm_home(|_dir| {
            // Index entry, no workspace dir -> ghost.
            store::touch_session("ghost", "/o").unwrap();
            assert!(!store::session_dir("ghost").unwrap().exists());
            let checks = consistency();
            let ghost = checks
                .iter()
                .find(|c| matches!(c.fix, Some(Fix::Ghost { .. })))
                .expect("ghost should be detected");
            assert!(matches!(ghost.status, Status::Warn));
        });
    }

    #[test]
    #[serial]
    fn detects_incomplete_session() {
        with_csm_home(|_dir| {
            scaffold_session("inc");
            let dir = store::session_dir("inc").unwrap();
            fs::remove_file(dir.join("state.md")).unwrap();
            let checks = consistency();
            let inc = checks
                .iter()
                .find(|c| matches!(c.fix, Some(Fix::Incomplete { .. })))
                .expect("missing-file session should be detected as incomplete");
            assert!(matches!(inc.status, Status::Warn));
        });
    }

    #[test]
    #[serial]
    fn detects_orphan_dir_as_error_not_fixable() {
        with_csm_home(|_dir| {
            // A session dir with no index entry.
            fs::create_dir_all(store::sessions_dir().unwrap().join("lonely")).unwrap();
            let checks = consistency();
            let orphan = checks
                .iter()
                .find(|c| c.label == "lonely")
                .expect("untracked dir should be flagged");
            assert!(matches!(orphan.status, Status::Error));
            assert!(orphan.fix.is_none(), "orphan must not be auto-fixable");
        });
    }

    #[test]
    #[serial]
    fn fix_ghost_scaffolds_and_is_idempotent() {
        with_csm_home(|_dir| {
            let _meta = store::touch_session("ghost", "/o").unwrap();
            assert!(!store::session_dir("ghost").unwrap().exists());
            // Detect -> apply the detected fix (end-to-end via `--yes`).
            let checks = consistency();
            let fix = checks
                .iter()
                .find_map(|c| c.fix.as_ref())
                .expect("ghost should produce a fix");
            assert!(
                apply_fix(fix, true).unwrap(),
                "yes=true scaffolds the ghost"
            );
            // Workspace now complete: no ghost, no incomplete.
            let after = consistency();
            assert!(
                after.iter().all(|c| matches!(c.status, Status::Ok)),
                "after fix the session should be healthy"
            );
            // Idempotent: re-running finds nothing left to fix.
            assert!(
                consistency().iter().find_map(|c| c.fix.as_ref()).is_none(),
                "no fixable finding should remain after repair"
            );
        });
    }

    #[test]
    #[serial]
    fn fix_incomplete_fills_missing_and_is_idempotent() {
        with_csm_home(|_dir| {
            scaffold_session("inc");
            let dir = store::session_dir("inc").unwrap();
            fs::remove_file(dir.join("state.md")).unwrap();
            fs::remove_file(dir.join("tasks/INDEX.md")).unwrap();
            let checks = consistency();
            let fix = checks
                .iter()
                .find_map(|c| c.fix.as_ref())
                .expect("incomplete should produce a fix");
            assert!(
                apply_fix(fix, true).unwrap(),
                "yes=true fills missing files"
            );
            // Missing files restored; pre-existing files (scripts/notes) kept.
            assert!(dir.join("state.md").exists());
            assert!(dir.join("tasks/INDEX.md").exists());
            assert!(
                dir.join("scripts/INDEX.md").exists(),
                "pre-existing files kept"
            );
            // Idempotent: re-running finds nothing left to fix.
            assert!(
                consistency().iter().find_map(|c| c.fix.as_ref()).is_none(),
                "no fixable finding should remain after repair"
            );
        });
    }
}
