//! The Model page and the Documents page's model chooser: one wide card per provider, edited
//! in place. Both are views of the same state ([`Purpose`]): a key typed for OpenAI on one page
//! is the key on the other, because it is one variable.
//!
//!
//! Each card says where the provider is reached (the endpoint, and where that value came from),
//! with what (the key, masked), and which models that endpoint offers — listed from the
//! endpoint itself, so putting one in the picker is one click. Fields save when they are left.
//! Explanations are tooltips ([`crate::hints`]); what stays on the card is data and controls.
//!
//! The values are environment variables, because that is where rig reads them
//! (`reactor_agent::connection`); "keep" additionally writes them to `~/.reactor/env`.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, Hsla, IntoElement, Pixels, Render, SharedString, Window,
    div, px,
};

use reactor_agent::connection::{self, EnvFile, ProviderSpec, Source};

use crate::app::ReactorApp;

/// Past this many chips a card shows a filter, and cuts the list.
const MANY: usize = 16;
const SHOWN: usize = 30;

/// What the cards are choosing a model for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// The chat models of the picker; several, one of them the default.
    Chat,
    /// The one embedding model of the documents.
    Embedding,
}

/// What the page the cards are drawn on stands at.
struct Standing<'a> {
    purpose: Purpose,
    default: Option<&'a str>,
    picker: &'a [String],
    session: Option<&'a str>,
    /// The embedding model in use, and the one chosen but not yet started.
    chosen: Option<&'a str>,
    draft: Option<&'a str>,
}

/// What listing a provider's models has come to.
enum Listing {
    /// Not asked: a key is needed first.
    Idle,
    Loading,
    Loaded(Vec<String>),
    Failed(String),
}

struct Card {
    spec: &'static ProviderSpec,
    /// `None` where the provider's endpoint is fixed.
    endpoint: Option<Entity<InputState>>,
    key: Entity<InputState>,
    filter: Entity<InputState>,
    listing: Listing,
    /// Bumped by every request, so a slow answer to an old one is ignored.
    generation: u64,
    /// The last edit was written.
    saved: bool,
}

pub struct ModelCards {
    app: Entity<ReactorApp>,
    file: EnvFile,
    cards: Vec<Card>,
    /// The card whose "add a model by name" field is open, and for which page.
    manual: Option<(usize, Purpose)>,
    /// Embedding view: list every model, not only those that look like embedding models.
    show_all: bool,
    manual_input: Entity<InputState>,
}

/// Colours the card is drawn with, copied out so the theme is not borrowed while listeners are
/// made.
#[derive(Clone)]
struct Palette {
    foreground: Hsla,
    muted: Hsla,
    border: Hsla,
    accent: Hsla,
    info: Hsla,
    success: Hsla,
    warning: Hsla,
    mono: SharedString,
    mono_size: Pixels,
    small: Pixels,
}

fn field(
    placeholder: impl Into<SharedString>,
    value: &str,
    window: &mut Window,
    cx: &mut Context<ModelCards>,
) -> Entity<InputState> {
    let placeholder: SharedString = placeholder.into();
    cx.new(|cx| {
        let mut s = InputState::new(window, cx).placeholder(placeholder);
        s.set_value(value, window, cx);
        s
    })
}

