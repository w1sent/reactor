//! The filesystem explorer: a lazily loaded tree of a [`Vfs`], relative to its root, with
//! filename and content search and a context menu of everyday file operations.
//!
//! It knows only the [`Vfs`] trait; the day a forensic image is mounted behind it, the same
//! panel browses that. Operations the filesystem cannot do (a read-only image cannot delete)
//! are greyed out rather than hidden.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{ClipboardItem, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString};
use gpui_kit::{Window, div, px};

use reactor_fs::{
    Algorithm, ContentHit, Entry, FileRef, Kind, SearchOptions, VPath, Vfs, search_content,
    search_names,
};

use super::props::PropertiesView;
use super::{Viewer, human_size, open_external};
use crate::app::ReactorApp;
use crate::panels::{close_button, panel_title_row};

/// What a menu item does to the panel.
type PanelAction = dyn Fn(&mut FilesPanel, &mut Window, &mut Context<FilesPanel>);

/// What the render closure needs to know about a row without asking the filesystem.
#[derive(Clone)]
struct Meta {
    kind: Kind,
    is_symlink: bool,
    size: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchMode {
    Names,
    Content,
}

enum Results {
    Names(Vec<VPath>),
    Content(Vec<ContentHit>),
}

impl Results {
    fn len(&self) -> usize {
        match self {
            Results::Names(v) => v.len(),
            Results::Content(v) => v.len(),
        }
    }
}

pub struct FilesPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    fs: Arc<dyn Vfs>,
    tree: Entity<TreeState>,
    /// Directories read so far.
    listing: HashMap<VPath, Vec<Entry>>,
    errors: HashMap<VPath, String>,
    expanded: HashSet<SharedString>,
    meta: Rc<RefCell<HashMap<SharedString, Meta>>>,
    query: Entity<InputState>,
    mode: SearchMode,
    results: Option<Results>,
    results_scroll: UniformListScrollHandle,
    searching: Option<Arc<AtomicBool>>,
    status: String,
    focus: FocusHandle,
}

fn id_of(path: &VPath) -> SharedString {
    SharedString::from(path.as_str().to_owned())
}

