//! Grading aggregation and the run report: one rep's check outcomes rolled up
//! to a behavior verdict, reps rolled up by majority, and the per-behavior
//! adherence matrix a run is judged by. `diff` turns two runs' matrices into
//! per-behavior deltas, which is the instrument's headline number.

use crate::checks::{ChecksFile, RepDir};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// One check's outcome inside one rep.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CheckOutcome {
    pub id: String,
    pub behavior: String,
    pub passed: bool,
    pub detail: String,
}

/// Which layer produced a verdict. The two measure different surfaces - the
/// deterministic layer reads artifacts, the judge layer reads what the agent
/// did - so a verdict is only readable alongside the layer that made it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    #[default]
    Deterministic,
    Judge,
}

impl Layer {
    pub fn id(&self) -> &'static str {
        match self {
            Layer::Deterministic => "deterministic",
            Layer::Judge => "judge",
        }
    }
}

/// One behavior's verdict inside one rep: every check naming that behavior
/// passed.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BehaviorGrade {
    pub behavior: String,
    /// Defaulted so a report written before the judge layer existed still
    /// loads: every verdict in one was deterministic.
    #[serde(default)]
    pub layer: Layer,
    pub passed: bool,
    pub checks: Vec<CheckOutcome>,
}

/// One rep's grades.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GradedRep {
    pub rep: String,
    pub verdicts: Vec<BehaviorGrade>,
}

/// One scenario's contribution to a run: its reps, each graded.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GradedScenario {
    pub scenario: String,
    /// The behavior the scenario seeds; the checks name their own, and this is
    /// the one the scenario is filed under.
    pub behavior: String,
    pub split: String,
    pub tags: Vec<String>,
    pub reps: Vec<GradedRep>,
}

/// The judge cell a run's judge verdicts came from, as the report states it.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JudgeCellRef {
    pub harness: String,
    pub model: Option<String>,
}

/// A whole run's report.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Report {
    pub schema: u32,
    pub run_id: String,
    pub arm: String,
    pub harness: String,
    pub model: Option<String>,
    pub harness_version: String,
    /// The cell that judged this run, or `None` when it was not judged (the
    /// deterministic layer alone).
    #[serde(default)]
    pub judge: Option<JudgeCellRef>,
    /// Repeats per scenario; the behavior verdict is the majority across them.
    pub k: usize,
    pub split: String,
    pub filter: Option<String>,
    pub prompt_git_sha: Option<String>,
    pub prompt_dirty: bool,
    pub block_sha256: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    pub scenarios: Vec<GradedScenario>,
}

/// The report schema this binary writes and reads.
pub const SCHEMA: u32 = 1;

/// One cell of the adherence matrix: behavior x scenario x arm.
#[derive(Serialize, Debug, Clone)]
pub struct Cell {
    pub behavior: String,
    pub scenario: String,
    pub arm: String,
    pub layer: &'static str,
    pub verdict: &'static str,
    pub reps_passed: usize,
    pub reps_total: usize,
}

impl GradedRep {
    /// The verdict for one behavior in this rep.
    pub fn verdict(&self, behavior: &str) -> Option<&BehaviorGrade> {
        self.verdicts.iter().find(|v| v.behavior == behavior)
    }
}

impl Report {
    /// The behavior ids the run covers, in first-seen order.
    fn behaviors(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for scenario in &self.scenarios {
            for rep in &scenario.reps {
                for v in &rep.verdicts {
                    if !out.contains(&v.behavior) {
                        out.push(v.behavior.clone());
                    }
                }
            }
        }
        out
    }

    /// The adherence matrix, one cell per behavior x scenario. A scenario whose
    /// checks name no check for a behavior does not measure it, so it carries
    /// no cell - the grid is the measured surface, not a full cross-product.
    pub fn cells(&self) -> Vec<Cell> {
        let behaviors = self.behaviors();
        let mut out = Vec::new();
        for scenario in &self.scenarios {
            for behavior in &behaviors {
                let graded: Vec<&BehaviorGrade> = scenario
                    .reps
                    .iter()
                    .filter_map(|r| r.verdict(behavior))
                    .collect();
                if graded.is_empty() {
                    continue;
                }
                let total = scenario.reps.len();
                let passed = graded.iter().filter(|v| v.passed).count();
                // The layer is a property of the behavior, not of the cell: a
                // behavior is measured by one layer or the other, never both.
                let layer = graded.first().map(|v| v.layer).unwrap_or_default().id();
                out.push(Cell {
                    behavior: behavior.clone(),
                    scenario: scenario.scenario.clone(),
                    arm: self.arm.clone(),
                    layer,
                    verdict: verdict_of(passed, total),
                    reps_passed: passed,
                    reps_total: total,
                });
            }
        }
        out
    }

