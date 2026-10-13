//! csm-eval - dev-side behavioral eval runner for the csm working-mode prompt.
//!
//! Not part of the distributed product: this binary exists to replay scripted
//! scenarios against a hermetic sandbox (fresh HOME / CSM_HOME / CODEX_HOME,
//! the working-tree [csm] block deployed by the real `csm init`, a tiny
//! fixture repo) and capture full traces per turn. The unit of evaluation is
//! the scenario; the headline number is a delta between contrast arms, never
//! an absolute score.

mod artifacts;
mod driver;
mod sandbox;
mod scenario;

use anyhow::{Context, Result};
use artifacts::RunDir;
use clap::{Parser, Subcommand};
use driver::driver_for;
use scenario::{Arm, Harness, Scenario};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    /// Run one scenario end to end: build the sandbox, deploy the arm,
    /// drive the scripted turns headless, capture artifacts, assert
    /// hermeticity.
    Run {
        /// Scenario name: the directory under evals/scenarios/.
        scenario: String,
        /// Contrast arm (working-tree = the claim under test, no-block = the
        /// unbound control).
        #[arg(long, default_value = "working-tree")]
        arm: String,
        /// Harness to drive (claude, codex).
        #[arg(long, default_value = "claude")]
        harness: String,
        /// Pin the model id; without it the harness's default resolution
        /// applies (recorded from the trace either way).
        #[arg(long)]
        model: Option<String>,
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
            timeout_secs,
            keep_sandbox,
            repo_root,
            no_seatbelt,
        } => cmd_run(&RunOpts {
            scenario: &scenario,
            arm: Arm::parse(&arm)?,
            harness: Harness::parse(&harness)?,
            model: model.as_deref(),
            timeout: Duration::from_secs(timeout_secs),
            keep_sandbox,
            repo_root: repo_root.as_deref(),
            seatbelt: !no_seatbelt,
        }),
    }
}

/// One `run` invocation: the CLI-parsed parameters handed to `cmd_run` as a
/// single argument.
#[derive(Clone, Copy)]
struct RunOpts<'a> {
    scenario: &'a str,
    arm: Arm,
    harness: Harness,
    model: Option<&'a str>,
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

/// The operator session's transcript dir (this session drives the eval and
/// writes its own transcript during the run; its writes are classified as
/// concurrent noise, not escapes).
fn operator_project_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let cwd = std::env::current_dir().ok()?;
    let escaped = cwd.display().to_string().replace('/', "-");
    Some(
        PathBuf::from(home)
            .join(".claude")
            .join("projects")
            .join(escaped),
    )
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

