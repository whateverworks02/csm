//! csm-eval - dev-side behavioral eval runner for the csm working-mode prompt.
//!
//! Not part of the distributed product: this binary exists to replay scripted
//! scenarios against a hermetic sandbox (fresh HOME / CSM_HOME / CODEX_HOME,
//! the working-tree [csm] block deployed by the real `csm init`, a tiny
//! fixture repo) and capture full traces per turn. The unit of evaluation is
//! the scenario; the headline number is a delta between contrast arms, never
//! an absolute score.
//!
//! Four subcommands: `run` drives the scripted turns (model calls) and judges
//! them, `judge` re-judges a recorded run (model calls, no turns), `grade`
//! re-applies both layers to what a run recorded (no model calls), and `diff`
//! turns two runs' adherence matrices into the delta the deploy loop gates on.

mod arms;
mod artifacts;
mod checks;
mod driver;
mod judge;
mod report;
mod sandbox;
mod scenario;

use anyhow::{bail, Context, Result};
use artifacts::RunDir;
use checks::{ChecksFile, RepDir};
use clap::{Parser, Subcommand};
use driver::driver_for;
use judge::JudgeCell;
use report::{BehaviorGrade, GradedRep, GradedScenario, JudgeCellRef, Report};
use scenario::{Arm, Harness, Scenario, SplitSelect};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Exit code for a `diff` that found a regression.
const EXIT_REGRESSION: i32 = 1;
/// Exit code for a run whose turns did not all complete.
const EXIT_TURN_FAILED: i32 = 2;
/// Exit code for a run that broke hermeticity.
const EXIT_HERMETICITY: i32 = 3;

#[derive(Parser)]
#[command(
    name = "csm-eval",
    about = "Dev-side behavioral eval runner for the csm working-mode prompt",
    after_help = "Not installed by anything and not part of the product; build from source with `cargo build`."
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run scenarios end to end: build a sandbox per repeat, deploy the arm,
    /// drive the scripted turns headless, capture per-turn artifacts, assert
    /// hermeticity, and grade.
    Run {
        /// Scenario name; without it, the corpus `--split` and `--filter`
        /// select.
        scenario: Option<String>,
        /// Contrast arm.
        #[arg(long, default_value = "working")]
        arm: String,
        /// Harness to drive (claude, codex).
        #[arg(long, default_value = "claude")]
        harness: String,
        /// Pin the model id; without it the harness's default resolution
        /// applies (recorded from the trace either way).
        #[arg(long)]
        model: Option<String>,
        /// Repeats per scenario; the behavior verdict is the majority.
        #[arg(long, default_value_t = 3)]
        k: usize,
        /// Which half of the corpus to run.
        #[arg(long, default_value = "dev")]
        split: String,
        /// Only scenarios carrying this tag.
        #[arg(long)]
        filter: Option<String>,
        /// Per-turn wall-clock cap in seconds; also caps one judge call.
        #[arg(long, default_value_t = 900)]
        timeout_secs: u64,
        /// Keep the sandbox for inspection after the run.
        #[arg(long)]
        keep_sandbox: bool,
        /// Repo root (defaults to walking up from the cwd).
        #[arg(long)]
        repo_root: Option<PathBuf>,
        /// Disable the seatbelt write confinement (the post-run filesystem
        /// assertion still runs).
        #[arg(long)]
        no_seatbelt: bool,
        /// Skip the judge layer: grade the deterministic checks only.
        #[arg(long)]
        no_judge: bool,
        /// Harness that runs the judge (defaults to the harness under test).
        #[arg(long)]
        judge_harness: Option<String>,
        /// Model that judges; without it the judge harness's default applies.
        #[arg(long)]
        judge_model: Option<String>,
    },
    /// Re-apply both layers to a run's recorded traces and snapshots and write
    /// its report. Zero model calls: the judge verdicts are the ones the run
    /// recorded.
    Grade {
        /// A run dir (grading every rep inside it) or one rep dir.
        run_dir: PathBuf,
        /// Repo root (defaults to walking up from the cwd).
        #[arg(long)]
        repo_root: Option<PathBuf>,
    },
    /// Judge a recorded run: one call per scenario x rep, no turns re-run.
    Judge {
        /// A run dir (judging every rep inside it) or one rep dir.
        run_dir: PathBuf,
        /// Harness that runs the judge (defaults to the run's own).
        #[arg(long)]
        judge_harness: Option<String>,
        /// Model that judges; without it the judge harness's default applies.
        #[arg(long)]
        judge_model: Option<String>,
        /// Wall-clock cap per judge call, in seconds.
        #[arg(long, default_value_t = 900)]
        timeout_secs: u64,
        /// Repo root (defaults to walking up from the cwd).
        #[arg(long)]
        repo_root: Option<PathBuf>,
    },
    /// Compare two runs' adherence matrices per behavior. Exits non-zero when
    /// the candidate regresses.
    Diff {
        /// The baseline run dir.
        baseline: PathBuf,
        /// The candidate run dir.
        candidate: PathBuf,
    },
}

