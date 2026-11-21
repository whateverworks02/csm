//! Command-line entry point for csm.

use csm::{agent, doctor, gc, hook, inject, markdown, needs, store, ui, workspace};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::collections::{HashMap, HashSet};
use std::io::Write;

#[derive(Parser)]
#[command(
    name = "csm",
    version,
    about = "Workspace memory for coding agents (cross-time, cross-repo)",
    after_help = "Run `csm <name>` to start a session; bare `csm` (or `csm -a <agent>`) picks a local or cross-repo session.\n\nIf the name matches a subcommand, put `--` before the name. For example, `csm -- sprint` launches the session named `sprint`. To choose an agent explicitly, use `csm -a codex -- sprint`."
)]
struct Cli {
    /// Agent to launch (claude, pi, codex, opencode; default claude). May lead
    /// (`csm -a pi <name>`) or trail (`csm <name> -a pi`); without a session
    /// name, `csm -a <agent>` opens the picker for this directory.
    #[arg(short = 'a', long = "agent", value_name = "AGENT", global = true)]
    agent: Option<String>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List active sessions (`--archived`: the archived ones).
    List {
        /// List archived sessions instead of active ones.
        #[arg(long)]
        archived: bool,
    },

    /// Show the active sessions selected for the current sprint.
    Sprint {
        #[command(subcommand)]
        action: Option<SprintAction>,
    },

    /// Pin a session so it is never garbage-collected.
    Pin { name: String },

    /// Unpin a session.
    Unpin { name: String },

    /// Archive a session: retained on disk, out of the daily surfaces and gc.
    Archive { name: String },

    /// Unarchive a session, returning it to the active surfaces.
    Unarchive { name: String },

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

    /// Garbage-collect unpinned sessions; sessions whose board still holds
    /// unfinished tasks are kept.
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
    /// ~/.codex/hooks.json) and the csm working-mode prompt (~/.claude/CLAUDE.md,
    /// ~/.pi/agent/CLAUDE.md, ~/.codex/AGENTS.md, ~/.config/opencode/AGENTS.md).
    Init,

    /// Print the version.
    #[command(hide = true)]
    Version,

    /// Internal: Claude Code SessionStart hook handler (reads `$CSM_SESSION`,
    /// emits state context JSON on stdout).
    #[command(hide = true)]
    Hook,

    /// `csm [<name>] [--agent <x>]`: start (or resume) a session by name and
    /// launch the agent; without a name, fall back to the cwd picker.
    #[command(external_subcommand)]
    Other(Vec<String>),
}

#[derive(Subcommand)]
enum SprintAction {
    /// Add an active session to the sprint.
    Add {
        /// Session name. Omit to pick an active session outside the sprint.
        name: Option<String>,
    },
    /// Remove a session from the sprint.
    Remove {
        /// Session name. Omit to pick an active session in the sprint.
        name: Option<String>,
    },
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
            let agent = resolve_agent(cli.agent, agent)?;
            match name {
                Some(name) => cmd_start(&name, &agent),
                None => cmd_pick_here(&agent),
            }
        }
        Some(Cmd::List { archived }) => cmd_list(archived),
        Some(Cmd::Sprint { action }) => cmd_sprint(action),
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
        Some(Cmd::Archive { name }) => cmd_set_archived(&name, true),
        Some(Cmd::Unarchive { name }) => cmd_set_archived(&name, false),
        Some(Cmd::Rm { name, force, yes }) => cmd_rm(&name, force, yes),
        Some(Cmd::Rename { old, new }) => cmd_rename(&old, &new),
        Some(Cmd::Show { name }) => cmd_show(name),
        Some(Cmd::Gc { older_than, yes }) => gc::run(older_than, yes),
        Some(Cmd::Doctor { fix, yes }) => doctor::run(fix, yes),
        Some(Cmd::Init) => cmd_init(),
        Some(Cmd::Version) => {
            println!("csm {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(Cmd::Hook) => hook::run_hook(),
        None => cmd_pick_here(&resolve_agent(cli.agent, None)?),
    }
}

