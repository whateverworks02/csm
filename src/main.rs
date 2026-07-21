//! csm - workspace memory for coding agents.
//!
//! Three pillars:
//!   1. A kv index of sessions (`~/.csm/index.json`).
//!   2. A per-session workspace memory directory (`~/.csm/sessions/<name>/`).
//!   3. A carefully maintained working-mode prompt injected into the global
//!      `~/.claude/CLAUDE.md` (by `csm init`), plus a SessionStart hook that
//!      auto-injects the active session's `state.md`.
//!
//! Launching: `csm <name>` sets up / refreshes the session, then runs `claude`
//! with `CSM_SESSION=<name>`. On `/clear`, Claude Code fires SessionStart again
//! (source=clear); the hook reads `CSM_SESSION` (still set, same process) and
//! re-injects `state.md` - reviving the workspace memory.

mod gc;
mod hook;
mod inject;
mod prompt;
mod store;
mod workspace;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// Printed when a command finds no sessions (e.g. `csm list`, the `csm show` picker).
const NO_SESSIONS_HINT: &str = "no csm sessions. start one with: csm <name>";

#[derive(Parser)]
#[command(
    name = "csm",
    version,
    about = "Workspace memory for coding agents (cross-agent, cross-time, cross-repo)"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start (or resume) a csm session by name and launch Claude Code.
    Start {
        name: String,
        /// Set up the session but do not launch `claude` (for other agents).
        /// Prints `export CSM_SESSION=<name>`.
        #[arg(long)]
        no_launch: bool,
        /// Also inject the csm prompt into this repo's AGENTS.md (for
        /// cross-agent support with Cursor/Codex). Off by default to avoid
        /// modifying tracked repo files.
        #[arg(long)]
        agents_md: bool,
    },

    /// List all sessions.
    List,

    /// Pin a session so it is never garbage-collected.
    Pin { name: String },

    /// Unpin a session.
    Unpin { name: String },

    /// Hard-delete a session (workspace dir + index entry).
    Rm {
        name: String,
        /// Allow deleting a pinned session.
        #[arg(short = 'f', long)]
        force: bool,
        /// Skip confirmation.
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Show a session's workspace path and state.md.
    Show {
        /// Session name. Defaults to `$CSM_SESSION`, else opens a picker.
        name: Option<String>,
    },

    /// Garbage-collect unpinned sessions.
    Gc {
        /// Delete unpinned sessions not accessed in the last N days.
        #[arg(long, value_name = "N")]
        older_than: Option<u64>,
        /// Skip confirmation.
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Install the SessionStart hook into ~/.claude/settings.json and inject
    /// the csm working-mode prompt into ~/.claude/CLAUDE.md.
    Init,

    /// Internal: Claude Code SessionStart hook handler (reads stdin JSON).
    Hook,

    /// Catch-all: `csm <name>` is shorthand for `csm start <name>`.
    #[command(external_subcommand)]
    Other(Vec<String>),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Cmd::Start {
            name,
            no_launch,
            agents_md,
        }) => cmd_start(&name, no_launch, agents_md),
        Some(Cmd::Other(vec)) => {
            let name = vec.first().cloned().unwrap_or_default();
            if name.is_empty() {
                anyhow::bail!("missing session name");
            }
            let no_launch = vec.iter().any(|a| a == "--no-launch" || a == "-n");
            let agents_md = vec.iter().any(|a| a == "--agents-md");
            cmd_start(&name, no_launch, agents_md)
        }
        Some(Cmd::List) => cmd_list(),
        Some(Cmd::Pin { name }) => {
            store::set_pinned(&name, true)?;
            println!("pinned: {}", name);
            Ok(())
        }
        Some(Cmd::Unpin { name }) => {
            store::set_pinned(&name, false)?;
            println!("unpinned: {}", name);
            Ok(())
        }
        Some(Cmd::Rm { name, force, yes }) => cmd_rm(&name, force, yes),
        Some(Cmd::Show { name }) => cmd_show(name),
        Some(Cmd::Gc { older_than, yes }) => gc::run(older_than, yes),
        Some(Cmd::Init) => cmd_init(),
        Some(Cmd::Hook) => hook::run_hook(),
        None => cmd_pick_here(),
    }
}