fn main() {
    if let Err(e) = try_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    match Cli::parse().command {
        Cmd::Run {
            scenario,
            arm,
            harness,
            model,
            k,
            split,
            filter,
            timeout_secs,
            keep_sandbox,
            repo_root,
            no_seatbelt,
            no_judge,
            judge_harness,
            judge_model,
        } => {
            let harness = Harness::parse(&harness)?;
            let judge = judge_cell(
                !no_judge,
                judge_harness.as_deref(),
                judge_model,
                harness,
                Duration::from_secs(timeout_secs),
            )?;
            let code = cmd_run(&RunOpts {
                scenario: scenario.as_deref(),
                arm: Arm::parse(&arm)?,
                harness,
                model: model.as_deref(),
                k,
                split: SplitSelect::parse(&split)?,
                filter: filter.as_deref(),
                timeout: Duration::from_secs(timeout_secs),
                keep_sandbox,
                repo_root: repo_root.as_deref(),
                seatbelt: !no_seatbelt,
                judge,
            })?;
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        Cmd::Grade { run_dir, repo_root } => {
            let repo_root = resolve_repo_root(repo_root.as_deref())?;
            cmd_grade(&run_dir, &repo_root)?;
            Ok(())
        }
        Cmd::Judge {
            run_dir,
            judge_harness,
            judge_model,
            timeout_secs,
            repo_root,
        } => {
            let repo_root = resolve_repo_root(repo_root.as_deref())?;
            let cell = judge_cell(
                true,
                judge_harness.as_deref(),
                judge_model,
                harness_of_run(&run_dir)?,
                Duration::from_secs(timeout_secs),
            )?
            .expect("judging is on");
            cmd_judge(&run_dir, &repo_root, &cell)?;
            Ok(())
        }
        Cmd::Diff {
            baseline,
            candidate,
        } => {
            let (text, regressed) = cmd_diff(&baseline, &candidate)?;
            print!("{text}");
            if regressed {
                std::process::exit(EXIT_REGRESSION);
            }
            Ok(())
        }
    }
}

/// One `run` invocation: the CLI-parsed parameters handed to `cmd_run` as a
/// single argument.
struct RunOpts<'a> {
    scenario: Option<&'a str>,
    arm: Arm,
    harness: Harness,
    model: Option<&'a str>,
    k: usize,
    split: SplitSelect,
    filter: Option<&'a str>,
    timeout: Duration,
    keep_sandbox: bool,
    repo_root: Option<&'a Path>,
    seatbelt: bool,
    /// The judge layer's cell, or `None` under `--no-judge`.
    judge: Option<JudgeCell>,
}

/// The judge cell, or `None` when judging is off. The judge runs on the
/// harness under test unless one is named: it is the same local CLI either way.
fn judge_cell(
    enabled: bool,
    harness: Option<&str>,
    model: Option<String>,
    default_harness: Harness,
    timeout: Duration,
) -> Result<Option<JudgeCell>> {
    if !enabled {
        if harness.is_some() || model.is_some() {
            bail!("--judge-harness/--judge-model cannot be combined with --no-judge");
        }
        return Ok(None);
    }
    Ok(Some(JudgeCell {
        harness: match harness {
            Some(id) => Harness::parse(id)?,
            None => default_harness,
        },
        model,
        timeout,
    }))
}

/// The harness a recorded run was driven with, from its manifest.
fn harness_of_run(run_dir: &Path) -> Result<Harness> {
    let path = run_dir.join("manifest.json");
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    let id = manifest
        .get("harness")
        .and_then(|h| h.as_str())
        .with_context(|| format!("{} names no harness", path.display()))?;
    Harness::parse(id)
}

/// Walk up from `start` to find the csm repo root (Cargo.toml + src/prompt.rs).
fn find_repo_root(start: &Path) -> Result<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join("Cargo.toml").is_file() && d.join("src").join("prompt.rs").is_file() {
            return Ok(d.to_path_buf());
        }
        dir = d.parent();
    }
    anyhow::bail!(
        "no csm repo root (Cargo.toml + src/prompt.rs) above {}; pass --repo-root",
        start.display()
    )
}

fn resolve_repo_root(explicit: Option<&Path>) -> Result<PathBuf> {
    match explicit {
        Some(p) => Ok(p.to_path_buf()),
        None => find_repo_root(&std::env::current_dir().context("getting cwd")?),
    }
}