fn cmd_set_archived(name: &str, archived: bool) -> Result<()> {
    let (verb, state) = if archived {
        ("archived", "already archived")
    } else {
        ("unarchived", "already active")
    };
    if store::set_archived(name, archived)? {
        ui::done(verb, name);
    } else {
        eprintln!("{}", ui::epaint(ui::DIM, &format!("{state}: {name}")));
    }
    Ok(())
}

fn cmd_start(name: &str, agent: &str) -> Result<()> {
    // Validate the agent before creating or touching the session.
    let agent_adapter = agent::agent_for(agent)?;

    let cwd = std::env::current_dir().context("getting current dir")?;
    let origin_pwd = cwd.display().to_string();

    let (meta, revived) = store::touch_session_reviving(name, &origin_pwd)?;
    if revived {
        ui::done("unarchived", name);
    }
    workspace::ensure_workspace(name, &meta)?;

    wait_for_agent(spawn_agent(name, agent_adapter.as_ref())?)
}

fn launch_picked_session(
    name: &str,
    meta: &store::SessionMeta,
    agent: &str,
    register_dir: Option<&str>,
) -> Result<()> {
    let agent_adapter = agent::agent_for(agent)?;
    let child = store::start_picked_session(name, &meta.created_at, register_dir, || {
        spawn_agent(name, agent_adapter.as_ref())
    })?;
    wait_for_agent(child)
}

fn spawn_agent(name: &str, agent_adapter: &dyn agent::Agent) -> Result<std::process::Child> {
    let dir = store::session_dir(name)?;
    eprintln!(
        "{} {} {}",
        ui::epaint(ui::CYAN_BOLD, name),
        ui::epaint(ui::DIM, ui::ARROW),
        ui::epaint(ui::DIM, &ui::abbrev_path(&dir)),
    );

    eprintln!(
        "{}",
        ui::epaint(ui::DIM, &format!("launching {}...", agent_adapter.id())),
    );
    agent_adapter.launch(name).spawn().context(format!(
        "failed to launch {} (is it installed and on PATH?)",
        agent_adapter.id()
    ))
}

fn wait_for_agent(mut child: std::process::Child) -> Result<()> {
    let status = child.wait().context("waiting for agent to exit")?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Parse trailing launch arguments captured by clap's external subcommand.
fn parse_start_args(args: Vec<String>) -> Result<(Option<String>, Option<String>)> {
    let mut name = None;
    let mut agent = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--agent=") {
            agent = merge_agent(agent, value.to_string())?;
        } else if arg == "--agent" || arg == "-a" {
            let value = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--agent requires a value"))?;
            agent = merge_agent(agent, value)?;
        } else if name.is_none() {
            if arg.is_empty() {
                anyhow::bail!("missing session name");
            }
            name = Some(arg);
        } else {
            anyhow::bail!("unexpected argument: {arg:?} (usage: csm <name> [--agent <x>])");
        }
    }
    Ok((name, agent))
}

fn merge_agent(agent: Option<String>, v: String) -> Result<Option<String>> {
    if let Some(prev) = &agent {
        if prev != &v {
            anyhow::bail!("conflicting --agent values: {prev:?} and {v:?}");
        }
    }
    Ok(Some(v))
}

/// Validate both agent IDs before checking for conflicts between flag positions.
fn resolve_agent(root: Option<String>, parsed: Option<String>) -> Result<String> {
    if let Some(a) = &root {
        agent::agent_for(a)?;
    }
    if let Some(a) = &parsed {
        agent::agent_for(a)?;
    }
    match (root, parsed) {
        (Some(a), Some(b)) if a != b => {
            anyhow::bail!("conflicting --agent values: {a:?} and {b:?}")
        }
        (Some(a), _) => Ok(a),
        (None, Some(b)) => Ok(b),
        (None, None) => Ok(String::from("claude")),
    }
}

