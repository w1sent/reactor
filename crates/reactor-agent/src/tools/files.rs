//! `read`, `write`, `edit`: plain file access relative to the working directory.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{BoxFut, Tool, ToolCtx, ToolOutput, str_arg, usize_arg};
use crate::llm::ToolSpec;

fn resolve(cwd: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Lines a `read` returns unless asked for more or fewer.
const DEFAULT_LINES: usize = 2_000;

pub struct Read;

impl Tool for Read {
    fn prints_much(&self) -> bool {
        true
    }
    fn guidelines(&self) -> &'static [&'static str] {
        &["Use `read` to look at a text file, not `cat` or `sed`."]
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: format!(
                "Read a text file. Returns up to {DEFAULT_LINES} lines; a longer file ends with a note giving the offset to continue from. Binary files: use bash (xxd, strings, objdump)."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to read, relative to the working directory or absolute." },
                    "offset": { "type": "integer", "description": "Line to start from (1-based, default 1)." },
                    "limit": { "type": "integer", "description": format!("Maximum number of lines (default {DEFAULT_LINES}).") }
                },
                "required": ["path"]
            }),
        }
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(p) = str_arg(&args, "path") else {
                return ToolOutput::err("read needs a `path`");
            };
            let path = resolve(&ctx.cwd, p);
            let bytes = match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(e) => return ToolOutput::err(format!("{}: {e}", path.display())),
            };
            if bytes.contains(&0) {
                return ToolOutput::err(format!(
                    "{}: looks binary ({} bytes); use bash (xxd, strings, file, objdump)",
                    path.display(),
                    bytes.len()
                ));
            }
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            let start = usize_arg(&args, "offset").unwrap_or(1).max(1) - 1;
            if start >= lines.len() && !lines.is_empty() {
                return ToolOutput::err(format!(
                    "{}: {} lines; offset {} is past the end",
                    path.display(),
                    lines.len(),
                    start + 1
                ));
            }
            let end = (start + usize_arg(&args, "limit").unwrap_or(DEFAULT_LINES)).min(lines.len());
            let mut out = lines[start.min(lines.len())..end].join("\n");
            if end < lines.len() {
                out.push_str(&format!(
                    "\n… [{} more line(s); continue with offset {}]",
                    lines.len() - end,
                    end + 1
                ));
            }
            ToolOutput::ok(out)
        })
    }
}

pub struct Write;

impl Tool for Write {
    fn guidelines(&self) -> &'static [&'static str] {
        &["Use `write` for a new file or a complete rewrite; change part of a file with `edit`."]
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".into(),
            description:
                "Write a text file. Creates parent directories and replaces any existing file."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to write, relative to the working directory or absolute." },
                    "content": { "type": "string", "description": "The whole new content of the file." }
                },
                "required": ["path", "content"]
            }),
        }
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let (Some(p), Some(content)) = (str_arg(&args, "path"), str_arg(&args, "content"))
            else {
                return ToolOutput::err("write needs `path` and `content`");
            };
            let path = resolve(&ctx.cwd, p);
            if let Some(dir) = path.parent()
                && let Err(e) = tokio::fs::create_dir_all(dir).await
            {
                return ToolOutput::err(format!("{}: {e}", dir.display()));
            }
            match tokio::fs::write(&path, content).await {
                Ok(()) => ToolOutput::ok(format!(
                    "wrote {} bytes to {}",
                    content.len(),
                    path.display()
                )),
                Err(e) => ToolOutput::err(format!("{}: {e}", path.display())),
            }
        })
    }
}

pub struct Edit;

/// One replacement of an `edit` call.
struct Replacement {
    old: String,
    new: String,
    all: bool,
}

/// The replacements an `edit` call asks for. `edits` is the form the definition shows; a
/// call that names one `old_string` and `new_string` at the top level -- the older shape --
/// is read as one more.
fn replacements(args: &Value) -> Result<Vec<Replacement>, String> {
    let one = |v: &Value, at: String| -> Result<Replacement, String> {
        let (Some(old), Some(new)) = (str_arg(v, "old_string"), str_arg(v, "new_string")) else {
            return Err(format!("{at}needs `old_string` and `new_string`"));
        };
        Ok(Replacement {
            old: old.to_string(),
            new: new.to_string(),
            all: v
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    };
    let mut out = Vec::new();
    match args.get("edits") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            for (i, e) in a.iter().enumerate() {
                out.push(one(e, format!("edit {}: ", i + 1))?);
            }
        }
        Some(_) => return Err("`edits` is a list of {old_string, new_string}".into()),
    }
    if args.get("old_string").is_some() || args.get("new_string").is_some() {
        out.push(one(args, String::new())?);
    }
    if out.is_empty() {
        return Err("edit needs `edits`: a list of {old_string, new_string}".into());
    }
    Ok(out)
}

