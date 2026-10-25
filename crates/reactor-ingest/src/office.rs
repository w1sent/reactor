//! PDF, Word, OpenDocument and EPUB: the text of the file, with its headings, lists and
//! tables kept as Markdown where the format says what they are.

use std::io::{Cursor, Read};

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::convert::{Error, Result};
use crate::xml::{local, push_ref};

/// The text of a PDF. Only what is *text* in it: a scanned page has none.
pub fn pdf(bytes: &[u8]) -> Result<String> {
    // The parser has panicked on damaged files; that is a damaged file, not a crash.
    let bytes = bytes.to_vec();
    let read = std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem(&bytes));
    match read {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(Error::Unreadable(format!("not a readable PDF: {e}"))),
        Err(_) => Err(Error::Unreadable("not a readable PDF".into())),
    }
}

/// The most one part of a document is read as: a few hundred bytes of archive can claim to
/// unpack to gigabytes, and the text of a real document is a small fraction of this.
const MAX_ENTRY: u64 = 256 << 20;

fn read_entry(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>> {
    let file = zip
        .by_name(name)
        .map_err(|_| Error::Unreadable(format!("no {name} in the file")))?;
    let mut out = Vec::new();
    file.take(MAX_ENTRY + 1)
        .read_to_end(&mut out)
        .map_err(|e| Error::Unreadable(format!("{name}: {e}")))?;
    if out.len() as u64 > MAX_ENTRY {
        return Err(Error::Unreadable(format!(
            "{name}: over {} MiB unpacked",
            MAX_ENTRY >> 20
        )));
    }
    Ok(out)
}

fn open(bytes: &[u8]) -> Result<zip::ZipArchive<Cursor<&[u8]>>> {
    zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| Error::Unreadable(format!("not a readable archive: {e}")))
}

/// What a block of the document is, for the Markdown that comes out.
enum Block {
    Paragraph,
    Heading(usize),
    Item,
}

fn emit(out: &mut String, block: &Block, text: &str) {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return;
    }
    match block {
        Block::Paragraph => out.push_str(&format!("{text}\n\n")),
        Block::Heading(level) => out.push_str(&format!("{} {text}\n\n", "#".repeat(*level))),
        Block::Item => out.push_str(&format!("- {text}\n")),
    }
}

/// A table row as Markdown, set off from a list or paragraph before it.
fn row_line(out: &mut String, row: &[String]) {
    if row.iter().all(|c| c.is_empty()) {
        return;
    }
    if !out.is_empty() && !out.ends_with("|\n") && !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(&format!("| {} |\n", row.join(" | ")));
}

