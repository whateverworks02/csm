use anyhow::Result;
use std::io::IsTerminal;
use std::io::Write;
use std::sync::OnceLock;

use anstyle::{AnsiColor, Color, Style};

pub const BOLD: Style = Style::new().bold();
pub const DIM: Style = Style::new().dimmed();
pub const CYAN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
pub const YELLOW: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
pub const GREEN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
pub const MAGENTA: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Magenta)));
pub const GREEN_BOLD: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Green)));
pub const RED_BOLD: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Red)));
pub const CYAN_BOLD: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
pub const YELLOW_BOLD: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Yellow)));

pub const ARROW: &str = "->";
pub const PIN_MARK: &str = "*";

pub const T_OPEN: &str = "○";
pub const T_REVIEW: &str = "◐";
pub const T_FIX: &str = "◑";
pub const T_DONE: &str = "✓";
pub const T_UNKNOWN: &str = "?";
pub const T_ALERT: &str = "!";
pub const T_VIEW: &str = "◆";

static STDOUT_COLOR: OnceLock<bool> = OnceLock::new();
static STDERR_COLOR: OnceLock<bool> = OnceLock::new();

/// `NO_COLOR` takes precedence over `CLICOLOR_FORCE` and terminal detection.
fn want_color(is_tty: bool) -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if let Some(v) = std::env::var_os("CLICOLOR_FORCE") {
        if v != "0" {
            return true;
        }
    }
    is_tty
}

fn stdout_color() -> bool {
    *STDOUT_COLOR.get_or_init(|| want_color(std::io::stdout().is_terminal()))
}

fn stderr_color() -> bool {
    *STDERR_COLOR.get_or_init(|| want_color(std::io::stderr().is_terminal()))
}

pub fn paint(style: Style, text: &str) -> String {
    render(style, text, stdout_color())
}

pub fn epaint(style: Style, text: &str) -> String {
    render(style, text, stderr_color())
}

fn render(style: Style, text: &str, color: bool) -> String {
    if color {
        format!("{}{}{}", style.render(), text, style.render_reset())
    } else {
        text.to_string()
    }
}

pub fn done(verb: &str, name: &str) {
    eprintln!("{}: {}", epaint(GREEN_BOLD, verb), epaint(CYAN_BOLD, name));
}

pub fn step(verb: &str, rest: &str) {
    eprintln!("{} {}", epaint(GREEN_BOLD, verb), epaint(DIM, rest));
}

pub fn warn(msg: &str) {
    eprintln!("{} {}", epaint(YELLOW_BOLD, "warning:"), msg);
}

pub fn hint(msg: &str) {
    eprintln!("{} {}", epaint(DIM, ARROW), epaint(DIM, msg));
}

pub fn no_sessions_hint() {
    eprintln!("{}", epaint(DIM, "no csm sessions yet."));
    hint("start one with: csm <name>");
}

pub fn no_active_sessions_hint() {
    eprintln!("{}", epaint(DIM, "no active sessions."));
    hint("csm list --archived");
}

pub fn confirm(msg: &str) -> Result<bool> {
    eprintln!("{}", epaint(BOLD, msg));
    eprint!("  {} ", epaint(DIM, "[y/N]"));
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().eq_ignore_ascii_case("y"))
}

/// Format a session row. The optional index is 1-based, and `marker` includes its own spacing.
pub fn session_row(
    index: Option<usize>,
    name: &str,
    last_access: &str,
    origin: &str,
    marker: &str,
) -> String {
    let index = match index {
        Some(i) => format!("{:>2}", i),
        None => "  ".to_string(),
    };
    format!(
        "  {}  {}  {}  {}{}",
        epaint(DIM, &index),
        epaint(CYAN_BOLD, &format!("{:<20}", name)),
        epaint(DIM, &format!("{:<16}", last_access)),
        epaint(DIM, &abbrev_home(origin)),
        marker
    )
}

pub fn abbrev_home(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home.as_ref()) {
            return format!("~{}", rest);
        }
    }
    path.to_string()
}

pub fn abbrev_path(path: &std::path::Path) -> String {
    abbrev_home(&path.display().to_string())
}

pub fn print_error(err: &anyhow::Error) {
    eprintln!("{} {}", epaint(RED_BOLD, "error:"), err);
    let causes: Vec<_> = err.chain().skip(1).collect();
    if causes.is_empty() {
        return;
    }
    eprintln!("{}", epaint(DIM, "Caused by:"));
    if causes.len() == 1 {
        eprintln!("    {}", causes[0]);
    } else {
        for (i, cause) in causes.iter().enumerate() {
            eprintln!("    {i}: {cause}");
        }
    }
}
