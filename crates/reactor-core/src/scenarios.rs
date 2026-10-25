//! The scenarios that ship with REactor, and where they live once seeded.
//!
//! A scenario is a directory of numbered step files under `prompts/scenarios/`. The copies in
//! the repository are compiled into the library (`build.rs`) the way `tools.toml` is, so a
//! bare binary can seed them: `reactor setup` writes them to `<config dir>/prompts/scenarios`,
//! which is where the GUI looks once it finds no checkout. Seeding never overwrites a file
//! that is there (ADR-0004): after that the files are the user's.
//!
//! The format is read and written here too, so `doctor` can check it: a phase is a Markdown
//! file with `title:`, `toolset:` and, in the first, `description:` frontmatter
//! (ADR-0017, ADR-0065). A scenario is looked up in the case first, then the config directory,
//! then the checkout ([`Library`]).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::paths::{Paths, Shipped};

/// Where the scenarios are, below a checkout and below the config directory.
pub const DIR: &str = "prompts/scenarios";

/// The compiled-in copies: the path below the scenarios directory, and the bytes.
static EMBEDDED: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/scenarios.rs"));

/// How a seeded file stands against the shipped one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Absent,
    Same,
    Differs,
}

/// One shipped file and how the installed one compares.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Below the scenarios directory, `/`-separated: `patching/01-orientation.md`.
    pub rel: String,
    pub bytes: Vec<u8>,
    pub standing: Standing,
}

impl Paths {
    /// Where the seeded scenarios are.
    pub fn scenarios_dir(&self) -> PathBuf {
        self.config_dir.join(DIR)
    }
}

/// The shipped scenario files: those of the checkout `REACTOR_PACKAGE_ROOT` names when it has
/// any, else the compiled-in copies.
pub fn shipped(paths: &Paths) -> Vec<(String, Vec<u8>)> {
    if let Shipped::Dir(root) = &paths.shipped {
        let mut found = Vec::new();
        walk(&root.join(DIR), "", &mut found);
        if !found.is_empty() {
            return found;
        }
    }
    EMBEDDED
        .iter()
        .map(|(rel, bytes)| (rel.to_string(), bytes.to_vec()))
        .collect()
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let path = e.path();
        if path.is_dir() {
            walk(&path, &rel, out);
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.push((rel, bytes));
        }
    }
}

/// Every shipped file with how the installed copy stands.
pub fn survey(paths: &Paths) -> Vec<Entry> {
    let dir = paths.scenarios_dir();
    shipped(paths)
        .into_iter()
        .map(|(rel, bytes)| {
            let standing = match std::fs::read(dir.join(&rel)) {
                Err(_) => Standing::Absent,
                Ok(have) if have == bytes => Standing::Same,
                Ok(_) => Standing::Differs,
            };
            Entry {
                rel,
                bytes,
                standing,
            }
        })
        .collect()
}

/// How many scenarios (directories with at least one step file) the seeded directory holds.
pub fn installed_count(paths: &Paths) -> usize {
    let Ok(read) = std::fs::read_dir(paths.scenarios_dir()) else {
        return 0;
    };
    read.flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            std::fs::read_dir(e.path()).is_ok_and(|steps| {
                steps
                    .flatten()
                    .any(|s| s.path().extension().is_some_and(|x| x == "md"))
            })
        })
        .count()
}

// -- the format -------------------------------------------------------------------

/// One phase of a scenario: one `NN-name.md` file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Phase {
    pub title: String,
    pub toolset: Option<String>,
    /// What the scenario is for. Read from the first phase only.
    pub description: Option<String>,
    pub body: String,
}

/// JS `\s` and `String.prototype.trim` whitespace: Unicode White_Space plus U+FEFF (the
/// format was first read by a JavaScript extension, and the port keeps its trimming).
fn space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

fn trim(s: &str) -> &str {
    s.trim_matches(space)
}

fn frontmatter_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)\A---\r?\n(.*?)\r?\n---\r?\n?(.*)\z").unwrap())
}

fn field_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // `.` in JS excludes the four line terminators, not just \n.
    RE.get_or_init(|| Regex::new(r"\A(\w+):\s*([^\r\n\u{2028}\u{2029}]*)\z").unwrap())
}

