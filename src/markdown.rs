//! Markdown text utilities: section parsing (`sections`, `strip_inline`) and
//! display truncation (`truncate`).
//!
//! `sections` splits a document into `## `-headed sections with inline markers
//! stripped. It serves both display (`csm detail` renders state.md and the task
//! board) and data extraction (`parse_tasks_board`, `read_context_lines` read
//! workspace files). `truncate` caps a line for the `csm show` card. No styling
//! here (that lives in `ui`), no Unicode glyphs - plain text that
//! `cmd_show`/`cmd_detail` wrap in cargo-style color.

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

#[cfg(test)]
mod tests {
    use super::*;

    mod strip_inline {
        use super::*;

        #[test]
        fn plain_text_unchanged() {
            assert_eq!(strip_inline("plain text"), "plain text");
            assert_eq!(strip_inline(""), "");
        }

        #[test]
        fn bold_and_italic() {
            assert_eq!(strip_inline("**bold**"), "bold");
            assert_eq!(strip_inline("*italic*"), "italic");
            assert_eq!(strip_inline("**a**b**c**"), "abc");
        }

        #[test]
        fn code_and_links() {
            assert_eq!(strip_inline("`code`"), "code");
            assert_eq!(strip_inline("[text](url)"), "text");
            assert_eq!(strip_inline("[[wikilink]]"), "wikilink");
        }

        #[test]
        fn html_comment_removed() {
            assert_eq!(strip_inline("a<!-- comment -->b"), "ab");
            // Unterminated comment falls through: "<!--" has no "-->" so the
            // marker chars are copied literally.
            assert_eq!(strip_inline("<!-- no close"), "<!-- no close");
        }

        #[test]
        fn unmatched_markers_kept_literal() {
            assert_eq!(strip_inline("**"), "**");
            assert_eq!(strip_inline("*stray*"), "stray");
            // A bullet-like "* " is not italic (next char is a space).
            assert_eq!(strip_inline("* item"), "* item");
        }

        #[test]
        fn multibyte_safe() {
            // Byte-offset markers must land on UTF-8 boundaries.
            assert_eq!(strip_inline("**日本**"), "日本");
            assert_eq!(strip_inline("[[リンク]]"), "リンク");
        }
    }

    mod truncate {
        use super::*;

        #[test]
        fn short_or_equal_passthrough() {
            assert_eq!(truncate("abc", 5), "abc");
            assert_eq!(truncate("abcde", 5), "abcde");
            assert_eq!(truncate("", 5), "");
        }

        #[test]
        fn over_length_cuts_at_word_boundary() {
            // "ab cd" -> last whitespace at byte 2 -> "ab…"
            assert_eq!(truncate("ab cdef", 5), "ab…");
            // No whitespace in the window -> hard cut at max.
            assert_eq!(truncate("abcdef", 5), "abcde…");
        }

        #[test]
        fn counts_chars_not_bytes() {
            // 6 chars, max 3 -> cut to 3 chars (would be 9 bytes if byte-counted).
            assert_eq!(truncate("日本語テスト", 3), "日本語…");
        }

        #[test]
        fn max_zero_yields_just_ellipsis() {
            // Characterizing current behavior: empty window, no whitespace, so
            // cut_chars == 0 and only the ellipsis is appended.
            assert_eq!(truncate("abc", 0), "…");
        }
    }

    mod sections {
        use super::*;

        #[test]
        fn no_headings_yields_empty() {
            assert!(sections("").is_empty());
            assert!(sections("# Title\n> quote\nplain").is_empty());
        }

        #[test]
        fn h1_and_quote_dropped() {
            let s = sections("# T\n> q\n## Task\nbody");
            assert_eq!(s.len(), 1);
            assert_eq!(s[0].title, "Task");
            assert_eq!(s[0].body, vec!["body"]);
        }

        #[test]
        fn multiple_sections_in_order() {
            let s = sections("## A\nx\n## B\ny");
            assert_eq!(s.len(), 2);
            assert_eq!(s[0].title, "A");
            assert_eq!(s[0].body, vec!["x"]);
            assert_eq!(s[1].title, "B");
            assert_eq!(s[1].body, vec!["y"]);
        }

        #[test]
        fn comment_only_lines_dropped() {
            let s = sections("## A\n<!-- c -->\nreal");
            assert_eq!(s[0].body, vec!["real"]);
        }

        #[test]
        fn leading_trailing_blanks_trimmed_internal_kept() {
            let s = sections("## A\n\n\npara1\n\npara2\n\n");
            // Leading/trailing blank lines removed; the break between para1 and
            // para2 is preserved as an empty body line.
            assert_eq!(s[0].body, vec!["para1", "", "para2"]);
        }

        #[test]
        fn bodies_are_inline_stripped() {
            let s = sections("## A\n**bold** and `code`");
            assert_eq!(s[0].body, vec!["bold and code"]);
        }

        #[test]
        fn substructure_preserved_as_text() {
            let s = sections("## A\n### Sub\n- item\n- [ ] todo");
            assert_eq!(s[0].body, vec!["### Sub", "- item", "- [ ] todo"]);
        }

        #[test]
        fn title_is_trimmed() {
            let s = sections("##   Spaced  \nbody");
            assert_eq!(s[0].title, "Spaced");
        }
    }
}
