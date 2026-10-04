//! The documents panel: the case's reference documents, searched by the person and the
//! results inspected ([ADR-0046](../../../docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)).
//!
//! It runs the same search the agent's `docs_search` tool does (`reactor_agent::embed`), so
//! what a person sees here is what the agent would have been handed. The choice of keyword,
//! meaning or both is offered only where a search by meaning can be made — as for the
//! agent — and the reason it cannot is behind an (i).

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle,
    SharedString, StyledText, Window, div, px,
};
use reactor_agent::embed::Mode;
use reactor_db::corpus::{self, Hit, Passage};

use crate::app::ReactorApp;
use crate::backend::Backend;
use crate::hints;
use crate::panels::{close_button, closer, panel_title_closing};

/// How many passages a search returns.
const LIMIT: usize = 30;

/// The words of a query worth marking in a passage: what the keyword search looked for.
pub fn query_words(text: &str) -> Vec<String> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    // A one-letter word marks half the page; keep them only if nothing else is left.
    let long: Vec<String> = words
        .iter()
        .filter(|w| w.chars().count() > 1)
        .cloned()
        .collect();
    if long.is_empty() { words } else { long }
}

/// Where `words` occur in `text` (ignoring ASCII case), as non-overlapping byte ranges in
/// order.
pub fn find_words(text: &str, words: &[String]) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut found: Vec<Range<usize>> = Vec::new();
    for word in words {
        let w = word.as_bytes();
        if w.is_empty() || w.len() > bytes.len() {
            continue;
        }
        for start in 0..=bytes.len() - w.len() {
            let end = start + w.len();
            if text.is_char_boundary(start)
                && text.is_char_boundary(end)
                && bytes[start..end].eq_ignore_ascii_case(w)
            {
                found.push(start..end);
            }
        }
    }
    found.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in found {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

/// A few words of `text` around the first thing that matched, for the list.
pub fn snippet(text: &str, words: &[String], max_chars: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let at = find_words(&flat, words).first().map_or(0, |r| r.start);
    // Back up a little so the match has some company, to a character boundary.
    let mut start = flat[..at]
        .char_indices()
        .rev()
        .nth(40)
        .map_or(0, |(i, _)| i);
    // …and to the start of a word.
    if start > 0 {
        start = flat[start..]
            .find(' ')
            .map_or(start, |i| start + i + 1)
            .min(at);
    }
    let body: String = flat[start..].chars().take(max_chars).collect();
    let more = flat[start..].chars().count() > max_chars;
    format!(
        "{}{body}{}",
        if start > 0 { "…" } else { "" },
        if more { "…" } else { "" }
    )
}

/// `hits` as listed: as given, or closest in meaning first with those that have no
/// similarity last. Stable, so equal ones keep the search's order.
fn ordered(hits: &[Hit], by_similarity: bool) -> Vec<Hit> {
    let mut out = hits.to_vec();
    if by_similarity {
        let key = |h: &Hit| h.similarity.unwrap_or(f32::NEG_INFINITY);
        out.sort_by(|a, b| key(b).total_cmp(&key(a)));
    }
    out
}

/// The order the results are listed in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    /// As the search ranked them: keyword and meaning ranks fused. A passage ranked high by
    /// its words can come before one that is closer in meaning.
    Relevance,
    /// Closest in meaning first; those with no similarity (found by words only) last.
    Similarity,
}

/// What a person can ask of the search.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    Both,
    Keyword,
    Meaning,
}

impl Choice {
    fn mode(self) -> Mode {
        match self {
            Choice::Both => Mode::Both,
            Choice::Keyword => Mode::Keyword,
            Choice::Meaning => Mode::Meaning,
        }
    }
}

pub struct DocsPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    focus: FocusHandle,
    backend: Option<Arc<Backend>>,
    input: Entity<InputState>,
    choice: Choice,
    /// A search by meaning can be made: the model's name, or why not.
    meaning: std::result::Result<String, String>,
    /// Documents and passages in the case.
    counts: (usize, usize),
    /// The results as the search ranked them, and as they are listed now.
    found: Vec<Hit>,
    hits: Vec<Hit>,
    sort: Sort,
    /// The words the results are for, to mark in them.
    words: Vec<String>,
    /// How the last search ran, in a line.
    status: String,
    error: Option<String>,
    searching: bool,
    /// Told apart so that a slow answer to an old question does not replace a new one.
    generation: u64,
    selected: Option<usize>,
    /// Show the passages around the selected one.
    around: bool,
    context: Vec<Passage>,
    /// The query the results are for, as typed.
    query: String,
    /// Mark the sentences of the selected passage that are closest to the query in meaning.
    why: bool,
    /// Those sentences, with how close each is (best first), for the selected passage.
    why_marks: Vec<reactor_agent::embed::Sentence>,
    why_note: Option<String>,
}