pub fn parse_phase(raw: &str) -> Phase {
    let Some(m) = frontmatter_re().captures(raw) else {
        return Phase {
            body: trim(raw).to_string(),
            ..Phase::default()
        };
    };
    let (mut title, mut toolset, mut description) = (String::new(), String::new(), String::new());
    for line in m[1].split('\n') {
        if let Some(kv) = field_re().captures(line) {
            let value = trim(&kv[2]).to_string();
            match &kv[1] {
                "title" => title = value,
                "toolset" => toolset = value,
                "description" => description = value,
                _ => {}
            }
        }
    }
    Phase {
        title,
        toolset: (!toolset.is_empty()).then_some(toolset),
        description: (!description.is_empty()).then_some(description),
        body: trim(&m[2]).to_string(),
    }
}

/// A phase as its file holds it. Frontmatter values are one line each.
pub fn render_phase(p: &Phase) -> String {
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = format!("---\ntitle: {}\n", one_line(&p.title));
    if let Some(t) = p.toolset.as_deref().map(one_line).filter(|t| !t.is_empty()) {
        out.push_str(&format!("toolset: {t}\n"));
    }
    if let Some(d) = p
        .description
        .as_deref()
        .map(one_line)
        .filter(|d| !d.is_empty())
    {
        out.push_str(&format!("description: {d}\n"));
    }
    out.push_str("---\n");
    out.push_str(trim(&p.body));
    out.push('\n');
    out
}

/// The phase files of one scenario directory, in name order.
fn phase_files(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.ends_with(".md"))
        .collect();
    files.sort();
    files
}

/// The phases of the scenario in `dir`. Unknown or unreadable is empty.
pub fn load_dir(dir: &Path) -> Vec<Phase> {
    phase_files(dir)
        .iter()
        .map(|f| parse_phase(&std::fs::read_to_string(dir.join(f)).unwrap_or_default()))
        .collect()
}

/// A scenario id is a directory name: lower-case letters, digits, `-` and `_`.
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && id.len() <= 64
}

/// `Static analysis` → `static-analysis`, for a phase's file name.
fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "phase".into() } else { out }
}

/// Write scenario `id` into `scope_dir` (a scenarios directory), replacing what is there under
/// that id. The description goes into the first phase. Written beside and swapped in, so a
/// failure leaves the old one whole.
pub fn save(
    scope_dir: &Path,
    id: &str,
    description: Option<&str>,
    phases: &[Phase],
) -> std::io::Result<PathBuf> {
    use std::io::{Error, ErrorKind};
    if !valid_id(id) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("\"{id}\" is not a scenario id: lower-case letters, digits, - and _"),
        ));
    }
    if phases.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "a scenario needs at least one phase",
        ));
    }
    if let Some(n) = phases.iter().position(|p| trim(&p.title).is_empty()) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("phase {} has no title", n + 1),
        ));
    }
    if let Some(n) = phases.iter().position(|p| trim(&p.body).is_empty()) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("phase {} has no body", n + 1),
        ));
    }
    std::fs::create_dir_all(scope_dir)?;
    let target = scope_dir.join(id);
    let staging = scope_dir.join(format!(".{id}.saving"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let width = phases.len().to_string().len().max(2);
    for (i, p) in phases.iter().enumerate() {
        let mut p = p.clone();
        p.description = if i == 0 {
            description
                .map(str::to_string)
                .filter(|d| !trim(d).is_empty())
        } else {
            None
        };
        let name = format!("{:0width$}-{}.md", i + 1, slug(&p.title));
        std::fs::write(staging.join(name), render_phase(&p))?;
    }
    let old = scope_dir.join(format!(".{id}.old"));
    let _ = std::fs::remove_dir_all(&old);
    if target.exists() {
        std::fs::rename(&target, &old)?;
    }
    if let Err(e) = std::fs::rename(&staging, &target) {
        let _ = std::fs::rename(&old, &target);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(target)
}

// -- where they are -----------------------------------------------------------------

/// The directory below a case's `.reactor/` that holds its own scenarios.
pub const CASE_DIR: &str = "scenarios";

/// Where a scenario was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    /// `<case>/.reactor/scenarios/`: this case only.
    Case,
    /// `<config dir>/prompts/scenarios/`: the user's, for every case.
    Global,
    /// A checkout's `prompts/scenarios/`.
    Shipped,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Case => "case",
            Scope::Global => "global",
            Scope::Shipped => "shipped",
        }
    }
    pub fn parse(s: &str) -> Option<Scope> {
        match s {
            "case" => Some(Scope::Case),
            "global" => Some(Scope::Global),
            "shipped" => Some(Scope::Shipped),
            _ => None,
        }
    }
}