fn cmd_run(opts: &RunOpts) -> Result<()> {
    let RunOpts {
        scenario: scenario_name,
        arm,
        harness,
        model,
        timeout,
        keep_sandbox,
        repo_root,
        seatbelt: seatbelt_enabled,
    } = *opts;
    let repo_root = match repo_root {
        Some(p) => p.to_path_buf(),
        None => find_repo_root(&std::env::current_dir().context("getting cwd")?)?,
    };
    let scenario_dir = repo_root
        .join("evals")
        .join("scenarios")
        .join(scenario_name);
    let scenario = Scenario::load(&scenario_dir)
        .with_context(|| format!("loading scenario {scenario_name:?}"))?;

    let run_id = format!(
        "{}.{}.{}.{}",
        scenario.name,
        arm.id(),
        harness.id(),
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
    let run = RunDir::create(&repo_root.join("evals").join("runs"), &run_id)?;
    println!("run: {run_id}");
    println!("artifacts: {}", run.path.display());

    let sb = sandbox::Sandbox::create()?;
    run.log(&format!("sandbox {}", sb.root.display()));

    // Hermeticity pre-snapshot before anything the run could attribute.
    let watch = artifacts::Watch::new(
        Path::new(&std::env::var("HOME").context("HOME is not set")?),
        &repo_root,
        operator_project_dir(),
    );
    let before = watch.snapshot()?;

    let started_at = chrono::Local::now().to_rfc3339();
    let git_sha = git_line(&repo_root, &["rev-parse", "HEAD"]);
    let git_dirty = std::process::Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(&repo_root)
        .args(["status", "--porcelain"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    // Deploy the arm, then self-check what the sandbox config carries.
    sandbox::deploy_wiring(&sb, arm, &scenario.session.name)?;
    let block_sha = match sandbox::deploy_check(&sb, arm) {
        Ok(hash) => hash,
        Err(e) => {
            run.write_manifest(&json!({
                "run_id": run_id, "status": "deploy-check-failed",
                "scenario": scenario.name, "arm": arm.id(), "harness": harness.id(),
                "error": e.to_string(), "started_at": started_at,
            }))?;
            let _ = sb.remove();
            anyhow::bail!("deploy check failed: {e:#}");
        }
    };
    run.log(&format!(
        "deployed arm {} (block sha256: {})",
        arm.id(),
        block_sha.as_deref().unwrap_or("(none)")
    ));

    sandbox::scaffold_fixture_session(&sb, &scenario)?;
    sandbox::scaffold_fixture_repo(&sb, &scenario)?;

    let agent = driver_for(harness);
    let harness_version = agent.version().unwrap_or_else(|e| format!("unknown ({e})"));
    let seatbelt = if seatbelt_enabled {
        sandbox::write_seatbelt_profile(&sb)?
    } else {
        None
    };
    run.log(&format!(
        "harness {} {} (seatbelt: {})",
        agent.id(),
        harness_version,
        seatbelt.is_some()
    ));

    // Drive the script.
    let mut session_id = driver::new_uuid()?;
    let mut observed_model: Option<String> = None;
    let mut turns = Vec::new();
    let mut status = "ok";
    for (i, prompt) in scenario.script.iter().enumerate() {
        let index = i + 1;
        println!("turn {index}: {prompt}");
        let args = driver::TurnArgs {
            sb: &sb,
            session_name: &scenario.session.name,
            prompt,
            first: i == 0,
            session_id: &session_id,
            model,
            seatbelt: &seatbelt,
        };
        let cmd = agent.turn_command(&args)?;
        let result = driver::run_turn(
            cmd,
            timeout,
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
        println!(
            "  -> {turn_status} ({}s, session {session_id})",
            result.duration.as_secs_f64().round()
        );
        run.log(&format!(
            "turn {index}: {turn_status} exit={:?} model={:?}",
            result.exit_code, observed_model
        ));
        turns.push(json!({
            "turn": index,
            "prompt": prompt,
            "status": turn_status,
            "exit_code": result.exit_code,
            "timed_out": result.timed_out,
            "duration_secs": result.duration.as_secs_f64(),
        }));
        if turn_status != "ok" {
            status = "turn-failed";
            break;
        }
    }

    // Final artifacts: the harness's own transcript, the fixture record
    // state, the repo end state.
    if let Some(transcript) = agent.transcript(&sb, &session_id) {
        let dst = run.path.join("transcript.jsonl");
        if std::fs::copy(&transcript, &dst).is_err() {
            run.log("transcript copy failed (session file not found)");
        }
    } else {
        run.log("no harness transcript located");
    }
    let session_dir = sb.csm_home.join("sessions").join(&scenario.session.name);
    artifacts::snapshot_fixture_session(&run, &session_dir)?;
    artifacts::snapshot_repo(&run, &sb.repo)?;

    // Hermeticity: the post-run assertion over the watch set.
    let hermetic_pass = artifacts::assert_hermeticity(&run, &watch, before)?;

    run.write_manifest(&json!({
            "run_id": run_id,
            "scenario": scenario.name,
            "behavior": scenario.behavior,
            "description": scenario.description,
            "arm": arm.id(),
            "harness": harness.id(),
            "harness_version": harness_version,
            "model": observed_model,
            "model_pinned": model,
            "session_id": session_id,
            "script": scenario.script,
            "turns": turns,
            "status": status,
            "prompt_git_sha": git_sha,
            "prompt_dirty": git_dirty,
            "block_sha256": block_sha,
            "auto_mode": {
                "permissions": "bypassed",
                "keep_going_system_instruction": driver::AUTO_MODE_PROMPT,
            },
            "auth": "provider env passthrough (ANTHROPIC_*, OPENAI_API_KEY); no GH_* passed so gh stays unauthenticated",
            "seatbelt": seatbelt.map(|p| p.display().to_string()),
            "sandbox": if keep_sandbox { serde_json::Value::String(sb.root.display().to_string()) } else { serde_json::Value::Null },
            "started_at": started_at,
            "finished_at": chrono::Local::now().to_rfc3339(),
            "hermeticity_passed": hermetic_pass,
        }),
    )?;

    let exit_code = if !hermetic_pass {
        3
    } else if status != "ok" {
        2
    } else {
        0
    };
    if keep_sandbox {
        println!("sandbox kept: {}", sb.root.display());
    } else {
        sb.remove()?;
    }
    println!(
        "status: {status}; hermeticity: {}; hermeticity.json, manifest.json, traces in {}",
        if hermetic_pass { "pass" } else { "FAIL" },
        run.path.display()
    );
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}
