//! The session view model: what the transcript, status bar and composer render.
//!
//! The GUI holds *no facts*: the transcript is a function of the session store — the
//! full current branch, including everything a context reduction has hidden from the
//! model — plus the little that is still streaming and not yet in it. [`Session::rebuild`]
//! is that function; [`Session::apply`] folds in the live events (text deltas, a running
//! tool's output) between rebuilds. There is nothing to reconcile and nothing to drift:
//! a resumed session and a live one build the same rows (SPEC.md §2).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use reactor_agent::agent::Event;
use reactor_agent::context::{SessionState, active_reductions, hidden_by};
use reactor_agent::entry::{Block, Kind, Mode};
use reactor_agent::store::Store;
use reactor_context::settings::Settings;
use serde_json::Value;

/// One renderable row of the transcript.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatItem {
    /// The user's prompt (plain text — markdown is the assistant's voice).
    User { text: String },
    /// Streaming or final assistant text, rendered as markdown.
    AssistantText { text: String, streaming: bool },
    /// A thinking block, rendered collapsed.
    Thinking { text: String, streaming: bool },
    /// One tool call with its live/final state.
    ToolCall {
        call_id: String,
        name: String,
        args: Value,
        output: String,
        is_error: bool,
        done: bool,
    },
    /// A context reduction, where it happened (ADR-0037). `active` is false once undone.
    Reduction {
        entry: u64,
        mode: String,
        trigger: String,
        covers: usize,
        before: u64,
        after: u64,
        active: bool,
        summary: Option<String>,
    },
    /// Something went wrong that the person should see.
    Error { message: String },
    /// What a finished prompt cost: when it finished, how long it took, what it streamed.
    Stats {
        date: String,
        time: String,
        duration_ms: u64,
        tokens: u64,
        stream_ms: u64,
    },
}

/// The key of the log entry a finished prompt's statistics are kept under.
pub const STATS_KEY: &str = "turn_stats";

/// The prompt that is running now: when it began, and what it has streamed so far.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    started: Instant,
    /// Output tokens the provider has reported for finished rounds.
    reported_tokens: u64,
    /// Characters streamed in the round in flight, which the provider has not counted yet.
    round_chars: u64,
    /// When the round in flight began and last streamed.
    round_span: Option<(Instant, Instant)>,
    /// Time spent streaming in finished rounds.
    streamed: Duration,
}

impl Run {
    pub fn new(now: Instant) -> Run {
        Run {
            started: now,
            reported_tokens: 0,
            round_chars: 0,
            round_span: None,
            streamed: Duration::ZERO,
        }
    }

    /// Some text or thinking streamed in.
    pub fn delta(&mut self, chars: usize, now: Instant) {
        self.round_chars += chars as u64;
        self.round_span = Some(match self.round_span {
            Some((first, _)) => (first, now),
            None => (now, now),
        });
    }

    /// The provider reported a finished round's output tokens: its count replaces the estimate.
    pub fn usage(&mut self, output_tokens: u64) {
        self.reported_tokens += output_tokens;
        self.round_chars = 0;
        if let Some((first, last)) = self.round_span.take() {
            self.streamed += last - first;
        }
    }

    /// Tokens streamed so far: the provider's count for finished rounds, and four characters to
    /// a token for the round in flight.
    pub fn tokens(&self) -> u64 {
        self.reported_tokens + self.round_chars.div_ceil(4)
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.started)
    }

    /// Time spent actually streaming: what tokens per second is measured over, so a long tool
    /// run does not make the model look slow.
    pub fn stream_time(&self) -> Duration {
        self.streamed
            + self
                .round_span
                .map(|(first, last)| last - first)
                .unwrap_or_default()
    }
}

/// `850ms`, `12s`, `1m 12s`, `1h 02m`.
pub fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0 => format!("{ms}ms"),
        1..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60),
    }
}

/// The line under a finished prompt. The date is added for a prompt from another day.
pub fn stats_label(
    date: &str,
    time: &str,
    duration_ms: u64,
    tokens: u64,
    stream_ms: u64,
    today: &str,
) -> String {
    let mut out = format!(
        "finished {}{time} · took {}",
        if date == today {
            String::new()
        } else {
            format!("{date} ")
        },
        format_duration(duration_ms)
    );
    if tokens > 0 {
        out.push_str(&format!(" · {tokens} tokens"));
        // Over the time spent streaming when there is enough of it to measure, else the whole.
        let over = if stream_ms >= 200 {
            stream_ms
        } else {
            duration_ms
        };
        if over > 0 {
            out.push_str(&format!(
                " · {:.1} tokens/s",
                tokens as f64 * 1000.0 / over as f64
            ));
        }
    }
    out
}

