//! The command catalogue behind the command palette and the `/` completion popup, and the
//! ranking both use.
//!
//! One list, two views. Every feature the GUI offers is reachable as a *slash command* —
//! `run_command` in `app.rs` is the single place that executes them — so an [`Entry`] is just
//! a title, what to run, and whether the command needs arguments from the person. The palette
//! shows all entries (fixed commands, plus ones generated from live state: each model, each
//! tool, each scenario …); the completion popup shows only the bare `/name` commands.
//!
//! **Ranking** is the same for both and is deliberately simple to state: the score of an
//! entry is the number of characters of the query it *matches* — the longest common
//! subsequence of the typed text and the entry's text, inside a window no wider than twice the
//! query, so a missing, extra, wrong or swapped character costs one match instead of the whole
//! entry (typos are tolerated), while characters scattered across a long title do not add up.
//! Entries are ordered by that score; equal scores are ordered by how often the person has run
//! them; equal frequency falls back to the shorter text, then the alphabet, so the order is
//! always stable. With nothing typed, frequency alone orders the list.

use std::collections::BTreeMap;

/// Whether a command takes arguments from the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Args {
    /// Runs as it is.
    None,
    /// Runs bare, and accepts arguments (`/identity`, `/reduce fade`).
    Optional,
    /// Needs something typed after it; choosing it puts `/name ` in the composer.
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// What frequency is counted under.
    pub key: String,
    pub title: String,
    /// The bare slash name this entry is the command of, if it is one (`goal`).
    pub name: Option<String>,
    pub detail: String,
    pub group: &'static str,
    pub args: Args,
    /// The slash command to run, without the `/`. For [`Args::Required`] entries that are
    /// bare commands this is the name; the composer is filled with it and a space.
    pub run: String,
}

impl Entry {
    fn fixed(name: &str, group: &'static str, args: Args, title: &str, detail: &str) -> Entry {
        Entry {
            key: name.to_string(),
            title: title.to_string(),
            name: Some(name.to_string()),
            detail: detail.to_string(),
            group,
            args,
            run: name.to_string(),
        }
    }

    fn generated(
        group: &'static str,
        title: impl Into<String>,
        detail: impl Into<String>,
        run: impl Into<String>,
    ) -> Entry {
        let run = run.into();
        Entry {
            key: run.clone(),
            title: title.into(),
            name: None,
            detail: detail.into(),
            group,
            args: Args::None,
            run,
        }
    }

    /// The texts a query is matched against.
    fn haystacks(&self, popup: bool) -> Vec<&str> {
        match (&self.name, popup) {
            (Some(name), true) => vec![name.as_str()],
            (Some(name), false) => vec![self.title.as_str(), name.as_str()],
            (None, _) => vec![self.title.as_str()],
        }
    }
}

