//! Skills: instructions the agent reads on demand.
//!
//! Only a skill's `name` and `description` cost context, permanently; the body and its
//! `references/` cost nothing until read. So the list is what is kept short.
//!
//! Two sources, gated differently (ADR-0008, `docs/package-resources.md`):
//!
//! - **Authored** skills (REactor's own `skills/`) name the catalogue tools they need in
//!   `requires:`, and are offered only while every one of them is present *and*
//!   active — a skill for a tool that is not here is a standing invitation to a dead end.
//! - **Upstream** skills, fetched into `~/.reactor/skills/<tool>/`, are gated by the
//!   registry itself: it reports the fetched skills of tools that are present and
//!   active, and those are exactly what is offered.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Catalogue tool ids that must be present and active. Empty = always.
    pub requires: Vec<String>,
    /// The `SKILL.md` to read.
    pub file: PathBuf,
}

/// `name`, `description` and `requires: [a, b]` from a SKILL.md's frontmatter.
pub fn parse(file: &Path) -> Option<Skill> {
    let text = std::fs::read_to_string(file).ok()?;
    let body = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let end = body.find("\n---")?;
    let (mut name, mut description, mut requires) = (None, None, Vec::new());
    for line in body[..end].lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "name" => name = Some(value.trim_matches(|c| c == '"' || c == '\'').to_string()),
            "description" => {
                description = Some(value.trim_matches(|c| c == '"' || c == '\'').to_string())
            }
            "requires" => {
                requires = value
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            _ => {}
        }
    }
    // A skill without a description is refused: there would be nothing to choose it by.
    let description = description.filter(|d| !d.is_empty())?;
    Some(Skill {
        name: name.filter(|n| !n.is_empty())?,
        description,
        requires,
        file: file.to_path_buf(),
    })
}

/// Every `<dir>/*/SKILL.md`, by name.
pub fn discover(dir: &Path) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut skills: Vec<Skill> = entries
        .flatten()
        .filter_map(|e| parse(&e.path().join("SKILL.md")))
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// The skills to offer: authored ones whose requirements are met, then the upstream
/// ones the registry reported (`skill_dirs`, each holding a `SKILL.md`).
pub fn offered(
    authored: &[Skill],
    usable_tools: &HashSet<String>,
    skill_dirs: &[String],
) -> Vec<Skill> {
    let mut out: Vec<Skill> = authored
        .iter()
        .filter(|s| s.requires.iter().all(|r| usable_tools.contains(r)))
        .cloned()
        .collect();
    for d in skill_dirs {
        if let Some(s) = parse(&Path::new(d).join("SKILL.md")) {
            out.push(s);
        }
    }
    out
}

/// The system-prompt block, or nothing when there is nothing to offer.
pub fn block(skills: &[Skill]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let mut out = String::from(
        "## Skills\n\nInstructions for specific jobs. When one fits the task, read its file with `read` before starting -- the description is all you have until you do.\n",
    );
    for s in skills {
        out.push_str(&format!(
            "\n- **{}** -- {} (`{}`)",
            s.name,
            s.description,
            s.file.display()
        ));
    }
    Some(out)
}