    /// Behaviors whose majority verdict is adherence.
    pub fn adherent(&self) -> (usize, usize) {
        adherent_of(&self.cells())
    }

    pub fn write(&self, dir: &Path) -> Result<()> {
        std::fs::write(
            dir.join("report.json"),
            serde_json::to_string_pretty(self)? + "\n",
        )
        .context("writing report.json")?;
        std::fs::write(dir.join("report.md"), self.markdown()).context("writing report.md")?;
        Ok(())
    }

    pub fn load(run_dir: &Path) -> Result<Report> {
        let path = run_dir.join("report.json");
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        // Check the schema before the body: a report from a future or past
        // layout should say so, not list the fields it failed to find.
        let head: serde_json::Value =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        let schema = head.get("schema").and_then(|s| s.as_u64());
        anyhow::ensure!(
            schema == Some(SCHEMA as u64),
            "{} carries report schema {}, this binary reads {SCHEMA}",
            path.display(),
            schema
                .map(|s| s.to_string())
                .unwrap_or_else(|| String::from("(none)"))
        );
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    /// The human summary: the matrix, the adherence line, and the failing
    /// checks behind every failing cell.
    pub fn markdown(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("# csm-eval report\n\n- run: `{}`\n", self.run_id));
        s.push_str(&format!(
            "- arm: {} | harness: {} | model: {} | k: {}\n",
            self.arm,
            self.harness,
            self.model.as_deref().unwrap_or("(harness default)"),
            self.k
        ));
        s.push_str(&format!(
            "- split: {} | filter: {}\n",
            self.split,
            self.filter.as_deref().unwrap_or("(none)")
        ));
        s.push_str(&format!(
            "- judge: {}\n",
            match &self.judge {
                Some(j) => format!(
                    "{} / {}",
                    j.harness,
                    j.model.as_deref().unwrap_or("(harness default)")
                ),
                None => String::from("(not judged)"),
            }
        ));
        s.push_str(&format!(
            "- prompt: {} ({}) | block: {}\n",
            self.prompt_git_sha.as_deref().unwrap_or("(unknown)"),
            if self.prompt_dirty { "dirty" } else { "clean" },
            self.block_sha256.as_deref().unwrap_or("(none deployed)")
        ));
        s.push_str(&format!(
            "- harness version: {}\n- ran: {} -> {}\n\n",
            self.harness_version, self.started_at, self.finished_at
        ));

        let cells = self.cells();
        if cells.is_empty() {
            s.push_str("No behavior was graded.\n");
            return s;
        }
        s.push_str(
            "| behavior | scenario | layer | verdict | reps |\n\
             | --- | --- | --- | --- | --- |\n",
        );
        for c in &cells {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {}/{} |\n",
                c.behavior, c.scenario, c.layer, c.verdict, c.reps_passed, c.reps_total
            ));
        }
        let (passed, total) = adherent_of(&cells);
        s.push_str(&format!(
            "\nadherence: {passed}/{total} behaviors ({} arm).\n",
            self.arm
        ));
        s.push_str(
            "the instrument's headline is the delta between arms: \
             `csm-eval diff <baseline-run-dir> <this-run-dir>`.\n",
        );

        let failing = self.failing_checks();
        if !failing.is_empty() {
            s.push_str("\n## failing checks\n\n");
            for line in failing {
                s.push_str(&format!("- {line}\n"));
            }
        }
        s
    }

    /// One line per failing check, deduplicated across reps (the first rep
    /// that failed it carries the detail).
    fn failing_checks(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for scenario in &self.scenarios {
            for rep in &scenario.reps {
                for v in &rep.verdicts {
                    for c in v.checks.iter().filter(|c| !c.passed) {
                        if seen.insert(c.id.clone()) {
                            out.push(format!(
                                "{} / {} / {}: {}",
                                scenario.scenario, c.behavior, c.id, c.detail
                            ));
                        }
                    }
                }
            }
        }
        out
    }
}

/// The adherence line for an already-built matrix: how many of its cells pass.
fn adherent_of(cells: &[Cell]) -> (usize, usize) {
    let passed = cells.iter().filter(|c| c.verdict == "pass").count();
    (passed, cells.len())
}

