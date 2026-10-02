//! The text editor: gpui-kit's code editor on a file, with syntax highlighting chosen from
//! the file's name (and `#!` line) and changeable by hand, line numbers, folding and
//! search. Any number of them can be open.

use gpui_kit::actions;
use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, Render, Window, div};

use reactor_fs::FileRef;

use super::{human_size, language};
use crate::app::ReactorApp;
use crate::panels::panel_title_row;
use crate::settings::Slot;

actions!(
    reactor_editor,
    [
        /// Ctrl+S / Cmd+S in an editor: write the file.
        SaveFile
    ]
);

const CONTEXT: &str = "ReactorEditor";

/// Larger files are not loaded into an editor; the hex viewer reads them lazily.
const MAX_BYTES: u64 = 16 << 20;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("secondary-s", SaveFile, Some(CONTEXT))]);
}

pub struct EditorPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    file: FileRef,
    state: Entity<EditorState>,
    language: &'static str,
    /// Why nothing is shown, when the file could not be loaded as text.
    problem: Option<String>,
    dirty: bool,
    readonly: bool,
    bytes: u64,
    focus: FocusHandle,
}

impl EditorPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        file: FileRef,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("text")
                .line_number(true)
                .folding(true)
                .searchable(true)
                .soft_wrap(false)
        });
        cx.subscribe_in(
            &state,
            window,
            |this, _, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event
                    && !this.dirty
                {
                    this.dirty = true;
                    cx.notify();
                }
            },
        )
        .detach();
        let mut this = Self {
            app,
            file,
            state,
            language: "text",
            problem: None,
            dirty: false,
            readonly: false,
            bytes: 0,
            focus: cx.focus_handle(),
        };
        this.load(line, window, cx);
        this
    }

    /// Read the file from its filesystem, replacing whatever is in the editor.
    fn load(&mut self, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        self.problem = None;
        let bytes = match self.file.read_all(MAX_BYTES) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.problem = Some(format!("could not read {}: {e}", self.file.path));
                return;
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                self.problem = Some("this file is not valid UTF-8 text".into());
                return;
            }
        };
        self.bytes = text.len() as u64;
        self.language = language::detect(&self.file.name(), text.lines().next().unwrap_or(""));
        self.readonly = !self.file.fs.writable();
        let (language, readonly) = (self.language, self.readonly);
        self.state.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.set_highlighter(language, cx);
            state.set_readonly(readonly, cx);
            if let Some(line) = line {
                state.set_cursor_position(
                    Position {
                        line: line.saturating_sub(1),
                        character: 0,
                    },
                    window,
                    cx,
                );
            }
        });
        self.dirty = false;
        cx.notify();
    }

    /// Jump to a 1-based line.
    pub fn goto_line(&mut self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.set_cursor_position(
                Position {
                    line: line.saturating_sub(1),
                    character: 0,
                },
                window,
                cx,
            );
        });
    }

    fn set_language(&mut self, name: &'static str, cx: &mut Context<Self>) {
        self.language = name;
        self.state
            .update(cx, |state, cx| state.set_highlighter(name, cx));
        cx.notify();
    }

    fn note(&self, kind: &'static str, message: String, cx: &mut Context<Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, _| app.push_note(kind, message));
        }
    }

    pub fn save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly || self.problem.is_some() {
            return;
        }
        let text = self.state.read(cx).value().to_string();
        match self.file.fs.write(&self.file.path, text.as_bytes()) {
            Ok(()) => {
                self.dirty = false;
                self.bytes = text.len() as u64;
                cx.notify();
            }
            Err(e) => self.note(
                "error",
                format!("could not save {}: {e}", self.file.path),
                cx,
            ),
        }
    }

    /// Reload from disk, asking first when that would throw edits away.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dirty {
            return self.load(None, window, cx);
        }
        self.confirm_discard("Reload", window, cx, |this, window, cx| {
            this.load(None, window, cx)
        });
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let close = |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            if let Some(app) = this.app.upgrade() {
                let panel = cx.entity();
                app.update(cx, |app, cx| app.close_panel(panel, window, cx));
            }
        };
        if self.dirty {
            self.confirm_discard("Close", window, cx, close);
        } else {
            close(self, window, cx);
        }
    }

    fn confirm_discard(
        &mut self,
        verb: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        use gpui_kit::component::WindowExt as _;
        use gpui_kit::component::button::ButtonVariant;
        use gpui_kit::component::dialog::DialogButtonProps;
        let this = cx.entity().downgrade();
        let name = self.file.name();
        let then = std::rc::Rc::new(then);
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let (this, then, name) = (this.clone(), then.clone(), name.clone());
            dialog
                .title(format!("{verb} {name}?"))
                .w(gpui_kit::px(440.))
                .content(|content, _, _| content.child("Unsaved changes will be lost."))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(format!("{verb} without saving"))
                        .ok_variant(ButtonVariant::Danger)
                        .show_cancel(true)
                        .on_ok(move |_, window, cx| {
                            let then = then.clone();
                            this.update(cx, |panel, cx| then(panel, window, cx)).is_ok()
                        }),
                )
        });
    }

    fn open_in_hex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.file.clone();
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.open_file(Some(super::Viewer::Hex), file, None, window, cx)
            });
        }
    }
}

