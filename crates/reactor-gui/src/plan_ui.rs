//! What a transcript pane shows of the plan besides the scenario: the goal, the guidelines
//! and the steps of the manifest, and the reporting switches -- as plain data from the
//! session's stored state and the settings -- and the two texts of the Frame popup.

use reactor_agent::context::SessionState;
use reactor_context::manifest;
use reactor_context::settings::Settings;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanView {
    pub goal: Option<String>,
    pub guidelines: Option<String>,
    /// `(status, summary)`, as the agent keeps them.
    pub steps: Vec<(String, String)>,
    /// The manifest switch: block and `update_steps` both go quiet when it is off.
    pub manifest_on: bool,
    pub report_on: bool,
    /// The level in force (the session's, else the default's), 0 to 2.
    pub report_level: u8,
    pub report_folder: String,
}

pub fn plan_of(state: &SessionState, settings: &Settings) -> PlanView {
    let m = &state.manifest;
    let clean = |t: &Option<String>| {
        t.as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    };
    PlanView {
        goal: clean(&m.goal),
        guidelines: clean(&m.guidelines),
        steps: m
            .steps
            .iter()
            .map(|s| (s.status.clone(), s.summary.clone()))
            .collect(),
        manifest_on: m.is_enabled(),
        report_on: state.reporting.is_enabled(),
        report_level: state.reporting.level(&settings.reporting).get(),
        report_folder: settings.reporting.folder.clone(),
    }
}

/// The two texts of the Frame popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// What the model is sent: the manifest block, verbatim. `None` when nothing is.
    pub sent: Option<String>,
    /// Why nothing is, when that is so.
    pub why_not: Option<&'static str>,
    /// What is kept: goal, guidelines, steps and the switch (what `/frame` prints).
    pub state: String,
}

pub fn frame_of(state: &manifest::State) -> Frame {
    let sent = manifest::block(state);
    let why_not = match (&sent, state.is_enabled(), state.has_content()) {
        (Some(_), _, _) => None,
        (None, false, _) => Some(
            "The manifest is switched off, so nothing is sent. The goal, guidelines and steps are kept for when it is switched on.",
        ),
        (None, true, _) => Some(
            "Nothing is set yet: with no goal and no guidelines there is no manifest to send, and the agent is not offered update_steps.",
        ),
    };
    Frame {
        sent,
        why_not,
        state: manifest::frame_view(state),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(f: impl FnOnce(&mut manifest::State)) -> SessionState {
        let mut s = SessionState::default();
        f(&mut s.manifest);
        s
    }

    #[test]
    fn the_plan_reads_the_manifest_and_the_reporting_switches() {
        let mut st = state(|m| {
            m.goal = Some("  find the check  ".into());
            m.guidelines = Some("   ".into());
            m.steps = vec![manifest::Step {
                summary: "locate it".into(),
                status: "in progress".into(),
            }];
        });
        st.reporting.enabled = Some(true);
        st.reporting.level = reactor_context::settings::Enforcement::new(2);
        let p = plan_of(&st, &Settings::default());
        assert_eq!(p.goal.as_deref(), Some("find the check"));
        assert_eq!(p.guidelines, None, "blank is none");
        assert_eq!(
            p.steps,
            [("in progress".to_string(), "locate it".to_string())]
        );
        assert!(p.manifest_on && p.report_on);
        assert_eq!((p.report_level, p.report_folder.as_str()), (2, "report"));
    }

    #[test]
    fn nothing_set_means_nothing_sent_and_the_popup_says_why() {
        let f = frame_of(&manifest::State::default());
        assert!(f.sent.is_none() && f.why_not.unwrap().contains("Nothing is set"));
        assert!(f.state.contains("goal: (none)") && f.state.contains("switch: on"));
    }

    #[test]
    fn a_switched_off_manifest_sends_nothing_but_keeps_what_it_has() {
        let m = manifest::State {
            goal: Some("g".into()),
            enabled: Some(false),
            ..Default::default()
        };
        let f = frame_of(&m);
        assert!(f.sent.is_none() && f.why_not.unwrap().contains("switched off"));
        assert!(f.state.contains("goal: g") && f.state.contains("switch: off"));
    }

    #[test]
    fn what_is_sent_is_the_block_verbatim() {
        let m = manifest::State {
            goal: Some("g".into()),
            guidelines: Some("be careful".into()),
            ..Default::default()
        };
        let f = frame_of(&m);
        assert_eq!(f.sent, manifest::block(&m));
        let sent = f.sent.unwrap();
        assert!(sent.contains("## Session Manifest") && sent.contains("## Session Guidelines"));
        assert!(f.why_not.is_none());
    }
}