/// Where in the turn cycle the agent is — drives the composer's mode (send vs queue) and
/// the interrupt affordance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentPhase {
    /// Settled; `Enter` sends.
    #[default]
    Idle,
    /// A turn is running; `Enter` queues a follow-up.
    Working,
    /// A manual context reduction is running.
    Compacting,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Session {
    /// Transcript rows, in log order.
    pub items: Vec<ChatItem>,
    /// Rows a context reduction currently hides from the model — shown dimmed, never removed.
    pub hidden: HashSet<usize>,
    /// The status bar's left side: key-sorted `(key, text)` — goal, identity, reporting, scenario.
    pub statuses: Vec<(String, String)>,
    /// Messages typed while a turn ran; sent, oldest first, as turns end.
    pub follow_up: Vec<String>,
    /// The model in use, `provider/name`.
    pub model: Option<String>,
    /// Estimated context use, percent of the usable window.
    pub context_percent: Option<u64>,
    pub phase: AgentPhase,
    pub last_error: Option<String>,
    /// The prompt running now.
    pub run: Option<Run>,
    // -- live, not yet in the store --
    live_text: String,
    live_thinking: String,
    live_tools: HashMap<String, String>,
    /// The last rebuild's row index of each tool card, for in-place output updates.
    card_at: HashMap<String, usize>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild every row from the store's current branch.
    pub fn rebuild(&mut self, store: &Store) {
        self.items.clear();
        self.hidden.clear();
        self.card_at.clear();
        let hidden = hidden_by(store);
        let active = active_reductions(store);

        for e in store.branch() {
            let first = self.items.len();
            match &e.kind {
                Kind::User { text } => self.items.push(ChatItem::User { text: text.clone() }),
                Kind::Assistant { blocks, .. } => {
                    for b in blocks {
                        match b {
                            Block::Thinking { text, .. } => self.items.push(ChatItem::Thinking {
                                text: text.clone(),
                                streaming: false,
                            }),
                            Block::Text { text } => self.items.push(ChatItem::AssistantText {
                                text: text.clone(),
                                streaming: false,
                            }),
                            Block::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                self.card_at.insert(id.clone(), self.items.len());
                                self.items.push(ChatItem::ToolCall {
                                    call_id: id.clone(),
                                    name: name.clone(),
                                    args: arguments.clone(),
                                    output: self.live_tools.get(id).cloned().unwrap_or_default(),
                                    is_error: false,
                                    done: false,
                                });
                            }
                        }
                    }
                }
                Kind::ToolResult {
                    call_id,
                    name,
                    content,
                    is_error,
                    ..
                } => {
                    match self.card_at.get(call_id).copied() {
                        Some(i) => {
                            if let ChatItem::ToolCall {
                                output,
                                is_error: err,
                                done,
                                ..
                            } = &mut self.items[i]
                            {
                                *output = content.clone();
                                *err = *is_error;
                                *done = true;
                            }
                        }
                        // A result whose call is not on this branch (a fork): still show it.
                        None => self.items.push(ChatItem::ToolCall {
                            call_id: call_id.clone(),
                            name: name.clone(),
                            args: Value::Null,
                            output: content.clone(),
                            is_error: *is_error,
                            done: true,
                        }),
                    }
                }
                Kind::Reduction(r) => self.items.push(ChatItem::Reduction {
                    entry: e.id,
                    mode: mode_word(r.mode).into(),
                    trigger: crate::backend::trigger_label(r.trigger).into(),
                    covers: r.covers.len(),
                    before: r.before_tokens,
                    after: r.after_tokens,
                    active: active.contains(&e.id),
                    summary: r.summary.clone(),
                }),
                Kind::Custom { key, data } if key == STATS_KEY => {
                    let text = |k: &str| {
                        data.get(k)
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string()
                    };
                    let number = |k: &str| data.get(k).and_then(Value::as_u64).unwrap_or(0);
                    self.items.push(ChatItem::Stats {
                        date: text("date"),
                        time: text("time"),
                        duration_ms: number("duration_ms"),
                        tokens: number("tokens"),
                        stream_ms: number("stream_ms"),
                    });
                }
                Kind::Session { .. }
                | Kind::Custom { .. }
                | Kind::Restore { .. }
                | Kind::Label { .. } => {}
            }
            if hidden.contains_key(&e.id) {
                self.hidden.extend(first..self.items.len());
            }
        }

        // What is streaming and not yet a log entry goes last.
        if !self.live_thinking.is_empty() {
            self.items.push(ChatItem::Thinking {
                text: self.live_thinking.clone(),
                streaming: true,
            });
        }
        if !self.live_text.is_empty() {
            self.items.push(ChatItem::AssistantText {
                text: self.live_text.clone(),
                streaming: true,
            });
        }
    }

    /// Fold in one live event. `store` is consulted only when an entry was appended.
    pub fn apply(&mut self, event: &Event, store: &Store) {
        match event {
            Event::Text(delta) => {
                if let Some(run) = &mut self.run {
                    run.delta(delta.chars().count(), Instant::now());
                }
                self.live_text.push_str(delta);
                match self.items.last_mut() {
                    Some(ChatItem::AssistantText {
                        text,
                        streaming: true,
                    }) => text.push_str(delta),
                    _ => self.items.push(ChatItem::AssistantText {
                        text: delta.clone(),
                        streaming: true,
                    }),
                }
            }
            Event::Thinking(delta) => {
                if let Some(run) = &mut self.run {
                    run.delta(delta.chars().count(), Instant::now());
                }
                self.live_thinking.push_str(delta);
                match self.items.iter_mut().rev().find(|i| {
                    matches!(
                        i,
                        ChatItem::Thinking {
                            streaming: true,
                            ..
                        }
                    )
                }) {
                    Some(ChatItem::Thinking { text, .. }) => text.push_str(delta),
                    _ => self.items.push(ChatItem::Thinking {
                        text: delta.clone(),
                        streaming: true,
                    }),
                }
            }
            Event::ToolOutput { id, chunk } => {
                self.live_tools
                    .entry(id.clone())
                    .or_default()
                    .push_str(chunk);
                if let Some(i) = self.card_at.get(id).copied()
                    && let Some(ChatItem::ToolCall {
                        output,
                        done: false,
                        ..
                    }) = self.items.get_mut(i)
                {
                    output.push_str(chunk);
                }
            }
            Event::Appended(id) => {
                // The reply that was streaming is an entry now; its live text is redundant.
                if matches!(
                    store.get(*id).map(|e| &e.kind),
                    Some(Kind::Assistant { .. })
                ) {
                    self.live_text.clear();
                    self.live_thinking.clear();
                }
                if matches!(
                    store.get(*id).map(|e| &e.kind),
                    Some(Kind::ToolResult { .. })
                ) {
                    self.live_tools.retain(|_, _| false);
                }
                self.rebuild(store);
            }
            Event::Finished => {
                self.live_text.clear();
                self.live_thinking.clear();
                self.live_tools.clear();
            }
            Event::Usage(usage) => {
                if let Some(run) = &mut self.run {
                    run.usage(usage.output_tokens);
                }
            }
            Event::ToolCallStarted { .. }
            | Event::ToolStart { .. }
            | Event::ToolEnd { .. }
            | Event::Reduced { .. }
            | Event::Notice(_) => {}
        }
    }

    /// The turn is over, however it ended: nothing is streaming any more.
    pub fn end_turn(&mut self, store: &Store) {
        self.live_text.clear();
        self.live_thinking.clear();
        self.live_tools.clear();
        self.phase = AgentPhase::Idle;
        self.rebuild(store);
    }
}

