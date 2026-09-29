//! Reporting: keep an agent honest about writing up findings as it goes,
//! instead of reconstructing a report from memory once the analysis is done.
//! Port of `extensions/reporting/` (ADR-0023).
//!
//! Three enforcement levels, all opt-in and off by default:
//!
//! - **0** the system-prompt block only.
//! - **1** the block, plus: once the agent has gone `stepThreshold` tool calls
//!   without the folder changing, every subsequent model call carries a reminder
//!   until it changes again.
//! - **2** the same tracking, but once the threshold is crossed and the turn
//!   settles without a change, the turn is reverted and the same prompt resent
//!   with a demand appended — at most `maxReverts` times, after which it falls
//!   back to level-1 nagging so a session can never spin forever.
//!
//! "Did the folder change" is answered by asking the filesystem — a snapshot of
//! every file's size and mtime under the folder, diffed on each tool call —
//! never by inspecting which tool ran or what path it touched (ADR-0023). A
//! `bash` redirect, `git checkout` and a hand edit all count the same.
//!
//! [`Tracker`] is the runtime half: the counters that live for a session and
//! belong in no model context.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::notice::Notice;
use crate::settings::{Enforcement, ReportingSettings};
use crate::text::js_trim;

pub const ENTRY_TYPE: &str = "reactor-reporting";

/// Per-session reporting state — the persisted half.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SessionState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<Enforcement>,
}

impl SessionState {
    pub fn normalize(raw: &Value) -> SessionState {
        let Some(r) = raw.as_object() else { return SessionState::default() };
        SessionState {
            enabled: r.get("enabled").and_then(Value::as_bool),
            level: r.get("level").and_then(Value::as_u64).and_then(|n| u8::try_from(n).ok()).and_then(Enforcement::new),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(false)
    }

    /// The session's level, else the global one — [`crate::settings::resolve`]'s rule.
    pub fn level(&self, cfg: &ReportingSettings) -> Enforcement {
        crate::settings::resolve(&cfg.level, self.level.as_ref()).value
    }
}

/// The block for the system prompt.
pub fn block(cfg: &ReportingSettings) -> String {
    let mut lines = vec![
        "## Reporting mode".to_string(),
        format!(
            "Document findings as you go in `{}/` -- short, factual, plainly written; cite where in the target each finding came from (file:line, function/address, packet #, timestamp, ...). See the `reactor-reporting` skill for structure and style.",
            cfg.folder
        ),
    ];
    if let Some(t) = &cfg.template_path {
        lines.push(format!("Use the report structure in `{t}`."));
    }
    lines.join("\n")
}

/// The one word the footer shows for a level: none at 0, `low` at 1, `strict` at 2.
pub fn status_word(session: &SessionState, cfg: &ReportingSettings) -> Option<&'static str> {
    if !session.is_enabled() {
        return None;
    }
    match session.level(cfg).get() {
        0 => None,
        1 => Some("low"),
        _ => Some("strict"),
    }
}

// -- the folder snapshot ------------------------------------------------------

/// Relative path → (size, mtime in ns). The only "was it documented" signal.
pub type Snapshot = BTreeMap<String, (u64, i128)>;

/// Recursive file listing under `dir`. Missing or unreadable reads as empty — a
/// folder that does not exist yet has nothing in it, not an error. Symlinks are
/// skipped, as they were (`Dirent.isFile()` is false for one).
pub fn take_snapshot(dir: &Path) -> Snapshot {
    fn walk(d: &Path, prefix: &str, out: &mut Snapshot) {
        let Ok(entries) = std::fs::read_dir(d) else { return };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            let Ok(ty) = e.file_type() else { continue };
            if ty.is_dir() {
                walk(&e.path(), &rel, out);
            } else if ty.is_file() {
                // A file that vanished between readdir and stat (a race with
                // the agent's own write) is not worth failing the check over.
                if let Ok(meta) = e.metadata() {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos() as i128)
                        .unwrap_or(0);
                    out.insert(rel, (meta.len(), mtime));
                }
            }
        }
    }
    let mut out = Snapshot::new();
    walk(dir, "", &mut out);
    out
}

// -- the runtime half ---------------------------------------------------------

/// Counters that live for a session and belong in no model context.
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    /// `None` until the first check — that call establishes a baseline rather
    /// than crediting whatever was already there.
    snapshot: Option<Snapshot>,
    pub steps_since_change: usize,
    reverts_this_turn: usize,
    max_reverts_warned: bool,
    /// The last genuinely user-submitted prompt — what level 2 re-sends.
    base_prompt: String,
    /// True from the moment a revert's resend is dispatched until the next
    /// `before_agent_start` observes it, so that handler does not mistake the
    /// resend for a fresh prompt.
    pending_revert: bool,
}

impl Tracker {
    /// A new session, or a reload.
    pub fn new() -> Tracker {
        Tracker::default()
    }

    fn reset_counts(&mut self) {
        self.steps_since_change = 0;
        self.reverts_this_turn = 0;
        self.max_reverts_warned = false;
    }

    /// The agent is about to run on `prompt`. Returns the block to append to the
    /// system prompt, when reporting is on.
    pub fn before_agent_start(&mut self, session: &SessionState, cfg: &ReportingSettings, prompt: &str) -> Option<String> {
        if self.pending_revert {
            // Our own resend, not a fresh instruction: keep the *original* prompt
            // as `base_prompt` so a second revert re-sends that, not an already
            // demand-appended copy, and leave the revert count alone.
            self.pending_revert = false;
        } else {
            self.base_prompt = prompt.to_string();
            self.reverts_this_turn = 0;
            self.max_reverts_warned = false;
        }
        session.is_enabled().then(|| block(cfg))
    }

