//! Headless multi-turn driving. One driver per harness, all behind a small
//! trait (mirroring the product's `dyn Agent` shape): build one command per
//! scripted turn - first turn starts a new conversation, later turns resume
//! it - with the full event stream captured per turn.

use crate::sandbox::{self, Sandbox};
use crate::scenario::Harness;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The auto-mode cell: permissions are bypassed at the harness level and this
/// keep-going system instruction rides on every turn, first and resumed. It
/// fills every pause the working protocol leaves unstated.
pub const AUTO_MODE_PROMPT: &str =
    "Keep going without stopping: make reasonable judgments and continue.";

/// Everything a driver needs to build one turn's command.
pub struct TurnArgs<'a> {
    pub sb: &'a Sandbox,
    pub session_name: &'a str,
    /// The scripted user turn, executed verbatim.
    pub prompt: &'a str,
    /// `true` for turn 1 (new conversation); later turns resume `session_id`.
    pub first: bool,
    /// Conversation id from the first turn's trace (resume target).
    pub session_id: &'a str,
    /// Optional explicit model id (the cell's model).
    pub model: Option<&'a str>,
    /// The `explicit` arm's correct-behavior instruction, appended to the
    /// scripted turn so the treatment rides on the user message rather than on
    /// the deployed block.
    pub explicit: Option<&'a str>,
    /// Seatbelt profile to route the command through, when active.
    pub seatbelt: &'a Option<PathBuf>,
}

/// The prompt the harness actually receives: the scripted turn, plus the
/// `explicit` arm's instruction when that arm is active.
fn composed_prompt(args: &TurnArgs) -> String {
    match args.explicit {
        Some(instruction) => format!("{}\n\n{instruction}", args.prompt),
        None => args.prompt.to_string(),
    }
}

pub struct TurnResult {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration: Duration,
    /// The captured event stream (also written to the trace file).
    pub trace_lines: Vec<String>,
}

pub trait EvalAgent {
    fn id(&self) -> &'static str;

    /// Harness version string for the manifest.
    fn version(&self) -> Result<String>;

    /// Build the command for one scripted turn. Env/cwd are set here so every
    /// turn runs inside the sandbox against the fixture repo.
    fn turn_command(&self, args: &TurnArgs) -> Result<Command>;

    /// The harness's own on-disk conversation transcript inside the sandbox
    /// (a richer trace than the event stream; copied into the run dir).
    fn transcript(&self, sb: &Sandbox, session_id: &str) -> Option<PathBuf>;

    /// Pull (session_id, model) out of a turn's event stream. `session_id` is
    /// required after turn 1 (resume target); `model` feeds the manifest.
    fn identify(&self, trace_lines: &[String]) -> (Option<String>, Option<String>);

    /// The command for one judge call: the same CLI the run used, outside any
    /// sandbox, with the rubric arriving on stdin. The call grants no tool
    /// permission and runs headless, so a tool use would be denied; the rubric
    /// carries the evidence the judge is meant to read.
    fn judge_command(&self, model: Option<&str>, cwd: &Path) -> Command;

    /// The answer text in a judge call's event stream. Shape-drift tolerant in
    /// the same way trace matching is: the assistant's text blocks, falling
    /// back to the harness's own result field.
    fn judge_reply(&self, trace_lines: &[String]) -> String {
        let text = crate::checks::assistant_text_all(trace_lines);
        if !text.trim().is_empty() {
            return text;
        }
        trace_lines
            .iter()
            .rev()
            .find_map(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .and_then(|v| v.get("result")?.as_str().map(String::from))
            })
            .unwrap_or_default()
    }
}

/// The environment one judge call runs under: the operator's own, minus the
/// csm session binding. The deployed block is dormant while `$CSM_SESSION` is
/// unset, so the judge is not itself running under the prompt it is judging.
fn judge_env(cmd: &mut Command) {
    cmd.env_remove("CSM_SESSION");
    cmd.env_remove("CSM_HOME");
}