fn cmd_start(name: &str, no_launch: bool, agents_md: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("getting current dir")?;
    let origin_pwd = cwd.display().to_string();

    let meta = store::touch_session(name, &origin_pwd)?;
    workspace::ensure_workspace(name, &meta)?;

    let dir = store::session_dir(name)?;
    eprintln!("csm: session `{}` -> {}", name, dir.display());

    // Opt-in: inject the csm prompt into this repo's AGENTS.md for cross-agent
    // (Cursor/Codex) support. Off by default to avoid touching tracked files.
    if agents_md {
        let p = inject::find_agents_md(&cwd).unwrap_or_else(|| cwd.join("AGENTS.md"));
        inject::inject_file(&p)?;
        eprintln!("csm: AGENTS.md: {}", p.display());
    }

    if no_launch {
        // For other coding agents: the user exports CSM_SESSION themselves, or
        // points the agent at the workspace via `csm show`.
        println!("export CSM_SESSION={}", name);
        return Ok(());
    }

    // Launch Claude Code with CSM_SESSION env. The SessionStart hook (installed
    // via `csm init`) reads it and injects state.md. On /clear the env is still
    // present (same process), so the hook revives the workspace memory.
    let status = Command::new("claude")
        .env("CSM_SESSION", name)
        .status()
        .context("failed to launch `claude` (is Claude Code installed and on PATH?)")?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Bare `csm` (no subcommand): list sessions whose `origin_pwd` is the current
/// directory and let the user pick one to start. Prints a hint and exits if
/// none match.
fn cmd_pick_here() -> Result<()> {
    let cwd = std::env::current_dir().context("getting current dir")?;
    let cwd_str = cwd.display().to_string();
    let idx = store::load_index()?;
    let rows: Vec<(String, store::SessionMeta)> = idx
        .sessions
        .iter()
        .filter(|(_, m)| m.origin_pwd == cwd_str)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if rows.is_empty() {
        println!("no csm sessions for {}.", cwd_str);
        println!("start one with: csm <name>");
        return Ok(());
    }
    let Some(name) = pick_session(&format!("sessions for {}", cwd_str), rows)? else {
        return Ok(());
    };
    cmd_start(&name, false, false)
}

/// Print a numbered list of sessions (most recently accessed first) and read a
/// 1-based selection from stdin. Returns the chosen name, or `None` if the user
/// aborted (empty/`q`) or entered an invalid index. `rows` must be non-empty;
/// callers handle the empty case with their own message.
fn pick_session(
    label: &str,
    mut rows: Vec<(String, store::SessionMeta)>,
) -> Result<Option<String>> {
    rows.sort_by(|a, b| b.1.last_access.cmp(&a.1.last_access));
    println!("{}:", label);
    for (i, (name, m)) in rows.iter().enumerate() {
        let last = store::format_last_access(&m.last_access);
        let pin = if m.pinned { "*" } else { "" };
        println!("  [{}] {:<20} {:<2} {:<16} {}", i + 1, name, pin, last, m.origin_pwd);
    }
    print!("\nselect a session (1-{}), 'q' to quit: ", rows.len());
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim();
    if line.is_empty() || line.eq_ignore_ascii_case("q") {
        println!("aborted");
        return Ok(None);
    }
    match line.parse::<usize>() {
        Ok(i) if i >= 1 && i <= rows.len() => Ok(Some(rows[i - 1].0.clone())),
        _ => {
            eprintln!("invalid selection: {}", line);
            Ok(None)
        }
    }
}

/// Picker over all sessions. Prints a hint and returns `None` if there are no
/// sessions or the user aborted.
fn pick_session_all() -> Result<Option<String>> {
    let idx = store::load_index()?;
    if idx.sessions.is_empty() {
        println!("{NO_SESSIONS_HINT}");
        return Ok(None);
    }
    let rows: Vec<(String, store::SessionMeta)> = idx
        .sessions
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    pick_session("all sessions", rows)
}

fn cmd_list() -> Result<()> {
    let idx = store::load_index()?;
    if idx.sessions.is_empty() {
        println!("{NO_SESSIONS_HINT}");
        return Ok(());
    }
    let mut rows: Vec<_> = idx.sessions.iter().collect();
    rows.sort_by(|a, b| b.1.last_access.cmp(&a.1.last_access));
    println!(
        "{:<20} {:<4} {:<20} ORIGIN",
        "NAME", "PIN", "LAST ACCESS"
    );
    for (name, m) in rows {
        let last = store::format_last_access(&m.last_access);
        let pin = if m.pinned { "*" } else { "" };
        println!("{:<20} {:<4} {:<20} {}", name, pin, last, m.origin_pwd);
    }
    Ok(())
}

fn cmd_rm(name: &str, force: bool, yes: bool) -> Result<()> {
    let idx = store::load_index()?;
    let meta = idx
        .sessions
        .get(name)
        .with_context(|| format!("no csm session named {:?}", name))?;
    if meta.pinned && !force {
        anyhow::bail!("session `{}` is pinned; pass --force to delete anyway", name);
    }
    if !yes {
        let dir = store::session_dir(name)?;
        if !gc::confirm(&format!(
            "delete session `{}` and its workspace at {}?",
            name,
            dir.display()
        ))? {
            println!("aborted");
            return Ok(());
        }
    }
    store::delete_session(name)?;
    println!("deleted: {}", name);
    Ok(())
}

fn cmd_show(name: Option<String>) -> Result<()> {
    let name = match name {
        Some(n) => n,
        None => match std::env::var("CSM_SESSION") {
            Ok(n) if !n.is_empty() => n,
            _ => {
                let Some(n) = pick_session_all()? else { return Ok(()) };
                n
            }
        },
    };
    let idx = store::load_index()?;
    let meta = idx
        .sessions
        .get(&name)
        .with_context(|| format!("no csm session named {:?}", name))?;
    let dir = store::session_dir(&name)?;
    println!("session: {}", name);
    println!("workspace: {}", dir.display());
    println!("origin: {}", meta.origin_pwd);
    println!("created: {}", meta.created_at);
    println!("last access: {}", meta.last_access);
    println!("pinned: {}", meta.pinned);
    println!();
    let state = workspace::read_state_md(&name)
        .unwrap_or_else(|| "(state.md not found)".to_string());
    println!("--- state.md ---");
    println!("{}", state);
    let scripts = workspace::list_scripts(&name);
    println!("--- scripts/ ---");
    if scripts.is_empty() {
        println!("(none)");
    } else {
        for s in scripts {
            println!("{}", s);
        }
    }
    Ok(())
}

fn cmd_init() -> Result<()> {
    let claude_dir = inject::claude_dir()?;
    std::fs::create_dir_all(&claude_dir)?;
    let settings_path = claude_dir.join("settings.json");

    // 1. Install the SessionStart hook (idempotent).
    let mut root: serde_json::Value = if settings_path.exists() {
        let data = std::fs::read_to_string(&settings_path)
            .with_context(|| format!("reading {}", settings_path.display()))?;
        if data.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&data)
                .with_context(|| format!("parsing {}", settings_path.display()))?
        }
    } else {
        serde_json::json!({})
    };
    if ensure_sessionstart_hook(&mut root) {
        std::fs::write(&settings_path, serde_json::to_string_pretty(&root)?)?;
        println!("csm: wrote SessionStart hook to {}", settings_path.display());
    } else {
        println!(
            "csm: SessionStart hook already present in {}",
            settings_path.display()
        );
    }

    // 2. Inject the csm working-mode prompt into the global CLAUDE.md.
    let claude_md = inject::claude_md_path()?;
    inject::inject_file(&claude_md)?;
    println!("csm: injected prompt into {}", claude_md.display());

    match which_csm() {
        Some(p) => println!("csm: found on PATH at {}", p.display()),
        None => eprintln!(
            "csm: warning: `csm` not found on PATH; the hook command `csm hook` will fail.\n\
             install with `cargo install --path .` (ensure ~/.cargo/bin is on PATH)."
        ),
    }
    Ok(())
}

