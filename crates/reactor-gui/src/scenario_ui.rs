//! What the Scenario chip and the pane's stepper show, as plain data: the scenarios there
//! are to choose from, or the one that is running with where it stands. Built from the
//! session's stored state and the scenario files, so it is the same whoever moved the
//! scenario on -- the agent with `reactor_phase_complete`, or the person with the stepper.

use reactor_context::scenario::{self, SKIPPED};
use reactor_core::scenarios::{Library, Listed, Scope};

/// One phase of the running scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase {
    pub title: String,
    /// The toolset it asks for.
    pub toolset: Option<String>,
    /// For a phase that is done: what it concluded (or that it was skipped, or that the
    /// summary is still to come).
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Not reached yet, or the one in progress.
    None,
    Summary(String),
    Skipped,
    /// Left by hand: the person's next prompt will say what it concluded.
    Awaiting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub id: String,
    /// Where the scenario was found: the case's own, the config directory's or a checkout's.
    pub scope: Scope,
    /// The phase in progress (0-based).
    pub current: usize,
    pub phases: Vec<Phase>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    /// The scenarios that can be started, with what each is for.
    pub available: Vec<Listed>,
    pub run: Option<Run>,
}

impl View {
    /// A scenario that can be started, by id.
    pub fn find(&self, id: &str) -> Option<&Listed> {
        self.available.iter().find(|l| l.id == id)
    }
}

pub fn view_of(state: Option<&scenario::State>, lib: &Library) -> View {
    let available = lib.list();
    let run = state.and_then(|s| {
        let scope = lib.find(&s.scenario_id)?.0;
        let steps = lib.load(&s.scenario_id);
        if steps.is_empty() {
            return None;
        }
        let phases = steps
            .iter()
            .enumerate()
            .map(|(i, step)| Phase {
                title: if step.title.is_empty() {
                    format!("Phase {}", i + 1)
                } else {
                    step.title.clone()
                },
                toolset: step.toolset.clone(),
                outcome: match s.summaries.get(i) {
                    Some(_) if s.pending_summary == Some(i) => Outcome::Awaiting,
                    Some(t) if t == SKIPPED => Outcome::Skipped,
                    Some(t) => Outcome::Summary(t.clone()),
                    // Left out at the start: it will be passed over (ADR-0065).
                    None if i > s.step_index && s.skip.contains(&i) => Outcome::Skipped,
                    None => Outcome::None,
                },
            })
            .collect();
        Some(Run {
            id: s.scenario_id.clone(),
            scope,
            current: s.step_index.min(steps.len() - 1),
            phases,
        })
    });
    View { available, run }
}

