//! csm-eval - dev-side behavioral eval runner for the csm working-mode prompt.
//!
//! Not part of the distributed product: this binary exists to replay scripted
//! scenarios against a hermetic sandbox (fresh HOME / CSM_HOME / CODEX_HOME,
//! the working-tree [csm] block deployed by the real `csm init`, a tiny
//! fixture repo) and capture full traces per turn. The unit of evaluation is
//! the scenario; the headline number is a delta between contrast arms, never
//! an absolute score.
//!
//! Three subcommands: `run` drives the scripted turns (model calls), `grade`
//! re-applies the checks to what a run recorded (no model calls), and `diff`
//! turns two runs' adherence matrices into the delta the deploy loop gates on.

mod arms;
mod artifacts;
mod checks;
mod driver;
mod report;
mod sandbox;
mod scenario;

use anyhow::{bail, Context, Result};
use artifacts::RunDir;
use checks::{ChecksFile, RepDir};
use clap::{Parser, Subcommand};
use driver::driver_for;
use report::{GradedRep, GradedScenario, Report};
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
        /// Per-turn wall-clock cap in seconds.
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
    },
    /// Re-apply the checks to a run's recorded traces and snapshots and write
    /// its report. Zero model calls.
    Grade {
        /// A run dir (grading every rep inside it) or one rep dir.
        run_dir: PathBuf,
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
        } => {
            let code = cmd_run(&RunOpts {
                scenario: scenario.as_deref(),
                arm: Arm::parse(&arm)?,
                harness: Harness::parse(&harness)?,
                model: model.as_deref(),
                k,
                split: SplitSelect::parse(&split)?,
                filter: filter.as_deref(),
                timeout: Duration::from_secs(timeout_secs),
                keep_sandbox,
                repo_root: repo_root.as_deref(),
                seatbelt: !no_seatbelt,
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

    let mut graded = Vec::new();
    let mut model_seen: Option<String> = None;
    let mut status = String::from("ok");
    for (name, scenario) in &selected {
        let checks_path = Scenario::checks_path(&scenarios_dir.join(name));
        let checks_file = ChecksFile::load(&checks_path)?;
        let mut reps = Vec::new();
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
            let verdicts = report::grade_rep(&checks_file, &outcome.rep_dir);
            let failed: Vec<&str> = verdicts
                .iter()
                .filter(|v| !v.passed)
                .map(|v| v.behavior.as_str())
                .collect();
            println!(
                "  -> {}{}",
                outcome.status,
                if failed.is_empty() {
                    String::from("; all behaviors pass")
                } else {
                    format!("; failed: {}", failed.join(", "))
                }
            );
            reps.push(GradedRep {
                rep: rep_name.clone(),
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

    let hermetic_pass = artifacts::assert_hermeticity(&run, &watch, before)?;
    let finished_at = chrono::Local::now().to_rfc3339();
    let report = Report {
        schema: report::SCHEMA,
        run_id: run_id.clone(),
        arm: opts.arm.id().to_string(),
        harness: opts.harness.id().to_string(),
        model: model_seen,
        harness_version: info.harness_version.clone(),
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

/// One rep's outcome, as much as the suite needs from it.
struct RepOutcome {
    status: String,
    model: Option<String>,
    rep_dir: RepDir,
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
    run.log(&format!(
        "deployed arm {} (deployed block sha256: {}; block ref: {})",
        ctx.opts.arm.id(),
        deployed.as_deref().unwrap_or("(none)"),
        ctx.info.block_ref.as_deref().unwrap_or("(none)")
    ));

    sandbox::scaffold_fixture_session(&sb, scenario)?;
    sandbox::scaffold_fixture_repo(&sb, scenario)?;

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
    })
}

// --- grade -------------------------------------------------------------------

/// Re-apply the checks to what a run recorded. Accepts a run dir (grading
/// every rep under `reps/`) or a single rep dir.
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
                let rep_dir = RepDir::new(&entry);
                reps.push(GradedRep {
                    rep: entry
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    verdicts: report::grade_rep(&checks_file, &rep_dir),
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

    // A single rep dir: its own manifest names the scenario.
    let manifest_path = run_dir.join("manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {}", manifest_path.display()))?,
    )
    .with_context(|| format!("parsing {}", manifest_path.display()))?;
    let name = manifest
        .get("scenario")
        .and_then(|s| s.as_str())
        .context("the rep manifest names no scenario")?;
    let checks_file = ChecksFile::load(&Scenario::checks_path(&scenarios_dir.join(name)))?;
    let grades = report::grade_rep(&checks_file, &RepDir::new(run_dir));
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
