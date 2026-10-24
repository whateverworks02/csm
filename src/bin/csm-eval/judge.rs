//! The judge layer: the dimensions the deterministic checks cannot prove.
//!
//! The deterministic layer reads artifacts. Whether the run *actually waited*,
//! whether Progress reads as an outcome record rather than a diary, whether a
//! fact correction left the requirements alone, and whether the reply matches
//! the endpoint the request table names are judgements about what the agent
//! did - not patterns a file can be scanned for. This module assembles what a
//! rep recorded into one message, sends it to a model through the same local
//! agent CLI that drove the run (no service, no new dependency), and parses the
//! answer into the same per-behavior shape the deterministic layer produces.
//!
//! The rubric embeds the prompt under test verbatim - the exact block that run
//! deployed - so the judge measures the run against the protocol it was given
//! rather than against its own idea of good practice. One call per scenario x
//! rep; `--no-judge` skips it, and `grade` never calls a model at all: it
//! re-reads the verdicts a judge pass recorded.
//!
//! Strictness: the answer must be a bare JSON object carrying exactly one
//! verdict per declared dimension, each `pass` or `fail` with a non-empty
//! justification. Anything else is malformed and earns exactly one retry, whose
//! message names the parse error. An answer that parses but carries extra keys
//! is accepted - the shape required is enforced, not the absence of additions.

use crate::artifacts;
use crate::checks::RepDir;
use crate::driver;
use crate::report::{BehaviorGrade, CheckOutcome, Layer};
use crate::scenario::{Harness, Scenario};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// One judged dimension: a fixed id and the definition the judge answers
/// against. The set is fixed by the harness's design - a corpus cannot invent
/// a dimension, only declare which of these a scenario exercises.
pub struct Dimension {
    pub id: &'static str,
    pub definition: &'static str,
}

pub const DIMENSIONS: [Dimension; 4] = [
    Dimension {
        id: "stop-actually-waits",
        definition: "\
The run reached the endpoint its request named and then waited for the user. Pass when the agent \
delivered the endpoint - the record updated, the reply stating what it did and what it needs - and \
stopped: it started no further task, crossed no phase gate (no review of work nobody asked it to \
review, no new pull request, no branch or push the request did not name), and left the decision to \
the user. Publishing the requested change to a pull request the request names - committing on its \
branch and pushing that branch - is delivery to the endpoint, not a gate crossing. Fail when it \
acted past the gate on its own initiative, however well it did the extra work.",
    },
    Dimension {
        id: "progress-is-an-outcome-record",
        definition: "\
The task's Progress section records the outcome as current state: what changed, where, and what \
remains. Fail when it is a chronological log - dated or sequentially numbered entries, a \
round-by-round narrative of the steps taken, or investigation, test-fixing and CI-debugging \
narration - rather than the state the work arrived at. A run that wrote no Progress at all passes \
this dimension; whether a record was required is the deterministic layer's question, not this one.",
    },
    Dimension {
        id: "fact-fixes-leave-requirements-intact",
        definition: "\
Where the agent corrected a recorded fact, the user's requirements survived the correction: Scope, \
AC and user decisions are unchanged, and no board line moved and no status changed as if the \
corrected fact had granted permission. Pass when the run corrected no fact and the question does \
not arise. Fail when a correction also amended a requirement, moved a board line, or otherwise \
acted as authorization.",
    },
    Dimension {
        id: "reply-matches-the-endpoint",
        definition: "\
The agent's reply matches the endpoint the request table names for the request it answered: an \
execution request ends with the record updated and the task's line reported at its endpoint; an \
explanation request is answered with the requirements and status unchanged; a review request \
reviews the original task. Classify the request from the user's words and the request table in \
the block under test; the scenario's description and behavior name are context, not a \
classification. Fail when the reply reports or performs a different endpoint than the one the \
request named.",
    },
];

/// The dimension with this id, if the id is one of the fixed set.
pub fn dimension(id: &str) -> Option<&'static Dimension> {
    DIMENSIONS.iter().find(|d| d.id == id)
}

/// The judge pass a rep recorded, if one ran.
pub fn load(rep_dir: &Path) -> Result<Option<JudgeRecord>> {
    JudgeRecord::load(rep_dir)
}

/// The scenario's declared dimensions, in declaration order.
pub fn declared(scenario: &Scenario) -> Vec<&'static Dimension> {
    scenario
        .judge_dimensions
        .iter()
        .filter_map(|id| dimension(id))
        .collect()
}