impl FilesPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        fs: Arc<dyn Vfs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        cx.subscribe(&tree, |this, _, event: &TreeEvent, cx| match event {
            TreeEvent::Expanded(id) => {
                this.expanded.insert(id.clone());
                this.load(&VPath::new(id), cx);
                this.rebuild(cx);
            }
            TreeEvent::Collapsed(id) => {
                this.expanded.remove(id);
            }
        })
        .detach();
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Find file names…"));
        cx.subscribe_in(
            &query,
            window,
            |this, _, event: &InputEvent, _window, cx| match event {
                InputEvent::PressEnter { .. } => this.search(cx),
                InputEvent::Change if this.query_is_empty(cx) => this.clear_search(cx),
                _ => {}
            },
        )
        .detach();
        let mut this = Self {
            app,
            fs,
            tree,
            listing: HashMap::new(),
            errors: HashMap::new(),
            expanded: HashSet::from([id_of(&VPath::root())]),
            meta: Rc::default(),
            query,
            mode: SearchMode::Names,
            results: None,
            results_scroll: UniformListScrollHandle::default(),
            searching: None,
            status: String::new(),
            focus: cx.focus_handle(),
        };
        this.load(&VPath::root(), cx);
        this.rebuild(cx);
        this
    }

    fn query_is_empty(&self, cx: &App) -> bool {
        self.query.read(cx).value().trim().is_empty()
    }

    // -- the tree -------------------------------------------------------------

    fn load(&mut self, dir: &VPath, _cx: &mut Context<Self>) {
        match self.fs.read_dir(dir) {
            Ok(entries) => {
                self.errors.remove(dir);
                self.listing.insert(dir.clone(), entries);
            }
            Err(e) => {
                self.listing.insert(dir.clone(), Vec::new());
                self.errors.insert(dir.clone(), e.to_string());
            }
        }
    }

    fn item_for(&self, entry: &Entry, meta: &mut HashMap<SharedString, Meta>) -> TreeItem {
        let id = id_of(&entry.path);
        meta.insert(
            id.clone(),
            Meta {
                kind: entry.kind,
                is_symlink: entry.is_symlink,
                size: entry.size,
            },
        );
        let item = TreeItem::new(id.clone(), entry.name.clone());
        if entry.kind != Kind::Dir {
            return item;
        }
        item.expanded(self.expanded.contains(&id))
            .children(self.children_of(&entry.path, meta))
    }

    /// The children of a directory; one placeholder row for a folder not read yet, so it
    /// still shows as expandable.
    fn children_of(&self, dir: &VPath, meta: &mut HashMap<SharedString, Meta>) -> Vec<TreeItem> {
        match self.listing.get(dir) {
            Some(entries) => entries.iter().map(|e| self.item_for(e, meta)).collect(),
            None => {
                vec![TreeItem::new(format!("{}\u{0}loading", dir.as_str()), "…").disabled(true)]
            }
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let mut meta = HashMap::new();
        let root = VPath::root();
        let root_id = id_of(&root);
        meta.insert(
            root_id.clone(),
            Meta {
                kind: Kind::Dir,
                is_symlink: false,
                size: None,
            },
        );
        let item = TreeItem::new(root_id, self.fs.label())
            .expanded(true)
            .children(self.children_of(&root, &mut meta));
        *self.meta.borrow_mut() = meta;
        let selected = self.tree.read(cx).selected_item().map(|i| i.id.clone());
        self.tree.update(cx, |state, cx| {
            state.set_items(vec![item], cx);
            if let Some(id) = selected
                && let Some(ix) = state.index_of(&id)
            {
                state.set_selected_index(Some(ix), cx);
            }
        });
        cx.notify();
    }

    /// Read every directory that is open again — what Refresh and every change to the
    /// filesystem do.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let open: Vec<VPath> = self.listing.keys().cloned().collect();
        for dir in open {
            self.load(&dir, cx);
        }
        self.rebuild(cx);
    }

    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.expanded = HashSet::from([id_of(&VPath::root())]);
        self.rebuild(cx);
    }

    fn file(&self, path: &VPath) -> FileRef {
        FileRef::new(self.fs.clone(), path.clone())
    }

    fn note(&self, kind: &'static str, message: String, cx: &mut Context<Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, _| app.push_note(kind, message));
        }
    }

    // -- search ---------------------------------------------------------------

    fn set_mode(&mut self, mode: SearchMode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        let placeholder = match mode {
            SearchMode::Names => "Find file names (* and ? are wildcards)…",
            SearchMode::Content => "Find in files (regular expression)…",
        };
        self.query.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx)
        });
        if !self.query_is_empty(cx) {
            self.search(cx);
        }
        cx.notify();
    }

    fn clear_search(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.searching.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.results = None;
        self.status.clear();
        cx.notify();
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().trim().to_string();
        if query.is_empty() {
            return self.clear_search(cx);
        }
        if let Some(previous) = self.searching.take() {
            previous.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.searching = Some(cancel.clone());
        self.status = "searching…".into();
        let (fs, mode) = (self.fs.clone(), self.mode);
        let flag = cancel.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let options = SearchOptions::default();
                    let root = VPath::root();
                    match mode {
                        SearchMode::Names => {
                            search_names(fs.as_ref(), &root, &query, &options, &flag)
                                .map(Results::Names)
                        }
                        SearchMode::Content => {
                            search_content(fs.as_ref(), &root, &query, &options, &flag)
                                .map(Results::Content)
                        }
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                this.searching = None;
                match outcome {
                    Ok(results) => {
                        let limit = SearchOptions::default().limit;
                        this.status = match results.len() {
                            0 => "no matches".into(),
                            n if n >= limit => format!("first {n} matches"),
                            n => format!("{n} matches"),
                        };
                        this.results = Some(results);
                    }
                    Err(message) => {
                        this.results = None;
                        this.status = message;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // -- actions on a path ------------------------------------------------------------

    fn open(
        &mut self,
        path: &VPath,
        viewer: Option<Viewer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_at(path, viewer, None, window, cx);
    }

    /// Open, scrolled to a 1-based line when the viewer has lines.
    fn open_at(
        &mut self,
        path: &VPath,
        viewer: Option<Viewer>,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file = self.file(path);
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| app.open_file(viewer, file, line, window, cx));
        }
    }

    fn preview(&mut self, path: &VPath, window: &mut Window, cx: &mut Context<Self>) {
        super::data::open_popup(self.file(path), window, cx);
    }

    fn open_externally(&mut self, path: &VPath, cx: &mut Context<Self>) {
        match self.fs.local_path(path) {
            Some(native) => {
                if let Err(e) = open_external(&native) {
                    self.note(
                        "error",
                        format!("could not open {}: {e}", native.display()),
                        cx,
                    );
                }
            }
            None => self.note(
                "warning",
                "that file is not on this machine's disk".into(),
                cx,
            ),
        }
    }

    fn open_with(&mut self, path: &VPath, cx: &mut Context<Self>) {
        match self.fs.local_path(path) {
            Some(native) => {
                if let Err(e) = super::open_with_picker(&native) {
                    self.note("error", format!("could not ask for a program: {e}"), cx);
                }
            }
            None => self.note(
                "warning",
                "that file is not on this machine's disk".into(),
                cx,
            ),
        }
    }

    fn reveal(&mut self, path: &VPath, cx: &mut Context<Self>) {
        self.open_externally(&path.parent().unwrap_or_else(VPath::root), cx);
    }

    fn copy(&self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn properties(
        &mut self,
        path: &VPath,
        hash: &[Algorithm],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt as _;
        let file = self.file(path);
        let title = file.name();
        let hash = hash.to_vec();
        let view = cx.new(|cx| PropertiesView::new(file, &hash, cx));
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let view = view.clone();
            dialog
                .title(format!("Properties — {title}"))
                .w(px(620.))
                .content(move |content, _window, _cx| content.child(view.clone()))
        });
    }

    /// A dialog with one line of text and a confirm button; `done` gets the text.
    fn ask_name(
        &mut self,
        title: &'static str,
        ok: &'static str,
        initial: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl Fn(&mut Self, String, &mut Window, &mut Context<Self>) + 'static,
    ) {
        use gpui_kit::component::WindowExt as _;
        use gpui_kit::component::dialog::DialogButtonProps;
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
        let this = cx.entity().downgrade();
        let done = Rc::new(done);
        window.open_dialog(cx, move |dialog, window, cx| {
            input.update(cx, |state, cx| state.focus(window, cx));
            let (input_for_ok, content_input) = (input.clone(), input.clone());
            let (this, done) = (this.clone(), done.clone());
            dialog
                .title(title)
                .w(px(420.))
                .content(move |content, _window, _cx| content.child(Input::new(&content_input)))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(ok)
                        .show_cancel(true)
                        .on_ok(move |_, window, cx| {
                            let name = input_for_ok.read(cx).value().trim().to_string();
                            if name.is_empty() {
                                return false;
                            }
                            let done = done.clone();
                            this.update(cx, |panel, cx| done(panel, name, window, cx))
                                .is_ok()
                        }),
                )
        });
    }

    fn new_entry(&mut self, parent: VPath, dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let title = if dir { "New folder" } else { "New file" };
        self.ask_name(
            title,
            "Create",
            String::new(),
            window,
            cx,
            move |this, name, window, cx| {
                let path = parent.join(&name);
                let made = if dir {
                    this.fs.create_dir(&path)
                } else {
                    this.fs.create_file(&path)
                };
                match made {
                    Ok(()) => {
                        this.expanded.insert(id_of(&parent));
                        this.load(&parent, cx);
                        this.rebuild(cx);
                        if !dir {
                            this.open(&path, None, window, cx);
                        }
                    }
                    Err(e) => this.note("error", format!("could not create {path}: {e}"), cx),
                }
            },
        );
    }

    fn rename(&mut self, path: VPath, window: &mut Window, cx: &mut Context<Self>) {
        let old = path.file_name().unwrap_or_default().to_owned();
        self.ask_name(
            "Rename",
            "Rename",
            old,
            window,
            cx,
            move |this, name, _window, cx| {
                let parent = path.parent().unwrap_or_else(VPath::root);
                match this.fs.rename(&path, &parent.join(&name)) {
                    Ok(()) => this.refresh(cx),
                    Err(e) => this.note("error", format!("could not rename {path}: {e}"), cx),
                }
            },
        );
    }

    fn delete(&mut self, path: VPath, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        use gpui_kit::component::dialog::DialogButtonProps;
        let this = cx.entity().downgrade();
        let shown = path.relative().to_owned();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let (this, path, shown) = (this.clone(), path.clone(), shown.clone());
            dialog
                .title("Delete")
                .w(px(460.))
                .content({
                    let shown = shown.clone();
                    move |content, _window, _cx| {
                        content.child(format!("Delete {shown}? This cannot be undone."))
                    }
                })
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete")
                        .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                        .show_cancel(true)
                        .on_ok(move |_, _window, cx| {
                            let path = path.clone();
                            this.update(cx, |panel, cx| match panel.fs.remove(&path) {
                                Ok(()) => panel.refresh(cx),
                                Err(e) => {
                                    panel.note("error", format!("could not delete {path}: {e}"), cx)
                                }
                            })
                            .is_ok()
                        }),
                )
        });
    }

    /// The context menu for one path.
    fn menu(
        this: gpui_kit::WeakEntity<Self>,
        fs: Arc<dyn Vfs>,
        path: VPath,
        kind: Kind,
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TreeState>,
    ) -> PopupMenu {
        let writable = fs.writable();
        let local = fs.local_path(&path).is_some();
        let is_file = kind == Kind::File;
        let is_root = path.is_root();
        let parent_for_new = if kind == Kind::Dir {
            path.clone()
        } else {
            path.parent().unwrap_or_else(VPath::root)
        };

        // One closure shape for every item: run `f` on the panel.
        let act = {
            let this = this.clone();
            move |f: Rc<PanelAction>| {
                let this = this.clone();
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    let _ = this.update(cx, |panel, cx| f(panel, window, cx));
                }
            }
        };

        let mut menu = menu;
        if is_file {
            let p = path.clone();
            menu = menu.item(
                PopupMenuItem::new("Open")
                    .icon(IconName::FileText)
                    .on_click(act(Rc::new(move |t, w, c| t.open(&p, None, w, c)))),
            );
            for viewer in Viewer::ALL {
                let p = path.clone();
                menu = menu.item(
                    PopupMenuItem::new(format!("Open in {}", viewer.label()))
                        .icon(viewer.icon())
                        .on_click(act(Rc::new(move |t, w, c| t.open(&p, Some(viewer), w, c)))),
                );
            }
        }
        if is_file {
            let p = path.clone();
            menu = menu.item(
                PopupMenuItem::new("Preview in popup")
                    .icon(IconName::Eye)
                    .on_click(act(Rc::new(move |t, w, c| t.preview(&p, w, c)))),
            );
        }
        {
            let p = path.clone();
            let q = path.clone();
            let w = path.clone();
            menu = menu
                .item(
                    PopupMenuItem::new(if is_file {
                        "Open in default program"
                    } else {
                        "Open in file manager"
                    })
                    .icon(IconName::ExternalLink)
                    .disabled(!local)
                    .on_click(act(Rc::new(move |t, _, c| t.open_externally(&p, c)))),
                )
                .item(
                    PopupMenuItem::new("Open with…")
                        .icon(IconName::ExternalLink)
                        .disabled(!local || !is_file)
                        .on_click(act(Rc::new(move |t, _, c| t.open_with(&w, c)))),
                )
                .item(
                    PopupMenuItem::new("Show in file manager")
                        .icon(IconName::FolderOpen)
                        .disabled(!local || is_root)
                        .on_click(act(Rc::new(move |t, _, c| t.reveal(&q, c)))),
                );
        }
        menu = menu.separator();
        {
            let p = path.clone();
            let q = path.clone();
            menu = menu
                .item(
                    PopupMenuItem::new("Copy path")
                        .icon(IconName::Copy)
                        .on_click(act(Rc::new(move |t, _, c| {
                            t.copy(p.relative().to_owned(), c)
                        }))),
                )
                .item(
                    PopupMenuItem::new("Copy full path")
                        .icon(IconName::Copy)
                        .disabled(!local)
                        .on_click(act(Rc::new(move |t, _, c| {
                            if let Some(native) = t.fs.local_path(&q) {
                                t.copy(native.display().to_string(), c);
                            }
                        }))),
                );
        }
        menu = menu.separator();
        {
            let p = path.clone();
            menu = menu.item(
                PopupMenuItem::new("Properties…")
                    .icon(IconName::Info)
                    .on_click(act(Rc::new(move |t, w, c| t.properties(&p, &[], w, c)))),
            );
        }
        if is_file {
            menu = menu.separator().label("Hash");
            for algorithm in Algorithm::ALL {
                let p = path.clone();
                menu = menu.item(
                    PopupMenuItem::new(algorithm.label())
                        .icon(IconName::Hash)
                        .on_click(act(Rc::new(move |t, w, c| {
                            t.properties(&p, &[algorithm], w, c)
                        }))),
                );
            }
            let p = path.clone();
            menu = menu.item(
                PopupMenuItem::new("All hashes")
                    .icon(IconName::Hash)
                    .on_click(act(Rc::new(move |t, w, c| {
                        t.properties(&p, &Algorithm::ALL, w, c)
                    }))),
            );
        }
        menu = menu.separator();
        {
            let (a, b, r, d, f) = (
                parent_for_new.clone(),
                parent_for_new,
                path.clone(),
                path.clone(),
                path,
            );
            menu = menu
                .item(
                    PopupMenuItem::new("New file…")
                        .icon(IconName::FilePlus)
                        .disabled(!writable)
                        .on_click(act(Rc::new(move |t, w, c| {
                            t.new_entry(a.clone(), false, w, c)
                        }))),
                )
                .item(
                    PopupMenuItem::new("New folder…")
                        .icon(IconName::FolderPlus)
                        .disabled(!writable)
                        .on_click(act(Rc::new(move |t, w, c| {
                            t.new_entry(b.clone(), true, w, c)
                        }))),
                )
                .item(
                    PopupMenuItem::new("Rename…")
                        .icon(IconName::Pencil)
                        .disabled(!writable || is_root)
                        .on_click(act(Rc::new(move |t, w, c| t.rename(r.clone(), w, c)))),
                )
                .item(
                    PopupMenuItem::new("Delete…")
                        .icon(IconName::Trash)
                        .disabled(!writable || is_root)
                        .on_click(act(Rc::new(move |t, w, c| t.delete(d.clone(), w, c)))),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Refresh")
                        .icon(IconName::RotateCw)
                        .on_click(act(Rc::new(move |t, _, c| {
                            t.load(&f.clone(), c);
                            t.refresh(c)
                        }))),
                );
        }
        menu
    }
}