/// The slash commands, in the order `/help` lists them.
pub fn commands() -> Vec<Entry> {
    use Args::*;
    vec![
        Entry::fixed(
            "goal",
            "Session",
            Optional,
            "Set the case goal",
            "/goal [text] — no text opens the editor — or /goal clear",
        ),
        Entry::fixed(
            "guidelines",
            "Session",
            Optional,
            "Set the case guidelines",
            "/guidelines [text] — no text opens the editor — or /guidelines clear",
        ),
        Entry::fixed(
            "manifest",
            "Session",
            Optional,
            "Manifest on, off or clear",
            "/manifest on | off | clear",
        ),
        Entry::fixed(
            "frame",
            "Session",
            None,
            "Show the frame",
            "what the model is told about goal, steps and guidelines, and what is kept",
        ),
        Entry::fixed(
            "identity",
            "Session",
            Optional,
            "Select or write the working persona",
            "/identity <name> | new [name] | edit <name> | manage | off | show [name] | write <text> | save <name> | delete <name>",
        ),
        Entry::fixed(
            "report",
            "Session",
            Optional,
            "Reporting",
            "/report on | off | level <0-2> | status",
        ),
        Entry::fixed(
            "reactor-scenario",
            "Session",
            Optional,
            "Scenarios",
            "/reactor-scenario list | start <id> | status | next | goto <n> | stop | new | edit <id> | promote <id>",
        ),
        Entry::fixed(
            "model",
            "Model",
            Required,
            "Switch model",
            "/model provider/name",
        ),
        Entry::fixed(
            "thinking",
            "Model",
            Required,
            "Thinking level",
            "/thinking off | on | low | medium | high | default",
        ),
        Entry::fixed(
            "context",
            "Model",
            Optional,
            "Context settings",
            "/context <mode|window|reserve|pct|keep|summarizer> <value> | default | inherit",
        ),
        Entry::fixed(
            "inspect",
            "Model",
            None,
            "Show the context window",
            "what the model would be sent, and where each piece comes from",
        ),
        Entry::fixed(
            "agents",
            "Model",
            Optional,
            "Child agents running now",
            "/agents — lists them — | stop [all|<session>]",
        ),
        Entry::fixed(
            "children",
            "Model",
            Optional,
            "How child agents run",
            "/children — shows it — | parallel [on|off] | serial | max <n>",
        ),
        Entry::fixed(
            "cache",
            "Model",
            None,
            "Show what the provider's cache did",
            "input read from and written to the cache over this branch, and the cold requests",
        ),
        Entry::fixed(
            "preview",
            "Model",
            Optional,
            "Preview a context reduction",
            "/preview [auto|fade|compact]",
        ),
        Entry::fixed(
            "reduce",
            "Model",
            Optional,
            "Reduce the context now",
            "/reduce [auto|fade|compact] [what to keep]",
        ),
        Entry::fixed(
            "undo",
            "Model",
            Optional,
            "Undo a reduction",
            "/undo — the latest — or /undo <entry>",
        ),
        Entry::fixed(
            "tool",
            "Tools",
            Required,
            "Enable or disable a tool",
            "/tool <id> [on|off]",
        ),
        Entry::fixed(
            "toolset",
            "Tools",
            Required,
            "Enable or disable a toolset",
            "/toolset <id> [on|off]",
        ),
        Entry::fixed(
            "install",
            "Tools",
            Required,
            "Install a tool",
            "/install <tool> — opens a console running it",
        ),
        Entry::fixed(
            "activation",
            "Tools",
            Required,
            "Tool activation scope",
            "/activation default | inherit",
        ),
        Entry::fixed(
            "refresh",
            "Tools",
            None,
            "Refresh tools, toolsets and services",
            "re-probes the machine",
        ),
        Entry::fixed(
            "branch",
            "Session",
            Required,
            "Continue from another reply",
            "/branch <entry id> — see the session tree",
        ),
        Entry::fixed(
            "interrupt",
            "Session",
            None,
            "Interrupt the agent",
            "cancels the running turn",
        ),
        Entry::fixed(
            "layout",
            "Window",
            Required,
            "Apply a window layout",
            "/layout default | focus | analysis | catalogue",
        ),
        Entry::fixed(
            "dock",
            "Window",
            Required,
            "Show or hide a dock",
            "/dock left | right | bottom",
        ),
        Entry::fixed(
            "panel",
            "Window",
            Required,
            "Show or hide a panel",
            "/panel transcript | tree | tools | toolsets | context | services | files | console",
        ),
        Entry::fixed(
            "open",
            "Window",
            Required,
            "Open a file in a viewer",
            "/open [edit|table|data|hex] <path>",
        ),
        Entry::fixed(
            "look",
            "Window",
            Required,
            "Look at a file (an image) in a popup",
            "/look <path>",
        ),
        Entry::fixed(
            "console",
            "Window",
            Optional,
            "Open a console",
            "/console [command]",
        ),
        Entry::fixed(
            "notifications",
            "Window",
            None,
            "Show notifications",
            "the history behind the bell, newest first",
        ),
        Entry::fixed(
            "palette",
            "Window",
            None,
            "Open the command palette",
            "Ctrl+P / Cmd+P",
        ),
        Entry::fixed(
            "settings",
            "Window",
            None,
            "Settings: fonts, sizes and behaviour",
            "Ctrl+, / Cmd+,",
        ),
        Entry::fixed(
            "save-case",
            "Case",
            None,
            "Save as Case",
            "makes this folder a case: the sessions move into its .reactor/ and the window reopens",
        ),
        Entry::fixed(
            "import",
            "Case",
            Args::Optional,
            "Import context documents",
            "/import [files, folders or URLs…] — none: choose in a dialog. Wikis, PDFs, Word, web pages, reports; searchable by the agent",
        ),
        Entry::fixed(
            "embedding",
            "Case",
            Args::Optional,
            "Embedding model for searching documents by meaning",
            "/embedding [provider/name | off | now] — none: show the state. The text of documents is sent to that provider",
        ),
        Entry::fixed("quit", "Window", None, "Quit REactor", "closes the window"),
        Entry::fixed("help", "Window", None, "List the slash commands", "/help"),
    ]
}

/// What the generated entries are built from — a snapshot of the app's state.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub models: Vec<String>,
    /// `(id, installed, active)`.
    pub tools: Vec<(String, bool, bool)>,
    /// `(id, active)`.
    pub toolsets: Vec<(String, bool)>,
    pub scenarios: Vec<String>,
    pub identities: Vec<String>,
    /// The saved ones among them: what can be edited or deleted.
    pub user_identities: Vec<String>,
    /// Entry ids of the reductions in force.
    pub reductions: Vec<u64>,
    pub working: bool,
}

