//! Plain-text markdown rendering for `csm show` (card gists) and `csm detail`
//! (full rendered state.md).
//!
//! `csm show` is a recognition aid (which session is this?) - it shows
//! one-line gists, so it only needs `strip_inline` + `truncate`. `csm detail`
//! is the deep read, so `sections` splits a state.md into `## Section`s and
//! inline-strips each body line. No styling here (that lives in `ui`), no
//! Unicode glyphs - plain text that `cmd_show`/`cmd_detail` wrap in cargo-style
//! color.

/// Strip inline markdown markers from `text`, returning plain text. Unmatched
/// markers (e.g. a lone `*`) are left literal.
pub fn strip_inline(input: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < input.len() {
        let rest = &input[i..];

        if let Some(after) = rest.strip_prefix("<!--") {
            if let Some(end) = after.find("-->") {
                i += 4 + end + 3;
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**") {
                out.push_str(&after[..end]);
                i += 2 + end + 2;
                continue;
            }
        }
        // *italic* - only when followed by a non-space, non-`*` char (avoids
        // `**bold**`, bullet-like `* `, stray `*` in prose). Must run after the
        // `**` branch so bold is matched first.
        if let Some(after) = rest.strip_prefix('*') {
            let ok = after
                .as_bytes()
                .first()
                .is_some_and(|&b| b != b' ' && b != b'*');
            if ok {
                if let Some(end) = after.find('*') {
                    out.push_str(&after[..end]);
                    i += 1 + end + 1;
                    continue;
                }
            }
        }
        if let Some(after) = rest.strip_prefix("[[") {
            if let Some(end) = after.find("]]") {
                out.push_str(&after[..end]);
                i += 2 + end + 2;
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                out.push_str(&after[..end]);
                i += 1 + end + 1;
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('[') {
            // [text](url)
            if let Some(close) = after.find(']') {
                let text = &after[..close];
                let after_close = &after[close + 1..];
                if let Some(url_body) = after_close.strip_prefix('(') {
                    if let Some(paren_close) = url_body.find(')') {
                        out.push_str(text);
                        // consumed: '[' + text + ']' + '(' + url + ')' = close + paren_close + 4
                        i += close + paren_close + 4;
                        continue;
                    }
                }
            }
        }

        // copy one char (keeps `i` on a UTF-8 boundary)
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Truncate `s` to at most `max` chars (by char count). If it's longer, cut at
/// the last whitespace within the window (avoids mid-word cuts) and append `…`.
pub fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let window: String = chars[..max].iter().collect();
    let cut_chars = match window.rfind(char::is_whitespace) {
        Some(byte_idx) => window[..byte_idx].chars().count(),
        None => max,
    };
    let mut t: String = chars[..cut_chars].iter().collect();
    t.push('…');
    t
}

/// A `## Section` of a state.md: its title (text after `## `) and the body
/// lines under it, each with inline markdown stripped. `csm detail` renders one
/// per section. The H1 title and `>` quote boilerplate before the first section
/// are dropped; genuine blank lines are kept as paragraph breaks.
pub struct Section {
    pub title: String,
    pub body: Vec<String>,
}

/// Split a markdown document into `## `-headed sections. Each section's body is
/// inline-stripped (so `**x**` reads as `x`) with leading/trailing blank lines
/// trimmed; markup-only lines (e.g. HTML comments) are dropped. Sub-structure
/// (list markers, checkboxes, `###` sub-headings) is preserved as text. Returns
/// sections in document order; an empty vec means no `## ` headings at all.
pub fn sections(content: &str) -> Vec<Section> {
    let mut out = Vec::new();
    let mut cur: Option<Section> = None;
    for line in content.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            if let Some(s) = cur.take() {
                out.push(trim_section(s));
            }
            cur = Some(Section {
                title: title.trim().to_string(),
                body: Vec::new(),
            });
            continue;
        }
        // Before the first section: drop the H1 and `>` boilerplate. Inside a
        // section: keep genuine blank lines as paragraph breaks, drop
        // markup-only lines, inline-strip the rest.
        let Some(s) = cur.as_mut() else {
            continue;
        };
        if line.trim().is_empty() {
            s.body.push(String::new());
        } else {
            let stripped = strip_inline(line);
            if !stripped.is_empty() {
                s.body.push(stripped);
            }
        }
    }
    if let Some(s) = cur.take() {
        out.push(trim_section(s));
    }
    out
}

/// Trim leading/trailing blank lines from a section's body so a section whose
/// only "content" was a comment or whitespace renders as `(none)`.
fn trim_section(mut s: Section) -> Section {
    while s.body.first().is_some_and(|l| l.is_empty()) {
        s.body.remove(0);
    }
    while s.body.last().is_some_and(|l| l.is_empty()) {
        s.body.pop();
    }
    s
}