fn git_line(repo: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The scenarios one run covers: the named one, or the corpus selection.
/// Naming a scenario is explicit, so `--split` and `--filter` do not narrow it.
fn select_scenarios(scenarios_dir: &Path, opts: &RunOpts) -> Result<Vec<(String, Scenario)>> {
    if let Some(name) = opts.scenario {
        let dir = scenarios_dir.join(name);
        if !dir.join("scenario.json").is_file() {
            bail!("no scenario {name:?} under {}", scenarios_dir.display());
        }
        let scenario =
            Scenario::load(&dir).with_context(|| format!("loading scenario {name:?}"))?;
        return Ok(vec![(name.to_string(), scenario)]);
    }
    let all = scenario::discover(scenarios_dir)
        .with_context(|| format!("reading the corpus under {}", scenarios_dir.display()))?;
    let selected: Vec<(String, Scenario)> = all
        .into_iter()
        .filter(|(_, s)| opts.split.accepts(s.split))
        .filter(|(_, s)| {
            opts.filter
                .map(|tag| s.tags.iter().any(|t| t == tag))
                .unwrap_or(true)
        })
        .collect();
    if selected.is_empty() {
        bail!(
            "no scenario under {} matches split={} filter={}",
            scenarios_dir.display(),
            opts.split.id(),
            opts.filter.unwrap_or("(none)")
        );
    }
    Ok(selected)
}

fn cmd_run(opts: &RunOpts) -> Result<i32> {
    if opts.k == 0 {
        bail!("--k must be at least 1");
    }
    let repo_root = resolve_repo_root(opts.repo_root)?;
    let scenarios_dir = repo_root.join("evals").join("scenarios");
    let selected = select_scenarios(&scenarios_dir, opts)?;

    let scope = opts
        .scenario
        .map(str::to_string)
        .unwrap_or_else(|| String::from("corpus"));
    let run_id = format!(
        "{}.{}.{}.{}",
        scope,
        opts.arm.id(),
        opts.harness.id(),
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
    let run = RunDir::create(&repo_root.join("evals").join("runs"), &run_id)?;
    println!("run: {run_id}");
    println!("artifacts: {}", run.path.display());

    // Hermeticity pre-snapshot before anything the run could attribute.
    let home = PathBuf::from(std::env::var("HOME").context("HOME is not set")?);
    let watch = artifacts::Watch::new(&home, &repo_root);
    let before = watch.snapshot()?;

    let agent = driver_for(opts.harness);
    let info = RunInfo {
        repo_root: repo_root.clone(),
        started_at: chrono::Local::now().to_rfc3339(),
        git_sha: git_line(&repo_root, &["rev-parse", "HEAD"]),
        git_dirty: std::process::Command::new("git")
            .arg("--no-optional-locks")
            .arg("-C")
            .arg(&repo_root)
            .args(["status", "--porcelain"])
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false),
        harness_version: agent.version().unwrap_or_else(|e| format!("unknown ({e})")),
        // The arm's block identity, resolved once: it is a fact about the arm,
        // not about any sandbox a rep deploys into.
        block_ref: sandbox::block_ref(&repo_root, opts.arm)?,
    };
    run.log(&format!(
        "run {run_id}: arm {} harness {} k={} split={} filter={}",
        opts.arm.id(),
        info.harness_version,
        opts.k,
        opts.split.id(),
        opts.filter.unwrap_or("(none)")
    ));

    // Every check set loads before the first model call: a corpus that cannot
    // be graded should fail before it has spent anything.
    let checks: Vec<ChecksFile> = selected
        .iter()
        .map(|(name, _)| ChecksFile::load(&Scenario::checks_path(&scenarios_dir.join(name))))
        .collect::<Result<_>>()?;

    let mut recorded: Vec<RecordedRep> = Vec::new();
    let mut model_seen: Option<String> = None;
    let mut status = String::from("ok");
    for (scenario_index, (name, scenario)) in selected.iter().enumerate() {
        for index in 1..=opts.k {
            let rep_name = format!("rep-{index:02}");
            println!("{name} / {rep_name} ({} turns)", scenario.script.len());
            let outcome = run_one_rep(&RepCtx {
                scenario,
                opts,
                info: &info,
                run: &run,
                name,
                rep_name: &rep_name,
            })?;
            if model_seen.is_none() {
                model_seen = outcome.model.clone();
            }
            if outcome.status != "ok" && status == "ok" {
                status = outcome.status.clone();
            }
            recorded.push(RecordedRep {
                scenario_index,
                rep_name,
                outcome,
            });
        }
    }

    let hermetic_pass = artifacts::assert_hermeticity(&run, &watch, before)?;

    // Grading and judging come after the hermeticity window closes. The
    // deterministic layer only reads recorded artifacts, but a judge call runs
    // the harness in the operator's own environment - the one thing that
    // window exists to keep the run out of.
    let mut graded = Vec::new();
    // The model the judge actually resolved to, for the report's own record.
    let mut judge_model: Option<String> = None;
    for (scenario_index, (_, scenario)) in selected.iter().enumerate() {
        let checks_file = &checks[scenario_index];
        let mut reps = Vec::new();
        for rep in recorded
            .iter()
            .filter(|r| r.scenario_index == scenario_index)
        {
            let mut verdicts = report::grade_rep(checks_file, &rep.outcome.rep_dir);
            if let Some(cell) = &opts.judge {
                if let Some(record) = judge_one_rep(
                    cell,
                    scenario,
                    &verdicts,
                    &rep.outcome.rep_dir,
                    rep.outcome.block.as_deref(),
                    &run,
                )? {
                    judge_model = judge_model.or(record.model.clone());
                    verdicts.extend(record.grades());
                }
            }
            let failed: Vec<&str> = verdicts
                .iter()
                .filter(|v| !v.passed)
                .map(|v| v.behavior.as_str())
                .collect();
            println!(
                "  {} -> {}{}",
                rep.rep_name,
                rep.outcome.status,
                if failed.is_empty() {
                    String::from("; all behaviors pass")
                } else {
                    format!("; failed: {}", failed.join(", "))
                }
            );
            reps.push(GradedRep {
                rep: rep.rep_name.clone(),
                verdicts,
            });
        }
        graded.push(GradedScenario {
            scenario: scenario.name.clone(),
            behavior: scenario.behavior.clone(),
            split: scenario.split.id().to_string(),
            tags: scenario.tags.clone(),
            reps,
        });
    }

    let finished_at = chrono::Local::now().to_rfc3339();
    let report = Report {
        schema: report::SCHEMA,
        run_id: run_id.clone(),
        arm: opts.arm.id().to_string(),
        harness: opts.harness.id().to_string(),
        model: model_seen,
        harness_version: info.harness_version.clone(),
        judge: opts.judge.as_ref().map(|c| JudgeCellRef {
            harness: c.harness.id().to_string(),
            model: judge_model.clone().or_else(|| c.model.clone()),
        }),
        k: opts.k,
        split: opts.split.id().to_string(),
        filter: opts.filter.map(str::to_string),
        prompt_git_sha: info.git_sha.clone(),
        prompt_dirty: info.git_dirty,
        block_sha256: info.block_ref.clone(),
        started_at: info.started_at.clone(),
        finished_at: finished_at.clone(),
        scenarios: graded,
    };
    report.write(&run.path)?;

    run.write_manifest(&json!({
        "run_id": run_id,
        "arm": opts.arm.id(),
        "harness": opts.harness.id(),
        "harness_version": &info.harness_version,
        "model": &report.model,
        "model_pinned": opts.model,
        "k": opts.k,
        "split": opts.split.id(),
        "filter": opts.filter,
        "scenarios": selected.iter().map(|(name, s)| json!({
            "scenario": name,
            "behavior": s.behavior,
            "split": s.split.id(),
            "tags": s.tags,
        })).collect::<Vec<_>>(),
        "prompt_git_sha": &report.prompt_git_sha,
        "prompt_dirty": info.git_dirty,
        "block_sha256": &report.block_sha256,
        "judge": &report.judge,
        "started_at": &info.started_at,
        "finished_at": finished_at,
        "status": &status,
        "hermeticity_passed": hermetic_pass,
        "report": "report.json",
    }))?;

    let (adherent, total) = report.adherent();
    println!(
        "status: {status}; hermeticity: {}; adherence: {adherent}/{total} behaviors",
        if hermetic_pass { "pass" } else { "FAIL" }
    );
    println!("report: {}", run.path.join("report.md").display());

    Ok(if !hermetic_pass {
        EXIT_HERMETICITY
    } else if status != "ok" {
        EXIT_TURN_FAILED
    } else {
        0
    })
}

/// Judge one rep and report what happened, on stdout and in the run log. The
/// caller takes the verdicts as grades; a rep whose answer never parsed
/// contributes none, so its judged behaviors carry no verdict rather than a
/// fabricated one. `None` means the scenario declared no dimension to judge.
fn judge_one_rep(
    cell: &JudgeCell,
    scenario: &Scenario,
    grades: &[BehaviorGrade],
    rep_dir: &RepDir,
    block: Option<&str>,
    run: &RunDir,
) -> Result<Option<judge::JudgeRecord>> {
    if judge::declared(scenario).is_empty() {
        run.log(&format!(
            "{}: no judge dimensions declared; the deterministic layer alone grades it",
            scenario.name
        ));
        return Ok(None);
    }
    let record = judge::judge_rep(cell, scenario, grades, rep_dir, block)?;
    let failed: Vec<&str> = record
        .verdicts
        .iter()
        .filter(|v| !v.passed)
        .map(|v| v.dimension.as_str())
        .collect();
    println!(
        "  judge: {}{}",
        record.status,
        if record.status == "ok" {
            if failed.is_empty() {
                String::from("; all dimensions pass")
            } else {
                format!("; failed: {}", failed.join(", "))
            }
        } else {
            match record.attempts.last().and_then(|a| a.error.as_deref()) {
                Some(e) => format!("; {e}"),
                None => String::new(),
            }
        }
    );
    run.log(&format!(
        "{} {}: judge {} ({} attempt(s), {} dimension(s))",
        scenario.name,
        rep_dir
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        record.status,
        record.attempts.len(),
        record.dimensions.len()
    ));
    Ok(Some(record))
}

/// The run-level facts every rep shares and that no rep can change: resolved
/// once, so a rep neither re-spawns the harness for its version nor re-hashes
/// the arm's block.
struct RunInfo {
    /// The resolved csm repo root: the corpus, the arms and the sources live
    /// under it.
    repo_root: PathBuf,
    started_at: String,
    git_sha: Option<String>,
    git_dirty: bool,
    harness_version: String,
    /// The arm's block identity (`None` for the arm that deploys none).
    block_ref: Option<String>,
}

/// Everything one rep needs. Bundled because the alternative is a nine-argument
/// function.
struct RepCtx<'a> {
    scenario: &'a Scenario,
    opts: &'a RunOpts<'a>,
    info: &'a RunInfo,
    /// The suite run dir; the rep's artifacts go under `reps/<scenario>/`.
    run: &'a RunDir,
    name: &'a str,
    rep_name: &'a str,
}

