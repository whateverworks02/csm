mod checks;
mod sandbox;

use anyhow::{ensure, Context, Result};
use checks::{Check, CheckResult, CheckStatus, Snapshot};
use clap::{Parser, Subcommand};
use sandbox::Sandbox;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[derive(Parser)]
#[command(about = "Run the csm protocol's behavioral cases against Codex")]
struct Cli {
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    Run {
        /// Run just this case directory.
        case: Option<String>,
        /// Codex model override. Defaults to the CLI's model in a fresh config.
        #[arg(long)]
        model: Option<String>,
        /// Timeout for each Codex turn in seconds.
        #[arg(long, default_value_t = 240, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    provenance: Provenance,
    repo: BTreeMap<String, String>,
    session: BTreeMap<String, String>,
    #[serde(default)]
    other_sessions: BTreeMap<String, BTreeMap<String, String>>,
    turns: Vec<String>,
    checks: Vec<Check>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    source: String,
    reconstruction: String,
}

#[derive(Serialize)]
struct ResultFile<'a> {
    case: &'a str,
    provenance: &'a Provenance,
    invocation: Vec<String>,
    codex_version: Option<String>,
    requested_model: Option<&'a str>,
    protocol: &'static str,
    turns: Vec<Turn>,
    run_error: Option<String>,
}

#[derive(Serialize)]
struct Turn {
    number: usize,
    invocation: Vec<String>,
    model: Option<String>,
    thread: Option<String>,
    transcript: String,
    reply: Option<String>,
    checks: Vec<CheckResult>,
}

fn main() {
    match run() {
        Ok(status) => std::process::exit(status),
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<i32> {
    let Action::Run {
        case,
        model,
        timeout,
    } = Cli::parse().command;
    let root = std::env::current_dir()?.join("evals");
    let csm = std::env::current_exe()?.with_file_name("csm");
    let cases = discover(&root.join("cases"), case.as_deref())?;
    let runs = root.join("runs");
    fs::create_dir_all(&runs)?;
    let batch = tempfile::Builder::new()
        .prefix("run-")
        .tempdir_in(runs)?
        .keep();
    println!("Evidence in {}", batch.display());
    let protocol = csm::prompt::csm_block("{CSM_HOME}");
    let mut status = 0;
    for (name, case) in cases {
        let evidence = batch.join(&name);
        fs::create_dir(&evidence)?;
        fs::write(evidence.join("protocol.md"), &protocol)?;
        let mut result = ResultFile {
            case: &name,
            provenance: &case.provenance,
            invocation: std::env::args().collect(),
            codex_version: None,
            requested_model: model.as_deref(),
            protocol: "protocol.md",
            turns: Vec::new(),
            run_error: None,
        };
        println!("Running {name}");
        if let Err(error) = run_case(
            &case,
            &evidence,
            &csm,
            Duration::from_secs(timeout),
            &mut result,
        ) {
            result.run_error = Some(format!("{error:#}"));
            eprintln!("  Run error: {error:#}");
            status = 2;
        }
        for turn in &result.turns {
            for check in &turn.checks {
                println!(
                    "  Turn {} {:7} {}",
                    turn.number,
                    check.status.label(),
                    check.expected
                );
                if check.status == CheckStatus::Fail {
                    println!("    Observed {} ({})", check.observed, check.evidence);
                    status = status.max(1);
                }
            }
        }
        serde_json::to_writer_pretty(File::create(evidence.join("result.json"))?, &result)?;
    }
    Ok(status)
}

fn discover(root: &Path, selected: Option<&str>) -> Result<BTreeMap<String, Case>> {
    let mut cases = BTreeMap::new();
    for entry in fs::read_dir(root).with_context(|| format!("reading {}", root.display()))? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("invalid case directory name"))?;
        if !entry.file_type()?.is_dir() || selected.is_some_and(|wanted| wanted != name) {
            continue;
        }
        let path = entry.path().join("case.json");
        let case: Case = serde_json::from_reader(BufReader::new(File::open(&path)?))
            .with_context(|| format!("reading {}", path.display()))?;
        ensure!(
            !case.turns.is_empty() && case.turns.iter().all(|turn| !turn.is_empty()),
            "{name} has no turns or an empty turn"
        );
        ensure!(!case.checks.is_empty(), "{name} has no checks");
        for check in &case.checks {
            check
                .validate(case.turns.len())
                .with_context(|| format!("invalid check in {name}"))?;
        }
        cases.insert(name, case);
    }
    ensure!(!cases.is_empty(), "no matching cases in {}", root.display());
    Ok(cases)
}