/// Add a SessionStart hook (`csm hook`) to the settings if not already present.
/// Returns true if the settings were modified.
fn ensure_sessionstart_hook(root: &mut serde_json::Value) -> bool {
    const CMD: &str = "csm hook";

    let already = root
        .get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .and_then(|s| s.as_array())
        .map(|groups| {
            groups.iter().any(|g| {
                g.get("matcher").and_then(|m| m.as_str()) == Some("")
                    && g
                        .get("hooks")
                        .and_then(|h| h.as_array())
                        .map(|hs| {
                            hs.iter().any(|h| {
                                h.get("type").and_then(|t| t.as_str()) == Some("command")
                                    && h.get("command").and_then(|c| c.as_str()) == Some(CMD)
                            })
                        })
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if already {
        return false;
    }

    if root.get("hooks").is_none() {
        root["hooks"] = serde_json::json!({});
    }
    if !root["hooks"]["SessionStart"].is_array() {
        root["hooks"]["SessionStart"] = serde_json::json!([]);
    }
    let arr = root["hooks"]["SessionStart"]
        .as_array_mut()
        .expect("SessionStart is an array");
    arr.push(serde_json::json!({
        "matcher": "",
        "hooks": [{ "type": "command", "command": CMD }]
    }));
    true
}

fn which_csm() -> Option<PathBuf> {
    Command::new("which")
        .arg("csm")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .map(PathBuf::from)
}