pub fn driver_for(harness: Harness) -> Box<dyn EvalAgent> {
    match harness {
        Harness::Claude => Box::new(ClaudeDriver),
        Harness::Codex => Box::new(CodexDriver),
    }
}

// --- claude -----------------------------------------------------------------

pub struct ClaudeDriver;

impl ClaudeDriver {
    const STREAM_FLAGS: [&str; 4] = [
        "--output-format",
        "stream-json",
        "--verbose",
        "--dangerously-skip-permissions",
    ];
}

impl EvalAgent for ClaudeDriver {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn version(&self) -> Result<String> {
        let out = Command::new("claude")
            .arg("--version")
            .output()
            .context("running claude --version (is claude installed?)")?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn turn_command(&self, args: &TurnArgs) -> Result<Command> {
        let mut cmd = Command::new("claude");
        cmd.arg("-p").arg(composed_prompt(args));
        cmd.args(Self::STREAM_FLAGS);
        cmd.args(["--append-system-prompt", AUTO_MODE_PROMPT]);
        if let Some(model) = args.model {
            cmd.args(["--model", model]);
        }
        if args.first {
            cmd.args(["--session-id", args.session_id]);
        } else {
            cmd.args(["--resume", args.session_id]);
        }
        sandbox::apply_env(&mut cmd, args.sb, args.session_name);
        sandbox::wrap_seatbelt(&mut cmd, args.seatbelt);
        cmd.current_dir(&args.sb.repo);
        Ok(cmd)
    }

    fn transcript(&self, sb: &Sandbox, session_id: &str) -> Option<PathBuf> {
        // The project dir name is the repo path with `/` folded to `-`; the
        // exact spelling varies with symlink resolution (/tmp vs /private/tmp
        // on macOS), so find by the session id, which is unique.
        let projects = sb.home.join(".claude").join("projects");
        let wanted = format!("{session_id}.jsonl");
        let mut found = None;
        if let Ok(entries) = std::fs::read_dir(&projects) {
            for entry in entries.flatten() {
                let candidate = entry.path().join(&wanted);
                if candidate.exists() {
                    found = Some(candidate);
                    break;
                }
            }
        }
        found
    }

    fn judge_command(&self, model: Option<&str>, cwd: &Path) -> Command {
        let mut cmd = Command::new("claude");
        // `-p` with no prompt argument reads the rubric from stdin: the
        // assembled evidence is far past what an argv can carry.
        cmd.args(["-p", "--output-format", "stream-json", "--verbose"]);
        if let Some(model) = model {
            cmd.args(["--model", model]);
        }
        judge_env(&mut cmd);
        cmd.current_dir(cwd);
        cmd
    }

    fn identify(&self, trace_lines: &[String]) -> (Option<String>, Option<String>) {
        // The `system/init` event carries the session id and resolved model.
        for line in trace_lines {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v.get("type").and_then(|t| t.as_str()) == Some("system")
                && v.get("subtype").and_then(|t| t.as_str()) == Some("init")
            {
                return (
                    v.get("session_id")
                        .and_then(|s| s.as_str())
                        .map(String::from),
                    v.get("model").and_then(|s| s.as_str()).map(String::from),
                );
            }
        }
        (None, None)
    }
}

// --- codex ------------------------------------------------------------------

pub struct CodexDriver;

/// Codex exec flags shared by first and resumed turns: JSONL events, cwd
/// pinned to the fixture repo, approvals and its own sandbox bypassed (the
/// seatbelt profile is the write boundary here), hook trust bypassed so the
/// csm SessionStart hook runs without an interactive `/hooks` review.
const CODEX_EXEC_FLAGS: [&str; 6] = [
    "--json",
    "-C",
    "REPO", // placeholder, replaced per turn
    "--dangerously-bypass-approvals-and-sandbox",
    "--dangerously-bypass-hook-trust",
    "--skip-git-repo-check",
];

impl CodexDriver {
    /// `codex exec` against `dir`, without the per-turn prompt. A judge call
    /// and a scripted turn differ only in what they run against and what they
    /// are asked, so the flags live here once.
    fn base_command_at(&self, dir: &Path) -> Command {
        let dir = dir.display().to_string();
        let flags: Vec<&str> = CODEX_EXEC_FLAGS
            .iter()
            .map(|f| if *f == "REPO" { dir.as_str() } else { *f })
            .collect();
        let mut cmd = Command::new("codex");
        cmd.arg("exec");
        cmd.args(&flags);
        cmd
    }