impl ModelCards {
    pub fn new(app: Entity<ReactorApp>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let file = EnvFile::load(&reactor_core::Paths::from_env().config_dir);
        let mut cards = Vec::new();
        for (ix, spec) in connection::SPECS.iter().enumerate() {
            let endpoint = spec.endpoint_var.map(|_| {
                let current = connection::endpoint(spec.name, &file);
                let set = current
                    .as_ref()
                    .filter(|v| v.source != Source::Default)
                    .and_then(|v| v.value.clone())
                    .unwrap_or_default();
                let state = field(spec.default_endpoint, &set, window, cx);
                cx.subscribe_in(&state, window, move |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.commit_endpoint(ix, window, cx);
                    }
                })
                .detach();
                state
            });
            let placeholder = SharedString::from(key_placeholder(spec, &file));
            let key = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(placeholder));
            cx.subscribe_in(&key, window, move |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    this.commit_key(ix, window, cx);
                }
            })
            .detach();
            let filter = field("Filter by name…", "", window, cx);
            cx.subscribe_in(&filter, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
            cards.push(Card {
                spec,
                endpoint,
                key,
                filter,
                listing: Listing::Idle,
                generation: 0,
                saved: false,
            });
        }
        let manual_input = field("provider model name, Enter to add", "", window, cx);
        cx.subscribe_in(&manual_input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.add_manual(window, cx);
            }
        })
        .detach();
        let mut this = Self {
            app,
            file,
            cards,
            manual: None,
            show_all: false,
            manual_input,
        };
        for ix in 0..this.cards.len() {
            this.refetch(ix, cx);
        }
        this
    }

    // -- writing ------------------------------------------------------------------------------

    fn write(&mut self, var: &str, value: &str, cx: &mut Context<Self>) -> bool {
        let keep = self.app.read(cx).ui.keep_keys;
        match connection::set_variable(&mut self.file, var, value, keep) {
            Ok(()) => true,
            Err(e) => {
                let path = self.file.path().display().to_string();
                self.app.update(cx, |app, _| {
                    app.push_note("error", format!("could not write {path}: {e}"))
                });
                // The process has the value even so; it is the file that did not take it.
                true
            }
        }
    }

    fn commit_endpoint(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let card = &self.cards[ix];
        let (Some(state), Some(var)) = (card.endpoint.clone(), card.spec.endpoint_var) else {
            return;
        };
        let spec = card.spec;
        let typed = state.read(cx).value().trim().trim_end_matches('/').to_string();
        let current = connection::endpoint(spec.name, &self.file);
        let shown = current
            .as_ref()
            .filter(|v| v.source != Source::Default)
            .and_then(|v| v.value.clone())
            .unwrap_or_default();
        if typed == shown {
            return;
        }
        if !typed.is_empty() && !connection::valid_endpoint(&typed) {
            self.app.update(cx, |app, _| {
                app.push_note(
                    "warning",
                    format!("{}: an endpoint is an http:// or https:// address", spec.name),
                )
            });
            state.update(cx, |s, cx| s.set_value(shown.as_str(), window, cx));
            return;
        }
        // The default typed out is the default: nothing to keep.
        let value = if typed == spec.default_endpoint { "" } else { typed.as_str() };
        self.write(var, value, cx);
        if value.is_empty() {
            state.update(cx, |s, cx| s.set_value("", window, cx));
        }
        self.cards[ix].saved = true;
        self.refetch(ix, cx);
        cx.notify();
    }

    fn commit_key(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (key, var, spec) = {
            let c = &self.cards[ix];
            (c.key.clone(), c.spec.key_var, c.spec)
        };
        let typed = key.read(cx).value().trim().to_string();
        if typed.is_empty() {
            return;
        }
        self.write(var, &typed, cx);
        // The secret does not stay in the field.
        key.update(cx, |s, cx| s.set_value("", window, cx));
        self.set_key_placeholder(ix, spec, window, cx);
        self.cards[ix].saved = true;
        self.refetch(ix, cx);
        cx.notify();
    }

    fn clear_key(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let spec = self.cards[ix].spec;
        self.write(spec.key_var, "", cx);
        self.set_key_placeholder(ix, spec, window, cx);
        self.cards[ix].saved = true;
        self.refetch(ix, cx);
        cx.notify();
    }

    fn set_key_placeholder(
        &mut self,
        ix: usize,
        spec: &'static ProviderSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = key_placeholder(spec, &self.file);
        self.cards[ix]
            .key
            .update(cx, |s, cx| s.set_placeholder(text, window, cx));
    }

    // -- the models ---------------------------------------------------------------------------

    /// Ask the provider's endpoint what it has. Without a key, where one is required, there is
    /// nothing to ask with.
    fn refetch(&mut self, ix: usize, cx: &mut Context<Self>) {
        let spec = self.cards[ix].spec;
        let key_set = connection::key(spec.name, &self.file)
            .and_then(|v| v.value)
            .is_some();
        let card = &mut self.cards[ix];
        card.generation += 1;
        if spec.key_required && !key_set {
            card.listing = Listing::Idle;
            return;
        }
        card.listing = Listing::Loading;
        let generation = card.generation;

        // On a thread of its own with a runtime of its own: the HTTP client wants tokio's
        // reactor and gpui's executor is not one.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let provider = spec.name;
        std::thread::spawn(move || {
            let result = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(connection::list_models(provider)),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.unwrap_or_else(|_| Err("cancelled".to_string()));
            let _ = this.update(cx, |this, cx| {
                let card = &mut this.cards[ix];
                if card.generation == generation {
                    card.listing = match result {
                        Ok(models) => Listing::Loaded(models),
                        Err(e) => Listing::Failed(e),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn add_manual(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ix, purpose)) = self.manual else {
            return;
        };
        let name = self.manual_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        // `provider/name` typed whole is taken as it is.
        let provider = self.cards[ix].spec.name;
        let spec = if name.starts_with(&format!("{provider}/")) {
            name
        } else {
            format!("{provider}/{name}")
        };
        match purpose {
            Purpose::Chat => self.app.update(cx, |app, cx| app.list_model(&spec, cx)),
            Purpose::Embedding => self.choose_embedding(ix, &spec, cx),
        }
        self.manual_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.manual = None;
        cx.notify();
    }

    /// Choose an embedding model: it becomes the draft, and starts when *Use* is pressed,
    /// because starting sends every passage of the documents to the endpoint.
    fn choose_embedding(&mut self, ix: usize, spec: &str, cx: &mut Context<Self>) {
        let provider = self.cards[ix].spec.name;
        let name = spec.strip_prefix(&format!("{provider}/")).unwrap_or(spec);
        self.app.update(cx, |app, cx| {
            app.embedding_draft.provider = provider.to_string();
            app.embedding_draft.name = name.to_string();
            cx.notify();
        });
    }

    // -- drawing ------------------------------------------------------------------------------

    fn palette(cx: &App) -> Palette {
        let t = cx.theme();
        Palette {
            foreground: t.foreground,
            muted: t.muted_foreground,
            border: t.border,
            accent: t.accent,
            info: t.info,
            success: t.success,
            warning: t.warning,
            mono: t.mono_font_family.clone(),
            mono_size: t.mono_font_size,
            small: t.font_size * 0.85,
        }
    }

    fn label(text: &'static str, p: &Palette) -> gpui_kit::Div {
        div()
            .text_size(p.small * 0.9)
            .text_color(p.muted)
            .child(text)
    }

    fn badge(
        id: impl Into<gpui_kit::ElementId>,
        text: impl Into<SharedString>,
        color: Hsla,
        tip: impl Into<SharedString>,
        p: &Palette,
    ) -> impl IntoElement {
        crate::hints::tip(
            div()
                .id(id)
                .px_1p5()
                .rounded_sm()
                .text_size(p.small * 0.9)
                .text_color(color)
                .bg(color.opacity(0.14))
                .child(text.into()),
            tip,
        )
    }

    fn render_card(
        &self,
        ix: usize,
        st: &Standing,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card = &self.cards[ix];
        let spec = card.spec;
        let purpose = st.purpose;
        let (default, picker) = (st.default, st.picker);
        let in_use = match purpose {
            Purpose::Chat => st.session,
            Purpose::Embedding => st.chosen,
        };
        let used = in_use.is_some_and(|m| m.starts_with(&format!("{}/", spec.name)));
        let key = connection::key(spec.name, &self.file);
        let key_set = key.as_ref().and_then(|k| k.value.as_deref()).is_some();
        let missing = spec.key_required && !key_set;

        // -- header: name, standing, saved
        let status: AnyElement = match &card.listing {
            Listing::Loading => Spinner::new().small().color(p.muted).into_any_element(),
            Listing::Failed(why) => crate::hints::tip(
                div()
                    .id(("failed", ix))
                    .child(Icon::new(IconName::TriangleAlert).small().text_color(p.warning)),
                format!("Could not list the models: {why}. Add one by name with +."),
            )
            .into_any_element(),
            Listing::Loaded(_) => {
                Self::badge(("ok", ix), "reachable", p.success, "The endpoint answered", p)
                    .into_any_element()
            }
            Listing::Idle if missing => {
                Self::badge(("key", ix), "key missing", p.warning, format!("Set {}", spec.key_var), p)
                    .into_any_element()
            }
            Listing::Idle => div().into_any_element(),
        };
        let header = h_flex()
            .items_center()
            .gap_2()
            .child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(spec.name))
            .when(used, |h| {
                h.child(Self::badge(
                    ("used", ix),
                    if purpose == Purpose::Chat { "this session" } else { "in use" },
                    p.accent,
                    if purpose == Purpose::Chat {
                        "The session's model comes from this provider"
                    } else {
                        "The embedding model comes from this provider"
                    },
                    p,
                ))
            })
            .child(status)
            .child(div().flex_1())
            .when(card.saved, |h| {
                h.child(crate::hints::tip(
                    div().id(("saved", ix)).child(
                        Icon::new(IconName::Check).small().text_color(p.success),
                    ),
                    "Saved",
                ))
            });

        // -- the endpoint
        let endpoint = connection::endpoint(spec.name, &self.file);
        let endpoint_col: AnyElement = match (&card.endpoint, spec.endpoint_var) {
            (Some(state), Some(var)) => {
                let ep = endpoint.unwrap();
                let (badge_text, color) = match ep.source {
                    Source::Default => ("default", p.muted),
                    Source::Environment => ("environment", p.info),
                    Source::Saved => ("saved", p.info),
                    Source::Session => ("this run", p.info),
                };
                let mut tip = format!("{var}. Empty means the default, {}.", spec.default_endpoint);
                if let Some(shadowed) = &ep.shadowed {
                    tip.push_str(&format!(
                        " The environment overrides the saved value {shadowed}."
                    ));
                }
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Self::label("Endpoint", p))
                            .child(Self::badge(("ep-src", ix), badge_text, color, tip, p)),
                    )
                    .child(Input::new(state).small())
                    .into_any_element()
            }
            _ => v_flex()
                .gap_1()
                .child(Self::label("Endpoint", p))
                .child(crate::hints::tip(
                    h_flex()
                        .id(("ep-fixed", ix))
                        .h(px(28.))
                        .px_2()
                        .gap_2()
                        .items_center()
                        .rounded_md()
                        .border_1()
                        .border_color(p.border)
                        .text_color(p.muted)
                        .font_family(p.mono.clone())
                        .text_size(p.mono_size * 0.9)
                        .child(Icon::new(IconName::Lock).small())
                        .child(spec.default_endpoint),
                    "Fixed by the provider: no setting moves it.",
                ))
                .into_any_element(),
        };

        // -- the key
        let key_col = {
            let (badge_text, color) = match key.as_ref().map(|k| k.source) {
                Some(Source::Environment) => ("environment", p.info),
                Some(Source::Saved) => ("saved", p.info),
                Some(Source::Session) => ("this run", p.info),
                _ if spec.key_required => ("not set", p.warning),
                _ => ("optional", p.muted),
            };
            let mut tip = format!("{}.", spec.key_var);
            if !spec.key_required {
                tip.push_str(" Only needed behind a proxy that asks for one.");
            }
            tip.push_str(" Typing one sets it for REactor.");
            if let Some(shadowed) = key.as_ref().and_then(|k| k.shadowed.as_ref()) {
                tip.push_str(&format!(
                    " The environment overrides a saved key ({}).",
                    connection::mask(shadowed)
                ));
            }
            let tail = key
                .as_ref()
                .and_then(|k| k.value.as_deref())
                .map(connection::mask);
            let weak = cx.entity();
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Self::label("API key", p))
                        .child(Self::badge(("key-src", ix), badge_text, color, tip, p))
                        .when_some(tail, |h, tail| {
                            h.child(
                                div()
                                    .font_family(p.mono.clone())
                                    .text_size(p.small)
                                    .text_color(p.muted)
                                    .child(tail),
                            )
                            .child(
                                Button::new(("key-clear", ix))
                                    .icon(IconName::X)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Remove the key")
                                    .on_click(move |_, window, cx| {
                                        weak.update(cx, |this, cx| {
                                            this.clear_key(ix, window, cx)
                                        });
                                    }),
                            )
                        }),
                )
                .child(Input::new(&card.key).small().mask_toggle())
        };

        // -- the models
        let prefix = format!("{}/", spec.name);
        let in_picker: Vec<&str> = picker
            .iter()
            .filter_map(|m| m.strip_prefix(&prefix))
            .collect();
        let mut names: Vec<String> = match purpose {
            Purpose::Chat => in_picker.iter().map(|m| m.to_string()).collect(),
            // The ones already chosen are shown even where the endpoint does not list them.
            Purpose::Embedding => [st.chosen, st.draft]
                .into_iter()
                .flatten()
                .filter_map(|m| m.strip_prefix(&prefix))
                .map(str::to_string)
                .collect(),
        };
        if let Listing::Loaded(listed) = &card.listing {
            for m in listed {
                let wanted = match purpose {
                    Purpose::Chat => !connection::is_embedding_model(m),
                    Purpose::Embedding => self.show_all || connection::is_embedding_model(m),
                };
                if wanted && !names.contains(m) {
                    names.push(m.clone());
                }
            }
        }
        names.dedup();
        let total = names.len();
        let filter = card.filter.read(cx).value().trim().to_ascii_lowercase();
        let many = total > MANY;
        if !filter.is_empty() {
            names.retain(|n| n.to_ascii_lowercase().contains(&filter));
        }
        let hidden = names.len().saturating_sub(SHOWN);
        names.truncate(SHOWN);

        let this = cx.entity();
        let refresh = {
            let this = this.clone();
            Button::new(("refresh", ix))
                .icon(IconName::RotateCw)
                .ghost()
                .small()
                .tooltip("List the models at this endpoint again")
                .on_click(move |_, _w, cx| this.update(cx, |this, cx| this.refetch(ix, cx)))
        };
        let add = {
            let this = this.clone();
            Button::new(("add", ix))
                .icon(IconName::Plus)
                .ghost()
                .small()
                .tooltip("Add a model by name — for one the endpoint does not list")
                .on_click(move |_, window, cx| {
                this.update(cx, |this, cx| {
                    this.manual = (this.manual != Some((ix, purpose))).then_some((ix, purpose));
                    let input = this.manual_input.clone();
                    input.update(cx, |s, cx| s.focus(window, cx));
                    cx.notify();
                })
                })
        };
        let models_header = h_flex()
            .gap_2()
            .items_center()
            .child(Self::label("Models", p))
            .child(
                div()
                    .text_size(p.small * 0.9)
                    .text_color(p.muted)
                    .child(match purpose {
                        Purpose::Chat => format!("{} · {} in the picker", total, in_picker.len()),
                        Purpose::Embedding => format!("{total} listed"),
                    }),
            )
            .child(div().flex_1())
            .when(many, |h| {
                h.child(div().w(px(220.)).child(Input::new(&card.filter).small()))
            })
            .when(purpose == Purpose::Embedding, |h| {
                let this = this.clone();
                let all = self.show_all;
                h.child(
                    Button::new(("all", ix))
                        .icon(IconName::Eye)
                        .ghost()
                        .small()
                        .when(all, |b| b.primary())
                        .tooltip(if all {
                            "Showing every model the endpoint lists. Click to show only those that look like embedding models."
                        } else {
                            "Showing the models that look like embedding models, judged by name. Click to show every model the endpoint lists."
                        })
                        .on_click(move |_, _w, cx| {
                            this.update(cx, |this, cx| {
                                this.show_all = !this.show_all;
                                cx.notify();
                            })
                        }),
                )
            })
            .child(refresh)
            .child(add);

        let chips = h_flex().flex_wrap().gap_1p5().children(
            names
                .into_iter()
                .enumerate()
                .map(|(n, name)| match purpose {
                    Purpose::Chat => self.chip(ix, n, spec.name, &name, default, p, cx),
                    Purpose::Embedding => self.embedding_chip(ix, n, spec.name, &name, st, p, cx),
                }),
        );

        let manual_row = (self.manual == Some((ix, purpose))).then(|| {
            h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().child(Input::new(&self.manual_input).small()))
        });

        v_flex()
            .id(("card", ix + if purpose == Purpose::Chat { 0 } else { 100 }))
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(if used {
                p.accent
            } else if missing {
                p.warning.opacity(0.5)
            } else {
                p.border
            })
            .child(header)
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(div().flex_1().min_w_0().child(endpoint_col))
                    .child(div().flex_1().min_w_0().child(key_col)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(models_header)
                    .children(manual_row)
                    .child(chips)
                    .when(hidden > 0, |v| {
                        v.child(
                            div()
                                .text_size(p.small)
                                .text_color(p.muted)
                                .child(format!("{hidden} more — filter to narrow")),
                        )
                    })
                    .when(matches!(card.listing, Listing::Idle) && !missing, |v| v),
            )
            .into_any_element()
    }

    /// One embedding model: a click chooses it. It then waits as the draft for *Use*.
    #[allow(clippy::too_many_arguments)]
    fn embedding_chip(
        &self,
        card: usize,
        n: usize,
        provider: &str,
        name: &str,
        st: &Standing,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spec = format!("{provider}/{name}");
        let in_use = st.chosen == Some(spec.as_str());
        let pending = !in_use && st.draft == Some(spec.as_str());
        let this = cx.entity();
        let color = if in_use { p.accent } else if pending { p.info } else { p.border };
        crate::hints::tip(
            h_flex()
                .id(("embedding-chip", card * 1000 + n))
                .px_2()
                .h(px(26.))
                .gap_1()
                .items_center()
                .rounded_full()
                .border_1()
                .border_color(color)
                .bg(color.opacity(if in_use || pending { 0.12 } else { 0. }))
                .cursor_pointer()
                .font_family(p.mono.clone())
                .text_size(p.mono_size * 0.9)
                .text_color(if in_use { p.accent } else { p.foreground })
                .when(in_use, |c| c.child(Icon::new(IconName::Check).small()))
                .child(name.to_string())
                .on_click(move |_, _w, cx| {
                    let spec = spec.clone();
                    this.update(cx, |this, cx| this.choose_embedding(card, &spec, cx));
                }),
            if in_use {
                "The embedding model in use."
            } else if pending {
                "Chosen. Press Use at the top to start making its vectors."
            } else {
                "Click to choose it. Nothing is sent until you press Use."
            },
        )
        .into_any_element()
    }

    /// One model: its name puts it in the picker or takes it out; once in, a star makes it the
    /// default and a play button switches this session to it.
    #[allow(clippy::too_many_arguments)]
    fn chip(
        &self,
        card: usize,
        n: usize,
        provider: &str,
        name: &str,
        default: Option<&str>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spec = format!("{provider}/{name}");
        let in_picker = self.app.read(cx).model_settings().1.contains(&spec);
        let is_default = default == Some(spec.as_str());
        let id = ("chip", card * 1000 + n);
        let app = self.app.clone();

        let toggle = {
            let (app, spec) = (app.clone(), spec.clone());
            crate::hints::tip(
                div()
                    .id(id)
                    .px_2()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .font_family(p.mono.clone())
                    .text_size(p.mono_size * 0.9)
                    .text_color(if in_picker { p.accent } else { p.foreground })
                    .child(name.to_string()),
                if in_picker {
                    "In the picker. Click to take it out."
                } else {
                    "Click to put it in the picker."
                },
            )
            .on_click(move |_, _w, cx| {
                let spec = spec.clone();
                app.update(cx, |app, cx| {
                    if in_picker {
                        app.unlist_model(&spec, cx)
                    } else {
                        app.list_model(&spec, cx)
                    }
                });
            })
        };

        let star = {
            let (app, spec) = (app.clone(), spec.clone());
            Button::new(("star", card * 1000 + n))
                .icon(IconName::Star)
                .ghost()
                .xsmall()
                .tooltip(if is_default {
                    "The default for new sessions. Click to clear."
                } else {
                    "Make this the default for new sessions"
                })
                .on_click(move |_, _w, cx| {
                let spec = spec.clone();
                app.update(cx, |app, cx| {
                    app.set_default_model((!is_default).then_some(spec), cx)
                });
            })
        };
        let play = {
            let (app, spec) = (app, spec.clone());
            Button::new(("play", card * 1000 + n))
                .icon(IconName::Play)
                .ghost()
                .xsmall()
                .tooltip("Use it in this session now")
                .on_click(move |_, _w, cx| {
                let spec = spec.clone();
                app.update(cx, |app, cx| app.set_model(&spec, cx));
            })
        };

        h_flex()
            .items_center()
            .rounded_full()
            .border_1()
            .border_color(if in_picker { p.accent } else { p.border })
            .bg(if in_picker {
                p.accent.opacity(0.12)
            } else {
                gpui_kit::hsla(0., 0., 0., 0.)
            })
            .child(toggle)
            .when(in_picker, |c| {
                c.child(
                    div()
                        .text_color(if is_default { p.accent } else { p.muted })
                        .child(star),
                )
                .child(play)
            })
            .into_any_element()
    }
}

