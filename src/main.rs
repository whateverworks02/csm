//! csm - workspace memory for coding agents.
//!
//! Three pillars:
//!   1. A kv index of sessions (`~/.csm/index.json`).
//!   2. A per-session workspace memory directory (`~/.csm/sessions/<name>/`).
//!   3. A carefully maintained working-mode prompt injected into the global
//!      `~/.claude/CLAUDE.md` (by `csm init`), plus a SessionStart hook that
//!      auto-injects the active session's `state.md`.
//!
//! Launching: `csm <name>` sets up / refreshes the session, then launches the
//! agent via a per-agent adapter (`--agent`, default `claude`). The Claude
//! adapter runs `claude` with `CSM_SESSION=<name>`; on `/clear`, Claude Code
//! fires SessionStart again (source=clear), the hook reads `CSM_SESSION` (still
//! set, same process) and re-injects `state.md` - reviving the workspace memory.
//! Other agents (e.g. `pi`) inject at launch instead; see `agent.rs`.

mod agent;
mod doctor;
mod gc;
mod hook;
mod inject;
mod markdown;
mod prompt;
mod skills;
mod store;
#[cfg(test)]
mod test_support;
mod ui;
mod workspace;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::Write;

#[derive(Parser)]
#[command(
    name = "csm",
    version,
    about = "Workspace memory for coding agents (cross-time, cross-repo)",
    after_help = "Run `csm <name>` to start a session and launch Claude Code."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List all sessions.
    List,

    /// Pin a session so it is never garbage-collected.
    Pin { name: String },

    /// Unpin a session.
    Unpin { name: String },

    /// Hard-delete a session (workspace dir + index entry).
    #[command(aliases = ["remove", "delete", "del"])]
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

    /// Show a session as a compact recognition card.
    Show {
        /// Session name. Defaults to `$CSM_SESSION`, else opens a picker.
        name: Option<String>,
    },

    /// Render a session's full state.md, read-only (the deep read).
    Detail {
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

    /// Diagnose session/index consistency and agent wiring; `--fix` repairs.
    Doctor {
        /// Repair fixable issues (scaffold ghosts, fill incomplete). Confirms
        /// each finding unless `--yes`.
        #[arg(long)]
        fix: bool,
        /// With `--fix`, skip confirmations (non-interactive / CI; requires `--fix`).
        #[arg(short = 'y', long, requires = "fix")]
        yes: bool,
    },

    /// Install agent wiring: SessionStart hooks (~/.claude/settings.json,
    /// ~/.codex/hooks.json), the csm working-mode prompt (~/.claude/CLAUDE.md,
    /// ~/.pi/agent/CLAUDE.md, ~/.codex/AGENTS.md), and the csm-plan skill
    /// (~/.claude/skills/csm-plan/SKILL.md + ~/.csm/skills/plan.md).
    Init,

    /// Print the version.
    #[command(hide = true)]
    Version,

    /// Internal: Claude Code SessionStart hook handler (reads `$CSM_SESSION`,
    /// emits state context JSON on stdout).
    #[command(hide = true)]
    Hook,

    /// `csm <name>`: start (or resume) a session by name and launch Claude Code.
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
        Some(Cmd::Other(vec)) => {
            let (name, agent) = parse_start_args(vec)?;
            cmd_start(&name, &agent)
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
        Some(Cmd::Detail { name }) => cmd_detail(name),
        Some(Cmd::Gc { older_than, yes }) => gc::run(older_than, yes),
        Some(Cmd::Doctor { fix, yes }) => doctor::run(fix, yes),
        Some(Cmd::Init) => cmd_init(),
        Some(Cmd::Version) => {
            println!("csm {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(Cmd::Hook) => hook::run_hook(),
        None => cmd_pick_here(),
    }
}

fn cmd_start(name: &str, agent: &str) -> Result<()> {
    // Resolve the agent adapter first so an unknown agent id errors before we
    // create or touch any session.
    let agent_adapter = agent::agent_for(agent)?;

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

    // Launch the agent via its adapter. Each adapter decides binary, args,
    // env, and how/when state is injected (Claude: env + SessionStart hook;
    // pi: --append-system-prompt at launch).
    eprintln!(
        "{}",
        ui::epaint(ui::DIM, &format!("launching {}...", agent_adapter.id())),
    );
    let status = agent_adapter.launch(name).status().context(format!(
        "failed to launch {} (is it installed and on PATH?)",
        agent_adapter.id()
    ))?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Parse `csm <name> [--agent <x>|-a <x>|--agent=<x>]` from the
/// external-subcommand arg vec. `agent` defaults to "claude".
fn parse_start_args(args: Vec<String>) -> Result<(String, String)> {
    let mut name = String::new();
    let mut agent = String::from("claude");
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(v) = a.strip_prefix("--agent=") {
            agent = v.to_string();
        } else if a == "--agent" || a == "-a" {
            i += 1;
            agent = args
                .get(i)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("--agent requires a value"))?;
        } else if name.is_empty() {
            name = a.clone();
        } else {
            anyhow::bail!("unexpected argument: {a:?} (usage: csm <name> [--agent <x>])");
        }
        i += 1;
    }
    if name.is_empty() {
        anyhow::bail!("missing session name");
    }
    Ok((name, agent))
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
    let Some(name) = pick_session(&format!("sessions for {}", ui::abbrev_home(&cwd_str)), rows)?
    else {
        return Ok(());
    };
    cmd_start(&name, "claude")
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
        ui::epaint(
            ui::DIM,
            &format!("select a session (1-{}), 'q' to quit:", rows.len())
        ),
    );
    std::io::stderr().flush()?;
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

/// Resolve a session name from an optional arg: an explicit name wins;
/// otherwise `$CSM_SESSION`; otherwise the interactive picker. Returns
/// `Ok(None)` only when the picker was offered and the user aborted (the
/// caller should then return cleanly). Does not validate the name exists -
/// callers validate when they look up session data.
fn resolve_session_name(name: Option<String>) -> Result<Option<String>> {
    match name {
        Some(n) => Ok(Some(n)),
        None => match std::env::var("CSM_SESSION") {
            Ok(n) if !n.is_empty() => Ok(Some(n)),
            _ => Ok(pick_session_all()?),
        },
    }
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
    let meta = store::require_session(name)?;
    if meta.pinned && !force {
        anyhow::bail!(
            "session `{}` is pinned; pass --force to delete anyway",
            name
        );
    }
    if !yes {
        let dir = store::session_dir(name)?;
        let msg = format!(
            "delete session `{}` and its workspace at {}?",
            name,
            ui::abbrev_path(&dir),
        );
        if !ui::confirm(&msg)? {
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

/// Width of the dim label column in the `csm show` card.
const CARD_LABEL_WIDTH: usize = 11;

/// One `csm show` card row: `  <dim label padded to CARD_LABEL_WIDTH> <value>`.
/// `value` is already styled by the caller.
fn card_row(label: &str, value: &str) {
    println!(
        "  {} {}",
        ui::paint(
            ui::DIM,
            &format!("{:<width$}", label, width = CARD_LABEL_WIDTH),
        ),
        value,
    );
}

/// Render a card row with one or more value lines: the first line carries the
/// label, continuation lines indent under the value column. "(none)" when empty.
/// Shared by the `context` row (truncated state.md lines) and the `open`/`done`
/// task-entry rows so the label/continuation layout can't drift between them.
fn card_multiline_row(label: &str, lines: &[String]) {
    let cont = " ".repeat(2 + CARD_LABEL_WIDTH + 1);
    if lines.is_empty() {
        card_row(label, &ui::paint(ui::DIM, "(none)"));
        return;
    }
    for (i, val) in lines.iter().enumerate() {
        if i == 0 {
            card_row(label, val);
        } else {
            println!("{}{}", cont, val);
        }
    }
}

fn cmd_show(name: Option<String>) -> Result<()> {
    let Some(name) = resolve_session_name(name)? else {
        return Ok(());
    };
    let meta = store::require_session(&name)?;

    // Card: name, then one row per field. Labels dim + fixed-width so values
    // align; metadata recedes (dim), context/open/done carry the recognition
    // signal (normal weight). No icons, no raw markdown - a glance should suffice.
    println!("{}", ui::paint(ui::CYAN_BOLD, &name));
    card_row(
        "origin",
        &ui::paint(ui::DIM, &ui::abbrev_home(&meta.origin_pwd)),
    );
    card_row(
        "last access",
        &ui::paint(ui::DIM, &store::format_ts(&meta.last_access)),
    );
    let pinned_str = if meta.pinned { "yes" } else { "no" };
    let pinned_styled = if meta.pinned {
        ui::paint(ui::YELLOW, pinned_str)
    } else {
        ui::paint(ui::DIM, pinned_str)
    };
    card_row("pinned", &pinned_styled);

    // context - first paragraph of the Context section; the recognizer. (Falls
    // back to a legacy `## Task` section for pre-tasks-model sessions.)
    let context_lines: Vec<String> = workspace::read_context_lines(&name, 5)
        .into_iter()
        .map(|l| markdown::truncate(&l, 100))
        .collect();
    card_multiline_row("context", &context_lines);

    // tasks - the board is the operational center. The card lists Open (work to
    // claim) and Done (accomplished) entries; Pending review/fix live on the
    // full board (`csm detail`) - the card is a glance, not the whole board.
    let board = workspace::read_tasks_board(&name).unwrap_or_default();
    card_entries_row("open", &board.open);
    card_entries_row("done", &board.done);

    let scripts = workspace::list_scripts(&name);
    card_list_row("scripts", &scripts);

    let notes = workspace::list_notes(&name);
    card_list_row("notes", &notes);
    Ok(())
}

/// Render a card row listing task entries (id + slug) for one board status.
/// Up to `CAP` entries show; a dim `... +N more` line follows when there are
/// more. The entry text is the board line after `- ` (e.g. `001 fix-cookie -
/// wire SameSite`); we show the `id slug` prefix (up to the first ` - `) and
/// drop the gist for a compact, recognizable row. Layout (label, continuation
/// indent, "(none)") is shared via [`card_multiline_row`].
fn card_entries_row(label: &str, entries: &[String]) {
    const CAP: usize = 3;
    let mut lines: Vec<String> = entries
        .iter()
        .take(CAP)
        .map(|raw| {
            raw.trim()
                .split_once(" - ")
                .map(|(head, _)| head.trim())
                .unwrap_or_else(|| raw.trim())
                .to_string()
        })
        .collect();
    if entries.len() > CAP {
        lines.push(ui::paint(ui::DIM, &format!("... +{} more", entries.len() - CAP)).to_string());
    }
    card_multiline_row(label, &lines);
}

/// Render a card row for a subdirectory listing: comma-joined names with a
/// count, or dim "(none)" when empty.
fn card_list_row(label: &str, items: &[String]) {
    let val = if items.is_empty() {
        ui::paint(ui::DIM, "(none)").to_string()
    } else {
        format!("{} ({})", items.join(", "), items.len())
    };
    card_row(label, &val);
}

/// `csm detail [name]`: render a session's full `state.md` read-only - the deep
/// read to complement `csm show`'s recognition card. Name resolution mirrors
/// `csm show` (explicit arg > `$CSM_SESSION` > picker), shared via
/// `resolve_session_name`. Each `## Section` becomes a bold header + an
/// inline-stripped body (cargo aesthetic: color, not icons; no raw markdown).
fn cmd_detail(name: Option<String>) -> Result<()> {
    let Some(name) = resolve_session_name(name)? else {
        return Ok(());
    };
    let meta = store::require_session(&name)?;

    // Header: name + last-access for context. (origin/pinned live in `show`.)
    println!(
        "{}  {}",
        ui::paint(ui::CYAN_BOLD, &name),
        ui::paint(ui::DIM, &store::format_ts(&meta.last_access)),
    );

    let content = match workspace::read_state_md(&name) {
        Some(c) => c,
        None => {
            eprintln!(
                "{}",
                ui::epaint(ui::DIM, &format!("no state.md for session `{name}`")),
            );
            return Ok(());
        }
    };

    print_sections(markdown::sections(&content));

    // The task board - the operational center. The deep read includes it so an
    // orienting coordinator/worker sees the full status board, not just the
    // state one-pager. Empty sections render as `(none)`, consistent with
    // state.md - that's the healthy-state signal, not noise.
    if let Some(board_content) = workspace::read_tasks_index_md(&name) {
        println!("{}", ui::paint(ui::BOLD, "tasks"));
        println!();
        print_sections(markdown::sections(&board_content));
    }
    Ok(())
}

/// Render `## Section`s as a bold header + body (cargo aesthetic), each
/// followed by a blank line; an empty body shows a dim `(none)`. Shared by
/// `csm detail` for state.md and the tasks/INDEX.md board so the two render
/// identically.
fn print_sections(sections: Vec<markdown::Section>) {
    for section in sections {
        println!("{}", ui::paint(ui::BOLD, &section.title));
        if section.body.is_empty() {
            println!("  {}", ui::paint(ui::DIM, "(none)"));
        } else {
            for line in &section.body {
                println!("{line}");
            }
        }
        println!();
    }
}

fn cmd_init() -> Result<()> {
    // Install every known agent's global state-injection wiring (Claude's
    // SessionStart hook + CLAUDE.md, pi's CLAUDE.md). Idempotent.
    agent::install_all()?;
    match inject::which_csm() {
        Some(p) => ui::step("found", &format!("csm on PATH at {}", ui::abbrev_path(&p))),
        None => ui::warn(
            "`csm` not on PATH; the hook command `csm hook` will fail. \
             install with `cargo install --path .` (ensure ~/.cargo/bin is on PATH).",
        ),
    }
    Ok(())
}
