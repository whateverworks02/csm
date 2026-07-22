//! Terminal styling for csm's user-facing output.
//!
//! Built on `anstyle` (already in the dep tree via clap). Stream-aware:
//! `paint` targets stdout (display commands), `epaint` targets stderr (status
//! and diagnostics). Both honor `NO_COLOR` / `CLICOLOR_FORCE` and isatty, so
//! color is stripped when piped - keeping machine-readable outputs (the
//! `--no-launch` `export` line, the hook's JSON) clean. Those two outputs use
//! raw `println!` anyway.
//!
//! Aesthetic: cargo-like restraint. Color, not icons - no checkmarks, stars,
//! or bullets. Verbs and labels carry the meaning; paths and timestamps recede
//! (dim, `~`-abbreviated). Errors use cargo's red `error:` prefix.

use std::io::IsTerminal;
use std::sync::OnceLock;

use anstyle::{AnsiColor, Color, Style};

// --- Styles -----------------------------------------------------------------

pub const BOLD: Style = Style::new().bold();
pub const DIM: Style = Style::new().dimmed();
pub const YELLOW: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
pub const GREEN_BOLD: Style = Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Green)));
pub const RED_BOLD: Style = Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Red)));
pub const CYAN_BOLD: Style = Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
pub const YELLOW_BOLD: Style = Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));

/// ASCII arrow for transformations (rename) and hints. Plain text, not an icon.
pub const ARROW: &str = "->";
/// ASCII marker for pinned sessions in tables. Plain text, not an icon.
pub const PIN_MARK: &str = "*";

// --- Color gate (per stream) ------------------------------------------------

static STDOUT_COLOR: OnceLock<bool> = OnceLock::new();
static STDERR_COLOR: OnceLock<bool> = OnceLock::new();

/// Decide color for a stream from the env + tty. Follows CLICOLOR / NO_COLOR:
/// `NO_COLOR` (any value) disables; `CLICOLOR_FORCE` (non-zero) forces on;
/// otherwise color iff the stream is a terminal.
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

/// Wrap `text` in `style` for stdout (display). No-op when color is off.
pub fn paint(style: Style, text: &str) -> String {
    if stdout_color() {
        format!("{}{}{}", style.render(), text, style.render_reset())
    } else {
        text.to_string()
    }
}

/// Wrap `text` in `style` for stderr (status / diagnostics). No-op when off.
pub fn epaint(style: Style, text: &str) -> String {
    if stderr_color() {
        format!("{}{}{}", style.render(), text, style.render_reset())
    } else {
        text.to_string()
    }
}

// --- Composite status lines (stderr) ----------------------------------------

/// `verb: name` - a completed action on a session (`pinned: api`, `deleted: api`).
pub fn done(verb: &str, name: &str) {
    eprintln!("{}: {}", epaint(GREEN_BOLD, verb), epaint(CYAN_BOLD, name));
}

/// `verb <rest>` - a status step with a green verb and dim detail
/// (`wrote SessionStart hook to ~/.claude/settings.json`).
pub fn step(verb: &str, rest: &str) {
    eprintln!("{} {}", epaint(GREEN_BOLD, verb), epaint(DIM, rest));
}

/// `warning: <msg>` - cargo-style warning line.
pub fn warn(msg: &str) {
    eprintln!("{} {}", epaint(YELLOW_BOLD, "warning:"), msg);
}

/// A dim `-> <msg>` hint line.
pub fn hint(msg: &str) {
    eprintln!("{} {}", epaint(DIM, ARROW), epaint(DIM, msg));
}

/// The "no csm sessions" empty state, with a dim hint.
pub fn no_sessions_hint() {
    eprintln!("{}", epaint(DIM, "no csm sessions yet."));
    hint("start one with: csm <name>");
}

// --- Path helpers -----------------------------------------------------------

/// Replace a leading `$HOME` with `~` for display. Falls back to the raw
/// string if `HOME` is unset or the path doesn't start with it.
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

// --- Error printing (cargo-style) -------------------------------------------

/// Print an error in cargo's style: a red-bold `error:` line, followed by a
/// dim `Caused by:` chain if the error has context. Goes to stderr.
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
