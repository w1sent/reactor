//! The identity editor: a window of its own for writing an identity — a name, and the text the
//! model is told it is. Opened from Settings › Identities (add, edit, view) and by `/identity
//! new` and `/identity edit <name>`.
//!
//! A saved identity is edited in place (and may be renamed). A built-in cannot be changed:
//! opened on one, the editor shows its text and saves what you make of it as a *copy* under a
//! new name.

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Root};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, SharedString, Window, div, px, size};

use crate::app::ReactorApp;
use crate::hints;

/// What the editor opens on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// A blank one, or one with a name already chosen (`/identity new <name>`).
    New(String),
    /// A saved identity, to change.
    Edit(String),
    /// Any identity, to read; saving makes a copy (the only way to change a built-in).
    Copy(String),
}

/// Open the editor window.
pub fn open(cx: &mut App, app: Entity<ReactorApp>, start: Start) {
    let rows = app.read(cx).identity_rows();
    let find = |name: &str| rows.iter().find(|r| r.name == name).cloned();
    // What the fields start with, whether this edits a saved identity (and under which name),
    // and whether it is a copy of something.
    let (name, text, editing, copy_of) = match &start {
        Start::New(name) => (name.clone(), String::new(), None, None),
        Start::Edit(name) => match find(name) {
            Some(r) if !r.builtin => (r.name.clone(), r.text, Some(r.name), None),
            // Not there, or built in: what can be done is a copy of it.
            Some(r) => (format!("{}-copy", r.name), r.text, None, Some(r.name)),
            None => (name.clone(), String::new(), None, None),
        },
        Start::Copy(name) => match find(name) {
            Some(r) => (format!("{}-copy", r.name), r.text, None, Some(r.name)),
            None => (String::new(), String::new(), None, None),
        },
    };
    let bounds = gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::centered(
        None,
        size(px(760.), px(640.)),
        cx,
    ));
    let opened = cx.open_window(crate::chrome::window_options(bounds), move |window, cx| {
        crate::theme::install_ayu_dark(cx);
        let view = cx.new(|cx| Editor::new(app, name, text, editing, copy_of, window, cx));
        let root: gpui_kit::AnyView = view.into();
        cx.new(|cx| Root::new(root, window, cx))
    });
    if opened.is_ok() {
        cx.activate(true);
    }
}

struct Editor {
    app: Entity<ReactorApp>,
    /// The saved identity being changed, by the name it has now.
    editing: Option<String>,
    /// The identity this is a copy of, if it is one.
    copy_of: Option<String>,
    name: Entity<InputState>,
    text: Entity<TextareaState>,
    error: Option<String>,
}

impl Editor {
    fn new(
        app: Entity<ReactorApp>,
        name: String,
        text: String,
        editing: Option<String>,
        copy_of: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("name, e.g. triage");
            s.set_value(&name, window, cx);
            s
        });
        let text_input = cx.new(|cx| {
            // Not auto-growing: a long text must not push the window's own edge out of reach.
            // The box has a height of its own (set when it is drawn) and scrolls inside it.
            let mut s = TextareaState::new(window, cx)
                .placeholder("You are … what the model is told it is, and how it works.");
            s.set_value(&text, window, cx);
            s
        });
        // A new one starts in the name; a change in the text.
        let focus_name = editing.is_none();
        let (n, t) = (name_input.clone(), text_input.clone());
        window.defer(cx, move |window, cx| {
            if focus_name {
                n.update(cx, |s, cx| s.focus(window, cx));
            } else {
                t.update(cx, |s, cx| s.focus(window, cx));
            }
        });
        Editor {
            app,
            editing,
            copy_of,
            name: name_input,
            text: text_input,
            error: None,
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().to_string();
        let text = self.text.read(cx).value().to_string();
        let old = self.editing.clone();
        let result = self.app.update(cx, |app, cx| {
            app.identity_save(old.as_deref(), &name, &text, cx)
        });
        match result {
            Ok(()) => window.remove_window(),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // What is left of the window under the title bar, the name and the buttons.
        let text_height = (f32::from(window.viewport_size().height) - 270.).max(160.);
        let heading = match (&self.editing, &self.copy_of) {
            (Some(name), _) => format!("Identity: {name}"),
            (None, Some(of)) => format!("A copy of {of}"),
            (None, None) => "A new identity".to_string(),
        };
        v_flex()
            .size_full()
            .child(crate::chrome::title_bar("REactor — Identity", None, None, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(div().text_color(theme.foreground).child(heading))
                            .child(hints::info_button(
                                "identity-info",
                                "An identity is a working persona: its text is added to the system prompt when it is selected, with /identity <name> or in Settings. A built-in cannot be changed: its text opens here as a copy, to save under a name of your own. The name cannot have a space in it, since /identity <name> selects by it.",
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().w(px(60.)).text_color(theme.muted_foreground).child("Name"))
                            .child(div().w(px(320.)).child(Input::new(&self.name))),
                    )
                    .child(Textarea::new(&self.text).h(px(text_height)))
                    .children(self.error.clone().map(|e| {
                        div().text_color(theme.danger).child(SharedString::from(e))
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .p_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("identity-cancel")
                            .label("Cancel")
                            .ghost()
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        Button::new("identity-save")
                            .label("Save")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}