fn cmd_pick_here(agent: &str) -> Result<()> {
    let cwd = std::env::current_dir().context("getting current dir")?;
    let cwd_str = cwd.display().to_string();
    let display_dir = ui::abbrev_home(&cwd_str);
    let idx = store::load_index()?;
    let (local, other): (Vec<_>, Vec<_>) = list_rows(&idx, false)
        .into_iter()
        .partition(|(_, m)| m.is_registered_in(&cwd_str));
    loop {
        print_session_choices(&format!("sessions for {display_dir}"), &local);
        if local.is_empty() {
            eprintln!("  no active sessions in this directory");
            let start = if agent == "claude" {
                "csm <name>".to_string()
            } else {
                format!("csm <name> -a {agent}")
            };
            ui::hint(&format!("start one with: {start}"));
            if idx
                .sessions
                .values()
                .any(|m| m.is_archived() && m.is_registered_in(&cwd_str))
            {
                ui::hint("csm list --archived");
            }
        }
        eprintln!("  o  choose from other repositories");
        match pick_launch_session(&local, "o", "other repositories")? {
            LaunchChoice::Session(name, meta) => {
                return launch_picked_session(name, meta, agent, None)
            }
            LaunchChoice::Quit => return Ok(()),
            LaunchChoice::Navigate => {}
        }

        loop {
            print_session_choices("sessions from other repositories", &other);
            if other.is_empty() {
                eprintln!("  no active sessions in other repositories");
            }
            let (name, meta) = match pick_launch_session(&other, "b", "back")? {
                LaunchChoice::Session(name, meta) => (name, meta),
                LaunchChoice::Navigate => break,
                LaunchChoice::Quit => return Ok(()),
            };

            eprintln!("Selected {name}. {agent} will launch in {display_dir}.");
            eprintln!("You can register this directory to pick {name} here next time.");
            let options = "'l' launch here, 'r' register and launch here, 'b' back, 'q' quit";
            loop {
                match read_picker_input(options)?.to_ascii_lowercase().as_str() {
                    "l" => return launch_picked_session(name, meta, agent, None),
                    "r" => {
                        return launch_picked_session(name, meta, agent, Some(&cwd_str));
                    }
                    "b" => break,
                    "q" => return Ok(()),
                    _ => eprintln!("Invalid selection. Choose {options}."),
                }
            }
        }
    }
}

enum LaunchChoice<'a> {
    Session(&'a str, &'a store::SessionMeta),
    Navigate,
    Quit,
}

fn pick_launch_session<'a>(
    rows: &[(&'a String, &'a store::SessionMeta)],
    nav_key: &str,
    nav_label: &str,
) -> Result<LaunchChoice<'a>> {
    let actions = format!("'{nav_key}' {nav_label}, 'q' quit");
    let options = if rows.is_empty() {
        actions
    } else {
        format!("1-{} select a session, {actions}", rows.len())
    };
    loop {
        let input = read_picker_input(&options)?;
        if input.eq_ignore_ascii_case("q") {
            return Ok(LaunchChoice::Quit);
        }
        if input.eq_ignore_ascii_case(nav_key) {
            return Ok(LaunchChoice::Navigate);
        }
        if let Ok(i) = input.parse::<usize>() {
            if (1..=rows.len()).contains(&i) {
                let (name, meta) = rows[i - 1];
                return Ok(LaunchChoice::Session(name, meta));
            }
        }
        eprintln!("Invalid selection. Choose {options}.");
    }
}

fn read_picker_input(options: &str) -> Result<String> {
    eprint!("\n{} ", ui::epaint(ui::DIM, &format!("{options}:")));
    std::io::stderr().flush()?;
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok("q".into());
    }
    Ok(line.trim().to_string())
}

fn print_session_choices(label: &str, rows: &[(&String, &store::SessionMeta)]) {
    eprintln!("{}:", ui::epaint(ui::BOLD, label));
    for (i, (name, m)) in rows.iter().enumerate() {
        let pin = if m.pinned {
            format!(" {}", ui::epaint(ui::YELLOW, ui::PIN_MARK))
        } else {
            String::new()
        };
        eprintln!(
            "{}",
            ui::session_row(
                Some(i + 1),
                name,
                &store::format_ts(&m.last_access),
                &m.origin_pwd,
                &pin
            )
        );
    }
}

