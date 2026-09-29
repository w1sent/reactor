//! JavaScript's string arithmetic, where the output has to match it.
//!
//! The prompt blocks are compared byte for byte with what the TypeScript
//! extensions produced, and `String.prototype.length`/`slice` count UTF-16 code
//! units, not characters. For text outside the BMP the two disagree, so the
//! truncation helpers count the way JS does.

/// `s.length`.
pub fn js_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `s.slice(0, n)` — by UTF-16 units. A cut that would split a surrogate pair
/// stops before the pair (JS would leave a lone surrogate, which a Rust `String`
/// cannot hold).
pub fn js_prefix(s: &str, n: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in s.chars() {
        let w = c.len_utf16();
        if units + w > n {
            break;
        }
        units += w;
        out.push(c);
    }
    out
}

/// `truncate(s, n)`: whole if it fits, else the first `n-1` units and an ellipsis.
pub fn truncate(s: &str, n: usize) -> String {
    if js_len(s) <= n {
        return s.to_string();
    }
    format!("{}…", js_prefix(s, n.saturating_sub(1)))
}

/// `s.trim().split(/\s+/).slice(0, n).join(" ")`.
pub fn truncate_words(s: &str, n: usize) -> String {
    let t = js_trim(s);
    if t.is_empty() {
        // "".split(/\s+/) is [""], which joins to "".
        return String::new();
    }
    t.split(js_space).filter(|w| !w.is_empty()).take(n).collect::<Vec<_>>().join(" ")
}

/// JS `\s` and `String.prototype.trim` whitespace: Unicode White_Space plus U+FEFF.
pub fn js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// `s.trim()`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}
