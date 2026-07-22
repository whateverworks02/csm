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
mod ui;
mod workspace;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::Command;

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

    /// Rename a session and re-point its origin_pwd to the current directory.
    Rename { old: String, new: String },

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

fn main() {
    if let Err(e) = try_main() {
        ui::print_error(&e);
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
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
            ui::done("pinned", &name);
            Ok(())
        }
        Some(Cmd::Unpin { name }) => {
            store::set_pinned(&name, false)?;
            ui::done("unpinned", &name);
            Ok(())
        }
        Some(Cmd::Rm { name, force, yes }) => cmd_rm(&name, force, yes),
        Some(Cmd::Rename { old, new }) => cmd_rename(&old, &new),
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
    eprintln!(
        "{} {} {}",
        ui::epaint(ui::CYAN_BOLD, name),
        ui::epaint(ui::DIM, ui::ARROW),
        ui::epaint(ui::DIM, &ui::abbrev_path(&dir)),
    );

    // Opt-in: inject the csm prompt into this repo's AGENTS.md for cross-agent
    // (Cursor/Codex) support. Off by default to avoid touching tracked files.
    if agents_md {
        let p = inject::find_agents_md(&cwd).unwrap_or_else(|| cwd.join("AGENTS.md"));
        inject::inject_file(&p)?;
        ui::step("wrote", &format!("AGENTS.md {}", ui::abbrev_path(&p)));
    }

    if no_launch {
        // For other coding agents: the user exports CSM_SESSION themselves, or
        // points the agent at the workspace via `csm show`. Plain stdout so it
        // can be `eval`'d - never styled.
        println!("export CSM_SESSION={}", name);
        return Ok(());
    }

    // Launch Claude Code with CSM_SESSION env. The SessionStart hook (installed
    // via `csm init`) reads it and injects state.md. On /clear the env is still
    // present (same process), so the hook revives the workspace memory.
    eprintln!("{}", ui::epaint(ui::DIM, "launching claude..."));
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
        eprintln!(
            "{} {}",
            ui::epaint(ui::DIM, "no csm sessions for"),
            ui::epaint(ui::BOLD, &ui::abbrev_home(&cwd_str)),
        );
        ui::hint("start one with: csm <name>");
        return Ok(());
    }
    let Some(name) = pick_session(
        &format!("sessions for {}", ui::abbrev_home(&cwd_str)),
        rows,
    )? else {
        return Ok(());
    };
    cmd_start(&name, false, false)
}

/// Print a numbered list of sessions (most recently accessed first) and read a
/// 1-based selection from stdin. Returns the chosen name, or `None` if the user
/// aborted (empty/`q`) or entered an invalid index. `rows` must be non-empty;
/// callers handle the empty case with their own message. List and prompt go to
/// stderr so stdout stays clean for piping.
fn pick_session(
    label: &str,
    mut rows: Vec<(String, store::SessionMeta)>,
) -> Result<Option<String>> {
    rows.sort_by(|a, b| b.1.last_access.cmp(&a.1.last_access));
    eprintln!("{}:", ui::epaint(ui::BOLD, label));
    for (i, (name, m)) in rows.iter().enumerate() {
        let last = store::format_ts(&m.last_access);
        let pin = if m.pinned {
            format!(" {}", ui::epaint(ui::YELLOW, ui::PIN_MARK))
        } else {
            String::new()
        };
        eprintln!(
            "  {}  {}  {}  {}{}",
            ui::epaint(ui::DIM, &format!("{:>2}", i + 1)),
            ui::epaint(ui::CYAN_BOLD, &format!("{:<20}", name)),
            ui::epaint(ui::DIM, &format!("{:<16}", last)),
            ui::epaint(ui::DIM, &ui::abbrev_home(&m.origin_pwd)),
            pin,
        );
    }
    eprint!(
        "\n{} ",
        ui::epaint(ui::DIM, &format!("select a session (1-{}), 'q' to quit:", rows.len())),
    );
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim();
    if line.is_empty() || line.eq_ignore_ascii_case("q") {
        eprintln!("{}", ui::epaint(ui::DIM, "aborted"));
        return Ok(None);
    }
    match line.parse::<usize>() {
        Ok(i) if i >= 1 && i <= rows.len() => Ok(Some(rows[i - 1].0.clone())),
        _ => {
            eprintln!(
                "{} {}",
                ui::epaint(ui::RED_BOLD, "invalid selection:"),
                line,
            );
            Ok(None)
        }
    }
}