fn pick_session(label: &str, rows: &[(&String, &store::SessionMeta)]) -> Result<Option<String>> {
    print_session_choices(label, rows);
    let numbers = if rows.len() == 1 {
        "1".to_string()
    } else {
        format!("1-{}", rows.len())
    };
    let options = format!("Enter {numbers} or q to cancel");
    let line = read_picker_input(&options)?;
    if line.is_empty() || line.eq_ignore_ascii_case("q") {
        eprintln!("{}", ui::epaint(ui::DIM, "Cancelled."));
        return Ok(None);
    }
    match line.parse::<usize>() {
        Ok(i) if i >= 1 && i <= rows.len() => Ok(Some(rows[i - 1].0.clone())),
        _ => {
            eprintln!(
                "{} {options}.",
                ui::epaint(ui::RED_BOLD, "Invalid selection."),
            );
            Ok(None)
        }
    }
}

fn pick_session_to_show() -> Result<Option<String>> {
    let idx = store::load_index()?;
    let rows = list_rows(&idx, false);
    if rows.is_empty() {
        eprintln!("No active sessions to show.");
        if idx.sessions.is_empty() {
            ui::hint("Start a session with `csm <name>`.");
        } else {
            ui::hint("List archived sessions with `csm list --archived`.");
        }
        return Ok(None);
    }
    pick_session("Select a session to show", &rows)
}

/// Resolve an explicit name, then `$CSM_SESSION`, then the interactive picker.
fn resolve_session_name(name: Option<String>) -> Result<Option<String>> {
    match name {
        Some(n) => Ok(Some(n)),
        None => match std::env::var("CSM_SESSION") {
            Ok(n) if !n.is_empty() => Ok(Some(n)),
            _ => pick_session_to_show(),
        },
    }
}

fn shown_stamp(meta: &store::SessionMeta, archived: bool) -> &str {
    if archived {
        meta.archived_at.as_deref().unwrap_or("")
    } else {
        &meta.last_access
    }
}

fn list_rows(idx: &store::Index, archived: bool) -> Vec<(&String, &store::SessionMeta)> {
    let mut rows: Vec<_> = idx
        .sessions
        .iter()
        .filter(|(_, m)| m.is_archived() == archived)
        .collect();
    rows.sort_by(|a, b| shown_stamp(b.1, archived).cmp(shown_stamp(a.1, archived)));
    rows
}

