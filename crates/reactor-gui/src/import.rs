//! Importing context documents into a case
//! ([ADR-0046](../../../docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)):
//! files, folders and web pages, converted, cut into passages and stored.
//!
//! Only the user does this — in the window, or with `reactor-gui ingest` — never the agent.
//! It is safe to repeat: a document whose source and content are already in the case is
//! skipped, so running a folder again adds only what is new, and a source whose content
//! changed replaces its old version.
//!
//! Nothing here needs a window, so the same code runs headless.

use std::path::{Path, PathBuf};

use reactor_db::Case;
use reactor_db::corpus::{self, Added, NewDocument};

/// What to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A file, named in the corpus by `label`.
    File {
        path: PathBuf,
        label: String,
    },
    Url(String),
}

impl Item {
    pub fn label(&self) -> &str {
        match self {
            Item::File { label, .. } => label,
            Item::Url(u) => u,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Stored { chunks: usize, replaced: usize },
    Unchanged,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    /// The source as the corpus names it, with a fragment for one page of a dump.
    pub source: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub stored: usize,
    pub unchanged: usize,
    pub replaced: usize,
    pub passages: usize,
    pub failures: Vec<(String, String)>,
}

fn is_url(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// Forward slashes, always: a source is named the same way on every system.
fn slashed(p: &Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        // Hidden files and folders (`.git`, `.reactor`) are not documents.
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&path, out)?,
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
    Ok(())
}

/// What a command line or a chooser means: files, the files under folders, and URLs, each
/// with the name the corpus will know it by — relative to the case where it is inside it,
/// else to the folder it was imported from, never an absolute path.
pub fn plan(case: &Case, args: &[String]) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    for arg in args {
        if is_url(arg) {
            items.push(Item::Url(arg.clone()));
            continue;
        }
        let path = std::fs::canonicalize(arg).map_err(|e| format!("{arg}: {e}"))?;
        let inside = path.strip_prefix(case.root()).ok().map(Path::to_path_buf);
        if path.is_dir() {
            let mut files = Vec::new();
            walk(&path, &mut files).map_err(|e| format!("{arg}: {e}"))?;
            let top = path.file_name().map(PathBuf::from).unwrap_or_default();
            for file in files {
                let label = match file.strip_prefix(case.root()) {
                    Ok(rel) => slashed(rel),
                    Err(_) => slashed(&top.join(file.strip_prefix(&path).unwrap_or(&file))),
                };
                items.push(Item::File { path: file, label });
            }
        } else {
            let label = match inside {
                Some(rel) => slashed(&rel),
                None => path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            };
            items.push(Item::File { path, label });
        }
    }
    // The case's own files are never documents.
    items.retain(|i| match i {
        Item::File { label, .. } => !label.starts_with(".reactor/"),
        Item::Url(_) => true,
    });
    Ok(items)
}

fn store(
    case: &Case,
    label: &str,
    kind: &str,
    doc: reactor_ingest::Converted,
    raw: Option<&[u8]>,
    fetched: Option<(String, Option<String>)>,
) -> Done {
    let source = match &doc.fragment {
        Some(f) => format!("{label}#{f}"),
        None => label.to_string(),
    };
    let chunks = reactor_ingest::chunk(&doc.markdown);
    if chunks.is_empty() {
        return Done {
            source,
            outcome: Outcome::Failed("no text in it".into()),
        };
    }
    let kind = if doc.mime == "text/x-wiki" {
        "wiki"
    } else {
        kind
    };
    let meta = match &fetched {
        Some((_, Some(ct))) => serde_json::json!({ "content_type": ct }),
        _ => serde_json::json!({}),
    };
    let new = NewDocument {
        source_uri: source.clone(),
        source_kind: kind.to_string(),
        mime: Some(doc.mime),
        title: doc.title,
        origin: corpus::ORIGIN_EXTERNAL.into(),
        fetched_at: fetched.map(|(at, _)| at),
        meta,
        // A document that is one of many keeps its Markdown as its source.
        raw: raw
            .map(<[u8]>::to_vec)
            .unwrap_or_else(|| doc.markdown.clone().into_bytes()),
        markdown: doc.markdown,
        chunker: reactor_ingest::CHUNKER.into(),
        chunks: chunks
            .into_iter()
            .map(|c| (c.heading_path, c.text))
            .collect(),
    };
    let outcome = match corpus::add_document(case, new) {
        Ok(Added::Stored {
            chunks, replaced, ..
        }) => Outcome::Stored { chunks, replaced },
        Ok(Added::Unchanged { .. }) => Outcome::Unchanged,
        Err(e) => Outcome::Failed(e.to_string()),
    };
    Done { source, outcome }
}

/// Import one item, reporting each document it yields.
pub fn import_item(case: &Case, item: &Item, on: &mut dyn FnMut(Done)) {
    match item {
        Item::File { path, label } => {
            let mut any = false;
            let result = reactor_ingest::convert_file(path, &mut |doc, raw| {
                any = true;
                on(store(case, label, "file", doc, raw, None));
                Ok(())
            });
            if let Err(e) = result {
                on(Done {
                    source: label.clone(),
                    outcome: Outcome::Failed(e.to_string()),
                });
            }
            let _ = any;
        }
        Item::Url(url) => {
            let fetched = match reactor_ingest::fetch(url) {
                Ok(f) => f,
                Err(e) => {
                    on(Done {
                        source: url.clone(),
                        outcome: Outcome::Failed(e.to_string()),
                    });
                    return;
                }
            };
            match reactor_ingest::convert(&fetched.url, &fetched.bytes) {
                Ok(docs) => {
                    let alone = docs.len() == 1;
                    for doc in docs {
                        on(store(
                            case,
                            &fetched.url,
                            "url",
                            doc,
                            alone.then_some(fetched.bytes.as_slice()),
                            Some((
                                reactor_db::time::now_rfc3339(),
                                fetched.content_type.clone(),
                            )),
                        ));
                    }
                }
                Err(e) => on(Done {
                    source: url.clone(),
                    outcome: Outcome::Failed(e.to_string()),
                }),
            }
        }
    }
}

/// Import everything, calling `on(done, item index, item count)` as documents are stored.
pub fn run(case: &Case, items: &[Item], on: &mut dyn FnMut(&Done, usize, usize)) -> Summary {
    let mut summary = Summary::default();
    for (i, item) in items.iter().enumerate() {
        import_item(case, item, &mut |done| {
            match &done.outcome {
                Outcome::Stored { chunks, replaced } => {
                    summary.stored += 1;
                    summary.passages += chunks;
                    summary.replaced += replaced;
                }
                Outcome::Unchanged => summary.unchanged += 1,
                Outcome::Failed(why) => summary.failures.push((done.source.clone(), why.clone())),
            }
            on(&done, i, items.len());
        });
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case() -> (tempfile::TempDir, Case) {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), None).unwrap();
        (dir, case)
    }

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }

    fn run_all(case: &Case, args: &[String]) -> Summary {
        let items = plan(case, args).unwrap();
        run(case, &items, &mut |_, _, _| {})
    }

    fn args(paths: &[&Path]) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_folder_is_imported_by_relative_names_and_repeating_it_adds_only_what_is_new() {
        let (_c, case) = case();
        let outside = tempfile::tempdir().unwrap();
        let wiki = outside.path().join("wiki");
        write(&wiki, "emotet.md", "# Emotet\n\nA banking trojan.");
        write(&wiki, "tools/ghidra.txt", "Ghidra is a disassembler.");
        write(&wiki, ".git/config", "not a document");
        write(&wiki, ".hidden.md", "# hidden");
        write(&wiki, "picture.png", "\u{0}PNG");

        let first = run_all(&case, &args(&[&wiki]));
        assert_eq!((first.stored, first.unchanged), (2, 0));
        assert_eq!(first.failures.len(), 1, "{:?}", first.failures);
        assert!(
            first.failures[0].0.ends_with("picture.png") && first.failures[0].1.contains("binary")
        );
        let names: Vec<String> = corpus::documents(case.db())
            .unwrap()
            .into_iter()
            .map(|d| d.source_uri)
            .collect();
        // Named from the folder down, never by an absolute path.
        assert!(names.contains(&"wiki/emotet.md".to_string()), "{names:?}");
        assert!(
            names.contains(&"wiki/tools/ghidra.txt".to_string()),
            "{names:?}"
        );
        assert!(names.iter().all(|n| !n.starts_with('/')));

        // Again: nothing is written. Then one changes and one is new.
        let again = run_all(&case, &args(&[&wiki]));
        assert_eq!((again.stored, again.unchanged), (0, 2));
        write(
            &wiki,
            "emotet.md",
            "# Emotet\n\nA banking trojan, since 2014.",
        );
        write(&wiki, "new.md", "# New\n\nfresh");
        let third = run_all(&case, &args(&[&wiki]));
        assert_eq!((third.stored, third.unchanged, third.replaced), (2, 1, 1));
        assert_eq!(corpus::counts(case.db()).unwrap().0, 3);
    }

    #[test]
    fn a_file_inside_the_case_is_named_relative_to_it_and_the_cases_own_files_are_left_alone() {
        let (dir, case) = case();
        write(dir.path(), "reference/vendor.md", "# Vendor\n\nnotes");
        let items = plan(&case, &args(&[dir.path()])).unwrap();
        let labels: Vec<&str> = items.iter().map(Item::label).collect();
        assert_eq!(labels, ["reference/vendor.md"], "{labels:?}");
        // `.reactor/` is never walked, and not even when named.
        assert!(
            plan(&case, &args(&[&case.dir().join("case.json")]))
                .unwrap()
                .is_empty()
        );
        assert!(plan(&case, &["nope/missing.md".to_string()]).is_err());
    }

    #[test]
    fn a_wiki_dump_is_one_document_per_page_named_by_fragment() {
        let (_c, case) = case();
        let outside = tempfile::tempdir().unwrap();
        let dump = write(
            outside.path(),
            "export.xml",
            r#"<mediawiki><page><title>A</title><ns>0</ns><revision><text>first page text</text></revision></page>
               <page><title>B</title><ns>0</ns><revision><text>second page text</text></revision></page></mediawiki>"#,
        );
        let s = run_all(&case, &args(&[&dump]));
        assert_eq!(s.stored, 2);
        let docs = corpus::documents(case.db()).unwrap();
        let mut names: Vec<_> = docs.iter().map(|d| d.source_uri.as_str()).collect();
        names.sort();
        assert_eq!(names, ["export.xml#A", "export.xml#B"]);
        // Searchable straight away, by keyword.
        let hits = corpus::search(
            case.db(),
            &corpus::Query {
                text: "second",
                vector: None,
                limit: 5,
            },
        )
        .unwrap();
        assert_eq!(hits[0].source_uri, "export.xml#B");
        assert_eq!(run_all(&case, &args(&[&dump])).unchanged, 2);
    }

    #[test]
    fn a_web_page_is_fetched_converted_and_named_by_its_url() {
        use std::io::{Read, Write};
        let (_c, case) = case();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let Ok((mut s, _)) = listener.accept() else {
                    return;
                };
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let body = "<html><head><title>Advisory</title></head><body><main><h1>Patch now</h1><p>heap overflow in the parser</p></main></body></html>";
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let url = format!("http://{addr}/advisory");
        let s = run_all(&case, std::slice::from_ref(&url));
        assert_eq!((s.stored, s.failures.len()), (1, 0), "{:?}", s.failures);
        let d = &corpus::documents(case.db()).unwrap()[0];
        assert_eq!(
            (d.source_uri.as_str(), d.title.as_deref()),
            (url.as_str(), Some("Advisory"))
        );
        assert_eq!(run_all(&case, std::slice::from_ref(&url)).unchanged, 1);
        // A page that cannot be fetched is reported, and nothing else is affected.
        let bad = run_all(&case, &["http://127.0.0.1:1/nothing".to_string()]);
        assert_eq!(bad.failures.len(), 1);
    }
}