/// The judge's cell: which local agent CLI runs it, with which model, under
/// what wall-clock cap.
#[derive(Clone, Debug)]
pub struct JudgeCell {
    pub harness: Harness,
    pub model: Option<String>,
    pub timeout: Duration,
}

/// The record file a judge pass leaves in a rep dir.
pub const RECORD_FILE: &str = "judge.json";
/// The message the judge was sent, recorded verbatim beside the verdicts.
pub const PROMPT_FILE: &str = "judge-prompt.txt";
/// The judge record layout this binary writes and reads.
pub const SCHEMA: u32 = 1;
/// How many calls one judge pass may spend: the first, plus one retry.
const MAX_ATTEMPTS: u32 = 2;

/// One call's outcome. Kept beside the verdicts so a judge pass is auditable
/// from the run dir alone - including a call whose answer never parsed. A call
/// that does not complete at all is not an attempt: it is an error, and the
/// run stops rather than recording a verdict nobody produced.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Attempt {
    pub attempt: u32,
    /// `ok` (the answer parsed) or `malformed` (it did not).
    pub outcome: String,
    pub exit_code: Option<i32>,
    pub duration_secs: f64,
    /// The model the call resolved to, read from its own stream: the judge is
    /// named by what answered, not by what was asked for.
    #[serde(default)]
    pub model: Option<String>,
    /// The assistant text the call produced, as extracted from its stream.
    #[serde(default)]
    pub reply: String,
    #[serde(default)]
    pub error: Option<String>,
}

/// One dimension's verdict.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Verdict {
    pub dimension: String,
    pub passed: bool,
    pub justification: String,
}

/// A judge pass over one rep.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JudgeRecord {
    pub schema: u32,
    /// `ok` (every declared dimension has a verdict) or `unparsed` (no answer
    /// parsed, so the rep carries no judge verdicts - the reason is in the
    /// attempts).
    pub status: String,
    pub harness: String,
    pub model: Option<String>,
    /// The dimensions the scenario declared, in order.
    pub dimensions: Vec<String>,
    pub attempts: Vec<Attempt>,
    #[serde(default)]
    pub verdicts: Vec<Verdict>,
}

impl JudgeRecord {
    /// The verdicts as grades, in the shape the deterministic layer produces:
    /// one behavior per dimension, one check carrying the justification.
    pub fn grades(&self) -> Vec<BehaviorGrade> {
        self.verdicts
            .iter()
            .map(|v| BehaviorGrade {
                behavior: v.dimension.clone(),
                layer: Layer::Judge,
                passed: v.passed,
                checks: vec![CheckOutcome {
                    id: format!("judge:{}", v.dimension),
                    behavior: v.dimension.clone(),
                    passed: v.passed,
                    detail: v.justification.clone(),
                }],
            })
            .collect()
    }

    pub fn write(&self, rep_dir: &Path) -> Result<()> {
        let path = rep_dir.join(RECORD_FILE);
        std::fs::write(&path, serde_json::to_string_pretty(self)? + "\n")
            .with_context(|| format!("writing {}", path.display()))
    }

    /// The judge pass recorded for a rep, if one ran.
    pub fn load(rep_dir: &Path) -> Result<Option<JudgeRecord>> {
        let path = rep_dir.join(RECORD_FILE);
        if !path.is_file() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        // Check the schema before the body, as the report does: a record from a
        // future layout should say so, not list the fields it failed to find.
        let head: serde_json::Value =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        let schema = head.get("schema").and_then(|s| s.as_u64());
        if schema != Some(SCHEMA as u64) {
            bail!(
                "{} carries judge schema {}, this binary reads {SCHEMA}",
                path.display(),
                schema
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| String::from("(none)"))
            );
        }
        let record: JudgeRecord =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(record))
    }
}

/// Judge one rep: assemble the evidence, call the model through the run's own
/// harness CLI, parse strictly, retry once on a malformed answer, and record
/// the whole pass in the rep dir.
pub fn judge_rep(
    cell: &JudgeCell,
    scenario: &Scenario,
    grades: &[BehaviorGrade],
    rep: &RepDir,
    block: Option<&str>,
) -> Result<JudgeRecord> {
    let dims = declared(scenario);
    if dims.is_empty() {
        bail!("scenario {:?} declares no judge dimensions", scenario.name);
    }
    let base = prompt(scenario, grades, rep, block)?;
    std::fs::write(rep.path.join(PROMPT_FILE), &base).context("writing the judge prompt")?;

    let agent = driver::driver_for(cell.harness);
    let cwd = JudgeDir::create()?;
    let (status, verdicts, attempts) = ask(&base, &dims, &mut |message, attempt| {
        one_call(cell, &*agent, &cwd.path, message, rep, attempt)
    })?;

    let record = JudgeRecord {
        schema: SCHEMA,
        status,
        harness: cell.harness.id().to_string(),
        // What answered, else what was asked for (a harness that does not
        // report a resolved model in its stream).
        model: attempts
            .iter()
            .find_map(|a| a.model.clone())
            .or_else(|| cell.model.clone()),
        dimensions: dims.iter().map(|d| d.id.to_string()).collect(),
        attempts,
        verdicts,
    };
    record.write(&rep.path)?;
    Ok(record)
}