fn cmd_list(archived: bool) -> Result<()> {
    let idx = store::load_index()?;
    if idx.sessions.is_empty() && !archived {
        ui::no_sessions_hint();
        return Ok(());
    }
    let rows = list_rows(&idx, archived);
    if rows.is_empty() {
        if archived {
            eprintln!("{}", ui::epaint(ui::DIM, "no archived sessions."));
        } else {
            ui::no_active_sessions_hint();
        }
        return Ok(());
    }
    let header = if archived { "ARCHIVED" } else { "LAST ACCESS" };
    println!(
        "{}  {}  {}  {}",
        ui::paint(ui::DIM, &format!("{:<20}", "NAME")),
        ui::paint(ui::DIM, &format!("{:<4}", "PIN")),
        ui::paint(ui::DIM, &format!("{:<19}", header)),
        ui::paint(ui::DIM, "ORIGIN"),
    );
    for (name, m) in rows {
        let last = store::format_ts(shown_stamp(m, archived));
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

fn cmd_sprint(action: Option<SprintAction>) -> Result<()> {
    match action {
        Some(SprintAction::Add { name }) => {
            let name = if let Some(name) = name {
                name
            } else {
                let idx = store::load_index()?;
                let mut rows = list_rows(&idx, false);
                rows.retain(|(_, meta)| !meta.in_sprint);
                if rows.is_empty() {
                    eprintln!("No active sessions to add to the sprint.");
                    return Ok(());
                }
                let Some(name) = pick_session("Select a session to add to the sprint", &rows)?
                else {
                    return Ok(());
                };
                name
            };
            store::set_in_sprint(&name, true)?;
            ui::done("added to sprint", &name);
        }
        Some(SprintAction::Remove { name }) => {
            let name = if let Some(name) = name {
                name
            } else {
                let idx = store::load_index()?;
                let mut rows = list_rows(&idx, false);
                rows.retain(|(_, meta)| meta.in_sprint);
                if rows.is_empty() {
                    eprintln!("No active sessions to remove from the sprint.");
                    return Ok(());
                }
                let Some(name) = pick_session("Select a session to remove from the sprint", &rows)?
                else {
                    return Ok(());
                };
                name
            };
            store::set_in_sprint(&name, false)?;
            ui::done("removed from sprint", &name);
        }
        None => {
            let idx = store::load_index()?;
            let mut rows = list_rows(&idx, false)
                .into_iter()
                .filter(|(_, meta)| meta.in_sprint)
                .peekable();
            if rows.peek().is_none() {
                eprintln!("No active sessions in the sprint. Run `csm sprint add` to add one.");
            }
            for (name, _) in rows {
                println!("{}", ui::paint(ui::CYAN_BOLD, name));
                card_multiline_row("context", &session_context_lines(name));
            }
        }
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

const CARD_LABEL_WIDTH: usize = 11;

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

fn card_multiline_row(label: &str, lines: &[String]) {
    let Some((first, rest)) = lines.split_first() else {
        card_row(label, &ui::paint(ui::DIM, "(none)"));
        return;
    };
    card_row(label, first);
    let continuation = " ".repeat(2 + CARD_LABEL_WIDTH + 1);
    for value in rest {
        println!("{continuation}{value}");
    }
}

fn cmd_show(name: Option<String>) -> Result<()> {
    let Some(name) = resolve_session_name(name)? else {
        return Ok(());
    };
    let meta = store::require_session(&name)?;

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
    if let Some(ts) = &meta.archived_at {
        card_row("archived", &ui::paint(ui::DIM, &store::format_ts(ts)));
    }

    card_multiline_row("context", &session_context_lines(&name));

    let board = workspace::read_tasks_board(&name).unwrap_or_default();
    let view_ids = workspace::view_task_ids(&name);
    let (lines, claimable) = tasks_block_lines(&board, &name, &view_ids);
    card_multiline_row("tasks", &lines);
    if !claimable.is_empty() {
        card_row("claimable", &claimable);
    }
    Ok(())
}

fn session_context_lines(name: &str) -> Vec<String> {
    workspace::read_context_lines(name, 5)
        .into_iter()
        .map(|line| markdown::truncate(&line, 100))
        .collect()
}

fn tasks_block_lines(
    board: &workspace::TasksBoard,
    name: &str,
    view_ids: &HashSet<String>,
) -> (Vec<String>, String) {
    let total = board.entry_count();
    let mut lines: Vec<String> = Vec::new();
    if total > 0 {
        lines.push(tasks_summary_line(board));
    }
    let mut foreign: HashMap<String, Option<workspace::TasksBoard>> = HashMap::new();
    let mut claimable = String::new();
    if let Some(view) = needs::build_view(
        board,
        name,
        &mut |s, t| {
            foreign
                .entry(s.to_string())
                .or_insert_with(|| workspace::read_tasks_board(s))
                .as_ref()
                .and_then(|b| b.status_of(t))
        },
        view_ids,
    ) {
        for section in view.sections {
            lines.push(section_label(section.status));
            lines.extend(section.lines);
        }
        lines.extend(view.warning_line);
        claimable = view.claimable_line;
    }
    lines.extend(tasks_done_section(board, view_ids));
    (lines, claimable)
}

fn section_label(status: workspace::Status) -> String {
    ui::paint(ui::DIM, status.label())
}

const TASKS_BAR_WIDTH: usize = 20;

fn tasks_summary_line(board: &workspace::TasksBoard) -> String {
    let total = board.entry_count();
    let filled = (TASKS_BAR_WIDTH * board.done.len() + total / 2)
        .checked_div(total)
        .unwrap_or(0);
    let mut s = format!("{} {}/{} done", tasks_bar(filled), board.done.len(), total);
    if !board.pending_review.is_empty() {
        s.push_str(&format!(" · {} review", board.pending_review.len()));
    }
    if !board.pending_fix.is_empty() {
        s.push_str(&format!(" · {} fix", board.pending_fix.len()));
    }
    s
}

fn tasks_bar(filled: usize) -> String {
    let mut bar = String::from("[");
    if filled > 0 {
        bar.push_str(&ui::paint(ui::GREEN, &"█".repeat(filled)));
    }
    let empty = TASKS_BAR_WIDTH - filled;
    if empty > 0 {
        bar.push_str(&ui::paint(ui::DIM, &"░".repeat(empty)));
    }
    bar.push(']');
    bar
}

const DONE_LINES_MAX: usize = 8;

fn tasks_done_section(board: &workspace::TasksBoard, view_ids: &HashSet<String>) -> Vec<String> {
    if board.done.is_empty() {
        return Vec::new();
    }
    let mut entries: Vec<&str> = board.done.iter().map(String::as_str).collect();
    entries.sort_by_key(|&entry| workspace::entry_id_key(entry));
    let mut lines = vec![section_label(workspace::Status::Done)];
    // Style each piece separately because the view marker emits its own ANSI reset.
    for &entry in entries.iter().take(DONE_LINES_MAX) {
        let has_view = view_ids.contains(workspace::entry_id(entry).unwrap_or(""));
        lines.push(format!(
            "{}{}",
            ui::paint(ui::DIM, workspace::entry_head(entry)),
            needs::view_mark(has_view)
        ));
    }
    let hidden = entries.len().saturating_sub(DONE_LINES_MAX);
    if hidden > 0 {
        lines.push(ui::paint(ui::DIM, &format!("… +{hidden} more")));
    }
    lines
}

fn cmd_init() -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn leading_agent_flag_parses_as_root() {
        for argv in [
            vec!["csm", "-a", "pi"],
            vec!["csm", "--agent", "pi"],
            vec!["csm", "--agent=pi"],
        ] {
            let cli = Cli::parse_from(argv);
            assert!(cli.command.is_none(), "expected no subcommand");
            assert_eq!(cli.agent.as_deref(), Some("pi"));
        }
    }

    #[test]
    fn leading_agent_flag_before_name() {
        let cli = Cli::parse_from(["csm", "-a", "pi", "foo"]);
        match cli.command {
            Some(Cmd::Other(v)) => assert_eq!(v, s(&["foo"])),
            _ => panic!("expected external subcommand"),
        }
        assert_eq!(cli.agent.as_deref(), Some("pi"));
    }

    #[test]
    fn trailing_agent_flag_is_captured_raw() {
        let cli = Cli::parse_from(["csm", "foo", "-a", "pi"]);
        match cli.command {
            Some(Cmd::Other(v)) => assert_eq!(v, s(&["foo", "-a", "pi"])),
            _ => panic!("expected external subcommand"),
        }
        assert!(cli.agent.is_none());
    }

    #[test]
    fn shell_shortcut_agent_flag_parses_after_builtin_commands() {
        let cli = Cli::try_parse_from(["csm", "list", "--archived", "-a", "codex"]).unwrap();
        assert!(matches!(cli.command, Some(Cmd::List { archived: true })));

        let cli = Cli::try_parse_from(["csm", "sprint", "add", "-a", "codex"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Cmd::Sprint {
                action: Some(SprintAction::Add { name: None })
            })
        ));
    }

    #[test]
    fn parse_start_args_forms() {
        let (name, agent) = parse_start_args(s(&["foo"])).unwrap();
        assert_eq!(name.as_deref(), Some("foo"));
        assert_eq!(agent, None);
        for args in [
            vec!["foo", "-a", "pi"],
            vec!["foo", "--agent", "pi"],
            vec!["--agent=pi", "foo"],
        ] {
            let (name, agent) = parse_start_args(s(&args)).unwrap();
            assert_eq!(name.as_deref(), Some("foo"), "{args:?}");
            assert_eq!(agent.as_deref(), Some("pi"), "{args:?}");
        }
        let (name, agent) = parse_start_args(s(&[])).unwrap();
        assert_eq!(name, None);
        assert_eq!(agent, None);
    }

    #[test]
    fn parse_start_args_rejects_bad_input() {
        assert!(parse_start_args(s(&["foo", "bar"])).is_err());
        assert!(parse_start_args(s(&["foo", "-a"])).is_err());
        let err = parse_start_args(s(&[""])).unwrap_err().to_string();
        assert!(err.contains("missing session name"), "{err}");
    }

    #[test]
    fn parse_start_args_repeated_trailing_agent() {
        let (name, agent) = parse_start_args(s(&["foo", "-a", "pi", "-a", "pi"])).unwrap();
        assert_eq!(name.as_deref(), Some("foo"));
        assert_eq!(agent.as_deref(), Some("pi"));
        let err = parse_start_args(s(&["foo", "-a", "pi", "--agent", "codex"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("conflicting --agent values"), "{err}");
    }

    #[test]
    fn resolve_agent_combines_and_defaults() {
        assert_eq!(resolve_agent(None, None).unwrap(), "claude");
        assert_eq!(resolve_agent(None, Some("pi".into())).unwrap(), "pi");
        assert_eq!(resolve_agent(Some("pi".into()), None).unwrap(), "pi");
        assert_eq!(
            resolve_agent(Some("pi".into()), Some("pi".into())).unwrap(),
            "pi"
        );
    }

    #[test]
    fn resolve_agent_rejects_conflict() {
        assert!(resolve_agent(Some("pi".into()), Some("codex".into())).is_err());
    }

    #[test]
    fn resolve_agent_reports_unknown_id_before_conflict() {
        let err = resolve_agent(Some("PI".into()), Some("pi".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown agent"), "{err}");
    }

    #[test]
    fn cli_parses_list_archive_and_unarchive() {
        match Cli::parse_from(["csm", "list"]).command {
            Some(Cmd::List { archived }) => assert!(!archived),
            _ => panic!("expected list"),
        }
        match Cli::parse_from(["csm", "list", "--archived"]).command {
            Some(Cmd::List { archived }) => assert!(archived),
            _ => panic!("expected list --archived"),
        }
        match Cli::parse_from(["csm", "archive", "api"]).command {
            Some(Cmd::Archive { name }) => assert_eq!(name, "api"),
            _ => panic!("expected archive"),
        }
        match Cli::parse_from(["csm", "unarchive", "api"]).command {
            Some(Cmd::Unarchive { name }) => assert_eq!(name, "api"),
            _ => panic!("expected unarchive"),
        }
    }

    fn meta(origin: &str, last_access: &str, archived_at: Option<&str>) -> store::SessionMeta {
        store::SessionMeta {
            origin_pwd: origin.into(),
            registered_dirs: Vec::new(),
            created_at: last_access.into(),
            last_access: last_access.into(),
            pinned: false,
            archived_at: archived_at.map(Into::into),
            in_sprint: false,
        }
    }

    #[test]
    fn list_rows_filters_by_archive_and_sorts_by_shown_stamp() {
        let mut idx = store::Index::default();
        for (name, last_access, archived_at) in [
            ("active-old", "2026-10-01T00:00:00+00:00", None),
            ("active-new", "2026-10-03T00:00:00+00:00", None),
            (
                "archived-old",
                "2026-10-02T00:00:00+00:00",
                Some("2026-10-05T00:00:00+00:00"),
            ),
            (
                "archived-new",
                "2026-10-04T00:00:00+00:00",
                Some("2026-10-06T00:00:00+00:00"),
            ),
        ] {
            idx.sessions
                .insert(name.into(), meta("/o", last_access, archived_at));
        }
        let names = |archived: bool| -> Vec<String> {
            list_rows(&idx, archived)
                .into_iter()
                .map(|(n, _)| n.clone())
                .collect()
        };
        assert_eq!(names(false), vec!["active-new", "active-old"]);
        assert_eq!(names(true), vec!["archived-new", "archived-old"]);
    }

    fn board(md: &str) -> workspace::TasksBoard {
        workspace::parse_tasks_board(md)
    }

    #[test]
    fn tasks_summary_line_bar_and_counts() {
        let b = board(
            "## Open\n- 001 a\n\n## Pending review\n- 002 b\n\n\
             ## Pending fix\n- 003 c\n\n## Done\n- 000 d\n",
        );
        assert_eq!(
            tasks_summary_line(&b),
            "[█████░░░░░░░░░░░░░░░] 1/4 done · 1 review · 1 fix"
        );
    }

    #[test]
    fn tasks_summary_line_full_and_empty_share() {
        let all = board("## Done\n- 000 a\n- 001 b\n- 002 c\n");
        assert_eq!(tasks_summary_line(&all), "[████████████████████] 3/3 done");
        let none = board("## Open\n- 001 a\n- 002 b\n- 003 c\n");
        assert_eq!(tasks_summary_line(&none), "[░░░░░░░░░░░░░░░░░░░░] 0/3 done");
        let two =
            board("## Open\n- 002 a\n- 003 b\n- 004 c\n- 005 d\n\n## Done\n- 000 e\n- 001 f\n");
        assert_eq!(tasks_summary_line(&two), "[███████░░░░░░░░░░░░░] 2/6 done");
    }

    #[test]
    fn tasks_done_section_marks_only_tasks_with_a_view() {
        let b = board("## Done\n- 001 a\n- 002 b\n");
        let one: HashSet<String> = ["001".to_string()].into_iter().collect();
        assert_eq!(
            tasks_done_section(&b, &one),
            vec![
                "Done".to_string(),
                format!("001 a {}", ui::T_VIEW),
                "002 b".to_string()
            ]
        );
    }

    #[test]
    fn tasks_done_section_lists_one_head_per_line_sorted_by_id() {
        let b = board("## Done\n- 004 d\n- 10 j\n- 9 i\n- 002 b needs: 001\n- 001 a\n");
        assert_eq!(
            tasks_done_section(&b, &HashSet::new()),
            vec!["Done", "001 a", "002 b", "004 d", "9 i", "10 j"]
        );
        assert!(tasks_done_section(&board("## Open\n- 001 a\n"), &HashSet::new()).is_empty());
    }

    #[test]
    fn tasks_done_section_caps_and_counts_the_rest() {
        let mut md = String::from("## Done\n");
        for i in 0..(DONE_LINES_MAX + 3) {
            md.push_str(&format!("- {i:03} t\n"));
        }
        let lines = tasks_done_section(&board(&md), &HashSet::new());
        assert_eq!(lines.len(), DONE_LINES_MAX + 2);
        assert_eq!(lines[1], "000 t");
        assert_eq!(lines[DONE_LINES_MAX], "007 t");
        assert_eq!(lines.last().unwrap(), "… +3 more");
    }

    #[test]
    fn tasks_block_labels_sections_in_board_order() {
        let b = board(
            "## Open\n- 002 a\n\n## Pending review\n- 003 b\n\n\
             ## Pending fix\n- 004 c\n\n## Done\n- 001 d\n",
        );
        let (lines, claimable) = tasks_block_lines(&b, "ws", &HashSet::new());
        assert_eq!(
            lines,
            vec![
                tasks_summary_line(&b),
                "Open".to_string(),
                "└── ○ 002 a".to_string(),
                "Pending review".to_string(),
                "└── ◐ 003 b".to_string(),
                "Pending fix".to_string(),
                "└── ◑ 004 c".to_string(),
                "Done".to_string(),
                "001 d".to_string(),
            ]
        );
        assert_eq!(claimable, "");
    }

    #[test]
    fn tasks_bar_skips_zero_width_sides() {
        assert_eq!(tasks_bar(0), "[░░░░░░░░░░░░░░░░░░░░]");
        assert_eq!(tasks_bar(TASKS_BAR_WIDTH), "[████████████████████]");
        assert_eq!(tasks_bar(7), "[███████░░░░░░░░░░░░░]");
    }
}