/// One rep held back until the run's hermeticity window has closed: grading
/// and judging both happen after it, and the judge calls a model in the
/// operator's own environment.
struct RecordedRep {
    /// Which entry of the selected scenarios this rep belongs to.
    scenario_index: usize,
    rep_name: String,
    outcome: RepOutcome,
}

/// One rep's outcome, as much as the suite needs from it.
struct RepOutcome {
    status: String,
    model: Option<String>,
    rep_dir: RepDir,
    /// The block this rep deployed, verbatim - the judge's copy of the prompt
    /// under test.
    block: Option<String>,
}

/// Drive one repeat of one scenario: a fresh sandbox, the arm deployed, the
/// scripted turns run verbatim, a checkpoint recorded after each.
fn run_one_rep(ctx: &RepCtx) -> Result<RepOutcome> {
    let scenario = ctx.scenario;
    let rep_root = ctx.run.path.join("reps").join(ctx.name).join(ctx.rep_name);
    std::fs::create_dir_all(&rep_root)?;
    let run = RunDir {
        path: rep_root.clone(),
    };
    let rep_dir = RepDir::new(&rep_root);
    let rep_id = format!("{}.{}", scenario.name, ctx.rep_name);

    // This rep's hermeticity window opens before the sandbox exists.
    let home = PathBuf::from(std::env::var("HOME").context("HOME is not set")?);
    let watch = artifacts::Watch::new(&home, &ctx.info.repo_root);
    let before = watch.snapshot()?;

    let sb = sandbox::Sandbox::create()?;
    run.log(&format!("sandbox {}", sb.root.display()));
    sandbox::deploy_wiring(
        &sb,
        ctx.opts.arm,
        &scenario.session.name,
        &ctx.info.repo_root,
    )?;
    let deployed = match sandbox::deploy_check(&sb, ctx.opts.arm, &ctx.info.repo_root) {
        Ok(hash) => hash,
        Err(e) => {
            run.write_manifest(&json!({
                "run_id": rep_id,
                "scenario": scenario.name,
                "rep": ctx.rep_name,
                "arm": ctx.opts.arm.id(),
                "harness": ctx.opts.harness.id(),
                "status": "deploy-check-failed",
                "error": e.to_string(),
            }))?;
            let _ = sb.remove();
            return Err(e.context("deploy check failed"));
        }
    };
    // Read after the check, so the text recorded is the block that was
    // verified rather than whatever happens to be on disk.
    let block = sandbox::deployed_block(&sb)?;
    run.log(&format!(
        "deployed arm {} (deployed block sha256: {}; block ref: {})",
        ctx.opts.arm.id(),
        deployed.as_deref().unwrap_or("(none)"),
        ctx.info.block_ref.as_deref().unwrap_or("(none)")
    ));

    sandbox::scaffold_fixture_session(&sb, scenario)?;
    sandbox::scaffold_fixture_repo(&sb, scenario)?;
    sandbox::scaffold_siblings(&sb, scenario)?;

    // Checkpoint 0: the seeded fixture, before any turn.
    let session_dir = sb.csm_home.join("sessions").join(&scenario.session.name);
    artifacts::snapshot_checkpoint(&run, &session_dir, &sb.repo, &artifacts::turn_checkpoint(0))?;

    let agent = driver_for(ctx.opts.harness);
    run.log(&format!(
        "harness {} {}",
        agent.id(),
        ctx.info.harness_version
    ));
    let seatbelt = if ctx.opts.seatbelt {
        sandbox::write_seatbelt_profile(&sb)?
    } else {
        None
    };
    let explicit = if ctx.opts.arm.appends_instruction() {
        Some(scenario.explicit_instruction.as_str())
    } else {
        None
    };

    let mut session_id = driver::new_uuid()?;
    let mut observed_model: Option<String> = None;
    let mut turns = Vec::new();
    let mut status = "ok";
    for (i, prompt) in scenario.script.iter().enumerate() {
        let index = i + 1;
        let args = driver::TurnArgs {
            sb: &sb,
            session_name: &scenario.session.name,
            prompt,
            first: i == 0,
            session_id: &session_id,
            model: ctx.opts.model,
            explicit,
            seatbelt: &seatbelt,
        };
        let cmd = agent.turn_command(&args)?;
        let result = driver::run_turn(
            cmd,
            ctx.opts.timeout,
            None,
            &run.turn_trace(index),
            &run.turn_stderr(index),
        )?;
        let (sid, turn_model) = agent.identify(&result.trace_lines);
        if i == 0 {
            session_id = driver::require_session_id(&sid)
                .context("first turn produced no session id")?
                .to_string();
            observed_model = turn_model;
        }
        let turn_status = if result.timed_out {
            "timeout"
        } else if result.exit_code == Some(0) {
            "ok"
        } else {
            "error"
        };
        run.log(&format!(
            "turn {index}: {turn_status} exit={:?} model={:?} ({}s)",
            result.exit_code,
            observed_model,
            result.duration.as_secs_f64().round()
        ));
        turns.push(json!({
            "turn": index,
            "prompt": prompt,
            "status": turn_status,
            "exit_code": result.exit_code,
            "timed_out": result.timed_out,
            "duration_secs": result.duration.as_secs_f64(),
        }));
        // The per-turn checkpoint: what this turn left behind, whether or not
        // the next one runs.
        artifacts::snapshot_checkpoint(
            &run,
            &session_dir,
            &sb.repo,
            &artifacts::turn_checkpoint(index),
        )?;
        if turn_status != "ok" {
            status = "turn-failed";
            break;
        }
    }

    // The end state, under the names a run has always written: addressable
    // without knowing how many turns the script had or where it stopped.
    artifacts::snapshot_checkpoint(&run, &session_dir, &sb.repo, &artifacts::final_checkpoint())?;
    if let Some(transcript) = agent.transcript(&sb, &session_id) {
        let dst = run.path.join("transcript.jsonl");
        if std::fs::copy(&transcript, &dst).is_err() {
            run.log("transcript copy failed (session file not found)");
        }
    } else {
        run.log("no harness transcript located");
    }

    let hermetic_pass = artifacts::assert_hermeticity(&run, &watch, before)?;

    run.write_manifest(&json!({
        "run_id": rep_id,
        "scenario": scenario.name,
        "behavior": scenario.behavior,
        "description": scenario.description,
        "split": scenario.split.id(),
        "tags": scenario.tags,
        "arm": ctx.opts.arm.id(),
        "harness": ctx.opts.harness.id(),
        "model": observed_model,
        "model_pinned": ctx.opts.model,
        "session_id": session_id,
        "script": scenario.script,
        "turns": turns,
        "status": status,
        "prompt_git_sha": &ctx.info.git_sha,
        "prompt_dirty": ctx.info.git_dirty,
        "block_sha256": &ctx.info.block_ref,
        // The deployed block verbatim: a hash identifies the prompt under test,
        // only its text can be judged against.
        "block_text": &block,
        "auto_mode": {
            "permissions": "bypassed",
            "keep_going_system_instruction": driver::AUTO_MODE_PROMPT,
        },
        "explicit_instruction": explicit,
        "auth": "provider env passthrough (ANTHROPIC_*, OPENAI_API_KEY); no GH_* passed so gh stays unauthenticated",
        "seatbelt": seatbelt.map(|p| p.display().to_string()),
        "sandbox": if ctx.opts.keep_sandbox { serde_json::Value::String(sb.root.display().to_string()) } else { serde_json::Value::Null },
        "started_at": &ctx.info.started_at,
        "finished_at": chrono::Local::now().to_rfc3339(),
        "hermeticity_passed": hermetic_pass,
    }))?;

    if ctx.opts.keep_sandbox {
        println!("sandbox kept: {}", sb.root.display());
    } else {
        sb.remove()?;
    }
    Ok(RepOutcome {
        status: status.to_string(),
        model: observed_model,
        rep_dir,
        block,
    })
}

