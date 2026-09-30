//! `read`, `write`, `edit`: plain file access relative to the working directory.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{BoxFut, Tool, ToolCtx, ToolOutput, str_arg, usize_arg};
use crate::llm::ToolSpec;

fn resolve(cwd: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() { path.to_path_buf() } else { cwd.join(path) }
}

/// Lines a `read` returns unless asked for more or fewer.
const DEFAULT_LINES: usize = 2_000;

pub struct Read;

impl Tool for Read {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: format!("Read a text file. Returns up to {DEFAULT_LINES} lines from `offset` (1-based); a longer file ends with a note giving the offset to continue from. For binaries use bash (xxd, strings, objdump)."),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "offset": { "type": "integer", "description": "First line, 1-based." },
                    "limit": { "type": "integer", "description": "How many lines." }
                },
                "required": ["path"]
            }),
        }
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(p) = str_arg(&args, "path") else { return ToolOutput::err("read needs a `path`") };
            let path = resolve(&ctx.cwd, p);
            let bytes = match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(e) => return ToolOutput::err(format!("{}: {e}", path.display())),
            };
            if bytes.contains(&0) {
                return ToolOutput::err(format!("{}: looks binary ({} bytes); use bash (xxd, strings, file, objdump)", path.display(), bytes.len()));
            }
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            let start = usize_arg(&args, "offset").unwrap_or(1).max(1) - 1;
            if start >= lines.len() && !lines.is_empty() {
                return ToolOutput::err(format!("{}: {} lines; offset {} is past the end", path.display(), lines.len(), start + 1));
            }
            let end = (start + usize_arg(&args, "limit").unwrap_or(DEFAULT_LINES)).min(lines.len());
            let mut out = lines[start.min(lines.len())..end].join("\n");
            if end < lines.len() {
                out.push_str(&format!("\n… [{} more line(s); continue with offset {}]", lines.len() - end, end + 1));
            }
            ToolOutput::ok(out)
        })
    }
}

pub struct Write;

impl Tool for Write {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".into(),
            description: "Write a text file, creating parent directories and replacing any existing file.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string" }, "content": { "type": "string" } },
                "required": ["path", "content"]
            }),
        }
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let (Some(p), Some(content)) = (str_arg(&args, "path"), str_arg(&args, "content")) else {
                return ToolOutput::err("write needs `path` and `content`");
            };
            let path = resolve(&ctx.cwd, p);
            if let Some(dir) = path.parent()
                && let Err(e) = tokio::fs::create_dir_all(dir).await
            {
                return ToolOutput::err(format!("{}: {e}", dir.display()));
            }
            match tokio::fs::write(&path, content).await {
                Ok(()) => ToolOutput::ok(format!("wrote {} bytes to {}", content.len(), path.display())),
                Err(e) => ToolOutput::err(format!("{}: {e}", path.display())),
            }
        })
    }
}

pub struct Edit;

impl Tool for Edit {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".into(),
            description: "Replace one exact occurrence of `old_string` with `new_string` in a file. It must match exactly once; add surrounding context to disambiguate.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string" }, "old_string": { "type": "string" }, "new_string": { "type": "string" } },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let (Some(p), Some(old), Some(new)) = (str_arg(&args, "path"), str_arg(&args, "old_string"), str_arg(&args, "new_string")) else {
                return ToolOutput::err("edit needs `path`, `old_string` and `new_string`");
            };
            let path = resolve(&ctx.cwd, p);
            let text = match tokio::fs::read_to_string(&path).await {
                Ok(t) => t,
                Err(e) => return ToolOutput::err(format!("{}: {e}", path.display())),
            };
            if old.is_empty() {
                return ToolOutput::err("old_string is empty");
            }
            match text.matches(old).count() {
                0 => ToolOutput::err(format!("{}: old_string not found", path.display())),
                1 => match tokio::fs::write(&path, text.replacen(old, new, 1)).await {
                    Ok(()) => ToolOutput::ok(format!("edited {}", path.display())),
                    Err(e) => ToolOutput::err(format!("{}: {e}", path.display())),
                },
                n => ToolOutput::err(format!("{}: old_string matches {n} times; add context so it matches once", path.display())),
            }
        })
    }
}
