//! Inline markdown -> plain-text stripper for `csm show` card gists.
//!
//! `csm show` is a recognition aid (which session is this?), not a document
//! viewer - it shows one-line gists, not rendered sections. So this module
//! only strips inline markers (`**bold**`, `*italic*`, `` `code` ``,
//! `[[wikilink]]`, `[text](url)`, `<!-- comments -->`) so a gist reads as text
//! instead of raw markdown source. No styling, no block-level parsing - for
//! the full file, `cat state.md`. Styling lives in `ui`; this is plain text.

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