/// Every palette entry: the commands, then what the current state makes possible.
pub fn build(snapshot: &Snapshot) -> Vec<Entry> {
    let mut entries = commands();
    let mut add = |e: Entry| entries.push(e);

    for (title, detail, run) in [
        (
            "Manifest: on",
            "include the manifest in the prompt",
            "manifest on",
        ),
        (
            "Manifest: off",
            "leave the manifest out of the prompt",
            "manifest off",
        ),
        (
            "Manifest: clear",
            "forget goal, steps and guidelines",
            "manifest clear",
        ),
        (
            "Identities: manage…",
            "add, edit and remove identities in Settings",
            "identity manage",
        ),
        (
            "Identity: new…",
            "write one in the editor window",
            "identity new",
        ),
        ("Goal: clear", "remove the session goal", "goal clear"),
        (
            "Guidelines: clear",
            "remove the session guidelines",
            "guidelines clear",
        ),
        ("Reporting: on", "documenting as the work goes", "report on"),
        ("Reporting: off", "", "report off"),
        ("Reporting: status", "", "report status"),
        ("Reporting: level 0", "", "report level 0"),
        ("Reporting: level 1", "", "report level 1"),
        ("Reporting: level 2", "", "report level 2"),
        (
            "Scenario: list",
            "the case's, yours and the shipped ones",
            "reactor-scenario list",
        ),
        ("Scenario: new", "write one in the editor", "reactor-scenario new"),
        ("Scenario: status", "", "reactor-scenario status"),
        (
            "Scenario: next phase",
            "advance by hand",
            "reactor-scenario next",
        ),
        ("Scenario: stop", "", "reactor-scenario stop"),
        (
            "Context: mode auto",
            "summarize, and drop what can be recovered",
            "context mode auto",
        ),
        (
            "Context: mode fade",
            "drop old messages, leave stubs",
            "context mode fade",
        ),
        (
            "Context: mode compact",
            "summarize old messages",
            "context mode compact",
        ),
        (
            "Context: make this session's settings the default",
            "for new sessions",
            "context default",
        ),
        (
            "Context: inherit the default settings",
            "drop this session's values",
            "context inherit",
        ),
        (
            "Reduce: preview summarize",
            "what it would do",
            "preview compact",
        ),
        ("Reduce: preview fade", "what it would do", "preview fade"),
        ("Reduce now: summarize", "", "reduce compact"),
        ("Reduce now: fade", "", "reduce fade"),
        ("Reduce now: auto", "", "reduce auto"),
        (
            "Tools: make this session's activation the default",
            "for new sessions",
            "activation default",
        ),
        (
            "Tools: inherit the default activation",
            "drop this session's override",
            "activation inherit",
        ),
        ("Layout: default", "", "layout default"),
        ("Layout: focus", "", "layout focus"),
        ("Layout: analysis", "", "layout analysis"),
        ("Layout: catalogue", "", "layout catalogue"),
        ("Toggle the left dock", "", "dock left"),
        ("Toggle the right dock", "", "dock right"),
        ("Toggle the bottom dock", "", "dock bottom"),
    ] {
        add(Entry::generated("Commands", title, detail, run));
    }

    for kind in crate::layout::PanelKind::ALL {
        add(Entry::generated(
            "Panels",
            format!("Panels: {}", kind.label()),
            "show or hide",
            format!("panel {}", kind.slug()),
        ));
    }

    for model in &snapshot.models {
        add(Entry::generated(
            "Model",
            format!("Model: switch to {model}"),
            "",
            format!("model {model}"),
        ));
    }
    for (id, installed, active) in &snapshot.tools {
        if !installed {
            add(Entry::generated(
                "Tools",
                format!("Tool: install {id}"),
                "not installed here",
                format!("install {id}"),
            ));
        } else if *active {
            add(Entry::generated(
                "Tools",
                format!("Tool: disable {id}"),
                "stop advertising it",
                format!("tool {id} off"),
            ));
        } else {
            add(Entry::generated(
                "Tools",
                format!("Tool: enable {id}"),
                "advertise it again",
                format!("tool {id} on"),
            ));
        }
    }
    for (id, active) in &snapshot.toolsets {
        if *active {
            add(Entry::generated(
                "Tools",
                format!("Toolset: disable {id}"),
                "",
                format!("toolset {id} off"),
            ));
        } else {
            add(Entry::generated(
                "Tools",
                format!("Toolset: enable {id}"),
                "",
                format!("toolset {id} on"),
            ));
        }
    }
    for id in &snapshot.scenarios {
        add(Entry::generated(
            "Session",
            format!("Scenario: start {id}"),
            "asks for a question, then begins",
            format!("reactor-scenario start {id}"),
        ));
        add(Entry::generated(
            "Session",
            format!("Scenario: edit {id}"),
            "opens it in the editor",
            format!("reactor-scenario edit {id}"),
        ));
    }
    for name in &snapshot.identities {
        add(Entry::generated(
            "Session",
            format!("Identity: {name}"),
            "",
            format!("identity {name}"),
        ));
    }
    for name in &snapshot.user_identities {
        add(Entry::generated(
            "Session",
            format!("Identity: edit {name}"),
            "in the editor window",
            format!("identity edit {name}"),
        ));
    }
    for id in &snapshot.reductions {
        add(Entry::generated(
            "Model",
            format!("Undo reduction #{id}"),
            "restore what it took out",
            format!("undo {id}"),
        ));
    }
    entries
}