impl Focusable for EditorPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.problem.is_some() {
            self.focus.clone()
        } else {
            self.state.focus_handle(cx)
        }
    }
}

impl EventEmitter<PanelEvent> for EditorPanel {}

impl gpui_kit::base::dock::Panel for EditorPanel {
    fn panel_name(&self) -> &'static str {
        "editor"
    }
}

impl Panel for EditorPanel {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let marker = if self.dirty { "● " } else { "" };
        panel_title_row(IconName::FileCode, &format!("{marker}{}", self.file.name()))
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![
            Button::new("editor-save")
                .icon(IconName::Save)
                .ghost()
                .small()
                .disabled(self.readonly || !self.dirty)
                .tooltip("Save (Ctrl/Cmd+S)")
                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            Button::new("editor-reload")
                .icon(IconName::RotateCw)
                .ghost()
                .small()
                .tooltip("Read the file again")
                .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            Button::new("close-panel")
                .icon(IconName::X)
                .ghost()
                .small()
                .tooltip("Close this editor")
                .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
        ])
    }
}

impl Render for EditorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ui = self
            .app
            .upgrade()
            .map(|a| a.read(cx).ui.clone())
            .unwrap_or_default();
        let (family, size) = ui.text(Slot::Mono, &theme);

        if let Some(problem) = self.problem.clone() {
            return v_flex()
                .p_3()
                .gap_2()
                .track_focus(&self.focus)
                .child(div().text_color(theme.warning).child(problem))
                .child(
                    Button::new("editor-open-hex")
                        .label("Open in the hex viewer")
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| this.open_in_hex(window, cx))),
                )
                .into_any_element();
        }

        let position = self.state.read(cx).cursor_position();
        let current = self.language;
        let weak = cx.entity().downgrade();
        let muted = theme.muted_foreground;
        let status = h_flex()
            .px_2()
            .py_1()
            .gap_3()
            .items_center()
            .text_color(muted)
            .child(format!(
                "Ln {}, Col {}",
                position.line + 1,
                position.character + 1
            ))
            .child(human_size(self.bytes))
            .when(self.readonly, |row| row.child("read-only"))
            .child(div().flex_1())
            .child(
                Button::new("editor-language")
                    .ghost()
                    .small()
                    .label(current)
                    .tooltip("Syntax highlighting for this file")
                    .dropdown_menu(move |mut menu, _window, _cx| {
                        menu = menu.max_h(gpui_kit::px(420.)).scrollable(true);
                        for name in language::all() {
                            let weak = weak.clone();
                            menu = menu.item(
                                PopupMenuItem::new(name).checked(name == current).on_click(
                                    move |_, _, cx| {
                                        let _ = weak.update(cx, |p, cx| p.set_language(name, cx));
                                    },
                                ),
                            );
                        }
                        menu
                    }),
            );

        v_flex()
            .size_full()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &SaveFile, window, cx| this.save(window, cx)))
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(&self.state)
                        .bordered(false)
                        .font_family(family)
                        .text_size(size)
                        .size_full(),
                ),
            )
            .child(status)
            .into_any_element()
    }
}