/// Apply replacements to `text`, each matched against the text as it was: where they land
/// is decided before any is made, so an earlier one cannot change what a later one finds.
/// Nothing is applied unless every one is.
fn apply_replacements(text: &str, edits: &[Replacement]) -> Result<String, String> {
    let numbered = |i: usize| {
        if edits.len() > 1 {
            format!("edit {}: ", i + 1)
        } else {
            String::new()
        }
    };
    // (start, end, edit index)
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    for (i, e) in edits.iter().enumerate() {
        if e.old.is_empty() {
            return Err(format!("{}old_string is empty", numbered(i)));
        }
        if e.old == e.new {
            return Err(format!(
                "{}old_string and new_string are the same",
                numbered(i)
            ));
        }
        let at: Vec<usize> = text.match_indices(&e.old).map(|(p, _)| p).collect();
        match (at.len(), e.all) {
            (0, _) => return Err(format!("{}old_string not found", numbered(i))),
            (1, _) | (_, true) => {
                spans.extend(at.into_iter().map(|p| (p, p + e.old.len(), i)));
            }
            (n, false) => {
                return Err(format!(
                    "{}old_string matches {n} times; add context so it matches once, or set replace_all",
                    numbered(i)
                ));
            }
        }
    }
    spans.sort_unstable();
    for w in spans.windows(2) {
        if w[1].0 < w[0].1 {
            return Err(format!(
                "edit {} and edit {} overlap; merge them into one edit",
                w[0].2 + 1,
                w[1].2 + 1
            ));
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    for (start, end, i) in spans {
        out.push_str(&text[from..start]);
        out.push_str(&edits[i].new);
        from = end;
    }
    out.push_str(&text[from..]);
    Ok(out)
}

impl Tool for Edit {
    fn guidelines(&self) -> &'static [&'static str] {
        &[
            "Make every change to one file in one `edit` call, with an entry in `edits` for each.",
            "Keep each `old_string` as short as is still unique; do not pad it with unchanged lines.",
        ]
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".into(),
            description: "Edit a file by exact text replacement. Every replacement is matched against the file as it was before the edit, must match once (or set `replace_all`), and must not overlap another. If one fails, nothing is written.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to edit, relative to the working directory or absolute." },
                    "edits": {
                        "type": "array",
                        "description": "The replacements. Merge changes to nearby lines into one.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_string": { "type": "string", "description": "Exact text to replace." },
                                "new_string": { "type": "string", "description": "What to put there." },
                                "replace_all": { "type": "boolean", "description": "Replace every occurrence (default false: it must be unique)." }
                            },
                            "required": ["old_string", "new_string"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
        }
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(p) = str_arg(&args, "path") else {
                return ToolOutput::err("edit needs a `path`");
            };
            let edits = match replacements(&args) {
                Ok(e) => e,
                Err(e) => return ToolOutput::err(e),
            };
            let path = resolve(&ctx.cwd, p);
            let text = match tokio::fs::read_to_string(&path).await {
                Ok(t) => t,
                Err(e) => return ToolOutput::err(format!("{}: {e}", path.display())),
            };
            let edited = match apply_replacements(&text, &edits) {
                Ok(t) => t,
                Err(e) => return ToolOutput::err(format!("{}: {e}", path.display())),
            };
            match tokio::fs::write(&path, edited).await {
                Ok(()) if edits.len() > 1 => ToolOutput::ok(format!(
                    "edited {} ({} replacements)",
                    path.display(),
                    edits.len()
                )),
                Ok(()) => ToolOutput::ok(format!("edited {}", path.display())),
                Err(e) => ToolOutput::err(format!("{}: {e}", path.display())),
            }
        })
    }
}