impl DocsPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search the documents…"));
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.search(cx);
            }
        })
        .detach();
        Self {
            app,
            focus: cx.focus_handle(),
            backend: None,
            input,
            choice: Choice::Both,
            meaning: Err(String::new()),
            counts: (0, 0),
            found: Vec::new(),
            hits: Vec::new(),
            sort: Sort::Relevance,
            words: Vec::new(),
            status: String::new(),
            error: None,
            searching: false,
            generation: 0,
            selected: None,
            around: false,
            context: Vec::new(),
            query: String::new(),
            why: false,
            why_marks: Vec::new(),
            why_note: None,
        }
    }

    /// The panel reads the case through the backend, handed over once it exists.
    pub fn set_backend(&mut self, backend: Option<Arc<Backend>>, cx: &mut Context<Self>) {
        self.backend = backend;
        self.refresh(cx);
    }

    /// Read what the case holds and whether meaning can be searched, again: after an
    /// import, and when the vectors are complete.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        self.counts = corpus::counts(b.case.db()).unwrap_or((0, 0));
        self.meaning = b.meaning_status();
        if self.meaning.is_err() && self.choice == Choice::Meaning {
            self.choice = Choice::Both;
        }
        cx.notify();
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.refresh(cx);
        self.generation += 1;
        let generation = self.generation;
        self.searching = true;
        self.error = None;
        self.status = "searching…".into();
        let handle = b.search_docs(text.clone(), LIMIT, self.choice.mode());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.searching = false;
                match outcome {
                    Ok(Ok(found)) => {
                        this.status = format!(
                            "{} passage{} {}",
                            found.hits.len(),
                            if found.hits.len() == 1 { "" } else { "s" },
                            {
                                let close = if found.min_similarity > 0.0 {
                                    format!(", similarity of at least {}", found.min_similarity)
                                } else {
                                    String::new()
                                };
                                match (this.choice, &found.meaning) {
                                    (Choice::Keyword, _) => "by keyword".to_string(),
                                    (Choice::Meaning, Ok(m)) => {
                                        format!("by meaning ({}{close})", m.name)
                                    }
                                    (_, Ok(m)) => {
                                        format!("by keyword and meaning ({}{close})", m.name)
                                    }
                                    (_, Err(_)) => "by keyword".to_string(),
                                }
                            }
                        );
                        this.words = query_words(&text);
                        this.query = text.clone();
                        this.found = found.hits;
                        this.arrange();
                        this.select(if this.hits.is_empty() { None } else { Some(0) }, cx);
                    }
                    Ok(Err(e)) => {
                        this.error = Some(e);
                        this.status.clear();
                    }
                    Err(e) => {
                        this.error = Some(format!("the search stopped: {e}"));
                        this.status.clear();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        self.selected = index;
        self.load_context();
        self.explain(cx);
        cx.notify();
    }

    /// List the results in the order chosen. The sort is stable: ties keep the search's order.
    fn arrange(&mut self) {
        self.hits = ordered(&self.found, self.sort == Sort::Similarity);
    }

    /// Change the order, keeping the same passage selected.
    fn set_sort(&mut self, sort: Sort, cx: &mut Context<Self>) {
        let chunk = self
            .selected
            .and_then(|i| self.hits.get(i))
            .map(|h| h.chunk_id);
        self.sort = sort;
        self.arrange();
        self.selected = chunk.and_then(|c| self.hits.iter().position(|h| h.chunk_id == c));
        self.load_context();
        cx.notify();
    }

    /// Show passages that were handed over — the agent's search result, opened from the
    /// transcript — as if they had been searched for here, with `selected` chosen.
    pub fn show(
        &mut self,
        query: String,
        hits: Vec<Hit>,
        selected: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh(cx);
        // Whatever search was in flight is no longer the one being looked at.
        self.generation += 1;
        self.searching = false;
        self.error = None;
        self.input
            .update(cx, |s, cx| s.set_value(query.clone(), window, cx));
        self.words = query_words(&query);
        self.status = format!(
            "the agent's search for `{query}`: {} passage{}",
            hits.len(),
            if hits.len() == 1 { "" } else { "s" }
        );
        self.query = query;
        self.found = hits;
        self.arrange();
        let selected = Some(selected).filter(|i| *i < self.hits.len());
        self.select(
            selected.or(if self.hits.is_empty() { None } else { Some(0) }),
            cx,
        );
    }

    /// Ask the embedding model which sentences of the selected passage are closest in
    /// meaning to the query, when that is being shown. The passage's text goes to the
    /// provider, so it is only done while the person has it switched on.
    fn explain(&mut self, cx: &mut Context<Self>) {
        self.why_marks.clear();
        self.why_note = None;
        let (Some(b), Some(i), true) = (self.backend.clone(), self.selected, self.why) else {
            return;
        };
        let Some(hit) = self.hits.get(i) else { return };
        let chunk = hit.chunk_id;
        let handle = b.explain_hit(self.query.clone(), hit.text.clone());
        self.generation += 1;
        let generation = self.generation;
        self.why_note = Some("asking the model…".into());
        cx.spawn(async move |this, cx| {
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                // Still the same passage, and nothing newer asked.
                let same = this
                    .selected
                    .and_then(|i| this.hits.get(i))
                    .map(|h| h.chunk_id)
                    == Some(chunk);
                if this.generation != generation || !same || !this.why {
                    return;
                }
                match outcome {
                    Ok(Ok(ranked)) => {
                        // The best, and whatever is nearly as good: up to three.
                        let best = ranked.first().map_or(0., |s| s.score);
                        this.why_marks = ranked
                            .into_iter()
                            .filter(|s| s.score >= best - 0.08)
                            .take(3)
                            .collect();
                        this.why_note = Some(match this.why_marks.as_slice() {
                            [] => "nothing to compare".to_string(),
                            marks => format!(
                                "closest in meaning: {}",
                                marks
                                    .iter()
                                    .map(|s| format!("{:.2}", s.score))
                                    .collect::<Vec<_>>()
                                    .join(" · ")
                            ),
                        });
                    }
                    Ok(Err(e)) => this.why_note = Some(e),
                    Err(e) => this.why_note = Some(format!("stopped: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The passages around the selected one, when they are being shown.
    fn load_context(&mut self) {
        self.context.clear();
        let (Some(b), Some(i), true) = (self.backend.clone(), self.selected, self.around) else {
            return;
        };
        if let Some(hit) = self.hits.get(i) {
            self.context = corpus::surrounding(b.case.db(), hit.chunk_id, 2, 2).unwrap_or_default();
        }
    }

    /// Open the document the selected passage came from: a file in its viewer, a page in
    /// the browser.
    fn open_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hit) = self.selected.and_then(|i| self.hits.get(i)) else {
            return;
        };
        let uri = hit.source_uri.split('#').next().unwrap_or("").to_string();
        if uri.starts_with("http://") || uri.starts_with("https://") {
            cx.open_url(&uri);
        } else if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.open_reference(&uri, None, Default::default(), window, cx)
            });
        }
    }

    fn copy_passage(&mut self, cx: &mut Context<Self>) {
        if let Some(hit) = self.selected.and_then(|i| self.hits.get(i)) {
            cx.write_to_clipboard(ClipboardItem::new_string(hit.text.clone()));
        }
    }
}

impl Focusable for DocsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for DocsPanel {}

impl gpui_kit::base::dock::Panel for DocsPanel {
    fn panel_name(&self) -> &'static str {
        "docs"
    }
}

