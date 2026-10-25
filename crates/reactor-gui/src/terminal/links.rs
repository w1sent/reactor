//! A link under the pointer: an OSC 8 hyperlink the program wrote, or a plain `http(s)://` URL in
//! the text.

use std::sync::OnceLock;

use regex::Regex;

use super::types::{Attrs, Cell};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The first and the last column it covers.
    pub start: usize,
    pub end: usize,
    pub url: String,
}

fn url_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"https?://[^\s<>"'`\\^{}|]+"#).expect("a valid pattern"))
}

/// What ends a sentence and not the address in it.
fn trim_end(url: &str) -> &str {
    let mut url = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
    for (open, close) in [('(', ')'), ('[', ']')] {
        while url.ends_with(close) && url.matches(close).count() > url.matches(open).count() {
            url = &url[..url.len() - 1];
        }
    }
    url
}

/// The link at `col` of a row, if there is one.
pub fn link_at(row: &[Cell], col: usize) -> Option<Link> {
    let cell = row.get(col)?;
    if let Some(target) = cell.link() {
        let same = |c: &Cell| c.link().is_some_and(|l| l == target);
        let mut start = col;
        while start > 0 && same(&row[start - 1]) {
            start -= 1;
        }
        let mut end = col;
        while end + 1 < row.len() && same(&row[end + 1]) {
            end += 1;
        }
        return Some(Link {
            start,
            end,
            url: target.to_string(),
        });
    }

    // The row's text, remembering which column each character starts at.
    let mut text = String::new();
    let mut columns = Vec::new(); // (byte offset, column) per character
    for (i, c) in row.iter().enumerate() {
        if c.attrs.has(Attrs::SPACER) {
            continue;
        }
        columns.push((text.len(), i));
        text.push(if c.c == '\0' { ' ' } else { c.c });
    }
    let col_of = |byte: usize| {
        columns
            .iter()
            .rev()
            .find(|(b, _)| *b <= byte)
            .map(|(_, c)| *c)
    };
    for found in url_pattern().find_iter(&text) {
        let url = trim_end(found.as_str());
        if url.len() < "https://".len() {
            continue;
        }
        let start = col_of(found.start())?;
        let last_char_byte = found.start() + url.len() - url.chars().last()?.len_utf8();
        let end = col_of(last_char_byte)?;
        if (start..=end).contains(&col) {
            return Some(Link {
                start,
                end,
                url: url.to_owned(),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::types::Extra;
    use std::sync::Arc;

    fn row(text: &str) -> Vec<Cell> {
        text.chars()
            .map(|c| Cell {
                c,
                ..Cell::default()
            })
            .collect()
    }

    #[test]
    fn a_plain_url_is_found_from_any_column_of_it() {
        let r = row("see https://example.com/a?b=1 for more");
        let want = Link {
            start: 4,
            end: 28,
            url: "https://example.com/a?b=1".into(),
        };
        assert_eq!(link_at(&r, 4), Some(want.clone()));
        assert_eq!(link_at(&r, 15), Some(want.clone()));
        assert_eq!(link_at(&r, 28), Some(want));
        assert_eq!(link_at(&r, 3), None);
        assert_eq!(link_at(&r, 29), None);
    }

    #[test]
    fn what_ends_the_sentence_is_not_the_address() {
        for (text, url) in [
            ("go to http://a.org/x.", "http://a.org/x"),
            ("(see https://a.org/x)", "https://a.org/x"),
            ("<https://a.org/x>, then", "https://a.org/x"),
            (
                "https://en.wikipedia.org/wiki/Rust_(language)",
                "https://en.wikipedia.org/wiki/Rust_(language)",
            ),
        ] {
            let r = row(text);
            let col = text.find("http").unwrap();
            assert_eq!(
                link_at(&r, col).map(|l| l.url).as_deref(),
                Some(url),
                "{text}"
            );
        }
    }

    #[test]
    fn a_bare_scheme_is_not_a_link() {
        assert_eq!(link_at(&row("https:// nothing"), 2), None);
    }

    #[test]
    fn a_hyperlink_the_program_wrote_covers_the_cells_it_marked() {
        let mut r = row("click here now");
        let target: Arc<str> = Arc::from("https://docs.example/");
        for cell in &mut r[6..10] {
            cell.extra = Some(Box::new(Extra {
                link: Some(target.clone()),
                ..Extra::default()
            }));
        }
        let got = link_at(&r, 8).unwrap();
        assert_eq!(
            (got.start, got.end, got.url.as_str()),
            (6, 9, "https://docs.example/")
        );
        assert_eq!(link_at(&r, 2), None);
    }

    #[test]
    fn columns_follow_wide_characters() {
        let mut r = row("日 https://a.org");
        // A wide character is a cell and its spacer.
        r[0].attrs = Attrs(Attrs::WIDE);
        r.insert(
            1,
            Cell {
                c: ' ',
                attrs: Attrs(Attrs::SPACER),
                ..Cell::default()
            },
        );
        let l = link_at(&r, 4).unwrap();
        assert_eq!((l.start, l.url.as_str()), (3, "https://a.org"));
    }
}