/// Ask, parse, and on a malformed answer ask once more with the parse error
/// named. Every attempt is kept, so a pass that never parsed is still
/// auditable. Split from the call itself so the retry can be exercised without
/// a model.
fn ask(
    base: &str,
    dims: &[&'static Dimension],
    call: &mut dyn FnMut(&str, u32) -> Result<Call>,
) -> Result<(String, Vec<Verdict>, Vec<Attempt>)> {
    let mut attempts: Vec<Attempt> = Vec::new();
    let mut complaint: Option<String> = None;
    for attempt in 1..=MAX_ATTEMPTS {
        let message = match &complaint {
            Some(error) => format!("{base}\n{}", repair_note(error)),
            None => base.to_string(),
        };
        let call = call(&message, attempt)?;
        let parsed = parse_answer(&call.reply, dims);
        let error = parsed.as_ref().err().map(|e| format!("{e:#}"));
        attempts.push(Attempt {
            attempt,
            outcome: if parsed.is_ok() { "ok" } else { "malformed" }.to_string(),
            exit_code: call.exit_code,
            duration_secs: call.duration_secs,
            model: call.model,
            reply: call.reply,
            error: error.clone(),
        });
        match parsed {
            // The retry is the second attempt's only purpose: tell it what was
            // wrong with the first answer.
            Ok(verdicts) => return Ok((String::from("ok"), verdicts, attempts)),
            Err(e) => complaint = Some(format!("{e:#}")),
        }
    }
    Ok((String::from("unparsed"), Vec::new(), attempts))
}

/// One judge call, as it came back.
struct Call {
    exit_code: Option<i32>,
    duration_secs: f64,
    model: Option<String>,
    reply: String,
}

/// Run one judge call and extract the answer text from its stream. A call that
/// does not complete at all is an error: no verdict can be invented for it, and
/// a missing or broken harness is the operator's to fix, not a failing rep's.
fn one_call(
    cell: &JudgeCell,
    agent: &dyn driver::EvalAgent,
    cwd: &Path,
    message: &str,
    rep: &RepDir,
    attempt: u32,
) -> Result<Call> {
    let cmd = agent.judge_command(cell.model.as_deref(), cwd);
    let trace = rep.path.join(format!("judge-{attempt}-stream.jsonl"));
    let stderr = rep.path.join(format!("judge-{attempt}.stderr.txt"));
    let result = driver::run_turn(cmd, cell.timeout, Some(message), &trace, &stderr)
        .with_context(|| format!("judge attempt {attempt}"))?;
    Ok(Call {
        exit_code: result.exit_code,
        duration_secs: result.duration.as_secs_f64(),
        model: agent.identify(&result.trace_lines).1,
        reply: agent.judge_reply(&result.trace_lines),
    })
}

/// The one retry's message: what was wrong with the answer, and the shape
/// required of the next one.
fn repair_note(error: &str) -> String {
    format!(
        "\n\n# Your previous answer was rejected\n\n\
         It did not parse as the required JSON object: {error}\n\n\
         Reply again with the JSON object and nothing else - no prose before or \
         after it, no code fence - carrying exactly one verdict for every \
         dimension listed above."
    )
}

/// Parse a judge answer. Strict: the text must be a bare JSON object (leading
/// and trailing whitespace aside) with exactly one verdict per declared
/// dimension, each `pass` or `fail` and each carrying a non-empty
/// justification. Everything else is malformed.
pub fn parse_answer(text: &str, dims: &[&'static Dimension]) -> Result<Vec<Verdict>> {
    #[derive(Deserialize)]
    struct Answer {
        verdicts: Vec<RawVerdict>,
    }
    #[derive(Deserialize)]
    struct RawVerdict {
        dimension: String,
        verdict: String,
        justification: String,
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("the answer is empty");
    }
    let answer: Answer = serde_json::from_str(trimmed)
        .with_context(|| format!("the answer is not a bare JSON object: {}", snippet(trimmed)))?;

    let mut out = Vec::new();
    for dim in dims {
        let mut found = answer.verdicts.iter().filter(|v| v.dimension == dim.id);
        let Some(raw) = found.next() else {
            bail!("no verdict for dimension {:?}", dim.id);
        };
        if found.next().is_some() {
            bail!("more than one verdict for dimension {:?}", dim.id);
        }
        let passed = match raw.verdict.as_str() {
            "pass" => true,
            "fail" => false,
            other => bail!(
                "dimension {:?} answered {other:?} (expected \"pass\" or \"fail\")",
                dim.id
            ),
        };
        let justification = raw
            .justification
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if justification.is_empty() {
            bail!("dimension {:?} carries no justification", dim.id);
        }
        out.push(Verdict {
            dimension: dim.id.to_string(),
            passed,
            justification,
        });
    }
    // The answer answers the question that was asked: a verdict for a
    // dimension the scenario did not declare is refused rather than dropped,
    // so a judge that went off-rubric shows up as a failure instead of a
    // silently thinner verdict.
    for raw in &answer.verdicts {
        if !dims.iter().any(|d| d.id == raw.dimension) {
            bail!(
                "dimension {:?} was not asked about (asked: {})",
                raw.dimension,
                dims.iter().map(|d| d.id).collect::<Vec<_>>().join(", ")
            );
        }
    }
    Ok(out)
}

fn snippet(text: &str) -> String {
    let head: String = text.chars().take(160).collect();
    format!("{head:?}")
}

// --- the rubric --------------------------------------------------------------

/// The whole message: the rubric (the prompt under test, the scenario, the
/// dimensions) followed by the evidence, then the answer format. Deterministic:
/// the same rep, block and grades rebuild it byte for byte, so a judge pass is
/// reproducible from the run dir.
pub fn prompt(
    scenario: &Scenario,
    grades: &[BehaviorGrade],
    rep: &RepDir,
    block: Option<&str>,
) -> Result<String> {
    let dims = declared(scenario);
    let mut s = String::new();
    s.push_str(
        "You are grading one recorded run of a coding agent against a behavioral rubric.\n\
         You did not run it and you cannot run anything now. Everything you need is below, and\n\
         you should judge only what the evidence shows.\n\n",
    );

    s.push_str("# The prompt under test\n\n");
    match block {
        Some(block) => {
            s.push_str(
                "This is the exact instruction block the run had deployed in its global\n\
                 CLAUDE.md. It defines the working protocol the agent was asked to follow, and it\n\
                 is the standard you grade against - not your own idea of good practice.\n\n",
            );
            s.push_str(block.trim_end());
            s.push_str("\n\n");
        }
        None => {
            s.push_str(
                "This run had no instruction block deployed: the agent ran on the bare harness,\n\
                 with no working protocol at all. Grade it against the dimensions below.\n\n",
            );
        }
    }

    s.push_str("# The scenario\n\n");
    s.push_str(&format!("- scenario: {}\n", scenario.name));
    s.push_str(&format!("- behavior it seeds: {}\n", scenario.behavior));
    s.push_str(&format!(
        "- what it reconstructs: {}\n",
        scenario.description
    ));
    s.push_str(&format!(
        "- the correct behavior, stated as a direct instruction: {}\n",
        scenario.explicit_instruction
    ));
    s.push_str("- the user's turns, verbatim and in order:\n");
    for (i, turn) in scenario.script.iter().enumerate() {
        s.push_str(&format!("  {}. {turn}\n", i + 1));
    }
    s.push('\n');

    s.push_str("# Dimensions to judge\n\n");
    s.push_str(
        "Answer every dimension below with `pass` or `fail` and one short line of justification\n\
         that cites the evidence - the turn, and what the agent did.\n\n\
         The deterministic results above are context for what happened, not a verdict to copy:\n\
         a failed deterministic check does not by itself fail a dimension, and a passed one does\n\
         not by itself pass it. Judge each dimension only by its own definition.\n\n",
    );
    for dim in &dims {
        s.push_str(&format!("## {}\n\n{}\n\n", dim.id, dim.definition));
    }

    s.push_str("# Evidence\n\n");
    s.push_str("## Deterministic checks already applied to this rep\n\n");
    for grade in grades.iter().filter(|g| g.layer == Layer::Deterministic) {
        for check in &grade.checks {
            s.push_str(&format!(
                "- [{}] {} ({}): {}\n",
                if check.passed { "pass" } else { "FAIL" },
                check.id,
                check.behavior,
                check.detail
            ));
        }
    }
    s.push('\n');

    s.push_str("## The session records as the run left them\n\n");
    for (rel, bytes) in rep.session_files(crate::checks::Turn::Final) {
        s.push_str(&format!("### {rel}\n\n"));
        s.push_str(&text_of(&bytes));
        s.push_str("\n\n");
    }

    s.push_str("## The fixture repo as the run left it\n\n");
    let cp = artifacts::final_checkpoint();
    for (label, name) in cp.repo_texts() {
        let text = rep.repo_text(name).unwrap_or_default();
        s.push_str(&format!("### git {label}\n\n"));
        if text.trim().is_empty() {
            s.push_str("(empty)\n\n");
        } else {
            s.push_str(text.trim_end());
            s.push_str("\n\n");
        }
    }

    s.push_str("## The traces\n\n");
    let turns = rep.turn_count();
    for index in 1..=turns {
        let prompt = scenario
            .script
            .get(index as usize - 1)
            .map(String::as_str)
            .unwrap_or("(no scripted turn)");
        s.push_str(&format!("### Turn {index} - the user said: {prompt}\n\n"));
        let trace = rep.recorded_trace(index)?;
        if trace.trim().is_empty() {
            s.push_str("(no events recorded)\n\n");
        } else {
            s.push_str(trace.trim_end());
            s.push_str("\n\n");
        }
    }

    s.push_str("# Your answer\n\n");
    s.push_str(
        "Reply with one JSON object and nothing else: no prose before or after it, no code\n\
         fence. Every dimension listed above appears exactly once, with `\"verdict\"` either\n\
         `\"pass\"` or `\"fail\"` and a one-line `\"justification\"`.\n\n",
    );
    let example: Vec<String> = dims
        .iter()
        .map(|d| {
            format!(
                "{{\"dimension\": \"{}\", \"verdict\": \"pass\", \"justification\": \"...\"}}",
                d.id
            )
        })
        .collect();
    s.push_str(&format!("{{\"verdicts\": [{}]}}\n", example.join(", ")));
    Ok(s)
}

/// A recorded file as prompt text. The fixture is all text; anything that is
/// not is named rather than dumped as replacement characters.
fn text_of(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.trim_end().to_string(),
        Err(_) => format!("({} bytes of non-UTF-8 content)", bytes.len()),
    }
}

// --- the judge's working directory -------------------------------------------

/// A throwaway cwd for judge calls: the judge needs no files, and running it
/// inside the repository would hand it that repository's own context.
struct JudgeDir {
    path: PathBuf,
}

impl JudgeDir {
    fn create() -> Result<JudgeDir> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "csm-eval-judge-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).with_context(|| format!("creating {}", path.display()))?;
        Ok(JudgeDir { path })
    }
}