/// The tools of `toolset` that are not here (not installed, or an MCP server that is down):
/// the phase starts anyway, and the agent is simply not told about them (ADR-0007) -- the
/// person is, beside the phase.
pub fn missing_tools(
    toolset: &str,
    toolsets: Option<&reactor_client::ToolsetsPayload>,
    tools: Option<&reactor_client::ToolsPayload>,
) -> Vec<String> {
    let (Some(toolsets), Some(tools)) = (toolsets, tools) else {
        return Vec::new();
    };
    let Some(row) = toolsets.toolsets.iter().find(|t| t.id == toolset) else {
        return Vec::new();
    };
    row.tools
        .iter()
        .filter(|id| {
            tools
                .tools
                .iter()
                .find(|t| &t.id == *id)
                .is_some_and(|t| t.status != "present")
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (id, n) in [("alpha", 3), ("beta", 1)] {
            let p = d.path().join(id);
            std::fs::create_dir_all(&p).unwrap();
            for i in 1..=n {
                let head = if i == 2 {
                    "no frontmatter".to_string()
                } else {
                    format!("---\ntitle: P{i}\ntoolset: t{i}\n---\nbody")
                };
                std::fs::write(p.join(format!("{i:02}.md")), head).unwrap();
            }
        }
        d
    }

    #[test]
    fn nothing_running_offers_the_scenarios() {
        let d = dir();
        let v = view_of(None, &Library::single(d.path()));
        let ids: Vec<&str> = v.available.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["alpha", "beta"]);
        assert!(v.run.is_none());
    }

    #[test]
    fn a_running_scenario_says_where_each_phase_stands() {
        let d = dir();
        let s = scenario::State {
            scenario_id: "alpha".into(),
            step_index: 2,
            summaries: vec![scenario::SKIPPED.into(), "found it".into()],
            pending_summary: None,
            skip: vec![],
        };
        let run = view_of(Some(&s), &Library::single(d.path())).run.unwrap();
        assert_eq!((run.id.as_str(), run.current), ("alpha", 2));
        assert_eq!(run.phases[0].title, "P1");
        assert_eq!(run.phases[0].toolset.as_deref(), Some("t1"));
        assert_eq!(run.phases[0].outcome, Outcome::Skipped);
        assert_eq!(run.phases[1].title, "Phase 2", "no title is numbered");
        assert_eq!(run.phases[1].outcome, Outcome::Summary("found it".into()));
        assert_eq!(run.phases[2].outcome, Outcome::None);
    }

    #[test]
    fn a_phase_left_out_at_the_start_shows_as_skipped_before_it_is_reached() {
        let d = dir();
        let s = scenario::State {
            scenario_id: "alpha".into(),
            step_index: 0,
            summaries: vec![],
            pending_summary: None,
            skip: vec![2],
        };
        let run = view_of(Some(&s), &Library::single(d.path())).run.unwrap();
        assert_eq!(run.phases[1].outcome, Outcome::None);
        assert_eq!(run.phases[2].outcome, Outcome::Skipped);
    }

    #[test]
    fn a_phase_left_by_hand_awaits_its_summary() {
        let d = dir();
        let s = scenario::State {
            scenario_id: "alpha".into(),
            step_index: 1,
            summaries: vec![scenario::MANUAL.into()],
            pending_summary: Some(0),
            skip: vec![],
        };
        assert_eq!(
            view_of(Some(&s), &Library::single(d.path()))
                .run
                .unwrap()
                .phases[0]
                .outcome,
            Outcome::Awaiting
        );
    }

    #[test]
    fn a_scenario_that_is_gone_or_a_stale_index_does_not_break_it() {
        let d = dir();
        let gone = scenario::State {
            scenario_id: "nope".into(),
            step_index: 0,
            summaries: vec![],
            pending_summary: None,
            skip: vec![],
        };
        assert!(
            view_of(Some(&gone), &Library::single(d.path()))
                .run
                .is_none()
        );
        let stale = scenario::State {
            scenario_id: "beta".into(),
            step_index: 9,
            summaries: vec![],
            pending_summary: None,
            skip: vec![],
        };
        assert_eq!(
            view_of(Some(&stale), &Library::single(d.path()))
                .run
                .unwrap()
                .current,
            0
        );
    }
}

#[cfg(test)]
mod missing_tests {
    use super::*;

    fn payloads() -> (
        reactor_client::ToolsetsPayload,
        reactor_client::ToolsPayload,
    ) {
        let toolsets = serde_json::json!({"schema": 1, "toolsets": [
            {"id": "forensics", "desc": "", "active": false, "tools": ["here", "gone", "down"]},
            {"id": "empty", "desc": "", "active": false, "tools": []},
        ]});
        let row = |id: &str, status: &str| {
            serde_json::json!({
                "id": id, "name": id, "desc": "", "status": status, "active": false
            })
        };
        let tools = serde_json::json!({"schema": 1, "tools": [
            row("here", "present"), row("gone", "absent"), row("down", "absent")
        ]});
        (
            serde_json::from_value(toolsets).unwrap(),
            serde_json::from_value(tools).unwrap(),
        )
    }

    #[test]
    fn the_tools_of_a_toolset_that_are_not_here_are_named() {
        let (sets, tools) = payloads();
        assert_eq!(
            missing_tools("forensics", Some(&sets), Some(&tools)),
            ["gone", "down"]
        );
        assert!(missing_tools("empty", Some(&sets), Some(&tools)).is_empty());
        // A toolset that does not exist, or no catalogue yet, says nothing is missing.
        assert!(missing_tools("nope", Some(&sets), Some(&tools)).is_empty());
        assert!(missing_tools("forensics", None, None).is_empty());
    }
}
