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
            Required,
            "Set the session goal",
            "/goal <text> — or /goal clear",
        ),
        Entry::fixed(
            "guidelines",
            "Session",
            Required,
            "Set the session guidelines",
            "/guidelines <text> — or /guidelines clear",
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
            "Show the manifest",
            "what the model is told about goal, steps and guidelines",
        ),
        Entry::fixed(
            "identity",
            "Session",
            Optional,
            "Select or write the working persona",
            "/identity <name> | custom <text>",
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
            "/reactor-scenario list | start <id> | status | next | stop",
        ),
        Entry::fixed(
            "model",
            "Model",
            Required,
            "Switch model",
            "/model provider/name",
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
            "/reduce [auto|fade|compact]",
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
            "/panel transcript | tree | tools | toolsets | context | services | console",
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
            "the shipped scenarios",
            "reactor-scenario list",
        ),
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
            "begins with its first briefing",
            format!("reactor-scenario start {id}"),
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