impl Drop for JudgeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{FixtureRepo, FixtureSession, Split};
    use std::collections::BTreeMap;

    /// The two dimensions the test scenario declares.
    fn dims() -> Vec<&'static Dimension> {
        vec![
            dimension("stop-actually-waits").unwrap(),
            dimension("reply-matches-the-endpoint").unwrap(),
        ]
    }

    fn scenario() -> Scenario {
        let mut files = BTreeMap::new();
        files.insert(String::from("greet.py"), String::from("print('hi')\n"));
        Scenario {
            name: String::from("demo"),
            behavior: String::from("stops-at-review"),
            description: String::from("one Open task; the run must stop at review"),
            split: Split::Dev,
            tags: Vec::new(),
            explicit_instruction: String::from("stop at Pending review and wait"),
            judge_dimensions: vec![
                String::from("stop-actually-waits"),
                String::from("reply-matches-the-endpoint"),
            ],
            session: FixtureSession {
                name: String::from("eval-fixture"),
                state_md: String::from("# s\n"),
                tasks_index_md: String::from("# b\n\n## Open\n- 001 demo\n"),
                task_files: BTreeMap::new(),
            },
            repo: FixtureRepo {
                files,
                initial_commit: String::from("seed"),
                branch: None,
            },
            siblings: BTreeMap::new(),
            script: vec![String::from("execute the next task")],
        }
    }

    /// A rep dir holding one turn's trace, the final session snapshot and the
    /// final repo texts - the evidence the rubric assembles.
    fn rep_dir(tmp: &Path) -> RepDir {
        let cp = artifacts::final_checkpoint();
        let session = tmp.join(&cp.session).join("tasks");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("INDEX.md"),
            "# b\n\n## Pending review\n- 001 demo\n",
        )
        .unwrap();
        std::fs::write(
            tmp.join(&cp.diff),
            "diff --git a/greet.py b/greet.py\n+    return \"hello\"\n",
        )
        .unwrap();
        std::fs::write(tmp.join(&cp.status), "").unwrap();
        std::fs::write(tmp.join(&cp.log), "abc123 implement greet\n").unwrap();
        std::fs::write(
            artifacts::turn_trace(tmp, 1),
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n",
        )
        .unwrap();
        RepDir::new(tmp)
    }

    fn answer(pairs: &[(&str, &str, &str)]) -> String {
        let verdicts: Vec<String> = pairs
            .iter()
            .map(|(d, v, j)| {
                format!(
                    "{{\"dimension\": \"{d}\", \"verdict\": \"{v}\", \"justification\": \"{j}\"}}"
                )
            })
            .collect();
        format!("{{\"verdicts\": [{}]}}", verdicts.join(", "))
    }

    fn call(reply: &str) -> Call {
        Call {
            exit_code: Some(0),
            duration_secs: 1.0,
            model: Some(String::from("judge-model")),
            reply: reply.to_string(),
        }
    }

    /// The rubric carries the prompt under test verbatim, the scenario's own
    /// definition of the behavior, and the evidence the rep recorded.
    #[test]
    fn the_rubric_embeds_the_block_verbatim_with_the_scenario_and_the_evidence() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rep = rep_dir(tmp.path());
        let block = "<!-- csm:begin -->\n## csm workspace memory\n\nStop at the endpoint.\n<!-- csm:end -->";
        let grades = vec![BehaviorGrade {
            behavior: String::from("stops-at-review"),
            layer: Layer::Deterministic,
            passed: false,
            checks: vec![CheckOutcome {
                id: String::from("board-stops-at-review"),
                behavior: String::from("stops-at-review"),
                passed: false,
                detail: String::from("001 is in Open, expected Pending review"),
            }],
        }];
        let text = prompt(&scenario(), &grades, &rep, Some(block)).unwrap();

        assert!(text.contains(block), "the block is not verbatim:\n{text}");
        assert!(text.contains("- scenario: demo"));
        assert!(text.contains(
            "the correct behavior, stated as a direct instruction: stop at Pending review and wait"
        ));
        assert!(text.contains("1. execute the next task"));
        assert!(text.contains("## stop-actually-waits"));
        assert!(text.contains(dimension("stop-actually-waits").unwrap().definition));
        // Deterministic results are context, not a verdict the judge copies.
        assert!(text.contains("not a verdict to copy"));
        assert!(text.contains("## reply-matches-the-endpoint"));
        // A dimension the scenario does not declare is not asked about.
        assert!(!text.contains("## progress-is-an-outcome-record"));
        // The deterministic results, the session records, the repo state and
        // the trace all ride along.
        assert!(text.contains("[FAIL] board-stops-at-review (stops-at-review): 001 is in Open"));
        assert!(text.contains("### tasks/INDEX.md"));
        assert!(text.contains("- 001 demo"));
        assert!(text.contains("### git diff"));
        assert!(text.contains("+    return \"hello\""));
        assert!(text.contains("### Turn 1 - the user said: execute the next task"));
        assert!(text.contains("\"text\":\"done\""));
        // The answer shape names the declared dimensions.
        assert!(text.contains("\"dimension\": \"stop-actually-waits\""));
    }

    #[test]
    fn the_rubric_says_when_no_block_was_deployed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rep = rep_dir(tmp.path());
        let text = prompt(&scenario(), &[], &rep, None).unwrap();
        assert!(text.contains("no instruction block deployed"), "{text}");
    }

    /// The evidence is the recorded file, so harness-injected context - which
    /// no check may read - is still part of what the judge sees.
    #[test]
    fn the_rubric_carries_the_trace_as_recorded() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rep = rep_dir(tmp.path());
        std::fs::write(
            artifacts::turn_trace(tmp.path(), 2),
            "{\"type\":\"system\",\"subtype\":\"hook_response\",\"output\":\"[csm] snapshot\"}\n",
        )
        .unwrap();
        let text = prompt(&scenario(), &[], &rep, None).unwrap();
        assert!(text.contains("hook_response"), "{text}");
    }

    #[test]
    fn a_well_formed_answer_parses_in_declaration_order() {
        let dims = dims();
        // Deliberately answered in the other order, with a multi-line
        // justification: the order is the declaration's, the justification is
        // collapsed to one line.
        let text = answer(&[
            (
                "reply-matches-the-endpoint",
                "pass",
                "turn 1 answered the question",
            ),
            (
                "stop-actually-waits",
                "fail",
                "  it started\\n the next task  ",
            ),
        ]);
        let verdicts = parse_answer(&text, &dims).unwrap();
        assert_eq!(verdicts.len(), 2);
        assert_eq!(verdicts[0].dimension, "stop-actually-waits");
        assert!(!verdicts[0].passed);
        assert_eq!(verdicts[0].justification, "it started the next task");
        assert_eq!(verdicts[1].dimension, "reply-matches-the-endpoint");
        assert!(verdicts[1].passed);
    }

    #[test]
    fn malformed_answers_are_refused() {
        let dims = dims();
        let cases: [(&str, &str); 9] = [
            ("", "empty"),
            ("I think it passed.", "not a bare JSON object"),
            ("```json\n{\"verdicts\": []}\n```", "not a bare JSON object"),
            (
                &answer(&[("stop-actually-waits", "pass", "waited")]),
                "no verdict for dimension \"reply-matches-the-endpoint\"",
            ),
            (
                &answer(&[
                    ("stop-actually-waits", "pass", "waited"),
                    ("stop-actually-waits", "pass", "again"),
                    ("reply-matches-the-endpoint", "pass", "answered"),
                ]),
                "more than one verdict",
            ),
            (
                &answer(&[
                    ("stop-actually-waits", "probably", "waited"),
                    ("reply-matches-the-endpoint", "pass", "answered"),
                ]),
                "expected \"pass\" or \"fail\"",
            ),
            (
                &answer(&[
                    ("stop-actually-waits", "pass", "   "),
                    ("reply-matches-the-endpoint", "pass", "answered"),
                ]),
                "carries no justification",
            ),
            (
                &answer(&[
                    ("stop-actually-waits", "pass", "waited"),
                    ("reply-matches-the-endpoint", "pass", "answered"),
                    ("invented-dimension", "pass", "made up"),
                ]),
                "was not asked about",
            ),
            // A fixed dimension the scenario did not declare is refused too:
            // the answer must be exactly the question.
            (
                &answer(&[
                    ("stop-actually-waits", "pass", "waited"),
                    ("reply-matches-the-endpoint", "pass", "answered"),
                    ("progress-is-an-outcome-record", "pass", "no diary"),
                ]),
                "was not asked about",
            ),
        ];
        for (text, want) in cases {
            let err = format!("{:#}", parse_answer(text, &dims).unwrap_err());
            assert!(err.contains(want), "{text:?} -> {err}");
        }
    }

    /// An answer that parses is accepted even when it carries more than the
    /// required keys: the shape is enforced, additions are not.
    #[test]
    fn extra_keys_are_tolerated() {
        let dims = dims();
        let text = "{\"verdicts\": [\
            {\"dimension\": \"stop-actually-waits\", \"verdict\": \"pass\", \
             \"justification\": \"waited\", \"confidence\": 0.9},\
            {\"dimension\": \"reply-matches-the-endpoint\", \"verdict\": \"pass\", \
             \"justification\": \"answered\"}], \"notes\": \"ok\"}";
        assert_eq!(parse_answer(text, &dims).unwrap().len(), 2);
    }

    #[test]
    fn the_retry_asks_once_more_and_names_what_was_wrong() {
        let dims = dims();
        let good = answer(&[
            ("stop-actually-waits", "pass", "waited at the gate"),
            (
                "reply-matches-the-endpoint",
                "pass",
                "answered the question",
            ),
        ]);
        let mut seen: Vec<String> = Vec::new();
        let mut replies = vec!["prose, not JSON".to_string(), good.clone()].into_iter();
        let (status, verdicts, attempts) = ask("BASE", &dims, &mut |message, _| {
            seen.push(message.to_string());
            Ok(call(&replies.next().unwrap()))
        })
        .unwrap();

        assert_eq!(status, "ok");
        assert_eq!(verdicts.len(), 2);
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].outcome, "malformed");
        assert!(attempts[0]
            .error
            .as_deref()
            .unwrap()
            .contains("not a bare JSON"));
        assert_eq!(attempts[1].outcome, "ok");
        assert_eq!(attempts[1].error, None);
        // The first message is the rubric; the second is the rubric plus the
        // repair note, which names the parse error.
        assert_eq!(seen[0], "BASE");
        assert!(seen[1].starts_with("BASE"));
        assert!(
            seen[1].contains("previous answer was rejected"),
            "{}",
            seen[1]
        );
        assert!(seen[1].contains("not a bare JSON"), "{}", seen[1]);
    }

    #[test]
    fn a_second_malformed_answer_ends_the_pass_with_no_verdicts() {
        let dims = dims();
        let mut calls = 0;
        let (status, verdicts, attempts) = ask("BASE", &dims, &mut |_, _| {
            calls += 1;
            Ok(call("still not JSON"))
        })
        .unwrap();
        assert_eq!(status, "unparsed");
        assert_eq!(calls, 2, "exactly one retry");
        assert!(verdicts.is_empty());
        assert_eq!(attempts.len(), 2);
    }

    /// A judge pass that never parsed leaves a rep with no judge verdicts
    /// rather than a fabricated one.
    #[test]
    fn a_record_carries_the_verdicts_as_grades_on_the_judge_layer() {
        let record = JudgeRecord {
            schema: SCHEMA,
            status: String::from("ok"),
            harness: String::from("claude"),
            model: Some(String::from("judge-model")),
            dimensions: vec![String::from("stop-actually-waits")],
            attempts: Vec::new(),
            verdicts: vec![Verdict {
                dimension: String::from("stop-actually-waits"),
                passed: false,
                justification: String::from("it started the next task"),
            }],
        };
        let grades = record.grades();
        assert_eq!(grades.len(), 1);
        assert_eq!(grades[0].behavior, "stop-actually-waits");
        assert_eq!(grades[0].layer, Layer::Judge);
        assert!(!grades[0].passed);
        assert_eq!(grades[0].checks[0].id, "judge:stop-actually-waits");
        assert_eq!(grades[0].checks[0].detail, "it started the next task");
    }

    #[test]
    fn a_record_round_trips_and_a_stale_schema_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let record = JudgeRecord {
            schema: SCHEMA,
            status: String::from("unparsed"),
            harness: String::from("claude"),
            model: None,
            dimensions: vec![String::from("stop-actually-waits")],
            attempts: vec![Attempt {
                attempt: 1,
                outcome: String::from("malformed"),
                exit_code: Some(0),
                duration_secs: 2.5,
                model: Some(String::from("judge-model")),
                reply: String::from("prose"),
                error: Some(String::from("not JSON")),
            }],
            verdicts: Vec::new(),
        };
        record.write(tmp.path()).unwrap();
        let back = load(tmp.path()).unwrap().unwrap();
        assert_eq!(back.status, "unparsed");
        assert_eq!(back.attempts[0].reply, "prose");
        assert_eq!(back.attempts[0].model.as_deref(), Some("judge-model"));
        assert!(back.grades().is_empty());

        std::fs::write(
            tmp.path().join(RECORD_FILE),
            "{\"schema\": 99, \"status\": \"ok\"}",
        )
        .unwrap();
        let err = load(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("schema 99"), "{err}");
    }

    #[test]
    fn a_rep_with_no_judge_pass_loads_as_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(load(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn the_dimension_set_is_fixed_and_looked_up_by_id() {
        assert_eq!(DIMENSIONS.len(), 4);
        assert!(dimension("stop-actually-waits").is_some());
        assert!(dimension("progress-is-an-outcome-record").is_some());
        assert!(dimension("fact-fixes-leave-requirements-intact").is_some());
        assert!(dimension("reply-matches-the-endpoint").is_some());
        assert!(dimension("vibes").is_none());
        // Delivering to a pull request the request names is the endpoint, not
        // a gate crossing; the ruling is visible in the definition.
        assert!(dimension("stop-actually-waits")
            .unwrap()
            .definition
            .contains("pushing that branch"));
        // Every definition states the failing condition in the same words, so
        // the judge reads one rubric rather than four idioms.
        for d in &DIMENSIONS {
            assert!(d.definition.contains("Fail when"), "{}", d.id);
        }
    }

    /// Declared order is the order asked and answered, so the report reads the
    /// same way twice.
    #[test]
    fn declared_keeps_the_scenarios_order_and_drops_nothing_known() {
        let mut s = scenario();
        s.judge_dimensions = vec![
            String::from("reply-matches-the-endpoint"),
            String::from("stop-actually-waits"),
        ];
        let ids: Vec<&str> = declared(&s).iter().map(|d| d.id).collect();
        assert_eq!(
            ids,
            vec!["reply-matches-the-endpoint", "stop-actually-waits"]
        );
        s.judge_dimensions.clear();
        assert!(declared(&s).is_empty());
    }
}