// -- parameters ---------------------------------------------------------------------------------

/// A value a command accepts, offered while its arguments are typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub value: String,
    pub detail: String,
    /// The command takes something after this (`/tool <id>` takes `on` or `off`; `/open
    /// edit` takes a path): choosing it completes the text and waits, where a last
    /// argument runs the command.
    pub more: bool,
}

impl Param {
    fn last(value: impl Into<String>, detail: impl Into<String>) -> Param {
        Param {
            value: value.into(),
            detail: detail.into(),
            more: false,
        }
    }
    fn more(value: impl Into<String>, detail: impl Into<String>) -> Param {
        Param {
            more: true,
            ..Param::last(value, detail)
        }
    }
}

/// The parameters on offer for the argument being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params {
    pub command: String,
    /// The text before the argument being typed, with the command and what is before it:
    /// `identity ` — and what completes it is this followed by a value.
    pub typed: String,
    /// What has been typed of the argument so far.
    pub partial: String,
    /// What matches it, best first.
    pub items: Vec<Param>,
}

impl Params {
    /// The same choices as entries, for the popup and the palette: the entry runs (or, when
    /// more can follow, completes) the whole command with the value in place.
    pub fn entries(&self) -> Vec<Entry> {
        self.items
            .iter()
            .map(|p| {
                let run = format!("{}{}", self.typed, p.value);
                Entry {
                    key: run.clone(),
                    title: format!("/{run}"),
                    name: None,
                    detail: p.detail.clone(),
                    group: "Parameter",
                    args: if p.more { Args::Required } else { Args::None },
                    run,
                }
            })
            .collect()
    }
}

fn choices(values: &[&str], more: bool, detail: &str) -> Vec<Param> {
    values
        .iter()
        .map(|v| {
            if more {
                Param::more(*v, detail)
            } else {
                Param::last(*v, detail)
            }
        })
        .collect()
}

/// What the argument after `done` can be, for a command: the values it accepts, from what is
/// fixed (its modes) and what the app has (models, tools, identities …).
fn candidates(command: &str, done: &[&str], snapshot: &Snapshot) -> Vec<Param> {
    let first = done.first().copied();
    let modes = ["auto", "fade", "compact"];
    match (command, done.len()) {
        ("identity", 0) => {
            let mut v: Vec<Param> = snapshot
                .identities
                .iter()
                .map(|n| Param::last(n, "select it for this session"))
                .collect();
            v.extend([
                Param::last("off", "no identity"),
                Param::more("show", "print an identity's text"),
                Param::more("new", "write a new one in the editor"),
                Param::more("edit", "change a saved one in the editor"),
                Param::more("delete", "remove a saved one"),
                Param::more("save", "keep the custom identity under a name"),
                Param::last("manage", "open the identities page in Settings"),
            ]);
            v
        }
        ("identity", 1) => match first {
            Some("show") => snapshot
                .identities
                .iter()
                .map(|n| Param::last(n, ""))
                .collect(),
            Some("edit" | "delete") => snapshot
                .user_identities
                .iter()
                .map(|n| Param::last(n, "saved"))
                .collect(),
            _ => Vec::new(),
        },
        ("model", 0) => snapshot.models.iter().map(|m| Param::last(m, "")).collect(),
        ("thinking", 0) => choices(&["off", "on", "low", "medium", "high", "default"], false, ""),
        ("layout", 0) => choices(&["default", "focus", "analysis", "catalogue"], false, ""),
        ("dock", 0) => choices(&["left", "right", "bottom"], false, ""),
        ("panel", 0) => crate::layout::PanelKind::ALL
            .iter()
            .map(|k| Param::last(k.slug(), k.label()))
            .collect(),
        ("open", 0) => choices(&["edit", "table", "data", "hex"], true, "then the path"),
        ("preview" | "reduce", 0) => choices(&modes, false, ""),
        ("undo", 0) => snapshot
            .reductions
            .iter()
            .map(|r| Param::last(r.to_string(), "a reduction in force"))
            .collect(),
        ("report", 0) => vec![
            Param::last("on", ""),
            Param::last("off", ""),
            Param::last("status", ""),
            Param::more("level", "0, 1 or 2"),
        ],
        ("report", 1) if first == Some("level") => choices(&["0", "1", "2"], false, ""),
        ("manifest", 0) => choices(&["on", "off", "clear"], false, ""),
        ("goal" | "guidelines", 0) => vec![Param::last("clear", "remove it")],
        ("reactor-scenario", 0) => vec![
            Param::last("list", ""),
            Param::last("status", ""),
            Param::last("next", ""),
            Param::last("stop", ""),
            Param::more("start", "then a scenario"),
            Param::more("goto", "then a phase number"),
            Param::last("new", "open the editor"),
            Param::more("edit", "then a scenario"),
            Param::more("promote", "then a case scenario"),
        ],
        ("reactor-scenario", 1) if matches!(first, Some("start" | "edit" | "promote")) => snapshot
            .scenarios
            .iter()
            .map(|s| Param::last(s, ""))
            .collect(),
        ("activation", 0) => choices(&["default", "inherit"], false, ""),
        ("embedding", 0) => choices(&["off", "now"], false, ""),
        ("context", 0) => {
            let mut v = choices(
                &[
                    "mode",
                    "window",
                    "reserve",
                    "pct",
                    "keep",
                    "mask",
                    "protect",
                    "verify",
                    "summarizer",
                ],
                true,
                "then a value",
            );
            v.extend(choices(&["default", "inherit"], false, ""));
            v
        }
        ("context", 1) if first == Some("mode") => choices(&modes, false, ""),
        ("tool", 0) => snapshot
            .tools
            .iter()
            .filter(|t| t.1)
            .map(|(id, _, _)| Param::more(id, "then on or off"))
            .collect(),
        ("tool" | "toolset", 1) => choices(&["on", "off"], false, ""),
        ("toolset", 0) => snapshot
            .toolsets
            .iter()
            .map(|(id, _)| Param::more(id, "then on or off"))
            .collect(),
        ("install", 0) => snapshot
            .tools
            .iter()
            .filter(|t| !t.1)
            .map(|(id, _, _)| Param::last(id, "not installed here"))
            .collect(),
        _ => Vec::new(),
    }
}