/// One scenario as the chooser lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub id: String,
    /// Where the one that is used was found.
    pub scope: Scope,
    pub description: Option<String>,
    pub titles: Vec<String>,
    /// The places further down that have one of the same id, hidden by this one.
    pub shadows: Vec<Scope>,
}

/// The scenario directories, in lookup order: the case's, the config directory's, a
/// checkout's. The first that has an id is the one used (ADR-0065).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Library {
    dirs: Vec<(Scope, PathBuf)>,
}

impl Library {
    /// `case` is the case's `.reactor/` (or unsaved state) directory; `shipped` a checkout's
    /// scenarios directory, if there is one. A directory named twice is kept at its first scope.
    pub fn new(case: Option<&Path>, global: PathBuf, shipped: Option<PathBuf>) -> Library {
        let mut dirs: Vec<(Scope, PathBuf)> = Vec::new();
        let candidates = case
            .map(|c| (Scope::Case, c.join(CASE_DIR)))
            .into_iter()
            .chain(std::iter::once((Scope::Global, global)))
            .chain(shipped.map(|s| (Scope::Shipped, s)));
        for (scope, dir) in candidates {
            if !dirs.iter().any(|(_, d)| same_dir(d, &dir)) {
                dirs.push((scope, dir));
            }
        }
        Library { dirs }
    }

    /// One directory, read as the global one (tests, and callers with only a directory).
    pub fn single(dir: impl Into<PathBuf>) -> Library {
        Library {
            dirs: vec![(Scope::Global, dir.into())],
        }
    }

    /// The directory of a scope, if this library has it.
    pub fn dir(&self, scope: Scope) -> Option<&Path> {
        self.dirs
            .iter()
            .find(|(s, _)| *s == scope)
            .map(|(_, d)| d.as_path())
    }

    pub fn dirs(&self) -> &[(Scope, PathBuf)] {
        &self.dirs
    }

    /// Where scenario `id` is, by the lookup order.
    pub fn find(&self, id: &str) -> Option<(Scope, PathBuf)> {
        if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
            return None;
        }
        self.dirs
            .iter()
            .map(|(s, d)| (*s, d.join(id)))
            .find(|(_, d)| !phase_files(d).is_empty())
    }

    /// Scenario `id`'s phases. Unknown is empty.
    pub fn load(&self, id: &str) -> Vec<Phase> {
        self.find(id).map(|(_, d)| load_dir(&d)).unwrap_or_default()
    }

    /// Every scenario id, sorted.
    pub fn ids(&self) -> Vec<String> {
        self.list().into_iter().map(|l| l.id).collect()
    }

    /// Every scenario, sorted by id, each as found first.
    pub fn list(&self) -> Vec<Listed> {
        let mut out: Vec<Listed> = Vec::new();
        for (scope, dir) in &self.dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for e in entries.flatten() {
                let id = e.file_name().to_string_lossy().into_owned();
                if id.starts_with('.') || !e.path().is_dir() {
                    continue;
                }
                let phases = load_dir(&e.path());
                if phases.is_empty() {
                    continue;
                }
                if let Some(seen) = out.iter_mut().find(|l| l.id == id) {
                    seen.shadows.push(*scope);
                    continue;
                }
                out.push(Listed {
                    id,
                    scope: *scope,
                    description: phases[0].description.clone(),
                    titles: phases.iter().map(|p| p.title.clone()).collect(),
                    shadows: vec![],
                });
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

// -- checking what a user wrote ------------------------------------------------------

/// Something wrong with a scenario's files. It still loads; this says why it reads oddly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// The file, or the scenario's directory.
    pub path: PathBuf,
    pub message: String,
}

fn phase_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\A(\d+)-[^/\\]+\.md\z").unwrap())
}