fn key_placeholder(spec: &ProviderSpec, file: &EnvFile) -> String {
    match connection::key(spec.name, file).and_then(|k| k.value) {
        Some(v) => format!("Set · {} · type to replace", connection::mask(&v)),
        None if spec.key_required => "Paste the key".to_string(),
        None => "Not set".to_string(),
    }
}

impl Render for ModelCards {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.view(Purpose::Chat, cx)
    }
}

/// The same cards, as the Documents page's model chooser.
pub struct EmbeddingCards {
    cards: Entity<ModelCards>,
}

impl EmbeddingCards {
    pub fn new(cards: Entity<ModelCards>) -> Self {
        Self { cards }
    }
}

impl Render for EmbeddingCards {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.cards
            .update(cx, |cards, cx| cards.view(Purpose::Embedding, cx))
    }
}

impl ModelCards {
    fn view(&mut self, purpose: Purpose, cx: &mut Context<Self>) -> AnyElement {
        let p = Self::palette(cx);
        let (default, picker) = self.app.read(cx).model_settings();
        let session = self
            .app
            .read(cx)
            .session
            .read(cx)
            .model
            .clone()
            .filter(|m| !m.is_empty());
        let chosen = self.app.read(cx).embedding_model();
        let draft = {
            let d = &self.app.read(cx).embedding_draft;
            (!d.name.trim().is_empty()).then(|| format!("{}/{}", d.provider, d.name.trim()))
        }
        .filter(|d| Some(d) != chosen.as_ref());
        let keep = self.app.read(cx).ui.keep_keys;
        let app = self.app.clone();
        let at = |model: &str, file: &EnvFile| {
            let provider = model.split('/').next().unwrap_or("");
            connection::endpoint(provider, file)
                .and_then(|e| e.value)
                .unwrap_or_default()
        };
        let mono = |text: String, color: Hsla| {
            div()
                .font_family(p.mono.clone())
                .text_color(color)
                .child(text)
        };
        let strip = h_flex()
            .gap_2()
            .items_center()
            .flex_wrap()
            .px_3()
            .py_2()
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .text_size(p.small)
            .text_color(p.muted);

        // What the page stands at, and where it goes: the model, and the endpoint it is asked at.
        let summary = match purpose {
            Purpose::Chat => {
                let active = session.as_ref().map(|m| (m.clone(), at(m, &self.file)));
                strip
                    .child("Default")
                    .child(mono(default.clone().unwrap_or_else(|| "none".to_string()), p.foreground))
                    .when_some(active, |h, (model, at)| {
                        h.child("· this session")
                            .child(mono(model, p.foreground))
                            .child("→")
                            .child(mono(at, p.info))
                    })
            }
            Purpose::Embedding => {
                let weak = cx.entity();
                let app_use = app.clone();
                let app_drop = app.clone();
                strip
                    .child("Embedding model")
                    .child(mono(
                        chosen.clone().unwrap_or_else(|| "none: searching by keyword only".into()),
                        p.foreground,
                    ))
                    .when_some(chosen.as_ref(), |h, m| {
                        h.child("→").child(mono(at(m, &self.file), p.info))
                    })
                    .when_some(draft.clone(), |h, d| {
                        let target = at(&d, &self.file);
                        let _ = &weak;
                        h.child("· next")
                            .child(mono(d.clone(), p.accent))
                            .child("→")
                            .child(mono(target.clone(), p.info))
                            .child(
                                Button::new("embedding-use")
                                    .label("Use")
                                    .small()
                                    .primary()
                                    .tooltip(format!(
                                        "Remember it as the embedding model and start making the vectors. Every passage of the documents is sent to {target}. Until it has finished, search keeps using the earlier vectors, or keywords."
                                    ))
                                    .on_click(move |_, _w, cx| {
                                        app_use.update(cx, |app, cx| app.set_typed_embedding(cx));
                                    }),
                            )
                            .child(
                                Button::new("embedding-drop")
                                    .icon(IconName::X)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Do not change the embedding model")
                                    .on_click(move |_, _w, cx| {
                                        app_drop.update(cx, |app, cx| {
                                            app.embedding_draft.name.clear();
                                            cx.notify();
                                        });
                                    }),
                            )
                    })
            }
        };

        let keep_switch = Switch::new("keep-values")
            .small()
            .checked(keep)
            .label("Keep typed values on this machine")
            .tooltip(
                "Endpoints and keys you type are written to ~/.reactor/env (owner-only, plain text) and \
                 exported when REactor starts; a variable already in the environment wins. Off: they \
                 last until REactor closes.",
            )
            .on_click(move |next, _w, cx| {
                let next = *next;
                app.update(cx, |app, cx| app.update_ui(|ui| ui.keep_keys = next, cx));
            });

        let st = Standing {
            purpose,
            default: default.as_deref(),
            picker: &picker,
            session: session.as_deref(),
            chosen: chosen.as_deref(),
            draft: draft.as_deref(),
        };
        let cards: Vec<AnyElement> = (0..self.cards.len())
            .filter(|ix| {
                purpose == Purpose::Chat
                    || reactor_agent::embed::EMBEDDING_PROVIDERS
                        .contains(&self.cards[*ix].spec.name)
            })
            .map(|ix| self.render_card(ix, &st, &p, cx))
            .collect();

        v_flex()
            .gap_3()
            .w_full()
            .child(h_flex().justify_between().items_center().child(summary).child(keep_switch))
            .children(cards)
            .into_any_element()
    }
}