/// What can complete the argument being typed in `text` (the composer's, or the palette's
/// query), without its leading slash: `None` while it is still a command name, or when the
/// command takes nothing here. A word that is not a command's name has no parameters.
pub fn params(text: &str, snapshot: &Snapshot, usage: &Usage) -> Option<Params> {
    let text = text.strip_prefix('/').unwrap_or(text);
    let (command, rest) = text.split_once(' ')?;
    let mut words: Vec<&str> = rest.split(' ').collect();
    let partial = words.pop().unwrap_or("");
    let done: Vec<&str> = words.into_iter().filter(|w| !w.is_empty()).collect();
    let all = candidates(command, &done, snapshot);
    if all.is_empty() {
        return None;
    }
    let typed = format!(
        "{command} {}",
        done.iter().map(|w| format!("{w} ")).collect::<String>()
    );
    let items: Vec<Param> = if partial.is_empty() {
        all
    } else {
        let q = normalize(partial);
        let mut scored: Vec<(usize, usize, Param)> = all
            .into_iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let matched = matching_normalized(&q, &normalize(&p.value));
                (matched >= q.len() - q.len() / 4).then_some((matched, i, p))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(
                    usage
                        .get(&format!("{typed}{}", b.2.value))
                        .copied()
                        .unwrap_or(0)
                        .cmp(
                            &usage
                                .get(&format!("{typed}{}", a.2.value))
                                .copied()
                                .unwrap_or(0),
                        ),
                )
                .then(a.1.cmp(&b.1))
        });
        scored.into_iter().map(|(_, _, p)| p).collect()
    };
    (!items.is_empty()).then(|| Params {
        command: command.to_string(),
        typed,
        partial: partial.to_string(),
        items,
    })
}

/// How often each command has been run, as `key → count`.
pub type Usage = BTreeMap<String, u32>;

fn normalize(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn lcs(a: &[char], b: &[char]) -> usize {
    let mut row = vec![0usize; b.len() + 1];
    for &x in a {
        let mut diagonal = 0;
        for (j, &y) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if x == y {
                diagonal + 1
            } else {
                row[j + 1].max(row[j])
            };
            diagonal = above;
        }
    }
    row[b.len()]
}

/// How many characters of `query` are matched in `candidate`: the longest common
/// subsequence, inside any window of the candidate no wider than twice the query.
/// Case, spaces and punctuation are ignored.
pub fn matching_chars(query: &str, candidate: &str) -> usize {
    matching_normalized(&normalize(query), &normalize(candidate))
}

fn matching_normalized(q: &[char], c: &[char]) -> usize {
    if q.is_empty() || c.is_empty() {
        return 0;
    }
    let width = c.len().min(q.len() * 2);
    (0..=c.len() - width)
        .map(|start| lcs(q, &c[start..start + width]))
        .max()
        .unwrap_or(0)
}