/// Check the scenario in `dir` against the format and the toolsets there are.
pub fn lint(dir: &Path, toolsets: &[String]) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut issue = |path: PathBuf, message: String| issues.push(Issue { path, message });
    let Ok(entries) = std::fs::read_dir(dir) else {
        issue(dir.to_path_buf(), "cannot be read".into());
        return issues;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    let mut numbers = Vec::new();
    for n in &names {
        if !n.ends_with(".md") {
            issue(dir.join(n), "not a phase: only `.md` files are read".into());
            continue;
        }
        match phase_name_re().captures(n) {
            Some(c) => numbers.push(c[1].parse::<usize>().unwrap_or(0)),
            None => issue(
                dir.join(n),
                "not named `NN-name.md`: it is ordered by name only".into(),
            ),
        }
    }
    let files = phase_files(dir);
    if files.is_empty() {
        issue(dir.to_path_buf(), "has no phases (no `.md` files)".into());
        return issues;
    }
    let mut sorted = numbers.clone();
    sorted.sort_unstable();
    for w in sorted.windows(2) {
        if w[0] == w[1] {
            issue(
                dir.to_path_buf(),
                format!("two phases are numbered {:02}", w[0]),
            );
        } else if w[1] != w[0] + 1 {
            issue(
                dir.to_path_buf(),
                format!("the phase numbers skip from {:02} to {:02}", w[0], w[1]),
            );
        }
    }
    for (i, f) in files.iter().enumerate() {
        let path = dir.join(f);
        let p = parse_phase(&std::fs::read_to_string(&path).unwrap_or_default());
        if p.title.is_empty() {
            issue(path.clone(), "has no `title:`".into());
        }
        if p.body.is_empty() {
            issue(path.clone(), "has no body: the briefing is empty".into());
        }
        if let Some(t) = &p.toolset
            && !toolsets.contains(t)
        {
            issue(
                path.clone(),
                format!("names toolset `{t}`, which is not in toolsets.toml"),
            );
        }
        if i == 0 && p.description.is_none() {
            issue(
                path,
                "has no `description:`: the chooser cannot say what the scenario is for".into(),
            );
        }
    }
    issues
}