fn mode_word(m: Mode) -> &'static str {
    match m {
        Mode::Auto => "auto",
        Mode::Fade => "fade",
        Mode::Compact => "compact",
    }
}

/// The status bar's left side, from the session's state modules: what the manifest, the
/// identity, reporting and the scenario are doing right now.
pub fn status_items(
    state: &SessionState,
    settings: &Settings,
    scenarios_dir: &std::path::Path,
) -> Vec<(String, String)> {
    use reactor_context::{identity, reporting, scenario};
    let mut out: Vec<(String, String)> = Vec::new();

    let m = &state.manifest;
    if m.is_enabled() && m.has_content() {
        let head = m
            .goal
            .as_deref()
            .map(str::trim)
            .filter(|g| !g.is_empty())
            .map(|g| reactor_context::text::truncate(g, 48))
            .unwrap_or_else(|| "manifest".into());
        let steps = match m.steps.len() {
            0 => String::new(),
            n => format!(" · {n} step{}", if n == 1 { "" } else { "s" }),
        };
        out.push(("goal".into(), format!("◎ {head}{steps}")));
    }
    if let Some(name) = state.identity.active_name(&settings.identity) {
        out.push(("identity".into(), format!("@ {name}")));
    }
    if let Some(word) = reporting::status_word(&state.reporting, &settings.reporting) {
        out.push(("reporting".into(), format!("¶ reporting · {word}")));
    } else if state.reporting.is_enabled() {
        out.push(("reporting".into(), "¶ reporting".into()));
    }
    if let Some(s) = &state.scenario {
        let total = scenario::load_steps(scenarios_dir, &s.scenario_id).len();
        out.push((
            "scenario".into(),
            format!("{} · phase {}/{}", s.scenario_id, s.step_index + 1, total),
        ));
    }
    let _ = identity::BUILTINS;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reactor_agent::entry::{Reduction, Stub, Trigger};
    use serde_json::json;

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::create(d.path().join("s"), "s", d.path()).unwrap();
        (d, s)
    }

    fn call(s: &mut Store, id: &str, args: Value, result: &str) {
        s.append(Kind::Assistant {
            blocks: vec![
                Block::Text {
                    text: "running".into(),
                },
                Block::ToolCall {
                    id: id.into(),
                    name: "bash".into(),
                    arguments: args,
                },
            ],
            model: None,
            usage: None,
            stop: None,
        })
        .unwrap();
        s.append(Kind::ToolResult {
            call_id: id.into(),
            name: "bash".into(),
            content: result.into(),
            is_error: false,
            blob: None,
        })
        .unwrap();
    }

    #[test]
    fn a_stores_branch_becomes_transcript_rows_with_tool_results_attached_to_their_calls() {
        let (_d, mut s) = store();
        s.append(Kind::User {
            text: "hello".into(),
        })
        .unwrap();
        s.append(Kind::Assistant {
            blocks: vec![
                Block::Thinking {
                    text: "hmm".into(),
                    signature: None,
                },
                Block::Text { text: "hi".into() },
            ],
            model: None,
            usage: None,
            stop: None,
        })
        .unwrap();
        call(&mut s, "c1", json!({"command": "ls"}), "a b");
        s.append(Kind::Custom {
            key: "manifest".into(),
            data: json!({}),
        })
        .unwrap();

        let mut session = Session::new();
        session.rebuild(&s);
        assert_eq!(session.items.len(), 5, "{:#?}", session.items);
        assert!(matches!(&session.items[0], ChatItem::User { text } if text == "hello"));
        assert!(
            matches!(&session.items[1], ChatItem::Thinking { text, streaming: false } if text == "hmm")
        );
        assert!(matches!(&session.items[2], ChatItem::AssistantText { text, .. } if text == "hi"));
        assert!(
            matches!(&session.items[4], ChatItem::ToolCall { name, output, done: true, is_error: false, .. } if name == "bash" && output == "a b")
        );
    }

    #[test]
    fn a_call_with_no_result_yet_is_a_running_card_that_streams_output_in_place() {
        let (_d, mut s) = store();
        s.append(Kind::User { text: "go".into() }).unwrap();
        s.append(Kind::Assistant {
            blocks: vec![Block::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: json!({"command": "make"}),
            }],
            model: None,
            usage: None,
            stop: None,
        })
        .unwrap();
        let mut session = Session::new();
        session.rebuild(&s);
        assert!(
            matches!(&session.items[1], ChatItem::ToolCall { done: false, output, .. } if output.is_empty())
        );

        session.apply(
            &Event::ToolOutput {
                id: "c1".into(),
                chunk: "compiling…\n".into(),
            },
            &s,
        );
        session.apply(
            &Event::ToolOutput {
                id: "c1".into(),
                chunk: "linking\n".into(),
            },
            &s,
        );
        assert!(
            matches!(&session.items[1], ChatItem::ToolCall { done: false, output, .. } if output == "compiling…\nlinking\n")
        );

        // A rebuild in the middle (some other entry appended) does not lose the live output.
        session.rebuild(&s);
        assert!(
            matches!(&session.items[1], ChatItem::ToolCall { output, .. } if output == "compiling…\nlinking\n")
        );

        // The result lands: the card is done and shows the final text.
        s.append(Kind::ToolResult {
            call_id: "c1".into(),
            name: "bash".into(),
            content: "built".into(),
            is_error: false,
            blob: None,
        })
        .unwrap();
        session.apply(&Event::Appended(s.head()), &s);
        assert!(
            matches!(&session.items[1], ChatItem::ToolCall { done: true, output, .. } if output == "built")
        );
    }

    #[test]
    fn streamed_text_grows_one_row_and_is_replaced_by_the_entry_when_it_lands() {
        let (_d, mut s) = store();
        s.append(Kind::User { text: "hi".into() }).unwrap();
        let mut session = Session::new();
        session.rebuild(&s);
        for d in ["Hel", "lo ", "there"] {
            session.apply(&Event::Text(d.into()), &s);
        }
        assert_eq!(session.items.len(), 2);
        assert!(
            matches!(&session.items[1], ChatItem::AssistantText { text, streaming: true } if text == "Hello there")
        );

        let id = s
            .append(Kind::Assistant {
                blocks: vec![Block::Text {
                    text: "Hello there".into(),
                }],
                model: None,
                usage: None,
                stop: None,
            })
            .unwrap();
        session.apply(&Event::Appended(id), &s);
        assert_eq!(
            session.items.len(),
            2,
            "no duplicate: the live copy is dropped"
        );
        assert!(matches!(
            &session.items[1],
            ChatItem::AssistantText {
                streaming: false,
                ..
            }
        ));
    }

    #[test]
    fn a_cancelled_turn_leaves_no_ghost_streaming_row() {
        let (_d, mut s) = store();
        s.append(Kind::User { text: "hi".into() }).unwrap();
        let mut session = Session::new();
        session.rebuild(&s);
        session.apply(&Event::Text("half a sent".into()), &s);
        session.phase = AgentPhase::Working;
        session.end_turn(&s);
        assert_eq!(session.items.len(), 1);
        assert_eq!(session.phase, AgentPhase::Idle);
    }

    #[test]
    fn a_reduction_shows_where_it_happened_and_dims_what_it_hides_until_undone() {
        let (_d, mut s) = store();
        let u = s
            .append(Kind::User {
                text: "old question".into(),
            })
            .unwrap();
        let a = s
            .append(Kind::Assistant {
                blocks: vec![Block::Text {
                    text: "old answer".into(),
                }],
                model: None,
                usage: None,
                stop: None,
            })
            .unwrap();
        s.append(Kind::User {
            text: "new question".into(),
        })
        .unwrap();
        let red = s
            .append(Kind::Reduction(Reduction {
                mode: Mode::Auto,
                trigger: Trigger::Budget,
                covers: vec![u, a],
                summary: Some("summary".into()),
                stubs: vec![Stub {
                    entry: a,
                    what: "assistant message".into(),
                    detail: "old answer".into(),
                    bytes: 10,
                }],
                before_tokens: 900,
                after_tokens: 300,
            }))
            .unwrap();

        let mut session = Session::new();
        session.rebuild(&s);
        assert_eq!(
            session.hidden,
            HashSet::from([0, 1]),
            "the two rows the model no longer sees"
        );
        assert!(
            matches!(&session.items[3], ChatItem::Reduction { covers: 2, before: 900, after: 300, active: true, mode, trigger, .. } if mode == "auto" && trigger == "automatic")
        );

        s.append(Kind::Restore { reduction: red }).unwrap();
        session.rebuild(&s);
        assert!(session.hidden.is_empty());
        assert!(matches!(
            &session.items[3],
            ChatItem::Reduction { active: false, .. }
        ));
    }

    #[test]
    fn the_transcript_follows_the_current_branch_not_the_whole_tree() {
        let (_d, mut s) = store();
        let u = s.append(Kind::User { text: "q".into() }).unwrap();
        s.append(Kind::Assistant {
            blocks: vec![Block::Text {
                text: "first answer".into(),
            }],
            model: None,
            usage: None,
            stop: None,
        })
        .unwrap();
        s.set_head(u).unwrap();
        s.append(Kind::Assistant {
            blocks: vec![Block::Text {
                text: "second answer".into(),
            }],
            model: None,
            usage: None,
            stop: None,
        })
        .unwrap();
        let mut session = Session::new();
        session.rebuild(&s);
        let texts: Vec<_> = session
            .items
            .iter()
            .filter_map(|i| match i {
                ChatItem::AssistantText { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["second answer"]);
    }

    #[test]
    fn statuses_say_what_the_manifest_identity_reporting_and_scenario_are_doing() {
        let mut state = SessionState::default();
        let settings = Settings::default();
        let none = std::path::Path::new("/nonexistent");
        assert!(status_items(&state, &settings, none).is_empty());

        state.manifest.goal = Some("crack the license check".into());
        state.manifest.steps = vec![reactor_context::manifest::Step {
            summary: "a".into(),
            status: "b".into(),
        }];
        state.identity.active = Some("publisher".into());
        state.reporting.enabled = Some(true);
        state.reporting.level = reactor_context::settings::Enforcement::new(2);
        let items = status_items(&state, &settings, none);
        let keys: Vec<&str> = items.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["goal", "identity", "reporting"], "key-sorted");
        assert_eq!(items[0].1, "◎ crack the license check · 1 step");
        assert_eq!(items[1].1, "@ publisher");
        assert_eq!(items[2].1, "¶ reporting · strict");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(format_duration(850), "850ms");
        assert_eq!(format_duration(12_400), "12s");
        assert_eq!(format_duration(72_000), "1m 12s");
        assert_eq!(format_duration(3_720_000), "1h 02m");
    }

    #[test]
    fn a_run_counts_the_providers_tokens_for_finished_rounds_and_estimates_the_one_in_flight() {
        let t0 = Instant::now();
        let mut run = Run::new(t0);
        run.delta(400, t0 + Duration::from_secs(1));
        assert_eq!(
            run.tokens(),
            100,
            "four characters to a token until the provider says"
        );
        run.usage(120);
        assert_eq!(
            run.tokens(),
            120,
            "the provider's count replaces the estimate"
        );
        run.delta(40, t0 + Duration::from_secs(5));
        assert_eq!(run.tokens(), 130, "and the next round is estimated on top");
        assert_eq!(
            run.elapsed(t0 + Duration::from_secs(9)),
            Duration::from_secs(9)
        );
    }

    #[test]
    fn streaming_time_leaves_out_the_time_between_rounds() {
        let t0 = Instant::now();
        let mut run = Run::new(t0);
        run.delta(10, t0 + Duration::from_secs(1));
        run.delta(10, t0 + Duration::from_secs(3));
        run.usage(5); // the round streamed for 2 s
        // a tool ran for a minute, then a second round streamed for 1 s
        run.delta(10, t0 + Duration::from_secs(63));
        run.delta(10, t0 + Duration::from_secs(64));
        assert_eq!(run.stream_time(), Duration::from_secs(3));
    }

    #[test]
    fn the_annotation_says_when_how_long_and_how_fast() {
        let line = stats_label("2026-10-01", "14:03:12", 72_000, 850, 25_000, "2026-10-01");
        assert_eq!(
            line,
            "finished 14:03:12 · took 1m 12s · 850 tokens · 34.0 tokens/s"
        );
        let other_day = stats_label("2026-09-30", "23:59:01", 5_000, 0, 0, "2026-10-01");
        assert_eq!(
            other_day, "finished 2026-09-30 23:59:01 · took 5s",
            "the date for another day, and no rate without tokens"
        );
    }

    #[test]
    fn a_stats_entry_in_the_log_becomes_a_row_after_the_reply() {
        let (_d, mut s) = store();
        s.append(Kind::User { text: "go".into() }).unwrap();
        s.append(Kind::Assistant {
            blocks: vec![Block::Text {
                text: "done".into(),
            }],
            model: None,
            usage: None,
            stop: None,
        })
        .unwrap();
        s.append(Kind::Custom { key: STATS_KEY.into(), data: json!({"date": "2026-10-01", "time": "14:03:12", "duration_ms": 5000, "tokens": 40, "stream_ms": 2000}) }).unwrap();
        let mut session = Session::new();
        session.rebuild(&s);
        assert!(
            matches!(
                session.items.last(),
                Some(ChatItem::Stats {
                    tokens: 40,
                    duration_ms: 5000,
                    ..
                })
            ),
            "{:?}",
            session.items
        );
    }
}
