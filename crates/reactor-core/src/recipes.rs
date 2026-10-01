//! Package managers and install recipes (ADR-0010).
//!
//! A recipe is only ever a *candidate* when its key names a manager that is
//! declared, matches this OS, and whose binary is on `PATH`. Everything else —
//! free-text keys like `manual`, a URL, a manager that is not here — is a note:
//! shown, never executed.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::catalogue::{Catalogue, OrderedMap, Tool};
use crate::util::which;

/// This platform, spelled the way Python's `sys.platform` spells it — that is
/// what `[platform.manager.*].os` in tools.toml is written against.
pub fn sys_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// Declared managers whose binary is here and whose os matches. → id → path.
pub fn available_managers(cat: &Catalogue) -> BTreeMap<String, String> {
    let platform = sys_platform();
    let mut out = BTreeMap::new();
    for (mid, m) in &cat.managers {
        if m.os.as_deref().is_some_and(|os| !platform.starts_with(os)) {
            continue;
        }
        if let Some(found) = which(&m.binary) {
            out.insert(mid.clone(), found.display().to_string());
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Recipe {
    pub method: String,
    pub command: String,
    pub sudo: bool,
}

impl Recipe {
    pub fn text(&self) -> String {
        format!("{}{}", if self.sudo { "sudo " } else { "" }, self.command)
    }
}

/// → (ranked runnable candidates, free-text notes). See ADR-0010.
pub fn rank_recipes(
    tool: &Tool,
    cat: &Catalogue,
    present: &BTreeMap<String, String>,
) -> (Vec<Recipe>, OrderedMap<String>) {
    let mut candidates = Vec::new();
    let mut notes = OrderedMap::default();
    for (key, command) in tool.install.iter() {
        if let (true, Some(m)) = (present.contains_key(key), cat.managers.get(key)) {
            candidates.push(Recipe {
                method: key.clone(),
                command: command.clone(),
                sudo: m.sudo,
            });
        } else {
            // Includes a known manager that is simply not on this machine: still
            // a note, so `reactor tools show` can say what would work elsewhere.
            notes.0.push((key.clone(), command.clone()));
        }
    }
    let order = |method: &str| {
        cat.prefer
            .iter()
            .position(|p| p == method)
            .unwrap_or(cat.prefer.len())
    };
    let declared = |method: &str| {
        tool.install
            .keys()
            .position(|k| k == method)
            .unwrap_or(usize::MAX)
    };
    candidates.sort_by_key(|c| (order(&c.method), declared(&c.method)));
    (candidates, notes)
}

/// `shlex.split` for the recipe strings tools.toml carries: POSIX quoting,
/// no expansion.
pub fn shlex_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            '\'' => {
                have = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '"' => {
                have = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => match chars.peek() {
                            Some(&n) if matches!(n, '"' | '\\' | '$' | '`') => {
                                cur.push(n);
                                chars.next();
                            }
                            _ => cur.push('\\'),
                        },
                        _ => cur.push(q),
                    }
                }
            }
            '\\' => {
                have = true;
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            _ => {
                have = true;
                cur.push(c);
            }
        }
    }
    if have {
        out.push(cur);
    }
    out
}

/// `shlex.join`: quote only what needs it.
pub fn shlex_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| shlex_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = |c: char| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c);
    if s.chars().all(safe) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}