impl Panel for DocsPanel {
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_closing(
            IconName::BookOpen,
            "Documents",
            closer(&self.app, cx.entity()),
        )
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![
            Button::new("docs-refresh")
                .icon(IconName::RotateCw)
                .ghost()
                .small()
                .tooltip("Read the case's documents again")
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            close_button(&self.app, cx.entity()),
        ])
    }
}

/// What a stretch of a passage is marked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// A word of the query.
    Word,
    /// A sentence close in meaning to the query.
    Sentence,
    /// A word of the query inside such a sentence.
    Both,
}

/// The stretches of a text that are marked, in order and without overlap, each for what
/// it is: a word, a sentence, or a word within a sentence.
pub fn layers(words: &[Range<usize>], sentences: &[Range<usize>]) -> Vec<(Range<usize>, Layer)> {
    let mut points: Vec<usize> = words
        .iter()
        .chain(sentences)
        .flat_map(|r| [r.start, r.end])
        .collect();
    points.sort_unstable();
    points.dedup();
    let inside =
        |set: &[Range<usize>], a: usize, b: usize| set.iter().any(|r| r.start <= a && b <= r.end);
    let mut out: Vec<(Range<usize>, Layer)> = Vec::new();
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let layer = match (inside(words, a, b), inside(sentences, a, b)) {
            (true, true) => Layer::Both,
            (true, false) => Layer::Word,
            (false, true) => Layer::Sentence,
            (false, false) => continue,
        };
        match out.last_mut() {
            Some((r, l)) if *l == layer && r.end == a => r.end = b,
            _ => out.push((a..b, layer)),
        }
    }
    out
}