/// Entries that match `query` well enough, best first. `popup` restricts matching to the bare
/// slash name (the completion popup); the palette matches titles too.
///
/// "Well enough" lets one character in four of the query go missing, so `raduce` and `modle`
/// still find what was meant, while two-letter queries must match whole.
pub fn rank<'a>(query: &str, entries: &'a [Entry], usage: &Usage, popup: bool) -> Vec<&'a Entry> {
    let q = normalize(query);
    let used = |e: &Entry| usage.get(&e.key).copied().unwrap_or(0);

    let mut scored: Vec<(usize, &Entry, usize)> = Vec::new();
    for entry in entries {
        if popup && entry.name.is_none() {
            continue;
        }
        if q.is_empty() {
            scored.push((0, entry, 0));
            continue;
        }
        let best = entry
            .haystacks(popup)
            .into_iter()
            .map(|h| {
                let c = normalize(h);
                (matching_normalized(&q, &c), c.len())
            })
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        if let Some((matched, len)) = best
            && matched >= q.len() - q.len() / 4
        {
            scored.push((matched, entry, len));
        }
    }
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(used(b.1).cmp(&used(a.1)))
            .then(a.2.cmp(&b.2))
            .then(a.1.title.cmp(&b.1.title))
    });
    scored.into_iter().map(|(_, e, _)| e).collect()
}

/// What the composer text asks the completion popup for: `Some(query)` while a command *name*
/// is being typed (`/mo`), `None` for anything else — ordinary prompts, or a command that
/// already has its arguments.
pub fn slash_query(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    (!rest.contains(char::is_whitespace)).then_some(rest)
}