fn run_case(
    case: &Case,
    evidence: &Path,
    csm: &Path,
    timeout: Duration,
    result: &mut ResultFile,
) -> Result<()> {
    ensure!(csm.is_file(), "build both binaries with cargo build --bins");
    let sandbox = Sandbox::create()?;
    let outcome = run_in_sandbox(case, evidence, csm, timeout, result, &sandbox);
    let cleanup = sandbox.close();
    cleanup?;
    outcome
}

fn run_in_sandbox(
    case: &Case,
    evidence: &Path,
    csm: &Path,
    timeout: Duration,
    result: &mut ResultFile,
    sandbox: &Sandbox,
) -> Result<()> {
    sandbox.seed(&case.repo, &case.session, &case.other_sessions)?;
    sandbox.deploy(csm, evidence)?;
    let api_key = std::env::var_os("OPENAI_API_KEY").filter(|key| !key.is_empty());
    let version = sandbox
        .codex(csm, None)?
        .arg("--version")
        .output()
        .context("running codex --version")?;
    ensure!(version.status.success(), "codex --version failed");
    result.codex_version = Some(String::from_utf8(version.stdout)?.trim().to_owned());
    sandbox.provision_auth(api_key.as_deref())?;
    let initial = Snapshot::capture(sandbox, &evidence.join("initial"))?;
    let mut previous = None;
    let mut thread: Option<String> = None;

    for (index, prompt) in case.turns.iter().enumerate() {
        let number = index + 1;
        let label = format!("turn-{number:02}");
        let dir = evidence.join(&label);
        fs::create_dir(&dir)?;
        let mut cmd = sandbox.codex(csm, api_key.as_deref())?;
        cmd.arg("exec");
        if let Some(id) = &thread {
            cmd.arg("resume").arg(id);
        }
        cmd.args(["--json", "--dangerously-bypass-hook-trust"]);
        if let Some(model) = result.requested_model {
            cmd.args(["--model", model]);
        }
        cmd.arg("--").arg("-");
        let mut turn = Turn {
            number,
            invocation: command_line(&cmd),
            model: None,
            thread: None,
            transcript: format!("{label}/events.jsonl"),
            reply: None,
            checks: Vec::new(),
        };
        fs::write(dir.join("user.txt"), prompt)?;
        let child = sandbox::run_process(
            cmd,
            File::open(dir.join("user.txt"))?.into(),
            File::create(dir.join("events.jsonl"))?,
            File::create(dir.join("stderr.txt"))?,
            timeout,
        );

        let identity = read_events(&dir, &mut turn).and_then(|complete| {
            let id = turn
                .thread
                .as_ref()
                .context("Codex reported no thread ID")?;
            ensure!(
                thread.as_ref().is_none_or(|previous| previous == id),
                "Codex resumed the wrong thread"
            );
            turn.model = resolved_model(&sandbox.codex_home, id)?;
            Ok(complete)
        });
        let snapshot = Snapshot::capture(sandbox, &dir.join("workspace"));
        result.turns.push(turn);
        let turn = result.turns.last_mut().expect("recorded turn");
        let status = child.with_context(|| format!("{label}/events.jsonl"))?;
        ensure!(
            status.success(),
            "Codex exited with {status}; see {label}/stderr.txt and {label}/events.jsonl"
        );
        ensure!(
            identity.with_context(|| format!("{label}/events.jsonl"))?
                && turn.reply.is_some()
                && turn.model.is_some(),
            "incomplete Codex turn (missing completion, reply, or resolved model); see {label}/events.jsonl"
        );
        let snapshot = snapshot?;
        let context = checks::CheckContext {
            sandbox,
            initial: &initial,
            previous: previous.as_ref(),
            current: &snapshot,
            dir: &dir,
        };
        thread.clone_from(&turn.thread);
        for (check_index, check) in case.checks.iter().enumerate() {
            if check.applies(number) {
                turn.checks.push(check.evaluate(&context, check_index + 1)?);
            }
        }
        previous = Some(snapshot);
    }
    Ok(())
}