/// A Word document: paragraphs, headings (by style name), list items, and tables as
/// Markdown rows.
pub fn docx(bytes: &[u8]) -> Result<String> {
    let mut zip = open(bytes)?;
    let xml = read_entry(&mut zip, "word/document.xml")?;
    let mut reader = Reader::from_reader(xml.as_slice());
    let (mut out, mut buf) = (String::new(), Vec::new());
    let (mut text, mut style, mut listed) = (String::new(), String::new(), false);
    let (mut in_text, mut depth_cell) = (false, 0usize);
    let (mut row, mut cell): (Vec<String>, String) = (Vec::new(), String::new());
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match local(e.local_name()).as_str() {
                "p" => {
                    text.clear();
                    style.clear();
                    listed = false;
                }
                "pStyle" => {
                    for a in e.attributes().flatten() {
                        if local(a.key.local_name()) == "val" {
                            style = a.value.to_string();
                        }
                    }
                }
                "numPr" => listed = true,
                "t" => in_text = true,
                "tab" => text.push('\t'),
                "br" | "cr" => text.push(' '),
                "tc" => {
                    depth_cell += 1;
                    cell.clear();
                }
                _ => {}
            },
            Ok(Event::Text(t)) if in_text => text.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if in_text => push_ref(&mut text, &r),
            Ok(Event::End(e)) => match local(e.local_name()).as_str() {
                "t" => in_text = false,
                "p" if depth_cell > 0 => {
                    if !cell.is_empty() {
                        cell.push(' ');
                    }
                    cell.push_str(text.trim());
                }
                "p" => {
                    let lower = style.to_lowercase();
                    let block = if lower == "title" {
                        Block::Heading(1)
                    } else if let Some(n) = lower.strip_prefix("heading") {
                        Block::Heading(n.trim().parse::<usize>().unwrap_or(1).clamp(1, 6))
                    } else if listed || lower.contains("list") {
                        Block::Item
                    } else {
                        Block::Paragraph
                    };
                    if matches!(block, Block::Item) {
                        emit(&mut out, &block, &text);
                    } else {
                        // A list ends with a blank line before whatever follows.
                        if out.ends_with("\n") && !out.ends_with("\n\n") {
                            out.push('\n');
                        }
                        emit(&mut out, &block, &text);
                    }
                }
                "tc" => {
                    depth_cell = depth_cell.saturating_sub(1);
                    row.push(cell.replace('|', "\\|").trim().to_string());
                    cell.clear();
                }
                "tr" => {
                    row_line(&mut out, &row);
                    row.clear();
                }
                "tbl" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(Error::Unreadable(format!("document.xml: {e}"))),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// An OpenDocument text file: headings by outline level, paragraphs, list items and
/// table rows.
pub fn odt(bytes: &[u8]) -> Result<String> {
    let mut zip = open(bytes)?;
    let xml = read_entry(&mut zip, "content.xml")?;
    let mut reader = Reader::from_reader(xml.as_slice());
    let (mut out, mut buf) = (String::new(), Vec::new());
    let (mut text, mut block, mut depth, mut in_item) =
        (String::new(), None::<Block>, 0usize, false);
    let (mut row, mut cell): (Vec<String>, String) = (Vec::new(), String::new());
    let mut in_cell = 0usize;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local(e.local_name()).as_str() {
                "h" | "p" => {
                    depth += 1;
                    if depth == 1 {
                        text.clear();
                        block = Some(if local(e.local_name()) == "h" {
                            let level = e
                                .attributes()
                                .flatten()
                                .find(|a| local(a.key.local_name()) == "outline-level")
                                .and_then(|a| a.value.parse().ok())
                                .unwrap_or(1usize);
                            Block::Heading(level.clamp(1, 6))
                        } else if in_item {
                            Block::Item
                        } else {
                            Block::Paragraph
                        });
                    }
                }
                "list-item" => in_item = true,
                "table-cell" => {
                    in_cell += 1;
                    cell.clear();
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => match local(e.local_name()).as_str() {
                "s" | "tab" | "line-break" if depth > 0 => text.push(' '),
                _ => {}
            },
            Ok(Event::Text(t)) if depth > 0 => text.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if depth > 0 => push_ref(&mut text, &r),
            Ok(Event::End(e)) => match local(e.local_name()).as_str() {
                "h" | "p" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0
                        && let Some(b) = block.take()
                    {
                        if in_cell > 0 {
                            if !cell.is_empty() {
                                cell.push(' ');
                            }
                            cell.push_str(text.trim());
                        } else {
                            if !matches!(b, Block::Item)
                                && out.ends_with('\n')
                                && !out.ends_with("\n\n")
                            {
                                out.push('\n');
                            }
                            emit(&mut out, &b, &text);
                        }
                    }
                }
                "list-item" => in_item = false,
                "table-cell" => {
                    in_cell = in_cell.saturating_sub(1);
                    row.push(cell.replace('|', "\\|").trim().to_string());
                    cell.clear();
                }
                "table-row" => {
                    row_line(&mut out, &row);
                    row.clear();
                }
                "table" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(Error::Unreadable(format!("content.xml: {e}"))),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// An EPUB: its pages in name order, each as a web page. The title is the package's.
pub fn epub(bytes: &[u8]) -> Result<(Option<String>, String)> {
    let mut zip = open(bytes)?;
    let mut pages: Vec<String> = zip
        .file_names()
        .filter(|n| {
            let n = n.to_lowercase();
            n.ends_with(".xhtml") || n.ends_with(".html") || n.ends_with(".htm")
        })
        .map(str::to_string)
        .collect();
    pages.sort();
    let opf = zip
        .file_names()
        .find(|n| n.ends_with(".opf"))
        .map(str::to_string);
    let title = opf.and_then(|name| {
        let xml = String::from_utf8_lossy(&read_entry(&mut zip, &name).ok()?).into_owned();
        let start = xml.find("<dc:title")?;
        let open_end = xml[start..].find('>')? + start + 1;
        let close = xml[open_end..].find("</dc:title>")? + open_end;
        Some(xml[open_end..close].trim().to_string()).filter(|t| !t.is_empty())
    });
    let mut out = String::new();
    for name in pages {
        let page = crate::html::to_markdown(&crate::convert::decode(&read_entry(&mut zip, &name)?));
        out.push_str(&page.markdown);
        out.push_str("\n\n");
    }
    Ok((title, out))
}