/// A passage's text with the query's words marked and, if asked, the sentences closest in
/// meaning to it.
fn marked(
    text: &str,
    words: &[String],
    sentences: &[Range<usize>],
    theme: &gpui_kit::component::Theme,
) -> StyledText {
    let style = |colour: Hsla| HighlightStyle {
        background_color: Some(colour),
        ..Default::default()
    };
    let highlights = layers(&find_words(text, words), sentences)
        .into_iter()
        .map(|(r, layer)| {
            (
                r,
                style(match layer {
                    Layer::Word => theme.warning.opacity(0.35),
                    Layer::Sentence => theme.primary.opacity(0.2),
                    Layer::Both => theme.warning.opacity(0.5),
                }),
            )
        })
        .collect::<Vec<_>>();
    StyledText::new(SharedString::from(text.to_string())).with_highlights(highlights)
}

impl Render for DocsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let small = theme.font_size * 0.85;
        let meaning_ok = self.meaning.is_ok();

        // How to search: only what can be done. Where meaning cannot be searched there is
        // no choice to make, and the reason is behind the (i).
        let choose = |id: &'static str, label: &'static str, tip: &'static str, this_choice| {
            let b = Button::new(id).small().label(label);
            let b = if self.choice == this_choice {
                b.primary()
            } else {
                b.ghost()
            };
            b.tooltip(tip).on_click(cx.listener(move |this, _, _, cx| {
                this.choice = this_choice;
                cx.notify();
            }))
        };
        let modes = if meaning_ok {
            h_flex()
                .gap_1()
                .child(choose(
                    "docs-both",
                    "Both",
                    "Keyword and meaning, fused: the default",
                    Choice::Both,
                ))
                .child(choose(
                    "docs-keyword",
                    "Keyword",
                    "The words as written: identifiers, hashes, addresses",
                    Choice::Keyword,
                ))
                .child(choose(
                    "docs-meaning",
                    "Meaning",
                    "By meaning: ask in your own words",
                    Choice::Meaning,
                ))
                .into_any_element()
        } else {
            h_flex()
                .child(hints::info_button(
                    "docs-no-meaning",
                    format!(
                        "Searching by keyword. {} Meaning can be searched once a model has made vectors for every passage.",
                        self.meaning.as_ref().err().cloned().unwrap_or_default()
                    ),
                ))
                .into_any_element()
        };

        let top = h_flex()
            .gap_2()
            .p_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(div().flex_1().child(Input::new(&self.input).small()))
            .child(modes)
            .child(
                Button::new("docs-search")
                    .small()
                    .primary()
                    .icon(IconName::Search)
                    .label("Search")
                    .on_click(cx.listener(|this, _, _, cx| this.search(cx))),
            );

        let status = if let Some(e) = &self.error {
            div()
                .text_color(theme.danger)
                .child(SharedString::from(e.clone()))
        } else if self.status.is_empty() {
            div().text_color(theme.muted_foreground).child(format!(
                "{} document{} · {} passage{}",
                self.counts.0,
                if self.counts.0 == 1 { "" } else { "s" },
                self.counts.1,
                if self.counts.1 == 1 { "" } else { "s" },
            ))
        } else {
            div()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(self.status.clone()))
        };
        // The order: only worth a choice where some result has a similarity to order by.
        let order = self
            .hits
            .iter()
            .any(|h| h.similarity.is_some())
            .then(|| {
                let pick = |id: &'static str, label: &'static str, tip: &'static str, sort| {
                    let b = Button::new(id).small().label(label);
                    let b = if self.sort == sort { b.primary() } else { b.ghost() };
                    b.tooltip(tip)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_sort(sort, cx)))
                };
                h_flex()
                    .gap_1()
                    .child(pick(
                        "docs-sort-relevance",
                        "Relevance",
                        "As the search ranked them: the keyword and meaning ranks fused, so a passage ranked high by its words can come before one that is closer in meaning",
                        Sort::Relevance,
                    ))
                    .child(pick(
                        "docs-sort-similarity",
                        "Similarity",
                        "Closest in meaning first; passages found only by their words come last",
                        Sort::Similarity,
                    ))
            });
        let status = h_flex()
            .px_2()
            .py_1()
            .justify_between()
            .items_center()
            .text_size(small)
            .child(status)
            .children(order);

        // The results.
        let list = v_flex()
            .id("docs-results")
            .size_full()
            .overflow_y_scrollbar()
            .children(self.hits.iter().enumerate().map(|(i, hit)| {
                let selected = self.selected == Some(i);
                let title = hit
                    .title
                    .clone()
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| hit.source_uri.clone());
                let ranks = format!(
                    "{}{}",
                    hit.keyword_rank.map_or(String::new(), |r| format!("K{r}")),
                    hit.meaning_rank
                        .map_or(String::new(), |r| format!("{}M{r}", if hit.keyword_rank.is_some() { " · " } else { "" })),
                );
                // The number after them is how close in meaning, which is what the threshold in
                // the settings is about.
                let ranks = match hit.similarity {
                    Some(sim) => format!("{ranks} · {sim:.2}"),
                    None => ranks,
                };
                let ranks_tip = format!(
                    "Rank in the keyword search (K) and the meaning search (M); the number is the similarity in meaning, cosine, 1 is identical, larger is closer (the Meaning threshold in the settings is a limit on it). The order is by a score made of the ranks: {:.4}",
                    hit.score
                );
                v_flex()
                    .id(("docs-hit", i))
                    .gap_0p5()
                    .px_2()
                    .py_1p5()
                    .border_b_1()
                    .border_color(theme.border.opacity(0.5))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme.primary.opacity(0.15)))
                    .hover(|s| s.bg(theme.list_hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(Some(i), cx)))
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_between()
                            .child(
                                div()
                                    .truncate()
                                    .text_color(theme.foreground)
                                    .child(SharedString::from(title)),
                            )
                            .child(hints::tip(
                                div()
                                    .id(("docs-rank", i))
                                    .flex_none()
                                    .text_size(small)
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from(ranks)),
                                ranks_tip,
                            )),
                    )
                    .when(!hit.heading_path.is_empty(), |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_size(small)
                                .text_color(theme.info)
                                .child(SharedString::from(hit.heading_path.clone())),
                        )
                    })
                    .child(
                        div()
                            .text_size(small)
                            .text_color(theme.muted_foreground)
                            .line_clamp(3)
                            .child(marked(&snippet(&hit.text, &self.words, 220), &self.words, &[], &theme)),
                    )
            }))
            .when(self.hits.is_empty() && !self.searching, |d| {
                d.child(
                    h_flex()
                        .p_3()
                        .gap_1()
                        .items_center()
                        .text_color(theme.muted_foreground)
                        .child(if self.counts.1 == 0 {
                            "No documents in this case"
                        } else if self.status.is_empty() {
                            "Type a search"
                        } else {
                            "Nothing found"
                        })
                        .child(hints::info_button(
                            "docs-empty-info",
                            "Documents are imported from the menu (Import Documents…) or with /import: files, folders, URLs and wiki dumps. They are searched by the words in them, and by meaning once an embedding model has made vectors for them.",
                        ))
                        .when(self.counts.1 == 0, |d| {
                            d.child(
                                Button::new("docs-import")
                                    .small()
                                    .label("Import…")
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        window.dispatch_action(
                                            Box::new(crate::app::ImportDocuments),
                                            cx,
                                        )
                                    })),
                            )
                        }),
                )
            });

        // The selected passage, in full.
        let detail = self.selected.and_then(|i| self.hits.get(i)).map(|hit| {
            let row = |label: &'static str, value: String| {
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .w(px(72.))
                            .flex_none()
                            .text_color(theme.muted_foreground)
                            .child(label),
                    )
                    .child(div().flex_1().min_w_0().child(SharedString::from(value)))
            };
            let mut meta: Vec<(String, String)> = hit
                .meta
                .as_object()
                .map(|o| {
                    o.iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                v.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| v.to_string()),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            meta.sort();
            let can_open = hit.source_kind != "import";
            let by_meaning: Vec<Range<usize>> =
                self.why_marks.iter().map(|s| s.range.clone()).collect();
            let passage = |text: &str, strong: bool| {
                div()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if strong {
                        theme.primary
                    } else {
                        theme.border.opacity(0.6)
                    })
                    .when(!strong, |d| d.text_color(theme.muted_foreground))
                    .child(marked(
                        text,
                        &self.words,
                        if strong { &by_meaning } else { &[] },
                        &theme,
                    ))
            };
            v_flex()
                .id("docs-detail")
                .size_full()
                .overflow_y_scrollbar()
                .p_2()
                .gap_2()
                .text_size(small)
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Button::new("docs-open")
                                .small()
                                .label("Open source")
                                .disabled(!can_open)
                                .tooltip("Open the document this came from")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.open_source(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("docs-copy")
                                .small()
                                .ghost()
                                .icon(IconName::Copy)
                                .tooltip("Copy the passage")
                                .on_click(cx.listener(|this, _, _, cx| this.copy_passage(cx))),
                        )
                        .when(meaning_ok, |row| {
                            let b = Button::new("docs-why")
                                .small()
                                .label("Why")
                                .tooltip("Mark the sentences closest in meaning to the query. Asks the embedding model, so this passage's text is sent to its provider");
                            let b = if self.why { b.primary() } else { b.ghost() };
                            row.child(b.on_click(cx.listener(|this, _, _, cx| {
                                this.why = !this.why;
                                this.explain(cx);
                                cx.notify();
                            })))
                        })
                        .child({
                            let b = Button::new("docs-around")
                                .small()
                                .label("Around it")
                                .tooltip("Show the passages before and after it in the document");
                            let b = if self.around { b.primary() } else { b.ghost() };
                            b.on_click(cx.listener(|this, _, _, cx| {
                                this.around = !this.around;
                                this.load_context();
                                cx.notify();
                            }))
                        }),
                )
                .children(self.why_note.clone().filter(|_| self.why).map(|n| {
                    div()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(n))
                }))
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(row(
                            "Document",
                            hit.title
                                .clone()
                                .filter(|t| !t.is_empty())
                                .unwrap_or_else(|| "(untitled)".into()),
                        ))
                        .when_some(hit.similarity, |d, sim| {
                            d.child(row("Meaning", format!("similarity {sim:.3}")))
                        })
                        .child(row("Source", hit.source_uri.clone()))
                        .when(!hit.heading_path.is_empty(), |d| {
                            d.child(row("Section", hit.heading_path.clone()))
                        })
                        .child(row(
                            "Kind",
                            format!(
                                "{} · {}{}",
                                hit.source_kind,
                                hit.mime.clone().unwrap_or_else(|| "unknown type".into()),
                                if hit.origin == "external" { "" } else { " · " },
                            ) + if hit.origin == "external" {
                                ""
                            } else {
                                hit.origin.as_str()
                            },
                        ))
                        .children(meta.into_iter().map(|(k, v)| row("", format!("{k}: {v}")))),
                )
                .children(if self.around && !self.context.is_empty() {
                    self.context
                        .iter()
                        .map(|p| {
                            let is_hit = p.chunk_id == hit.chunk_id;
                            v_flex()
                                .gap_0p5()
                                .when(!p.heading_path.is_empty() && !is_hit, |d| {
                                    d.child(
                                        div()
                                            .text_color(theme.info)
                                            .child(SharedString::from(p.heading_path.clone())),
                                    )
                                })
                                .child(passage(&p.text, is_hit))
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                } else {
                    vec![passage(&hit.text, true).into_any_element()]
                })
        });

        v_flex()
            .id("docs-panel")
            .track_focus(&self.focus)
            .size_full()
            .bg(theme.background)
            .child(top)
            .child(status)
            .child(
                // The list and the passage share the width; the divider between them is
                // dragged. (The size is kept while the window is open.)
                div()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        h_resizable("docs-split")
                            .child(
                                resizable_panel()
                                    .size(px(380.))
                                    .size_range(px(160.)..px(1200.))
                                    .child(list),
                            )
                            .child(resizable_panel().size_range(px(240.)..px(4000.)).child(
                                detail.map_or_else(
                                    || div().size_full().into_any_element(),
                                    |d| d.into_any_element(),
                                ),
                            )),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Layer, find_words, layers, ordered, query_words, snippet};

    fn words(s: &str) -> Vec<String> {
        query_words(s)
    }

    #[test]
    fn words_are_the_queries_without_punctuation_and_without_stray_letters() {
        assert_eq!(
            words("decrypt_config, (heap) overflow?"),
            ["decrypt_config", "heap", "overflow"]
        );
        assert_eq!(words("a b"), ["a", "b"]);
        assert_eq!(words("a heap"), ["heap"]);
    }

    #[test]
    fn matches_ignore_case_merge_and_stay_on_character_boundaries() {
        let text = "Heap OVERFLOW in heapoverflow, ünïcode Ünï";
        let found = find_words(text, &words("heap overflow ünï"));
        let spans: Vec<&str> = found.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(spans, ["Heap", "OVERFLOW", "heapoverflow", "ünï"]);
        assert!(find_words(text, &[]).is_empty());
        // Never inside a character: a word that is a prefix of a multi-byte one is not cut.
        assert!(find_words("é", &["\u{c3}".into()]).is_empty());
    }

    #[test]
    fn a_snippet_is_centred_on_the_first_match_and_says_where_it_was_cut() {
        let text = format!("{} needle {}", "lorem ".repeat(40), "ipsum ".repeat(40));
        let s = snippet(&text, &words("needle"), 80);
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert!(s.contains("needle") && s.chars().count() <= 84, "{s}");
        // A short passage is whole.
        assert_eq!(snippet("two words", &words("zzz"), 80), "two words");
        // Newlines are spaces.
        assert_eq!(snippet("a\n\nb", &[], 80), "a b");
    }

    #[test]
    fn words_in_a_sentence_are_marked_as_both_and_the_rest_of_it_as_a_sentence() {
        // A sentence at 0..20; one word inside it and one outside.
        let sentence = 0..20;
        let got = layers(&[5..9, 30..34], std::slice::from_ref(&sentence));
        assert_eq!(
            got,
            [
                (0..5, Layer::Sentence),
                (5..9, Layer::Both),
                (9..20, Layer::Sentence),
                (30..34, Layer::Word),
            ]
        );
        // Touching stretches of one kind are one; nothing marked is nothing.
        assert_eq!(layers(&[0..3, 3..6], &[]), [(0..6, Layer::Word)]);
        assert!(layers(&[], &[]).is_empty());
    }

    fn hit(id: i64, similarity: Option<f32>) -> reactor_db::corpus::Hit {
        reactor_db::corpus::Hit {
            chunk_id: id,
            document_id: String::new(),
            source_uri: String::new(),
            title: None,
            heading_path: String::new(),
            text: String::new(),
            origin: "external".into(),
            source_kind: "file".into(),
            mime: None,
            meta: serde_json::Value::Null,
            score: 0.0,
            keyword_rank: None,
            meaning_rank: None,
            similarity,
        }
    }

    #[test]
    fn listing_by_similarity_puts_the_closest_first_and_words_only_last() {
        // As the search ranked them: a keyword favourite first, the closest one last.
        let found = [
            hit(1, Some(0.4)),
            hit(2, None),
            hit(3, Some(0.7)),
            hit(4, Some(0.4)),
        ];
        let ids =
            |v: Vec<reactor_db::corpus::Hit>| v.iter().map(|h| h.chunk_id).collect::<Vec<_>>();
        assert_eq!(ids(ordered(&found, false)), [1, 2, 3, 4]);
        // Equal similarities keep the search's order (1 before 4).
        assert_eq!(ids(ordered(&found, true)), [3, 1, 4, 2]);
    }
}