/// Check the scenarios the user has: the config directory's that are not the shipped copies
/// as they are, and those of the case at `paths.cwd`, if it is one.
pub fn lint_installed(paths: &Paths, toolsets: &[String]) -> Vec<(String, Scope, Issue)> {
    let shipped = shipped(paths);
    let case = paths
        .cwd
        .as_ref()
        .map(|c| c.join(".reactor"))
        .filter(|d| d.is_dir());
    let lib = Library::new(case.as_deref(), paths.scenarios_dir(), None);
    let mut out = Vec::new();
    for (scope, dir) in lib.dirs() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut ids: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|id| !id.starts_with('.'))
            .collect();
        ids.sort();
        for id in ids {
            let mut have = Vec::new();
            walk(&dir.join(&id), &id, &mut have);
            let theirs: Vec<&(String, Vec<u8>)> = shipped
                .iter()
                .filter(|(rel, _)| rel.starts_with(&format!("{id}/")))
                .collect();
            let as_shipped = *scope == Scope::Global
                && have.len() == theirs.len()
                && have.iter().all(|f| theirs.contains(&f));
            if as_shipped {
                continue;
            }
            out.extend(
                lint(&dir.join(&id), toolsets)
                    .into_iter()
                    .map(|i| (id.clone(), *scope, i)),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiled_in_copies_are_the_repository_ones() {
        let on_disk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts/scenarios");
        let mut disk = Vec::new();
        walk(&on_disk, "", &mut disk);
        assert!(!disk.is_empty(), "the repository has scenarios");
        let embedded: Vec<(String, Vec<u8>)> = EMBEDDED
            .iter()
            .map(|(r, b)| (r.to_string(), b.to_vec()))
            .collect();
        assert_eq!(embedded, disk);
    }

    #[test]
    fn a_phase_round_trips_through_its_file() {
        let p = Phase {
            title: "Static analysis".into(),
            toolset: Some("native".into()),
            description: Some("What it is for".into()),
            body: "Do this.\n\nThen that.".into(),
        };
        assert_eq!(parse_phase(&render_phase(&p)), p);
    }

    #[test]
    fn the_case_wins_then_the_config_directory_then_the_checkout() {
        let root = tempfile::tempdir().unwrap();
        let (case, global, shipped) = (
            root.path().join("c"),
            root.path().join("g"),
            root.path().join("s"),
        );
        let one = |t: &str| {
            vec![Phase {
                title: t.into(),
                body: "b".into(),
                ..Phase::default()
            }]
        };
        save(&shipped, "x", Some("shipped"), &one("S")).unwrap();
        save(&global, "x", None, &one("G")).unwrap();
        save(&global, "y", None, &one("Y")).unwrap();
        save(&case.join(CASE_DIR), "x", None, &one("C")).unwrap();
        let lib = Library::new(Some(&case), global.clone(), Some(shipped));
        assert_eq!(lib.load("x")[0].title, "C");
        let listed = lib.list();
        assert_eq!(
            listed.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(),
            ["x", "y"]
        );
        assert_eq!(listed[0].scope, Scope::Case);
        assert_eq!(listed[0].shadows, [Scope::Global, Scope::Shipped]);
        assert!(lib.find("../g/x").is_none());
        // The same directory twice is one place, at its first scope.
        let lib = Library::new(None, global.clone(), Some(global));
        assert_eq!(lib.dirs().len(), 1);
    }

    #[test]
    fn saving_replaces_and_numbers_the_phases() {
        let root = tempfile::tempdir().unwrap();
        let ph = |t: &str| Phase {
            title: t.into(),
            body: "b".into(),
            ..Phase::default()
        };
        save(
            root.path(),
            "s",
            Some("d"),
            &[ph("One"), ph("Two"), ph("Three")],
        )
        .unwrap();
        save(root.path(), "s", Some("d"), &[ph("Only / phase")]).unwrap();
        let names = phase_files(&root.path().join("s"));
        assert_eq!(names, ["01-only-phase.md"]);
        assert_eq!(
            load_dir(&root.path().join("s"))[0].description.as_deref(),
            Some("d")
        );
        assert!(save(root.path(), "Bad Id", None, &[ph("x")]).is_err());
        assert!(save(root.path(), "s", None, &[]).is_err());
        assert!(
            save(
                root.path(),
                "s",
                None,
                &[Phase {
                    title: "t".into(),
                    ..Phase::default()
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn lint_names_what_is_wrong() {
        let root = tempfile::tempdir().unwrap();
        let d = root.path().join("s");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("01-a.md"), "---\ntitle: A\ntoolset: nope\n---\nbody").unwrap();
        std::fs::write(d.join("03-c.md"), "---\ntoolset: triage\n---\n").unwrap();
        std::fs::write(d.join("notes.txt"), "x").unwrap();
        let issues: Vec<String> = lint(&d, &["triage".into()])
            .into_iter()
            .map(|i| {
                format!(
                    "{}: {}",
                    i.path.file_name().unwrap().to_string_lossy(),
                    i.message
                )
            })
            .collect();
        assert!(
            issues
                .iter()
                .any(|i| i.starts_with("notes.txt: not a phase")),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|i| i.contains("skip from 01 to 03")),
            "{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.starts_with("01-a.md: names toolset `nope`")),
            "{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.starts_with("01-a.md: has no `description:`")),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|i| i == "03-c.md: has no `title:`"),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|i| i.starts_with("03-c.md: has no body")),
            "{issues:?}"
        );
    }

    #[test]
    fn the_shipped_scenarios_pass_their_own_check() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path(), Shipped::Embedded);
        let toolsets: Vec<String> = crate::commands::toolsets_list(&paths)
            .unwrap()
            .report
            .toolsets
            .into_iter()
            .map(|t| t.id)
            .collect();
        let root = tempfile::tempdir().unwrap();
        for (rel, bytes) in shipped(&paths) {
            let dest = root.path().join(&rel);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(dest, bytes).unwrap();
        }
        let lib = Library::single(root.path());
        assert!(!lib.ids().is_empty());
        for id in lib.ids() {
            let issues = lint(&root.path().join(&id), &toolsets);
            assert!(issues.is_empty(), "{id}: {issues:?}");
        }
    }

    #[test]
    fn the_check_passes_over_seeded_copies_and_names_what_the_user_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("cfg"), Shipped::Embedded).with_cwd(dir.path());
        for (rel, bytes) in shipped(&paths) {
            let dest = paths.scenarios_dir().join(&rel);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(dest, bytes).unwrap();
        }
        let case = dir.path().join(".reactor").join(CASE_DIR).join("mine");
        std::fs::create_dir_all(&case).unwrap();
        std::fs::write(case.join("01-a.md"), "---\ntitle: A\n---\nbody").unwrap();
        let found = lint_installed(&paths, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].0.as_str(), found[0].1), ("mine", Scope::Case));
        assert!(found[0].2.message.contains("description"));
    }
}