/// Picker over all sessions. Prints a hint and returns `None` if there are no
/// sessions or the user aborted.
fn pick_session_all() -> Result<Option<String>> {
    let idx = store::load_index()?;
    if idx.sessions.is_empty() {
        ui::no_sessions_hint();
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
        ui::no_sessions_hint();
        return Ok(());
    }
    let mut rows: Vec<_> = idx.sessions.iter().collect();
    rows.sort_by(|a, b| b.1.last_access.cmp(&a.1.last_access));
    println!(
        "{}  {}  {}  {}",
        ui::paint(ui::DIM, &format!("{:<20}", "NAME")),
        ui::paint(ui::DIM, &format!("{:<4}", "PIN")),
        ui::paint(ui::DIM, &format!("{:<19}", "LAST ACCESS")),
        ui::paint(ui::DIM, "ORIGIN"),
    );
    for (name, m) in rows {
        let last = store::format_ts(&m.last_access);
        let pin_field = if m.pinned { ui::PIN_MARK } else { "" };
        println!(
            "{}  {}  {}  {}",
            ui::paint(ui::CYAN_BOLD, &format!("{:<20}", name)),
            ui::paint(ui::YELLOW, &format!("{:<4}", pin_field)),
            ui::paint(ui::DIM, &format!("{:<19}", last)),
            ui::paint(ui::DIM, &ui::abbrev_home(&m.origin_pwd)),
        );
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
        let msg = format!(
            "delete session `{}` and its workspace at {}?",
            name,
            ui::abbrev_path(&dir),
        );
        if !gc::confirm(&msg)? {
            eprintln!("{}", ui::epaint(ui::DIM, "aborted"));
            return Ok(());
        }
    }
    store::delete_session(name)?;
    ui::done("deleted", name);
    Ok(())
}

/// `csm rename <old> <new>`: rename a session and re-point its `origin_pwd` to
/// the current directory, so bare `csm` lists it here. Does not launch claude.
/// `csm rename <name> <name>` is a pure re-home (rename to itself).
fn cmd_rename(old: &str, new: &str) -> Result<()> {
    let cwd = std::env::current_dir().context("getting current dir")?;
    let origin_pwd = cwd.display().to_string();
    store::rename_session(old, new, &origin_pwd)?;
    let dir = store::session_dir(new)?;
    if old == new {
        eprintln!(
            "{} {} {}",
            ui::epaint(ui::GREEN_BOLD, "re-homed"),
            ui::epaint(ui::CYAN_BOLD, new),
            ui::epaint(ui::DIM, &format!("to {}", ui::abbrev_home(&origin_pwd))),
        );
    } else {
        eprintln!(
            "{} {} {} {}",
            ui::epaint(ui::GREEN_BOLD, "renamed"),
            ui::epaint(ui::CYAN_BOLD, old),
            ui::epaint(ui::DIM, ui::ARROW),
            ui::epaint(ui::CYAN_BOLD, new),
        );
        eprintln!(
            "  {} {}",
            ui::epaint(ui::DIM, "re-homed to"),
            ui::epaint(ui::DIM, &ui::abbrev_home(&origin_pwd)),
        );
    }
    eprintln!(
        "  {}  {}",
        ui::epaint(ui::DIM, "workspace"),
        ui::epaint(ui::DIM, &ui::abbrev_path(&dir)),
    );
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
    println!("{}", ui::paint(ui::CYAN_BOLD, &name));
    println!(
        "  {} {}",
        ui::paint(ui::DIM, &format!("{:<11}", "workspace")),
        ui::paint(ui::DIM, &ui::abbrev_path(&dir)),
    );
    println!(
        "  {} {}",
        ui::paint(ui::DIM, &format!("{:<11}", "origin")),
        ui::paint(ui::DIM, &ui::abbrev_home(&meta.origin_pwd)),
    );
    println!(
        "  {} {}",
        ui::paint(ui::DIM, &format!("{:<11}", "created")),
        ui::paint(ui::DIM, &store::format_ts(&meta.created_at)),
    );
    println!(
        "  {} {}",
        ui::paint(ui::DIM, &format!("{:<11}", "last access")),
        ui::paint(ui::DIM, &store::format_ts(&meta.last_access)),
    );
    let pinned_str = if meta.pinned { "yes" } else { "no" };
    let pinned_styled = if meta.pinned {
        ui::paint(ui::YELLOW, pinned_str)
    } else {
        ui::paint(ui::DIM, pinned_str)
    };
    println!(
        "  {} {}",
        ui::paint(ui::DIM, &format!("{:<11}", "pinned")),
        pinned_styled,
    );
    println!();
    let state = workspace::read_state_md(&name)
        .unwrap_or_else(|| "(state.md not found)".to_string());
    println!("{}", ui::paint(ui::DIM, "--- state.md ---"));
    println!("{}", state);
    let scripts = workspace::list_scripts(&name);
    println!();
    println!("{}", ui::paint(ui::DIM, "--- scripts/ ---"));
    if scripts.is_empty() {
        println!("  {}", ui::paint(ui::DIM, "(none)"));
    } else {
        for s in scripts {
            println!("  {}", s);
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
        ui::step(
            "wrote",
            &format!("SessionStart hook to {}", ui::abbrev_path(&settings_path)),
        );
    } else {
        eprintln!(
            "{} {}",
            ui::epaint(ui::DIM, "SessionStart hook already present at"),
            ui::epaint(ui::DIM, &ui::abbrev_path(&settings_path)),
        );
    }

    // 2. Inject the csm working-mode prompt into the global CLAUDE.md.
    let claude_md = inject::claude_md_path()?;
    inject::inject_file(&claude_md)?;
    ui::step(
        "injected",
        &format!("prompt into {}", ui::abbrev_path(&claude_md)),
    );

    match which_csm() {
        Some(p) => ui::step(
            "found",
            &format!("csm on PATH at {}", ui::abbrev_path(&p)),
        ),
        None => ui::warn(
            "`csm` not on PATH; the hook command `csm hook` will fail. \
             install with `cargo install --path .` (ensure ~/.cargo/bin is on PATH).",
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