// --- grade -------------------------------------------------------------------

/// A rep's grades from what is already on disk: the deterministic checks,
/// re-applied, plus whatever judge verdicts the rep recorded. Both layers come
/// back through here, so `grade` and `judge` cannot disagree about a rep.
fn recorded_grades(checks_file: &ChecksFile, rep_dir: &Path) -> Result<Vec<BehaviorGrade>> {
    let mut grades = report::grade_rep(checks_file, &RepDir::new(rep_dir));
    if let Some(record) = judge::load(rep_dir)? {
        grades.extend(record.grades());
    }
    Ok(grades)
}

/// Re-apply the layers to what a run recorded. Accepts a run dir (grading
/// every rep under `reps/`) or a single rep dir. Zero model calls.
fn cmd_grade(run_dir: &Path, repo_root: &Path) -> Result<()> {
    let scenarios_dir = repo_root.join("evals").join("scenarios");
    if run_dir.join("reps").is_dir() {
        let mut report = Report::load(run_dir)?;
        for scenario in report.scenarios.iter_mut() {
            let checks_path = Scenario::checks_path(&scenarios_dir.join(&scenario.scenario));
            let checks_file = ChecksFile::load(&checks_path)?;
            let scenario_root = run_dir.join("reps").join(&scenario.scenario);
            let mut reps = Vec::new();
            for entry in sorted_dirs(&scenario_root)? {
                reps.push(GradedRep {
                    rep: entry
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    verdicts: recorded_grades(&checks_file, &entry)?,
                });
            }
            scenario.reps = reps;
        }
        report.write(run_dir)?;
        let (passed, total) = report.adherent();
        println!("graded {}: {passed}/{total} behaviors", report.run_id);
        println!("report: {}", run_dir.join("report.md").display());
        return Ok(());
    }

    let (name, grades) = grade_one_rep(run_dir, &scenarios_dir)?;
    let passed = grades.iter().filter(|g| g.passed).count();
    std::fs::write(
        run_dir.join("grade.json"),
        serde_json::to_string_pretty(&grades)? + "\n",
    )
    .context("writing grade.json")?;
    for g in &grades {
        println!("{}: {}", g.behavior, if g.passed { "pass" } else { "fail" });
    }
    println!(
        "graded {name} in {}: {passed}/{} behaviors",
        run_dir.display(),
        grades.len()
    );
    Ok(())
}

