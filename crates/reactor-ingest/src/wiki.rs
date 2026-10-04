//! MediaWiki export dumps (`Special:Export`, `pages-articles.xml`): one document per page,
//! read as a stream so a dump of any size costs time and not memory.
//!
//! Wikitext is turned into readable Markdown, not rendered: headings, lists, links and
//! tables keep their words; templates, references and markup that has no words are
//! dropped. Redirects and discussion pages are skipped.

use std::io::BufRead;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::convert::{Converted, Error, Result};
use crate::xml::{local, push_ref};

/// The namespaces worth searching: articles, the project's own pages, help.
fn wanted(ns: i64) -> bool {
    matches!(ns, 0 | 4 | 12)
}

/// Read every page of a dump, handing each to `sink`.
pub fn pages(input: impl BufRead, sink: &mut dyn FnMut(Converted) -> Result<()>) -> Result<()> {
    let mut reader = Reader::from_reader(input);
    let mut buf = Vec::new();
    // Which element's text is being collected.
    #[derive(PartialEq)]
    enum In {
        None,
        Title,
        Ns,
        Text,
    }
    let (mut at, mut title, mut ns, mut text) = (In::None, String::new(), 0i64, String::new());
    let (mut redirect, mut in_page) = (false, false);
    let mut ns_text = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local(e.local_name()).as_str() {
                "page" => {
                    in_page = true;
                    title.clear();
                    text.clear();
                    ns = 0;
                    redirect = false;
                }
                "title" if in_page => at = In::Title,
                "ns" if in_page => {
                    at = In::Ns;
                    ns_text.clear();
                }
                "text" if in_page => at = In::Text,
                _ => {}
            },
            Ok(Event::Empty(e)) if local(e.local_name()) == "redirect" => redirect = true,
            Ok(Event::Text(t)) => match at {
                In::Title => title.push_str(&t.xml10_content()),
                In::Ns => ns_text.push_str(&t.xml10_content()),
                In::Text => text.push_str(&t.xml10_content()),
                In::None => {}
            },
            Ok(Event::GeneralRef(r)) => match at {
                In::Title => push_ref(&mut title, &r),
                In::Ns => push_ref(&mut ns_text, &r),
                In::Text => push_ref(&mut text, &r),
                In::None => {}
            },
            Ok(Event::End(e)) => match local(e.local_name()).as_str() {
                "title" | "text" => at = In::None,
                "ns" => {
                    ns = ns_text.trim().parse().unwrap_or(0);
                    at = In::None;
                }
                "page" => {
                    in_page = false;
                    let title = title.trim().to_string();
                    if !redirect && wanted(ns) && !title.is_empty() {
                        let body = wikitext(&text);
                        if !body.trim().is_empty() {
                            sink(Converted {
                                markdown: format!("# {title}\n\n{body}"),
                                title: Some(title.clone()),
                                mime: "text/x-wiki".into(),
                                fragment: Some(title),
                            })?;
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(Error::Unreadable(format!("not a readable dump: {e}"))),
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

/// Remove `open ... close` spans, nested ones included (`{{a|{{b}}}}`).
fn strip_nested(s: &str, open: &str, close: &str) -> String {
    let (mut out, mut depth, mut i) = (String::with_capacity(s.len()), 0usize, 0usize);
    while i < s.len() {
        if s[i..].starts_with(open) {
            depth += 1;
            i += open.len();
        } else if depth > 0 && s[i..].starts_with(close) {
            depth -= 1;
            i += close.len();
        } else {
            let c = s[i..].chars().next().unwrap();
            if depth == 0 {
                out.push(c);
            }
            i += c.len_utf8();
        }
    }
    out
}

/// Remove `<tag ...>...</tag>` spans (`ref`, `gallery`), and self-closing ones.
fn strip_tag(s: &str, tag: &str) -> String {
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let lower = s.to_lowercase();
    let (mut out, mut i) = (String::new(), 0);
    while let Some(start) = lower[i..].find(&open).map(|p| p + i) {
        out.push_str(&s[i..start]);
        let tag_end = match lower[start..].find('>') {
            Some(p) => start + p,
            None => {
                i = s.len();
                break;
            }
        };
        i = if s[start..=tag_end].ends_with("/>") {
            tag_end + 1
        } else {
            lower[tag_end..]
                .find(&close)
                .map(|p| tag_end + p + close.len())
                .unwrap_or(s.len())
        };
    }
    out.push_str(&s[i.min(s.len())..]);
    out
}

/// The words of a `[[link]]`, `[url label]` or `''emphasis''` and nothing of its syntax.
fn inline(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("]]") else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let inner = &rest[start + 2..start + end];
        let lower = inner.to_lowercase();
        let hidden = ["file:", "image:", "category:", "media:"]
            .iter()
            .any(|p| lower.starts_with(p));
        if !hidden {
            out.push_str(inner.rsplit('|').next().unwrap_or(inner));
        }
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    // [http://x label] → label
    let mut plain = String::new();
    let mut rest = out.as_str();
    while let Some(start) = rest.find('[') {
        plain.push_str(&rest[..start]);
        match rest[start..].find(']') {
            Some(end) => {
                let inner = &rest[start + 1..start + end];
                if inner.starts_with("http") || inner.starts_with("//") {
                    plain.push_str(inner.split_once(' ').map_or(inner, |(_, label)| label));
                } else {
                    plain.push_str(&rest[start..=start + end]);
                }
                rest = &rest[start + end + 1..];
            }
            None => {
                plain.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    plain.push_str(rest);
    plain
        .replace("'''''", "")
        .replace("'''", "")
        .replace("''", "")
}

/// Wikitext as readable Markdown.
pub fn wikitext(src: &str) -> String {
    let mut s = strip_nested(src, "{{", "}}");
    s = strip_nested(&s, "<!--", "-->");
    for tag in [
        "ref",
        "gallery",
        "math",
        "nowiki",
        "noinclude",
        "references",
    ] {
        s = strip_tag(&s, tag);
    }
    let s = s
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    // Any tag that is left carries no words.
    let mut plain = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => plain.push(c),
            _ => {}
        }
    }
    let mut out = String::new();
    let mut in_table = false;
    for raw in plain.lines() {
        let line = raw.trim();
        if line.starts_with("{|") {
            in_table = true;
            continue;
        }
        if in_table {
            if line.starts_with("|}") {
                in_table = false;
                out.push('\n');
            } else if let Some(cells) = line
                .strip_prefix('|')
                .or_else(|| line.strip_prefix('!'))
                .filter(|c| !c.starts_with('-') && !c.starts_with('+'))
            {
                let cells: Vec<String> = cells
                    .split("||")
                    .flat_map(|c| c.split("!!"))
                    // `style="..." | text`: what follows the last bar is the cell's text.
                    .map(|c| inline(c.rsplit('|').next().unwrap_or(c)).trim().to_string())
                    .collect();
                out.push_str(&format!("| {} |\n", cells.join(" | ")));
            }
            continue;
        }
        if line.starts_with('=') && line.ends_with('=') && line.len() > 2 {
            let level = line.chars().take_while(|c| *c == '=').count().min(6);
            let words = line.trim_matches('=').trim();
            if !words.is_empty() {
                // `= Title =` is the page's own title, which is already the document's.
                out.push_str(&format!(
                    "\n{} {}\n\n",
                    "#".repeat(level.max(2)),
                    inline(words)
                ));
            }
        } else if let Some(item) = line.strip_prefix('*') {
            let depth = 1 + item.chars().take_while(|c| *c == '*').count();
            out.push_str(&format!(
                "{}- {}\n",
                "  ".repeat(depth - 1),
                inline(item.trim_start_matches('*').trim())
            ));
        } else if let Some(item) = line.strip_prefix('#') {
            out.push_str(&format!(
                "1. {}\n",
                inline(item.trim_start_matches('#').trim())
            ));
        } else if line.starts_with("__") || line.starts_with("----") {
            continue;
        } else if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&inline(line.trim_start_matches(':')));
            out.push('\n');
        }
    }
    crate::convert::tidy(&out)
}
