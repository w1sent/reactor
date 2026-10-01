//! Upstream skills REactor fetches next to the catalogue (ADR-0008).
//!
//! `git` and `curl` are the transports — subprocesses, like everything else in
//! this crate. A failed fetch is a warning, never an error.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use crate::catalogue::Tool;
use crate::error::{ReactorError, Result};
use crate::json::{io_reason, write_json_atomic};
use crate::paths::Paths;
use crate::util::{first_line, run, which};

#[derive(Debug, Clone, Serialize)]
pub struct SkillStatus {
    pub source: Option<String>,
    pub path: Option<String>,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub fetched: bool,
    pub dir: String,
    pub commit: Option<String>,
    pub fetched_at: Option<Value>,
}

pub fn skill_status(paths: &Paths, tool: &Tool) -> Option<SkillStatus> {
    let spec = tool.skill.as_ref()?;
    let dir = paths.skills_dir().join(&tool.id);
    let meta: Value = std::fs::read_to_string(dir.join(".reactor-skill.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    Some(SkillStatus {
        source: spec.source.clone(),
        path: spec.path.clone(),
        git_ref: spec.git_ref.clone(),
        fetched: dir.join("SKILL.md").is_file(),
        dir: dir.display().to_string(),
        commit: meta
            .get("commit")
            .and_then(Value::as_str)
            .map(str::to_string),
        fetched_at: meta.get("fetched_at").filter(|v| !v.is_null()).cloned(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchResult {
    pub ok: bool,
    pub tool: String,
    pub message: String,
    /// Present (possibly null) on success, absent on failure — the shape the
    /// Python CLI emitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<Option<String>>,
}

/// Never fails; a failed fetch is a warning.
pub fn fetch_skill(paths: &Paths, tool: &Tool) -> FetchResult {
    let default = Default::default();
    let spec = tool.skill.as_ref().unwrap_or(&default);
    let source = spec.source.clone().unwrap_or_default();
    let dest = paths.skills_dir().join(&tool.id);
    let fail = |message: String| FetchResult {
        ok: false,
        tool: tool.id.clone(),
        message,
        commit: None,
    };

    let info = if let Some(url) = source.strip_prefix("git+") {
        fetch_git(url, spec.git_ref.as_deref(), spec.path.as_deref(), &dest)
    } else if source.starts_with("http://") || source.starts_with("https://") {
        fetch_url(&source, &dest)
    } else {
        return fail(format!("unsupported skill source '{source}'"));
    };
    let commit = match info {
        Ok(c) => c,
        Err(e) => return fail(e.to_string()),
    };

    let meta = json!({
        "source": source,
        "path": spec.path,
        "ref": spec.git_ref,
        "commit": commit,
        "fetched_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    });
    if let Err(e) = write_json_atomic(&dest.join(".reactor-skill.json"), &meta) {
        return fail(e.to_string());
    }
    FetchResult {
        ok: true,
        tool: tool.id.clone(),
        message: dest.display().to_string(),
        commit: Some(commit),
    }
}

fn s(x: &str) -> String {
    x.to_string()
}

fn fetch_git(
    url: &str,
    git_ref: Option<&str>,
    subpath: Option<&str>,
    dest: &Path,
) -> Result<Option<String>> {
    if which("git").is_none() {
        return Err(ReactorError::new("git is not on PATH"));
    }
    let tmp = tempdir("reactor-skill-")?;
    let clone = tmp.join("repo");
    let mut argv = vec![s("git"), s("clone"), s("--quiet"), s("--depth"), s("1")];
    if let Some(r) = git_ref {
        argv.extend([s("--branch"), s(r)]);
    }
    argv.extend([s(url), clone.display().to_string()]);
    let r = run(&argv, Duration::from_secs(120));
    let result = (|| {
        if !r.ok() {
            let why = first_line(&r.output);
            return Err(ReactorError::new(format!(
                "clone failed: {}",
                if why.is_empty() { s("timeout") } else { why }
            )));
        }
        let head = run(
            &[
                s("git"),
                s("-C"),
                clone.display().to_string(),
                s("rev-parse"),
                s("HEAD"),
            ],
            Duration::from_secs(20),
        );
        let src = match subpath {
            Some(p) if !p.is_empty() => clone.join(p),
            _ => clone.clone(),
        };
        if !src.is_dir() {
            return Err(ReactorError::new(format!(
                "path '{}' not found in {url}",
                subpath.unwrap_or("")
            )));
        }
        if !src.join("SKILL.md").is_file() {
            return Err(ReactorError::new(format!(
                "no SKILL.md at {} in {url}",
                subpath.filter(|p| !p.is_empty()).unwrap_or("<repo root>")
            )));
        }
        if dest.exists() {
            std::fs::remove_dir_all(dest)
                .map_err(|e| ReactorError::new(format!("{}: {}", dest.display(), io_reason(&e))))?;
        }
        copy_tree(&src, dest)
            .map_err(|e| ReactorError::new(format!("{}: {}", dest.display(), io_reason(&e))))?;
        let commit = first_line(&head.output);
        Ok((!commit.is_empty()).then_some(commit))
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let (from, to) = (entry.path(), dest.join(&name));
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_tree(&from, &to)?;
        } else if ty.is_symlink() {
            let target = std::fs::read_link(&from)?;
            let _ = std::fs::remove_file(&to);
            std::os::unix::fs::symlink(target, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn fetch_url(url: &str, dest: &Path) -> Result<Option<String>> {
    if which("curl").is_none() {
        return Err(ReactorError::new("download failed: curl is not on PATH"));
    }
    let r = run(
        &[
            s("curl"),
            s("--silent"),
            s("--show-error"),
            s("--fail"),
            s("--location"),
            s("--max-time"),
            s("30"),
            s(url),
        ],
        Duration::from_secs(35),
    );
    if !r.ok() {
        return Err(ReactorError::new(format!(
            "download failed: {}",
            first_line(&r.output)
        )));
    }
    if !r.output.trim_start().starts_with("---") {
        return Err(ReactorError::new(
            "downloaded file has no YAML frontmatter; not a SKILL.md",
        ));
    }
    std::fs::create_dir_all(dest)
        .map_err(|e| ReactorError::new(format!("{}: {}", dest.display(), io_reason(&e))))?;
    std::fs::write(dest.join("SKILL.md"), r.output)
        .map_err(|e| ReactorError::new(format!("{}: {}", dest.display(), io_reason(&e))))?;
    Ok(None)
}

/// The commit `ref` (default HEAD) points at upstream right now, if knowable.
pub fn skill_remote_head(tool: &Tool) -> Option<String> {
    let spec = tool.skill.as_ref()?;
    let url = spec.source.as_deref()?.strip_prefix("git+")?;
    which("git")?;
    let r = run(
        &[
            s("git"),
            s("ls-remote"),
            s(url),
            spec.git_ref.clone().unwrap_or_else(|| s("HEAD")),
        ],
        Duration::from_secs(30),
    );
    if !r.ok() {
        return None;
    }
    first_line(&r.output)
        .split_whitespace()
        .next()
        .map(str::to_string)
}

/// A scratch directory under the system temp dir, removed by the caller.
pub(crate) fn tempdir(prefix: &str) -> Result<PathBuf> {
    let base = std::env::temp_dir();
    for n in 0..1000u32 {
        let p = base.join(format!("{prefix}{}-{n}", std::process::id()));
        match std::fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(ReactorError::new(format!(
                    "{}: {}",
                    p.display(),
                    io_reason(&e)
                )));
            }
        }
    }
    Err(ReactorError::new("could not create a scratch directory"))
}