/// One rep dir's grades; its own manifest names the scenario.
fn grade_one_rep(rep_dir: &Path, scenarios_dir: &Path) -> Result<(String, Vec<BehaviorGrade>)> {
    let manifest = rep_manifest(rep_dir)?;
    let name = manifest
        .get("scenario")
        .and_then(|s| s.as_str())
        .context("the rep manifest names no scenario")?
        .to_string();
    let checks_file = ChecksFile::load(&Scenario::checks_path(&scenarios_dir.join(&name)))?;
    Ok((name, recorded_grades(&checks_file, rep_dir)?))
}

fn rep_manifest(rep_dir: &Path) -> Result<serde_json::Value> {
    let path = rep_dir.join("manifest.json");
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

// --- judge -------------------------------------------------------------------

/// The block a rep deployed, from its manifest. Judging a run against a block it
/// did not deploy would measure the wrong protocol, so an arm that deployed one
/// and did not record it is an error rather than a silent "no block deployed".
fn rep_block(manifest: &serde_json::Value, rep_dir: &Path) -> Result<Option<String>> {
    let arm = manifest
        .get("arm")
        .and_then(|a| a.as_str())
        .unwrap_or_default();
    let block = manifest
        .get("block_text")
        .and_then(|b| b.as_str())
        .map(String::from);
    anyhow::ensure!(
        block.is_some() || arm == Arm::NoBlock.id(),
        "{} records no block text (arm {arm}); the judge needs the exact prompt under test, so \
         re-run the arm",
        rep_dir.display()
    );
    Ok(block)
}

/// Judge a recorded run: one call per scenario x rep over the traces and
/// snapshots it recorded, then rewrite the report with the judge verdicts
/// merged in. Accepts a run dir or one rep dir; no turn is re-run either way.
fn cmd_judge(run_dir: &Path, repo_root: &Path, cell: &JudgeCell) -> Result<()> {
    let scenarios_dir = repo_root.join("evals").join("scenarios");
    if !run_dir.join("reps").is_dir() {
        return cmd_judge_rep(run_dir, &scenarios_dir, cell);
    }
    let mut report = Report::load(run_dir)?;
    report.judge = Some(JudgeCellRef {
        harness: cell.harness.id().to_string(),
        model: cell.model.clone(),
    });
    let mut judge_model: Option<String> = None;
    for scenario_report in report.scenarios.iter_mut() {
        let dir = scenarios_dir.join(&scenario_report.scenario);
        let scenario = Scenario::load(&dir)?;
        let checks_file = ChecksFile::load(&Scenario::checks_path(&dir))?;
        let scenario_root = run_dir.join("reps").join(&scenario_report.scenario);
        let mut reps = Vec::new();
        for entry in sorted_dirs(&scenario_root)? {
            let rep = RepDir::new(&entry);
            let rep_name = entry
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let mut verdicts = report::grade_rep(&checks_file, &rep);
            let block = rep_block(&rep_manifest(&entry)?, &entry)?;
            println!("{} / {rep_name}: judging", scenario.name);
            if let Some(record) = judge_one_rep(
                cell,
                &scenario,
                &verdicts,
                &rep,
                block.as_deref(),
                &RunDir {
                    path: run_dir.to_path_buf(),
                },
            )? {
                judge_model = judge_model.or(record.model.clone());
                verdicts.extend(record.grades());
            }
            reps.push(GradedRep {
                rep: rep_name,
                verdicts,
            });
        }
        scenario_report.reps = reps;
    }
    if let Some(cell_ref) = report.judge.as_mut() {
        cell_ref.model = judge_model.or_else(|| cell_ref.model.take());
    }
    report.write(run_dir)?;
    let (passed, total) = report.adherent();
    println!("judged {}: {passed}/{total} behaviors", report.run_id);
    println!("report: {}", run_dir.join("report.md").display());
    Ok(())
}

/// Judge one rep dir. There is no run report to rewrite, so the pass lands in
/// the rep's own `judge.json` for the next `grade` to pick up.
fn cmd_judge_rep(rep_dir: &Path, scenarios_dir: &Path, cell: &JudgeCell) -> Result<()> {
    let manifest = rep_manifest(rep_dir)?;
    let name = manifest
        .get("scenario")
        .and_then(|s| s.as_str())
        .context("the rep manifest names no scenario")?
        .to_string();
    let dir = scenarios_dir.join(&name);
    let scenario = Scenario::load(&dir)?;
    let checks_file = ChecksFile::load(&Scenario::checks_path(&dir))?;
    let rep = RepDir::new(rep_dir);
    let block = rep_block(&manifest, rep_dir)?;
    let grades = report::grade_rep(&checks_file, &rep);
    let record = judge_one_rep(
        cell,
        &scenario,
        &grades,
        &rep,
        block.as_deref(),
        &RunDir {
            path: rep_dir.to_path_buf(),
        },
    )?;
    match record {
        Some(record) => println!(
            "judged {name} in {}: {} dimension(s)",
            rep_dir.display(),
            record.dimensions.len()
        ),
        None => println!(
            "{name} declares no judge dimension; {} is graded by the deterministic layer alone",
            rep_dir.display()
        ),
    }
    Ok(())
}

fn sorted_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(root).with_context(|| format!("reading {}", root.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

// --- diff --------------------------------------------------------------------

fn cmd_diff(baseline: &Path, candidate: &Path) -> Result<(String, bool)> {
    let a = Report::load(baseline)?;
    let b = Report::load(candidate)?;
    let deltas = report::diff(&a, &b);
    Ok(report::render_diff(&a, &b, &deltas))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_judge_cell_defaults_to_the_harness_under_test() {
        let cell = judge_cell(true, None, None, Harness::Codex, Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(cell.harness, Harness::Codex);
        assert_eq!(cell.model, None);
        assert_eq!(cell.timeout, Duration::from_secs(5));
    }

    #[test]
    fn the_judge_cell_takes_a_named_harness_and_model() {
        let cell = judge_cell(
            true,
            Some("claude"),
            Some(String::from("judge-model")),
            Harness::Codex,
            Duration::from_secs(5),
        )
        .unwrap()
        .unwrap();
        assert_eq!(cell.harness, Harness::Claude);
        assert_eq!(cell.model.as_deref(), Some("judge-model"));
        assert!(judge_cell(
            true,
            Some("pi"),
            None,
            Harness::Claude,
            Duration::from_secs(5)
        )
        .is_err());
    }

    /// `--no-judge` skips the layer, and asking for a judge cell it would
    /// ignore is a contradiction rather than a silently dropped flag.
    #[test]
    fn no_judge_skips_the_layer_and_refuses_a_judge_cell() {
        assert!(
            judge_cell(false, None, None, Harness::Claude, Duration::from_secs(5))
                .unwrap()
                .is_none()
        );
        for (harness, model) in [(Some("claude"), None), (None, Some(String::from("m")))] {
            let err = judge_cell(
                false,
                harness,
                model,
                Harness::Claude,
                Duration::from_secs(5),
            )
            .unwrap_err()
            .to_string();
            assert!(err.contains("--no-judge"), "{err}");
        }
    }
}