    /// A tool call finished; `now` is the folder as it is now.
    pub fn tool_end(&mut self, session: &SessionState, now: Snapshot) {
        if !session.is_enabled() {
            return;
        }
        match &self.snapshot {
            None => self.snapshot = Some(now),
            Some(prev) if *prev != now => {
                self.snapshot = Some(now);
                self.reset_counts();
            }
            Some(_) => self.steps_since_change += 1,
        }
    }

    /// Level 1: the reminder to append to the messages of a model call, if one is
    /// due.
    pub fn nag(&self, session: &SessionState, cfg: &ReportingSettings) -> Option<String> {
        if !session.is_enabled() || session.level(cfg).get() < 1 || self.steps_since_change < cfg.step_threshold {
            return None;
        }
        Some(format!(
            "[reactor-reporting] {} step(s) since `{}/` last changed (threshold {}). Stop and document your findings there now, per the reactor-reporting skill, before doing anything else.",
            self.steps_since_change, cfg.folder, cfg.step_threshold
        ))
    }

    /// Level 2: the turn has genuinely settled.
    pub fn settled(&mut self, session: &SessionState, cfg: &ReportingSettings) -> Settled {
        if !session.is_enabled() || session.level(cfg).get() < 2 || self.steps_since_change < cfg.step_threshold {
            return Settled::Nothing;
        }
        if self.reverts_this_turn >= cfg.max_reverts {
            if self.max_reverts_warned {
                return Settled::Nothing;
            }
            self.max_reverts_warned = true;
            return Settled::GaveUp(Notice::warning(format!(
                "reactor-reporting: gave up reverting after {} attempt(s) -- nagging instead. Document in {}/ to clear it.",
                cfg.max_reverts, cfg.folder
            )));
        }
        self.reverts_this_turn += 1;
        Settled::Revert
    }

    /// The revert has been carried out; the text to resend.
    pub fn demand(&mut self, cfg: &ReportingSettings) -> String {
        self.pending_revert = true;
        format!(
            "{}\n\n[reactor-reporting] You did not document your findings in {}/ before finishing that turn. Do it now, then continue.",
            self.base_prompt, cfg.folder
        )
    }
}

/// What a settled turn calls for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    Nothing,
    /// Revert the last turn and resend with [`Tracker::demand`].
    Revert,
    /// Reverting has not worked; say so once, and nag instead.
    GaveUp(Notice),
}

// -- /report ------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    pub notices: Vec<Notice>,
    /// The session state changed and must be stored.
    pub persist: bool,
    /// The global folder setting changed and must be written.
    pub save_folder: Option<String>,
}

fn just(n: Notice) -> Effects {
    Effects { notices: vec![n], ..Default::default() }
}

/// `Number(s)`, for the three values that matter.
fn js_level(s: Option<&str>) -> Option<Enforcement> {
    let n: f64 = s?.parse().ok()?;
    if n.fract() != 0.0 || !(0.0..=2.0).contains(&n) {
        return None;
    }
    Enforcement::new(n as u8)
}

/// `/report [on|off|level <n>|status|folder <path>|reset]`.
pub fn command(session: &mut SessionState, tracker: &mut Tracker, cfg: &mut ReportingSettings, args: &str) -> Effects {
    let mut words = args.split(crate::text::js_space).filter(|w| !w.is_empty());
    let sub = words.next();
    let rest: Vec<&str> = words.collect();

    match sub.unwrap_or("status") {
        "on" => {
            session.enabled = Some(true);
            Effects {
                notices: vec![Notice::info(format!("reactor-reporting: on, level {}", session.level(cfg).get()))],
                persist: true,
                ..Default::default()
            }
        }
        "off" => {
            session.enabled = Some(false);
            Effects { notices: vec![Notice::info("reactor-reporting: off")], persist: true, ..Default::default() }
        }
        "level" => match js_level(rest.first().copied()) {
            None => just(Notice::error("reactor-reporting: level needs 0, 1, or 2")),
            Some(n) => {
                *session = SessionState { enabled: Some(true), level: Some(n) };
                Effects {
                    notices: vec![Notice::info(format!("reactor-reporting: on, level {}", n.get()))],
                    persist: true,
                    ..Default::default()
                }
            }
        },
        "status" => just(Notice::info(if session.is_enabled() {
            format!(
                "reactor-reporting: on, level {}, folder \"{}\", {}/{} step(s) since it last changed",
                session.level(cfg).get(),
                cfg.folder,
                tracker.steps_since_change,
                cfg.step_threshold
            )
        } else {
            "reactor-reporting: off".to_string()
        })),
        "folder" => {
            let f = rest.join(" ");
            if f.is_empty() {
                return just(Notice::error("reactor-reporting: folder needs a path, e.g. `folder report`"));
            }
            cfg.folder = f.clone();
            // A different folder means whatever was counted against the old one is
            // meaningless: re-baseline and start the count over, as `reset` does.
            tracker.reset_counts();
            tracker.snapshot = None;
            Effects { notices: vec![Notice::info(format!("reactor-reporting: folder set to \"{f}\""))], persist: false, save_folder: Some(f) }
        }
        "reset" => {
            tracker.reset_counts();
            tracker.snapshot = None;
            just(Notice::info("reactor-reporting: counters reset"))
        }
        other => just(Notice::error(format!(
            "reactor-reporting: unknown subcommand \"{other}\" -- try on, off, level <0|1|2>, status, folder <path>, or reset"
        ))),
    }
}

/// Whether `s` would be accepted as a folder name at all (`writeGlobalConfig`
/// stores whatever it is given; only an empty one is refused, above).
pub fn folder_is_usable(s: &str) -> bool {
    !js_trim(s).is_empty()
}