    fn base_command(&self, args: &TurnArgs) -> Command {
        let mut cmd = self.base_command_at(&args.sb.repo);
        if let Some(model) = args.model {
            cmd.args(["-m", model]);
        }
        cmd
    }
}

impl EvalAgent for CodexDriver {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn version(&self) -> Result<String> {
        let out = Command::new("codex")
            .arg("--version")
            .output()
            .context("running codex --version (is codex installed?)")?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn turn_command(&self, args: &TurnArgs) -> Result<Command> {
        let mut cmd = self.base_command(args);
        let prompt = composed_prompt(args);
        if args.first {
            cmd.arg(prompt);
        } else {
            cmd.arg("resume").arg(args.session_id).arg(prompt);
        }
        sandbox::apply_env(&mut cmd, args.sb, args.session_name);
        sandbox::wrap_seatbelt(&mut cmd, args.seatbelt);
        cmd.current_dir(&args.sb.repo);
        Ok(cmd)
    }

    fn transcript(&self, sb: &Sandbox, session_id: &str) -> Option<PathBuf> {
        // Rollout files live under $CODEX_HOME/sessions/<y>/<m>/<d>/; match by
        // session id inside the filename.
        let sessions = sb.codex_home.join("sessions");
        let mut found = None;
        let Ok(stack) = std::fs::read_dir(&sessions) else {
            return None;
        };
        let mut dirs = vec![stack];
        while let Some(rd) = dirs.pop() {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Ok(sub) = std::fs::read_dir(&path) {
                        dirs.push(sub);
                    }
                } else if path
                    .file_name()
                    .map(|n| n.to_string_lossy().contains(session_id))
                    .unwrap_or(false)
                {
                    found = Some(path);
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        found
    }

    fn judge_command(&self, model: Option<&str>, cwd: &Path) -> Command {
        let mut cmd = self.base_command_at(cwd);
        if let Some(model) = model {
            cmd.args(["-m", model]);
        }
        judge_env(&mut cmd);
        cmd.current_dir(cwd);
        cmd
    }

    fn identify(&self, trace_lines: &[String]) -> (Option<String>, Option<String>) {
        // Codex event shapes drift between versions; hunt for a session/thread
        // id anywhere in the stream, top level or nested.
        for line in trace_lines {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            for candidate in [
                v.get("session_id"),
                v.get("thread_id"),
                v.get("msg").and_then(|m| m.get("session_id")),
            ] {
                if let Some(s) = candidate.and_then(|s| s.as_str()) {
                    if !s.is_empty() {
                        return (Some(s.to_string()), None);
                    }
                }
            }
        }
        (None, None)
    }
}

// --- turn runner --------------------------------------------------------------

/// Run one turn: spawn, stream stdout line-by-line into the trace file
/// (progressive, so a crash or timeout still leaves a usable trace), cap
/// stderr, kill on timeout. Without `stdin_text` stdin is null - a harness
/// prompting for input reads EOF instead of hanging; with it, the text is fed
/// on a writer thread, so a message larger than the pipe buffer (the judge's
/// assembled evidence) cannot deadlock against the child's output.
pub fn run_turn(
    cmd: Command,
    timeout: Duration,
    stdin_text: Option<&str>,
    trace_path: &Path,
    stderr_path: &Path,
) -> Result<TurnResult> {
    let mut cmd = cmd;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    match stdin_text {
        Some(_) => cmd.stdin(Stdio::piped()),
        None => cmd.stdin(Stdio::null()),
    };
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning {}", cmd.get_program().to_string_lossy()))?;
    let stdout = child.stdout.take().context("turn stdout")?;
    let stderr = child.stderr.take().context("turn stderr")?;
    if let Some(text) = stdin_text {
        let mut stdin = child.stdin.take().context("turn stdin")?;
        let payload = text.as_bytes().to_vec();
        std::thread::spawn(move || {
            // A child that exits before reading closes the pipe; the broken
            // write is the child's answer, not ours to report.
            let _ = stdin.write_all(&payload);
        });
    }

    let trace_path = trace_path.to_path_buf();
    let reader = std::thread::spawn(move || {
        let mut lines = Vec::new();
        if let Ok(file) = std::fs::File::create(&trace_path) {
            let mut writer = std::io::BufWriter::new(file);
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if writeln!(writer, "{l}").is_err() {
                            break;
                        }
                        lines.push(l);
                    }
                    Err(_) => break,
                }
            }
        }
        lines
    });