impl Focusable for FilesPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for FilesPanel {}

impl gpui_kit::base::dock::Panel for FilesPanel {
    fn panel_name(&self) -> &'static str {
        "files"
    }
}

impl Panel for FilesPanel {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_row(IconName::FolderTree, "Files")
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![
            Button::new("files-refresh")
                .icon(IconName::RotateCw)
                .ghost()
                .small()
                .tooltip("Read the open folders again")
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            Button::new("files-collapse")
                .icon(IconName::ChevronsDownUp)
                .ghost()
                .small()
                .tooltip("Collapse all folders")
                .on_click(cx.listener(|this, _, _, cx| this.collapse_all(cx))),
            close_button(&self.app, cx.entity()),
        ])
    }
}

fn file_icon(meta: &Meta, expanded: bool) -> IconName {
    match (meta.kind, expanded) {
        (Kind::Dir, true) => IconName::FolderOpen,
        (Kind::Dir, false) => IconName::Folder,
        _ if meta.is_symlink => IconName::Link,
        _ => IconName::File,
    }
}

impl Render for FilesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let this = cx.entity().downgrade();
        let mode = self.mode;

        let toolbar = v_flex()
            .gap_1()
            .p_1()
            .child(Input::new(&self.query).small().cleanable(true))
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("files-mode-names")
                            .label("Names")
                            .small()
                            .when(mode == SearchMode::Names, |b| b.primary())
                            .when(mode != SearchMode::Names, |b| b.ghost())
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.set_mode(SearchMode::Names, w, cx)
                            })),
                    )
                    .child(
                        Button::new("files-mode-content")
                            .label("Contents")
                            .small()
                            .when(mode == SearchMode::Content, |b| b.primary())
                            .when(mode != SearchMode::Content, |b| b.ghost())
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.set_mode(SearchMode::Content, w, cx)
                            })),
                    )
                    .child(div().text_color(muted).child(self.status.clone())),
            );

        let body: AnyElement = match &self.results {
            Some(results) => {
                let rows = results.len();
                let this = this.clone();
                let list = uniform_list(
                    "files-results",
                    rows,
                    cx.processor(move |panel, range: std::ops::Range<usize>, _window, _cx| {
                        range
                            .map(|ix| {
                                let (path, detail) = match panel.results.as_ref() {
                                    Some(Results::Names(v)) => (v[ix].clone(), None),
                                    Some(Results::Content(v)) => {
                                        (v[ix].path.clone(), Some((v[ix].line, v[ix].text.clone())))
                                    }
                                    None => return div().into_any_element(),
                                };
                                let open = path.clone();
                                let line = detail.as_ref().map(|(line, _)| *line as u32);
                                let this = this.clone();
                                h_flex()
                                    .id(("files-hit", ix))
                                    .gap_2()
                                    .px_2()
                                    .items_baseline()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(muted.opacity(0.15)))
                                    .on_click(move |_, window, cx| {
                                        let _ = this.update(cx, |p, cx| {
                                            p.open_at(&open, None, line, window, cx)
                                        });
                                    })
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(path.relative().to_owned()),
                                    )
                                    .when_some(detail, |row, (line, text)| {
                                        row.child(
                                            div()
                                                .flex_none()
                                                .text_color(muted)
                                                .child(format!(":{line}")),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .text_color(muted)
                                                .child(text),
                                        )
                                    })
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.results_scroll)
                .size_full();
                div()
                    .size_full()
                    .relative()
                    .child(list)
                    .vertical_scrollbar(&self.results_scroll)
                    .into_any_element()
            }
            None => {
                let meta = self.meta.clone();
                let fs = self.fs.clone();
                let (highlight, strong) = (theme.info, gpui_kit::black());
                let open_this = this.clone();
                let menu_this = this.clone();
                tree(&self.tree, move |ix, entry, selected, _window, _cx| {
                    let id = entry.item().id.clone();
                    let info = meta.borrow().get(&id).cloned();
                    let icon = info
                        .as_ref()
                        .map(|m| file_icon(m, entry.is_expanded()))
                        .unwrap_or(IconName::File);
                    let size = info.as_ref().and_then(|m| m.size).map(human_size);
                    let is_file = info.as_ref().is_some_and(|m| m.kind == Kind::File);
                    let open_this = open_this.clone();
                    // The tree marks the selected row with the library's faint style after this
                    // returns, over anything set on the item itself. So the item has no padding
                    // and the content below fills it edge to edge and paints the selection.
                    ListItem::new(ix)
                        .p_0()
                        .on_click(move |event, window, cx| {
                            if is_file && event.click_count() == 2 {
                                let path = VPath::new(&id);
                                let _ =
                                    open_this.update(cx, |p, cx| p.open(&path, None, window, cx));
                            }
                        })
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .h_full()
                                .items_center()
                                .py_1()
                                .pl(px(6. + 14. * entry.depth() as f32))
                                .pr_2()
                                .when(selected, |row| row.bg(highlight).text_color(strong))
                                .child(Icon::new(icon).small().flex_none())
                                .child(
                                    // One line, cut with an ellipsis: rows have a fixed height,
                                    // so a wrapped name would run into the next row.
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(entry.item().label.clone()),
                                )
                                .when_some(size, |row, size| {
                                    row.child(
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .text_color(if selected { strong } else { muted })
                                            .child(size),
                                    )
                                }),
                        )
                })
                .context_menu(move |_ix, entry, menu, window, cx| {
                    let id = entry.item().id.clone();
                    let kind = if entry.is_folder() || id.as_ref() == "/" {
                        Kind::Dir
                    } else {
                        Kind::File
                    };
                    FilesPanel::menu(
                        menu_this.clone(),
                        fs.clone(),
                        VPath::new(&id),
                        kind,
                        menu,
                        window,
                        cx,
                    )
                })
                .into_any_element()
            }
        };

        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
    }
}