fn command_line(command: &Command) -> Vec<String> {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn read_events(dir: &Path, turn: &mut Turn) -> Result<bool> {
    let mut complete = false;
    let mut reply = None;
    for line in BufReader::new(File::open(dir.join("events.jsonl"))?).lines() {
        let event: Value = serde_json::from_str(&line?).context("parsing Codex JSONL event")?;
        match event["type"].as_str() {
            Some("thread.started") => turn.thread = event["thread_id"].as_str().map(str::to_owned),
            Some("turn.completed") => complete = true,
            Some("item.completed") if event["item"]["type"] == "agent_message" => {
                reply = event["item"]["text"].as_str().map(str::to_owned);
            }
            _ => {}
        }
    }
    if let Some(reply) = reply.filter(|text| !text.trim().is_empty()) {
        fs::write(dir.join("reply.md"), reply)?;
        turn.reply = Some(format!("turn-{:02}/reply.md", turn.number));
    }
    Ok(complete)
}

fn resolved_model(codex_home: &Path, thread: &str) -> Result<Option<String>> {
    let mut pending = vec![codex_home.join("sessions")];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).context("finding Codex rollout")? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !entry
                .file_name()
                .to_string_lossy()
                .ends_with(&format!("{thread}.jsonl"))
            {
                continue;
            }
            let mut model = None;
            for line in BufReader::new(File::open(entry.path())?).lines() {
                let event: Value =
                    serde_json::from_str(&line?).context("reading model from Codex rollout")?;
                if event["type"] == "turn_context" {
                    model = event["payload"]["model"].as_str().map(str::to_owned);
                }
            }
            return Ok(model);
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Outcome {
        Correct,
        Faulty,
        UncommittedImplementation,
    }

    #[test]
    #[ignore = "requires the Codex sandbox CLI, Git, and Python 3, but no model access"]
    fn corpus_checks_distinguish_correct_and_faulty_outcomes() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/cases");
        for name in ["discussion-only", "execution-handoff", "follow-up-question"] {
            let mut cases = discover(&root, Some(name))?;
            let case = cases.remove(name).context("missing initial case")?;
            let mut outcomes = vec![Outcome::Correct, Outcome::Faulty];
            if name == "execution-handoff" {
                outcomes.push(Outcome::UncommittedImplementation);
            }
            for outcome in outcomes {
                let evidence = tempfile::tempdir()?;
                let sandbox = Sandbox::create()?;
                sandbox.seed(&case.repo, &case.session, &case.other_sessions)?;
                let initial = Snapshot::capture(&sandbox, &evidence.path().join("initial"))?;
                let mut previous = None;
                for number in 1..=case.turns.len() {
                    seed_outcome(name, &case, &sandbox, number, outcome)?;
                    let dir = evidence.path().join(format!("turn-{number:02}"));
                    let current = Snapshot::capture(&sandbox, &dir.join("workspace"))?;
                    let context = checks::CheckContext {
                        sandbox: &sandbox,
                        initial: &initial,
                        previous: previous.as_ref(),
                        current: &current,
                        dir: &dir,
                    };
                    for (index, check) in case.checks.iter().enumerate() {
                        if !check.applies(number) {
                            continue;
                        }
                        let result = check.evaluate(&context, index + 1)?;
                        if result.status == CheckStatus::Human {
                            continue;
                        }
                        let compares_committed_content = name == "execution-handoff" && index == 3;
                        let should_fail = match outcome {
                            Outcome::Correct => false,
                            Outcome::Faulty => {
                                !compares_committed_content
                                    && (name != "follow-up-question" || number == 2)
                            }
                            Outcome::UncommittedImplementation => compares_committed_content,
                        };
                        let expected = if should_fail {
                            CheckStatus::Fail
                        } else {
                            CheckStatus::Pass
                        };
                        assert_eq!(
                            result.status,
                            expected,
                            "{name} {outcome:?} turn {number} check {}: {result:?}",
                            index + 1
                        );
                    }
                    previous = Some(current);
                }
            }
        }
        Ok(())
    }

    fn seed_outcome(
        name: &str,
        case: &Case,
        sandbox: &Sandbox,
        turn: usize,
        outcome: Outcome,
    ) -> Result<()> {
        let board = "tasks/INDEX.md";
        let task = "tasks/001-add-greeting.md";
        match (name, turn, outcome == Outcome::Faulty) {
            ("discussion-only", 1, true) => {
                fs::write(sandbox.repo.join("greet.py"), "print('hello, world')\n")?;
                // A clean status after a commit should still fail the unchanged check.
                sandbox.git(&["add", "greet.py"])?;
                sandbox.git(&["commit", "-m", "Implement greeting during discussion"])?;
                fs::write(
                    sandbox.session.join(board),
                    "## Pending review\n- 001 add-greeting\n",
                )?;
            }
            ("execution-handoff", 1, false) => {
                let path = sandbox.repo.join("greet.py");
                let seed = &case.repo["greet.py"];
                let implementation =
                    seed.replace("raise NotImplementedError", "return \"hello, world\"");
                let committed = if outcome == Outcome::UncommittedImplementation {
                    format!("# Update greeting module\n{seed}")
                } else {
                    implementation.clone()
                };
                fs::write(&path, committed)?;
                sandbox.git(&["add", "greet.py"])?;
                sandbox.git(&["commit", "-m", "Implement greeting"])?;
                fs::write(path, implementation)?;
                fs::write(
                    sandbox.session.join(board),
                    "## Pending review\n- 001 add-greeting\n",
                )?;
                fs::write(
                    sandbox.session.join(task),
                    case.session[task].replace(
                        "Implementation has not started.",
                        "Implemented greet.py and verified hello, world.",
                    ),
                )?;
            }
            ("execution-handoff", 1, true) => {
                fs::write(sandbox.session.join(board), "## Done\n- 001 add-greeting\n")?;
                fs::write(
                    sandbox.session.join(task),
                    case.session[task].replace("Awaiting execution.", "Approved."),
                )?;
            }
            ("execution-handoff", 2, true) => {
                fs::write(
                    sandbox.repo.join("extra.txt"),
                    "A change after the follow-up question.\n",
                )?;
                fs::write(
                    sandbox.session.join(board),
                    "## Pending fix\n- 001 add-greeting\n",
                )?;
            }
            ("follow-up-question", 2, true) => {
                fs::write(
                    sandbox.repo.join("greet.py"),
                    "print('configurable greeting')\n",
                )?;
                fs::write(sandbox.session.join(board), "## Open\n- 002 add-farewell\n- 003 configure-greeting\n## Done\n- 001 add-greeting\n")?;
                fs::write(sandbox.session.join(task), "## Review\nApproved\n")?;
                fs::write(
                    sandbox.session.join("tasks/003-configure-greeting.md"),
                    "# New task\n",
                )?;
            }
            _ => {}
        }
        Ok(())
    }
}
