//! Cutting a document into passages that can be found and quoted.
//!
//! A passage should be about one thing and small enough to read in a result: so cuts fall
//! on Markdown structure first (a heading, a paragraph, a code block, a list) and inside a
//! sentence only when nothing larger fits. Each passage keeps the headings it sits under,
//! so a hit says *where* it is — "Installation > Linux > Packages" — and not just what.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use text_splitter::{ChunkConfig, MarkdownSplitter};

/// Names the way a chunk was cut. A change to how documents are chunked is a new name, so
/// stored passages can be told from what the current code would produce.
pub const CHUNKER: &str = "reactor/markdown/800-2000ch/v1";

/// Smallest and largest passage, in characters: roughly 200 to 500 tokens.
const SIZE: std::ops::Range<usize> = 800..2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Position in the document, from 0.
    pub ord: usize,
    /// `Installation > Linux`: the headings above it, outermost first. Empty before the
    /// first heading.
    pub heading_path: String,
    pub text: String,
}

/// Where each heading starts, how deep it is, and its words.
fn headings(markdown: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut open: Option<(usize, usize, String)> = None;
    for (event, range) in Parser::new_ext(markdown, Options::empty()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let depth = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                open = Some((range.start, depth, String::new()));
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, _, words)) = &mut open {
                    words.push_str(&t);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(h) = open.take() {
                    out.push(h);
                }
            }
            _ => {}
        }
    }
    out
}

/// Cut Markdown into passages, in order.
pub fn chunk(markdown: &str) -> Vec<Chunk> {
    let splitter = MarkdownSplitter::new(ChunkConfig::new(SIZE).with_trim(true));
    let headings = headings(markdown);
    let (mut stack, mut next): (Vec<(usize, String)>, usize) = (Vec::new(), 0);
    let mut out = Vec::new();
    for (offset, text) in splitter.chunk_indices(markdown) {
        if text.trim().is_empty() {
            continue;
        }
        // Headings that start at or before this passage: the ones it sits under. A heading
        // of the same or a higher level closes those below it.
        while next < headings.len() && headings[next].0 <= offset {
            let (_, depth, words) = &headings[next];
            stack.retain(|(d, _)| d < depth);
            stack.push((*depth, words.trim().to_string()));
            next += 1;
        }
        out.push(Chunk {
            ord: out.len(),
            heading_path: stack
                .iter()
                .map(|(_, w)| w.as_str())
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
                .join(" > "),
            text: text.to_string(),
        });
    }
    out
}