/// What `/reduce` was given: a mode, if the first word is one, and what is left of it as what
/// the summary should keep ("the C2 findings"). Without a mode, a reduction by hand summarizes.
pub fn reduce_args(args: &str) -> (reactor_agent::entry::Mode, Option<String>) {
    use reactor_agent::entry::Mode;
    let args = args.trim();
    let (first, rest) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
    let (mode, focus) = match first {
        "fade" => (Mode::Fade, rest),
        "auto" => (Mode::Auto, rest),
        "compact" => (Mode::Compact, rest),
        _ => (Mode::Compact, args),
    };
    let focus = focus.trim();
    (mode, (!focus.is_empty()).then(|| focus.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduce_takes_a_mode_then_what_to_keep() {
        use reactor_agent::entry::Mode;
        assert_eq!(reduce_args(""), (Mode::Compact, None));
        assert_eq!(reduce_args("fade"), (Mode::Fade, None));
        assert_eq!(
            reduce_args("auto keep the C2 findings"),
            (Mode::Auto, Some("keep the C2 findings".into()))
        );
        assert_eq!(
            reduce_args("keep the C2 findings"),
            (Mode::Compact, Some("keep the C2 findings".into())),
            "no mode named: the whole of it is what to keep"
        );
        assert_eq!(reduce_args("  compact   "), (Mode::Compact, None));
    }

    fn titles<'a>(ranked: &[&'a Entry]) -> Vec<&'a str> {
        ranked.iter().map(|e| e.title.as_str()).collect()
    }

    fn names(ranked: &[&Entry]) -> Vec<String> {
        ranked.iter().map(|e| e.run.clone()).collect()
    }

    #[test]
    fn matching_chars_counts_what_lines_up_and_forgives_a_typo() {
        assert_eq!(matching_chars("reduce", "reduce"), 6);
        assert_eq!(
            matching_chars("raduce", "reduce"),
            5,
            "one wrong letter costs one match"
        );
        assert_eq!(matching_chars("redcue", "reduce"), 5, "a swap costs one");
        assert_eq!(
            matching_chars("reduce", "rduce"),
            5,
            "a missing letter costs one"
        );
        assert_eq!(
            matching_chars("Re-Duce", "reduce"),
            6,
            "case and punctuation do not matter"
        );
        assert_eq!(matching_chars("", "reduce"), 0);
    }

    #[test]
    fn scattered_letters_in_a_long_title_do_not_add_up() {
        // r…e…d…u…c…e are all in there, but nowhere near each other.
        assert!(
            matching_chars(
                "reduce",
                "Tool: install ridiculously-unusual-compiler-extras"
            ) < 5
        );
    }

    #[test]
    fn a_typo_still_finds_the_command() {
        let entries = commands();
        let ranked = rank("raduce", &entries, &Usage::new(), true);
        assert_eq!(ranked[0].run, "reduce");
        let ranked = rank("modle", &entries, &Usage::new(), true);
        assert_eq!(ranked[0].run, "model");
    }

    #[test]
    fn more_matching_characters_rank_higher() {
        let entries = commands();
        let ranked = rank("prev", &entries, &Usage::new(), true);
        assert_eq!(ranked[0].run, "preview");
        let ranked = rank("reactor-sc", &entries, &Usage::new(), true);
        assert_eq!(ranked[0].run, "reactor-scenario");
    }

    #[test]
    fn equal_matches_are_ordered_by_how_often_they_were_used() {
        let entries = commands();
        // `re` matches reduce, report, refresh, reactor-scenario … equally well.
        let plain = rank("re", &entries, &Usage::new(), true);
        let first_plain = plain[0].run.clone();
        let mut usage = Usage::new();
        usage.insert("report".into(), 5);
        usage.insert("refresh".into(), 2);
        let used = rank("re", &entries, &usage, true);
        assert_eq!(used[0].run, "report");
        assert_eq!(used[1].run, "refresh");
        assert_ne!(
            first_plain, "report",
            "the test needs frequency to have changed something"
        );
    }

    #[test]
    fn frequency_never_outranks_a_better_match() {
        let entries = commands();
        let mut usage = Usage::new();
        usage.insert("reduce".into(), 1000);
        let ranked = rank("report", &entries, &usage, true);
        assert_eq!(
            ranked[0].run, "report",
            "an exact match beats a much-used near one"
        );
    }

    #[test]
    fn an_empty_query_lists_by_frequency_and_then_keeps_the_catalogue_order() {
        let entries = commands();
        let mut usage = Usage::new();
        usage.insert("undo".into(), 3);
        let ranked = rank("", &entries, &usage, true);
        assert_eq!(ranked[0].run, "undo");
        assert_eq!(ranked.len(), entries.len());
    }

    #[test]
    fn nonsense_matches_nothing() {
        assert!(rank("zzzzqx", &commands(), &Usage::new(), true).is_empty());
        assert!(rank("zzzzqx", &build(&Snapshot::default()), &Usage::new(), false).is_empty());
    }

    #[test]
    fn the_popup_offers_commands_only_and_the_palette_everything() {
        let snapshot = Snapshot {
            models: vec!["ollama/qwen".into()],
            tools: vec![("adb".into(), true, true)],
            ..Default::default()
        };
        let entries = build(&snapshot);
        let popup = rank("", &entries, &Usage::new(), true);
        assert!(popup.iter().all(|e| e.name.is_some()));
        let palette = rank("", &entries, &Usage::new(), false);
        assert!(titles(&palette).contains(&"Model: switch to ollama/qwen"));
        assert!(titles(&palette).contains(&"Tool: disable adb"));
    }

    #[test]
    fn the_palette_finds_a_generated_entry_by_its_title() {
        let snapshot = Snapshot {
            tools: vec![("frida".into(), false, false), ("adb".into(), true, false)],
            scenarios: vec!["investigation".into()],
            ..Default::default()
        };
        let entries = build(&snapshot);
        let ranked = rank("install frida", &entries, &Usage::new(), false);
        assert_eq!(ranked[0].run, "install frida");
        let ranked = rank("enable adb", &entries, &Usage::new(), false);
        assert_eq!(ranked[0].run, "tool adb on");
        let ranked = rank("start invest", &entries, &Usage::new(), false);
        assert_eq!(names(&ranked)[0], "reactor-scenario start investigation");
    }

    #[test]
    fn a_tool_offers_only_the_action_that_changes_something() {
        let snapshot = Snapshot {
            tools: vec![("adb".into(), true, true)],
            ..Default::default()
        };
        let runs: Vec<String> = build(&snapshot).into_iter().map(|e| e.run).collect();
        assert!(runs.contains(&"tool adb off".to_string()));
        assert!(!runs.contains(&"tool adb on".to_string()));
    }

    #[test]
    fn the_popup_is_asked_for_only_while_a_name_is_being_typed() {
        assert_eq!(slash_query("/mo"), Some("mo"));
        assert_eq!(slash_query("/"), Some(""));
        assert_eq!(slash_query("/goal find the loader"), None);
        assert_eq!(slash_query("/goal "), None);
        assert_eq!(slash_query("hello /mo"), None);
    }

    #[test]
    fn every_slash_command_is_unique_and_documented() {
        let mut seen = std::collections::BTreeSet::new();
        for e in commands() {
            assert!(seen.insert(e.run.clone()), "{} twice", e.run);
            assert!(!e.detail.is_empty() && !e.title.is_empty());
        }
    }

    fn snap() -> Snapshot {
        Snapshot {
            models: vec!["anthropic/claude-x".into(), "ollama/llama".into()],
            tools: vec![
                ("ghidra".into(), true, true),
                ("radare2".into(), true, false),
                ("ida".into(), false, false),
            ],
            toolsets: vec![("re".into(), true)],
            scenarios: vec!["malware-triage".into()],
            identities: vec![
                "reverse-engineer".into(),
                "forensics".into(),
                "triage".into(),
            ],
            user_identities: vec!["triage".into()],
            reductions: vec![7, 9],
            ..Default::default()
        }
    }

    fn values(p: &Params) -> Vec<&str> {
        p.items.iter().map(|i| i.value.as_str()).collect()
    }

    #[test]
    fn a_commands_arguments_are_offered_once_its_name_and_a_space_are_typed() {
        let s = snap();
        let u = Usage::new();
        // Still a name: no parameters, the command popup's business.
        assert!(params("identity", &s, &u).is_none());
        assert!(params("/mod", &s, &u).is_none());
        // The identities, then the words /identity takes.
        let p = params("/identity ", &s, &u).unwrap();
        assert_eq!(
            &values(&p)[..3],
            ["reverse-engineer", "forensics", "triage"]
        );
        assert!(values(&p).contains(&"new") && values(&p).contains(&"manage"));
        assert_eq!(p.typed, "identity ");
        // What is typed narrows it, typos forgiven.
        let p = params("/identity forn", &s, &u).unwrap();
        assert_eq!(values(&p)[0], "forensics", "{:?}", values(&p));
        // A model, a layout, a panel, a reduction.
        assert_eq!(
            values(&params("model ", &s, &u).unwrap()),
            ["anthropic/claude-x", "ollama/llama"]
        );
        assert_eq!(values(&params("layout fo", &s, &u).unwrap())[0], "focus");
        assert!(values(&params("panel ", &s, &u).unwrap()).contains(&"graph"));
        assert_eq!(values(&params("undo ", &s, &u).unwrap()), ["7", "9"]);
        // A command with nothing to offer, or one that is not a command.
        assert!(
            params("goal ", &s, &u)
                .map(|p| values(&p) == ["clear"])
                .unwrap()
        );
        assert!(params("look ", &s, &u).is_none());
        assert!(params("nonsense x", &s, &u).is_none());
    }

    #[test]
    fn a_second_argument_depends_on_the_first() {
        let s = snap();
        let u = Usage::new();
        assert_eq!(
            values(&params("identity edit ", &s, &u).unwrap()),
            ["triage"],
            "only saved ones are edited"
        );
        assert_eq!(
            values(&params("identity delete ", &s, &u).unwrap()),
            ["triage"]
        );
        assert_eq!(values(&params("identity show ", &s, &u).unwrap()).len(), 3);
        assert!(
            params("identity new ", &s, &u).is_none(),
            "a new name is typed, not chosen"
        );
        assert_eq!(
            values(&params("context mode ", &s, &u).unwrap()),
            ["auto", "fade", "compact"]
        );
        assert_eq!(
            values(&params("report level ", &s, &u).unwrap()),
            ["0", "1", "2"]
        );
        assert_eq!(
            values(&params("reactor-scenario start ", &s, &u).unwrap()),
            ["malware-triage"]
        );
        // Tools: installed ones to switch, not-installed ones to install; on/off after the id.
        assert_eq!(
            values(&params("tool ", &s, &u).unwrap()),
            ["ghidra", "radare2"]
        );
        assert_eq!(values(&params("install ", &s, &u).unwrap()), ["ida"]);
        let p = params("tool ghidra ", &s, &u).unwrap();
        assert_eq!(values(&p), ["on", "off"]);
        assert_eq!(p.typed, "tool ghidra ");
        // Past the last argument there is nothing more to offer.
        assert!(params("tool ghidra on ", &s, &u).is_none());
    }

    #[test]
    fn choosing_a_parameter_runs_the_command_or_completes_it_to_go_on() {
        let s = snap();
        let u = Usage::new();
        let entries = params("tool gh", &s, &u).unwrap().entries();
        assert_eq!(entries[0].run, "tool ghidra");
        assert_eq!(
            entries[0].args,
            Args::Required,
            "on or off can follow: complete and wait"
        );
        let entries = params("model ", &s, &u).unwrap().entries();
        assert_eq!(entries[1].run, "model ollama/llama");
        assert_eq!(
            entries[1].args,
            Args::None,
            "a last argument runs the command"
        );
        assert_eq!(entries[1].title, "/model ollama/llama");
        // The most used comes first among equals.
        let mut used = Usage::new();
        used.insert("layout focus".into(), 5);
        assert_eq!(
            values(&params("layout ", &s, &used).unwrap())[0],
            "default",
            "no query: the order given"
        );
        // Equal matches: the one run more often first. A better match beats frequency.
        let p = params("layout c", &s, &used).unwrap();
        assert_eq!(values(&p)[0], "focus");
        let p = params("layout cat", &s, &used).unwrap();
        assert_eq!(values(&p)[0], "catalogue");
    }
}
