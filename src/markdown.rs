/// Strip inline markers while leaving unmatched markers literal.
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
        // Match bold first and leave bullet markers literal.
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
            if let Some(close) = after.find(']') {
                let text = &after[..close];
                let after_close = &after[close + 1..];
                if let Some(url_body) = after_close.strip_prefix('(') {
                    if let Some(paren_close) = url_body.find(')') {
                        out.push_str(text);
                        i += close + paren_close + 4;
                        continue;
                    }
                }
            }
        }

        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Limit text by character count, preferring a word boundary before the ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    let Some((end, _)) = s.char_indices().nth(max) else {
        return s.to_string();
    };
    let window = &s[..end];
    let cut = window.rfind(char::is_whitespace).unwrap_or(end);
    format!("{}…", &s[..cut])
}

pub struct Section {
    pub title: String,
    pub body: Vec<String>,
}

/// Split on level-two headings and strip inline markup from each section.
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

fn trim_section(mut s: Section) -> Section {
    let first = s.body.iter().take_while(|line| line.is_empty()).count();
    s.body.drain(..first);
    while s.body.last().is_some_and(|line| line.is_empty()) {
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
            assert_eq!(strip_inline("<!-- no close"), "<!-- no close");
        }

        #[test]
        fn unmatched_markers_kept_literal() {
            assert_eq!(strip_inline("**"), "**");
            assert_eq!(strip_inline("*stray*"), "stray");
            assert_eq!(strip_inline("* item"), "* item");
        }

        #[test]
        fn multibyte_safe() {
            assert_eq!(strip_inline("**café**"), "café");
            assert_eq!(strip_inline("[[naïve]]"), "naïve");
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
            assert_eq!(truncate("ab cdef", 5), "ab…");
            assert_eq!(truncate("abcdef", 5), "abcde…");
        }

        #[test]
        fn counts_chars_not_bytes() {
            assert_eq!(truncate("Ελλάδα", 3), "Ελλ…");
        }

        #[test]
        fn max_zero_yields_just_ellipsis() {
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