    let stderr_path = stderr_path.to_path_buf();
    let err_reader = std::thread::spawn(move || {
        let mut lines = Vec::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            lines.push(line);
        }
        lines
    });

    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(Some(status)),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break Ok(None);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => break Err(e),
        }
    };
    let duration = start.elapsed();

    let trace_lines = reader.join().unwrap_or_default();
    let stderr_lines = err_reader.join().unwrap_or_default();
    let mut err_file = std::fs::File::create(stderr_path).context("writing turn stderr")?;
    for line in stderr_lines {
        writeln!(err_file, "{line}")?;
    }

    let Some(status) = status? else {
        return Ok(TurnResult {
            exit_code: None,
            timed_out,
            duration,
            trace_lines,
        });
    };
    Ok(TurnResult {
        exit_code: status.code(),
        timed_out,
        duration,
        trace_lines,
    })
}

/// Generate a random UUID string for `--session-id` (16 bytes from
/// /dev/urandom, formatted as a canonical v4 UUID - no extra dependency).
pub fn new_uuid() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .context("reading /dev/urandom for a session id")?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        h[0..4].concat(),
        h[4..6].concat(),
        h[6..8].concat(),
        h[8..10].concat(),
        h[10..16].concat()
    ))
}

/// Fail fast with a readable message when a resumed turn has no session id.
pub fn require_session_id(id: &Option<String>) -> Result<&str> {
    id.as_deref()
        .context("no session id found in the first turn's trace; cannot resume")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::Harness;

    /// Lookup one env var on a built Command (std exposes get_envs, not get_env).
    fn env_of(cmd: &Command, key: &str) -> Option<String> {
        let want = std::ffi::OsStr::new(key);
        cmd.get_envs()
            .find(|(k, _)| *k == want)
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().to_string()))
    }

    fn sandbox() -> Sandbox {
        Sandbox::create().unwrap()
    }

    fn args<'a>(
        sb: &'a Sandbox,
        first: bool,
        session_id: &'a str,
        seatbelt: &'a Option<PathBuf>,
    ) -> TurnArgs<'a> {
        TurnArgs {
            sb,
            session_name: "eval-fixture",
            prompt: "execute the next task",
            first,
            session_id,
            model: Some("test-model"),
            explicit: None,
            seatbelt,
        }
    }

    #[test]
    fn claude_first_turn_starts_session() {
        let sb = sandbox();
        let keep = sb.root.clone();
        let seatbelt = None;
        let cmd = ClaudeDriver
            .turn_command(&args(&sb, true, "sid-1", &seatbelt))
            .unwrap();
        let flat: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(cmd.get_program(), "claude");
        assert!(flat.contains(&"-p".to_string()));
        assert!(flat.contains(&"--session-id".to_string()));
        assert!(!flat.contains(&"--resume".to_string()));
        assert!(flat.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(flat.contains(&"--output-format".to_string()));
        let api = flat
            .iter()
            .position(|a| a == "--append-system-prompt")
            .unwrap();
        assert_eq!(flat[api + 1], AUTO_MODE_PROMPT);
        // Sandbox env + seatbelt-free program.
        assert_eq!(
            env_of(&cmd, "CSM_SESSION"),
            Some("eval-fixture".to_string())
        );
        let _ = sb.remove();
        assert!(!keep.exists());
    }

    #[test]
    fn claude_later_turns_resume() {
        let sb = sandbox();
        let seatbelt = None;
        let cmd = ClaudeDriver
            .turn_command(&args(&sb, false, "sid-1", &seatbelt))
            .unwrap();
        let flat: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(flat.contains(&"--resume".to_string()));
        assert!(!flat.contains(&"--session-id".to_string()));
        let _ = sb.remove();
    }

    #[test]
    fn codex_exec_shape_first_and_resume() {
        let sb = sandbox();
        let seatbelt = None;
        let first = CodexDriver
            .turn_command(&args(&sb, true, "tid-1", &seatbelt))
            .unwrap();
        let flat: Vec<String> = first
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(first.get_program(), "codex");
        assert!(flat.contains(&"exec".to_string()));
        assert!(flat.contains(&"--json".to_string()));
        assert!(flat.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(flat.contains(&"--dangerously-bypass-hook-trust".to_string()));
        assert!(flat.contains(&"execute the next task".to_string()));
        assert!(!flat.contains(&"resume".to_string()));

        let resumed = CodexDriver
            .turn_command(&args(&sb, false, "tid-1", &seatbelt))
            .unwrap();
        let flat: Vec<String> = resumed
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let resume_at = flat.iter().position(|a| a == "resume").unwrap();
        assert_eq!(flat[resume_at + 1], "tid-1");
        let _ = sb.remove();
    }

    #[test]
    fn seatbelt_wraps_the_program() {
        let sb = sandbox();
        let profile = Some(sb.root.join("sandbox.sb"));
        let cmd = ClaudeDriver
            .turn_command(&args(&sb, true, "sid-1", &profile))
            .unwrap();
        assert_eq!(cmd.get_program(), "sandbox-exec");
        let flat: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(flat[0], "-f");
        assert!(flat.iter().any(|a| a == "claude"));
        // The curated env survives the wrap.
        assert!(env_of(&cmd, "HOME").is_some());
        let _ = sb.remove();
    }

    #[test]
    fn identify_reads_claude_init_event() {
        let init =
            r#"{"type":"system","subtype":"init","session_id":"abc-123","model":"glm-5.3-flash"}"#;
        let other = r#"{"type":"assistant","message":{}}"#;
        let (sid, model) = ClaudeDriver.identify(&[other.to_string(), init.to_string()]);
        assert_eq!(sid.as_deref(), Some("abc-123"));
        assert_eq!(model.as_deref(), Some("glm-5.3-flash"));
        let (sid, _) = ClaudeDriver.identify(&[other.to_string()]);
        assert_eq!(sid, None);
    }

    #[test]
    fn identify_reads_codex_thread_id() {
        let started = r#"{"type":"thread.started","thread_id":"th-9"}"#;
        let (sid, _) = CodexDriver.identify(&[started.to_string()]);
        assert_eq!(sid.as_deref(), Some("th-9"));
    }

    /// The judge call is the same CLI as the run, outside any sandbox: no
    /// session binding, no scratch HOME, and the rubric on stdin.
    #[test]
    fn claude_judge_call_reads_stdin_outside_the_sandbox() {
        let cwd = std::env::temp_dir();
        let cmd = ClaudeDriver.judge_command(Some("judge-model"), &cwd);
        let flat: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(cmd.get_program(), "claude");
        assert!(flat.contains(&"-p".to_string()));
        assert!(flat.contains(&"--output-format".to_string()));
        assert!(flat.contains(&"--verbose".to_string()));
        assert_eq!(
            flat.iter()
                .position(|a| a == "--model")
                .map(|i| &flat[i + 1]),
            Some(&"judge-model".to_string())
        );
        assert_eq!(cmd.get_current_dir(), Some(cwd.as_path()));
        // The prompt arrives on stdin, so `-p` carries no argument.
        let p = flat.iter().position(|a| a == "-p").unwrap();
        assert_eq!(flat[p + 1], "--output-format");
        // The operator's own environment, minus the session binding that would
        // wake the deployed block up in the judge's own context.
        assert_eq!(env_of(&cmd, "CSM_SESSION"), None);
        assert_eq!(env_of(&cmd, "CSM_HOME"), None);
        assert_eq!(
            env_of(&cmd, "HOME"),
            None,
            "HOME is inherited, not replaced"
        );
    }

    #[test]
    fn codex_judge_call_runs_exec_against_the_judge_dir() {
        let cwd = std::env::temp_dir();
        let cmd = CodexDriver.judge_command(None, &cwd);
        let flat: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(cmd.get_program(), "codex");
        assert_eq!(flat[0], "exec");
        assert!(flat.contains(&"--json".to_string()));
        let c = flat.iter().position(|a| a == "-C").unwrap();
        assert_eq!(flat[c + 1], cwd.display().to_string());
        // No positional prompt: it arrives on stdin.
        assert_eq!(flat.last().unwrap(), "--skip-git-repo-check");
        assert_eq!(env_of(&cmd, "CSM_SESSION"), None);
    }

    #[test]
    fn the_judge_reply_is_the_assistant_text_or_the_result_field() {
        let blocks = [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"{\"verdicts\":[]}"}]}}"#,
            r#"{"type":"result","subtype":"success","result":"ignored when text exists"}"#,
        ];
        let lines: Vec<String> = blocks.iter().map(|s| s.to_string()).collect();
        assert_eq!(ClaudeDriver.judge_reply(&lines), "{\"verdicts\":[]}");

        // A stream with no text block at all falls back to the harness's own
        // result field.
        let only_result = vec![
            r#"{"type":"system","subtype":"init","session_id":"s"}"#.to_string(),
            r#"{"type":"result","subtype":"success","result":"{\"verdicts\":[]}"}"#.to_string(),
        ];
        assert_eq!(ClaudeDriver.judge_reply(&only_result), "{\"verdicts\":[]}");
        assert_eq!(ClaudeDriver.judge_reply(&[]), "");
    }

    #[test]
    fn harness_dispatch() {
        assert_eq!(driver_for(Harness::Claude).id(), "claude");
        assert_eq!(driver_for(Harness::Codex).id(), "codex");
    }

    #[test]
    fn the_explicit_arm_appends_its_instruction_to_the_turn() {
        let sb = sandbox();
        let seatbelt = None;
        let mut a = args(&sb, true, "sid-1", &seatbelt);
        a.explicit = Some("stop at Pending review");
        let cmd = ClaudeDriver.turn_command(&a).unwrap();
        let flat: Vec<String> = cmd
            .get_args()
            .map(|x| x.to_string_lossy().to_string())
            .collect();
        let p = flat.iter().position(|x| x == "-p").unwrap();
        assert_eq!(
            flat[p + 1],
            "execute the next task\n\nstop at Pending review"
        );

        // Codex takes the prompt positionally, on both the fresh and the
        // resumed path.
        let first = CodexDriver.turn_command(&a).unwrap();
        let flat: Vec<String> = first
            .get_args()
            .map(|x| x.to_string_lossy().to_string())
            .collect();
        assert!(flat
            .iter()
            .any(|x| x == "execute the next task\n\nstop at Pending review"));

        // Without the arm the turn is verbatim.
        let cmd = ClaudeDriver
            .turn_command(&args(&sb, true, "sid-1", &seatbelt))
            .unwrap();
        let flat: Vec<String> = cmd
            .get_args()
            .map(|x| x.to_string_lossy().to_string())
            .collect();
        let p = flat.iter().position(|x| x == "-p").unwrap();
        assert_eq!(flat[p + 1], "execute the next task");
        let _ = sb.remove();
    }
}