/// A behavior's verdict from its rep tally: majority, with a tie failing.
pub fn verdict_of(passed: usize, total: usize) -> &'static str {
    if total == 0 {
        return "missing";
    }
    if passed * 2 > total {
        "pass"
    } else {
        "fail"
    }
}

/// Grade one rep: every check applied, grouped by behavior in file order.
pub fn grade_rep(checks_file: &ChecksFile, rep: &RepDir) -> Vec<BehaviorGrade> {
    let mut order: Vec<String> = Vec::new();
    let mut by_behavior: BTreeMap<String, Vec<CheckOutcome>> = BTreeMap::new();
    for check in &checks_file.checks {
        let outcome = crate::checks::evaluate(check, rep);
        if !by_behavior.contains_key(&check.behavior) {
            order.push(check.behavior.clone());
        }
        by_behavior
            .entry(check.behavior.clone())
            .or_default()
            .push(CheckOutcome {
                id: check.id.clone(),
                behavior: check.behavior.clone(),
                passed: outcome.passed,
                detail: outcome.detail,
            });
    }
    order
        .into_iter()
        .map(|behavior| {
            let checks = by_behavior.remove(&behavior).unwrap_or_default();
            let passed = checks.iter().all(|c| c.passed);
            BehaviorGrade {
                behavior,
                layer: Layer::Deterministic,
                passed,
                checks,
            }
        })
        .collect()
}

// --- diff ---------------------------------------------------------------------

/// How one behavior x scenario cell moved from the baseline run to the
/// candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaKind {
    Regressed,
    Improved,
    Unchanged,
    /// In the baseline, absent from the candidate.
    Missing,
    /// In the candidate, absent from the baseline.
    Added,
}

impl DeltaKind {
    fn label(&self) -> &'static str {
        match self {
            DeltaKind::Regressed => "REGRESSED",
            DeltaKind::Improved => "improved",
            DeltaKind::Unchanged => "=",
            DeltaKind::Missing => "MISSING",
            DeltaKind::Added => "added",
        }
    }

    /// A regression: a cell the baseline adhered to that the candidate no
    /// longer does, or one the candidate no longer measures at all.
    pub fn blocks(&self) -> bool {
        matches!(self, DeltaKind::Regressed | DeltaKind::Missing)
    }
}

#[derive(Debug, Clone)]
pub struct Delta {
    pub behavior: String,
    pub scenario: String,
    pub from: String,
    pub to: String,
    pub kind: DeltaKind,
}

/// Per-behavior deltas from `baseline` to `candidate`, ordered by behavior
/// then scenario.
pub fn diff(baseline: &Report, candidate: &Report) -> Vec<Delta> {
    // One matrix per report, keyed by cell: the lookup is a walk of the whole
    // scenario x behavior x rep grid otherwise, once per cell compared.
    let index = |report: &Report| -> BTreeMap<(String, String), String> {
        report
            .cells()
            .into_iter()
            .map(|c| ((c.behavior, c.scenario), c.verdict.to_string()))
            .collect()
    };
    let (base, cand) = (index(baseline), index(candidate));
    let mut keys: Vec<(String, String)> = base.keys().chain(cand.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .map(|(behavior, scenario)| {
            let from = base.get(&(behavior.clone(), scenario.clone())).cloned();
            let to = cand.get(&(behavior.clone(), scenario.clone())).cloned();
            let kind = match (from.as_deref(), to.as_deref()) {
                (Some("pass"), Some("pass")) => DeltaKind::Unchanged,
                (Some("fail"), Some("fail")) => DeltaKind::Unchanged,
                (Some("fail"), Some("pass")) => DeltaKind::Improved,
                (Some("pass"), Some("fail")) => DeltaKind::Regressed,
                (Some(_), Some(_)) => DeltaKind::Unchanged,
                (Some(_), None) => DeltaKind::Missing,
                (None, Some(_)) => DeltaKind::Added,
                (None, None) => DeltaKind::Unchanged,
            };
            Delta {
                behavior,
                scenario,
                from: from.unwrap_or_else(|| String::from("-")),
                to: to.unwrap_or_else(|| String::from("-")),
                kind,
            }
        })
        .collect()
}

/// The diff as a printable block, plus whether it blocks a deploy.
pub fn render_diff(baseline: &Report, candidate: &Report, deltas: &[Delta]) -> (String, bool) {
    let mut s = String::new();
    s.push_str(&format!(
        "baseline:  {} (arm {}, {} -> {})\n",
        baseline.run_id, baseline.arm, baseline.started_at, baseline.finished_at
    ));
    s.push_str(&format!(
        "candidate: {} (arm {}, {} -> {})\n\n",
        candidate.run_id, candidate.arm, candidate.started_at, candidate.finished_at
    ));
    // Column width: the widest cell in the column, never narrower than its
    // heading.
    let width = |heading: &str, field: fn(&Delta) -> &str| {
        deltas
            .iter()
            .map(|d| field(d).len())
            .chain(std::iter::once(heading.len()))
            .max()
            .unwrap_or(heading.len())
    };
    let (width, swidth) = (
        width("behavior", |d| &d.behavior),
        width("scenario", |d| &d.scenario),
    );
    s.push_str(&format!(
        "{:<width$}  {:<swidth$}  {:<6} {:<6} {}\n",
        "behavior", "scenario", "base", "cand", "delta"
    ));
    for d in deltas {
        s.push_str(&format!(
            "{:<width$}  {:<swidth$}  {:<6} {:<6} {}\n",
            d.behavior,
            d.scenario,
            d.from,
            d.to,
            d.kind.label()
        ));
    }
    let count = |k: DeltaKind| deltas.iter().filter(|d| d.kind == k).count();
    let blocking = deltas.iter().filter(|d| d.kind.blocks()).count();
    s.push_str(&format!(
        "\n{} regressed, {} missing, {} improved, {} unchanged\n",
        count(DeltaKind::Regressed),
        count(DeltaKind::Missing),
        count(DeltaKind::Improved),
        count(DeltaKind::Unchanged) + count(DeltaKind::Added)
    ));
    s.push_str(if blocking == 0 {
        "verdict: no regression\n"
    } else {
        "verdict: REGRESSION\n"
    });
    (s, blocking > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::ChecksFile;
    use std::path::PathBuf;

    fn rep_dir(tmp: &Path) -> PathBuf {
        let rep = tmp.join("rep-01");
        for cp in [
            crate::artifacts::turn_checkpoint(1),
            crate::artifacts::final_checkpoint(),
        ] {
            std::fs::create_dir_all(rep.join(&cp.session).join("tasks")).unwrap();
            std::fs::create_dir_all(rep.join(&cp.repo)).unwrap();
        }
        rep
    }

    fn checks_file(tmp: &Path, json: &str) -> ChecksFile {
        let p = tmp.join("checks.json");
        std::fs::write(&p, json).unwrap();
        ChecksFile::load(&p).unwrap()
    }

    /// A rep whose board moved 001 to Pending review.
    fn stopped_rep(tmp: &Path) -> PathBuf {
        let rep = rep_dir(tmp);
        let cp = crate::artifacts::final_checkpoint();
        std::fs::write(
            rep.join(&cp.session).join("tasks").join("INDEX.md"),
            "# b\n\n## Pending review\n- 001 demo - x\n",
        )
        .unwrap();
        rep
    }

    const TWO_BEHAVIORS: &str = r#"{"checks":[
        {"id":"board-stopped","behavior":"stops-at-review","kind":"board",
         "task":"001","section":"Pending review"},
        {"id":"board-seed","behavior":"stops-at-review","kind":"board",
         "task":"001","section":"Open","turn":0},
        {"id":"replies","behavior":"answers-the-question","kind":"reply","turn":1}
    ]}"#;

    fn report_for(arm: &str, scenarios: Vec<GradedScenario>) -> Report {
        Report {
            schema: SCHEMA,
            run_id: format!("run-{arm}"),
            arm: arm.to_string(),
            harness: String::from("claude"),
            model: Some(String::from("test-model")),
            harness_version: String::from("1.0"),
            judge: None,
            k: 1,
            split: String::from("dev"),
            filter: None,
            prompt_git_sha: Some(String::from("deadbeef")),
            prompt_dirty: false,
            block_sha256: None,
            started_at: String::from("t0"),
            finished_at: String::from("t1"),
            scenarios,
        }
    }

    fn scenario(name: &str, rep_verdicts: Vec<(&str, bool)>) -> GradedScenario {
        GradedScenario {
            scenario: name.to_string(),
            behavior: String::from("stops-at-review"),
            split: String::from("dev"),
            tags: Vec::new(),
            reps: rep_verdicts
                .into_iter()
                .enumerate()
                .map(|(i, (behavior, passed))| GradedRep {
                    rep: format!("rep-{:02}", i + 1),
                    verdicts: vec![BehaviorGrade {
                        behavior: behavior.to_string(),
                        layer: Layer::Deterministic,
                        passed,
                        checks: vec![CheckOutcome {
                            id: String::from("c"),
                            behavior: behavior.to_string(),
                            passed,
                            detail: String::from("detail"),
                        }],
                    }],
                })
                .collect(),
        }
    }

    #[test]
    fn grade_rep_groups_by_behavior_and_needs_every_check() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rep = stopped_rep(tmp.path());
        let checks = checks_file(tmp.path(), TWO_BEHAVIORS);
        let grades = grade_rep(&checks, &RepDir::new(&rep));
        assert_eq!(grades.len(), 2);
        // stops-at-review: the final board check passes, the turn-0 one has no
        // snapshot -> the behavior fails as a whole.
        assert_eq!(grades[0].behavior, "stops-at-review");
        assert!(!grades[0].passed);
        assert_eq!(grades[0].checks.len(), 2);
        // answers-the-question: no trace recorded at all.
        assert_eq!(grades[1].behavior, "answers-the-question");
        assert!(!grades[1].passed);
    }

    #[test]
    fn verdict_is_the_majority_with_ties_failing() {
        assert_eq!(verdict_of(3, 3), "pass");
        assert_eq!(verdict_of(2, 3), "pass");
        assert_eq!(verdict_of(1, 3), "fail");
        assert_eq!(verdict_of(0, 3), "fail");
        assert_eq!(verdict_of(1, 2), "fail"); // a tie is not adherence
        assert_eq!(verdict_of(0, 0), "missing");
    }

    #[test]
    fn cells_carry_the_three_dimensions_and_the_rep_tally() {
        let report = report_for(
            "working",
            vec![
                scenario(
                    "a",
                    vec![
                        ("stops-at-review", true),
                        ("stops-at-review", true),
                        ("stops-at-review", false),
                    ],
                ),
                scenario(
                    "b",
                    vec![("stops-at-review", false), ("stops-at-review", false)],
                ),
            ],
        );
        let cells = report.cells();
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].behavior, "stops-at-review");
        assert_eq!(cells[0].scenario, "a");
        assert_eq!(cells[0].arm, "working");
        assert_eq!(cells[0].verdict, "pass");
        assert_eq!((cells[0].reps_passed, cells[0].reps_total), (2, 3));
        assert_eq!(cells[1].verdict, "fail");
        assert_eq!(report.adherent(), (1, 2));
    }

    #[test]
    fn a_scenario_that_measures_no_check_for_a_behavior_carries_no_cell() {
        let report = report_for(
            "working",
            vec![
                scenario("a", vec![("stops-at-review", true)]),
                scenario("b", vec![("answers-the-question", true)]),
            ],
        );
        let cells = report.cells();
        assert_eq!(cells.len(), 2);
        assert!(cells
            .iter()
            .any(|c| c.behavior == "answers-the-question" && c.scenario == "b"));
        // Scenario "a" carries no check for the other behavior, so it does not
        // measure it and the grid does not invent a failing cell for it.
        assert!(!cells
            .iter()
            .any(|c| c.behavior == "answers-the-question" && c.scenario == "a"));
        assert_eq!(report.adherent(), (2, 2));
    }

    #[test]
    fn report_round_trips_through_json_and_renders_markdown() {
        let tmp = tempfile::TempDir::new().unwrap();
        let report = report_for(
            "working",
            vec![scenario("a", vec![("stops-at-review", false)])],
        );
        report.write(tmp.path()).unwrap();
        let back = Report::load(tmp.path()).unwrap();
        assert_eq!(back.run_id, report.run_id);
        assert_eq!(back.cells().len(), 1);
        let md = back.markdown();
        assert!(
            md.contains("| stops-at-review | a | deterministic | fail | 0/1 |"),
            "{md}"
        );
        assert!(md.contains("- judge: (not judged)"), "{md}");
        assert!(
            md.contains("adherence: 0/1 behaviors (working arm)"),
            "{md}"
        );
        assert!(md.contains("failing checks"), "{md}");
        assert!(md.contains("a / stops-at-review / c: detail"), "{md}");
    }

    /// A judged behavior sits in the same grid as a deterministic one, marked
    /// with the layer that produced it.
    #[test]
    fn a_judged_behavior_is_marked_with_its_layer() {
        let mut report = report_for(
            "working",
            vec![scenario("a", vec![("stops-at-review", true)])],
        );
        report.scenarios[0].reps[0].verdicts.push(BehaviorGrade {
            behavior: String::from("stop-actually-waits"),
            layer: Layer::Judge,
            passed: false,
            checks: vec![CheckOutcome {
                id: String::from("judge:stop-actually-waits"),
                behavior: String::from("stop-actually-waits"),
                passed: false,
                detail: String::from("the agent started the next task"),
            }],
        });
        report.judge = Some(JudgeCellRef {
            harness: String::from("claude"),
            model: Some(String::from("judge-model")),
        });
        let cells = report.cells();
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].layer, "deterministic");
        assert_eq!(cells[1].layer, "judge");
        let md = report.markdown();
        assert!(
            md.contains("| stop-actually-waits | a | judge | fail | 0/1 |"),
            "{md}"
        );
        assert!(md.contains("- judge: claude / judge-model"), "{md}");
        assert!(
            md.contains("a / stop-actually-waits / judge:stop-actually-waits: the agent started"),
            "{md}"
        );
    }

    /// A report written before the judge layer existed still loads: every
    /// verdict in one was deterministic, and that is what the default says.
    #[test]
    fn a_report_from_before_the_judge_layer_loads() {
        let tmp = tempfile::TempDir::new().unwrap();
        let json = r#"{
            "schema": 1, "run_id": "old", "arm": "working", "harness": "claude",
            "model": null, "harness_version": "1.0", "k": 1, "split": "dev",
            "filter": null, "prompt_git_sha": null, "prompt_dirty": false,
            "block_sha256": null, "started_at": "t0", "finished_at": "t1",
            "scenarios": [{"scenario": "a", "behavior": "b", "split": "dev",
                "tags": [], "reps": [{"rep": "rep-01", "verdicts": [
                    {"behavior": "b", "passed": true, "checks": []}]}]}]}"#;
        std::fs::write(tmp.path().join("report.json"), json).unwrap();
        let report = Report::load(tmp.path()).unwrap();
        assert!(report.judge.is_none());
        assert_eq!(
            report.scenarios[0].reps[0].verdicts[0].layer,
            Layer::Deterministic
        );
        assert_eq!(report.cells()[0].layer, "deterministic");
    }

    #[test]
    fn a_stale_report_schema_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("report.json"), r#"{"schema":99}"#).unwrap();
        let err = Report::load(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("schema 99"), "{err}");
    }

    #[test]
    fn diff_names_regressions_improvements_and_disappearances() {
        let baseline = report_for(
            "no-block",
            vec![
                scenario("a", vec![("stops-at-review", false)]),
                scenario("b", vec![("stops-at-review", true)]),
            ],
        );
        let candidate = report_for(
            "working",
            vec![
                scenario("a", vec![("stops-at-review", true)]),
                scenario("c", vec![("stops-at-review", true)]),
            ],
        );
        let deltas = diff(&baseline, &candidate);
        let by = |scenario: &str| deltas.iter().find(|d| d.scenario == scenario).unwrap().kind;
        assert_eq!(by("a"), DeltaKind::Improved);
        assert_eq!(by("b"), DeltaKind::Missing); // measured by the baseline, gone now
        assert_eq!(by("c"), DeltaKind::Added);
        assert!(DeltaKind::Missing.blocks());
        assert!(DeltaKind::Regressed.blocks());
        assert!(!DeltaKind::Added.blocks());
        assert!(!DeltaKind::Improved.blocks());

        let (text, blocks) = render_diff(&baseline, &candidate, &deltas);
        assert!(blocks, "{text}");
        assert!(text.contains("MISSING"), "{text}");
        assert!(text.contains("verdict: REGRESSION"), "{text}");
    }

    #[test]
    fn a_candidate_that_keeps_every_cell_does_not_block() {
        let baseline = report_for(
            "no-block",
            vec![scenario("a", vec![("stops-at-review", false)])],
        );
        let candidate = report_for(
            "working",
            vec![scenario("a", vec![("stops-at-review", true)])],
        );
        let deltas = diff(&baseline, &candidate);
        let (text, blocks) = render_diff(&baseline, &candidate, &deltas);
        assert!(!blocks, "{text}");
        assert!(text.contains("verdict: no regression"), "{text}");

        // And a candidate that loses a cell the baseline held does block.
        let worse = report_for(
            "working",
            vec![scenario("a", vec![("stops-at-review", false)])],
        );
        let back = report_for(
            "working",
            vec![scenario("a", vec![("stops-at-review", true)])],
        );
        let (text, blocks) = render_diff(&back, &worse, &diff(&back, &worse));
        assert!(blocks, "{text}");
        assert!(text.contains("REGRESSED"), "{text}");
    }
}
