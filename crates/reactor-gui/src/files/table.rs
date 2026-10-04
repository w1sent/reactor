//! The table viewer: delimited text (CSV, TSV, …) in gpui-kit's data table, with sorting,
//! a filter box, a delimiter picker and a user-chosen number of header rows. Any number of
//! them can be open.
//!
//! The grid comes from [`super::table_model`], which holds [`Sheet`]s so another format can
//! feed the same panel later. A SQLite file is one: its tables are picked from the toolbar and
//! read through `reactor_db::browse`, which opens the file read-only and never changes it —
//! the agent's `reactor_data.sqlite` is read here.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString, Window, div, px,
};

use reactor_fs::FileRef;

use super::table_model::{self as model, Delimiter, Sheet};
use crate::app::ReactorApp;
use crate::panels::{close_button, closer, panel_title_closing};
use crate::settings::Slot;

/// Larger files are not parsed into a grid.
const MAX_BYTES: u64 = 64 << 20;

/// What the data table asks for: the sheet through the current header count, filter and sort.
struct Grid {
    sheet: Arc<Sheet>,
    header_rows: usize,
    filter: String,
    sort: Option<(usize, bool)>,
    order: Vec<usize>,
    widths: Vec<f32>,
    font: (SharedString, Pixels),
}

impl Grid {
    fn new(sheet: Arc<Sheet>, font: (SharedString, Pixels)) -> Self {
        let mut grid = Self {
            sheet,
            header_rows: 1,
            filter: String::new(),
            sort: None,
            order: Vec::new(),
            widths: Vec::new(),
            font,
        };
        grid.reorder();
        grid
    }

    fn reorder(&mut self) {
        self.order = model::order(&self.sheet, self.header_rows, &self.filter, self.sort);
        // Column widths from the first rows, so a column fits what is in it.
        let size = self.font.1.as_f32();
        self.widths = (0..self.sheet.width())
            .map(|col| {
                let chars = (0..self.sheet.rows.len().min(200))
                    .map(|row| self.sheet.cell(row, col).chars().count())
                    .chain([model::column_name(&self.sheet, self.header_rows, col)
                        .chars()
                        .count()
                        + 3])
                    .max()
                    .unwrap_or(4);
                (chars as f32 * size * 0.62 + 24.).clamp(70., 420.)
            })
            .collect();
    }
}

impl TableDelegate for Grid {
    fn columns_count(&self, _: &App) -> usize {
        self.sheet.width()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.order.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let sort = match self.sort {
            Some((col, true)) if col == col_ix => ColumnSort::Ascending,
            Some((col, false)) if col == col_ix => ColumnSort::Descending,
            _ => ColumnSort::Default,
        };
        Column::new(
            format!("col-{col_ix}"),
            model::column_name(&self.sheet, self.header_rows, col_ix),
        )
        .width(px(self.widths.get(col_ix).copied().unwrap_or(120.)))
        .sort(sort)
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) {
        self.sort = match sort {
            ColumnSort::Default => None,
            ColumnSort::Ascending => Some((col_ix, true)),
            ColumnSort::Descending => Some((col_ix, false)),
        };
        self.reorder();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let text = self
            .order
            .get(row_ix)
            .map_or("", |&row| self.sheet.cell(row, col_ix))
            .to_owned();
        div()
            .size_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .font_family(self.font.0.clone())
            .text_size(self.font.1)
            .child(text)
    }
}

pub struct TablePanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    file: FileRef,
    text: Arc<String>,
    delimiter: Delimiter,
    /// Whether the delimiter was worked out rather than chosen.
    auto: bool,
    table: Option<Entity<TableState<Grid>>>,
    filter: Entity<InputState>,
    header_rows: usize,
    problem: Option<String>,
    font: (SharedString, Pixels),
    focus: FocusHandle,
    /// A SQLite file: where it is on disk, its tables, and which one is shown.
    sqlite: Option<Database>,
}

struct Database {
    path: PathBuf,
    tables: Vec<reactor_db::browse::TableInfo>,
    shown: Option<String>,
    /// The table has more rows than were read.
    truncated: bool,
}

impl TablePanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        file: FileRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter rows…"));
        cx.subscribe_in(
            &filter,
            window,
            |this, _, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.apply(cx);
                }
            },
        )
        .detach();
        // The app is mid-update while it opens a panel, so it cannot be read here; the
        // user's mono font is picked up in `render`.
        let font = (
            cx.theme().mono_font_family.clone(),
            cx.theme().mono_font_size,
        );
        let mut this = Self {
            app,
            file,
            text: Arc::new(String::new()),
            delimiter: Delimiter::Comma,
            auto: true,
            table: None,
            filter,
            header_rows: 1,
            problem: None,
            font,
            focus: cx.focus_handle(),
            sqlite: None,
        };
        this.load(window, cx);
        this
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let head = self
            .file
            .open()
            .and_then(|r| r.read_range(0, 16))
            .unwrap_or_default();
        if reactor_db::browse::is_sqlite(&head) {
            self.load_database(window, cx);
            return;
        }
        self.sqlite = None;
        let bytes = match self.file.read_all(MAX_BYTES) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.problem = Some(format!("could not read {}: {e}", self.file.path));
                return;
            }
        };
        self.text = Arc::new(String::from_utf8_lossy(&bytes).into_owned());
        self.delimiter = self
            .file
            .path
            .extension()
            .and_then(|ext| Delimiter::from_extension(&ext))
            .unwrap_or_else(|| model::detect_delimiter(&self.text));
        self.auto = true;
        self.reparse(window, cx);
    }

    /// A SQLite file: list its tables and show the one that was open, or the first.
    fn load_database(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.file.fs.local_path(&self.file.path) else {
            self.problem = Some(format!(
                "{} is a SQLite database, which can only be read from the local disk",
                self.file.path
            ));
            self.table = None;
            return;
        };
        let tables = match reactor_db::browse::tables(&path) {
            Ok(t) => t,
            Err(e) => {
                self.problem = Some(e.to_string());
                self.table = None;
                return;
            }
        };
        let keep = self.sqlite.as_ref().and_then(|d| d.shown.clone());
        let shown = keep
            .filter(|k| tables.iter().any(|t| t.name == *k))
            .or_else(|| tables.first().map(|t| t.name.clone()));
        self.sqlite = Some(Database {
            path,
            tables,
            shown,
            truncated: false,
        });
        self.show_table(window, cx);
    }

    /// Read the chosen table into the grid.
    fn show_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(db) = &mut self.sqlite else { return };
        let Some(name) = db.shown.clone() else {
            self.problem = Some("this database has no tables".into());
            self.table = None;
            cx.notify();
            return;
        };
        match reactor_db::browse::read_table(&db.path, &name, reactor_db::browse::MAX_ROWS) {
            Ok(read) => {
                db.truncated = read.truncated;
                self.problem = None;
                self.header_rows = 1;
                let sheet = Sheet {
                    name,
                    rows: read.rows,
                };
                let mut grid = Grid::new(Arc::new(sheet), self.font.clone());
                grid.header_rows = self.header_rows;
                grid.filter = self.filter.read(cx).value().to_string();
                grid.reorder();
                self.table = Some(cx.new(|cx| TableState::new(grid, window, cx)));
            }
            Err(e) => self.problem = Some(e.to_string()),
        }
        cx.notify();
    }

    /// Parse the text again with the current delimiter and rebuild the table.
    fn reparse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match model::parse(&self.text, self.delimiter) {
            Ok(sheet) => {
                self.problem = None;
                let mut grid = Grid::new(Arc::new(sheet), self.font.clone());
                grid.header_rows = self.header_rows;
                grid.filter = self.filter.read(cx).value().to_string();
                grid.reorder();
                self.table = Some(cx.new(|cx| TableState::new(grid, window, cx)));
            }
            Err(e) => {
                self.problem = Some(format!(
                    "could not parse as {}: {e}",
                    self.delimiter.label()
                ))
            }
        }
        cx.notify();
    }

    /// Push the header count and filter into the grid.
    fn apply(&mut self, cx: &mut Context<Self>) {
        let (header_rows, filter) = (self.header_rows, self.filter.read(cx).value().to_string());
        if let Some(table) = &self.table {
            table.update(cx, |table, cx| {
                let grid = table.delegate_mut();
                grid.header_rows = header_rows;
                grid.filter = filter;
                grid.reorder();
                table.refresh(cx);
            });
        }
        cx.notify();
    }

    fn set_header_rows(&mut self, rows: usize, cx: &mut Context<Self>) {
        self.header_rows = rows;
        self.apply(cx);
    }
}

impl Focusable for TablePanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.table {
            Some(table) => table.focus_handle(cx),
            None => self.focus.clone(),
        }
    }
}

impl EventEmitter<PanelEvent> for TablePanel {}

impl gpui_kit::base::dock::Panel for TablePanel {
    fn panel_name(&self) -> &'static str {
        "table"
    }
}

impl Panel for TablePanel {
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_closing(
            IconName::Table,
            &self.file.name(),
            closer(&self.app, cx.entity()),
        )
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![
            Button::new("table-reload")
                .icon(IconName::RotateCw)
                .ghost()
                .small()
                .tooltip("Read the file again")
                .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
            close_button(&self.app, cx.entity()),
        ])
    }
}

impl Render for TablePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;

        let ui = self
            .app
            .upgrade()
            .map(|a| a.read(cx).ui.clone())
            .unwrap_or_default();
        let font = ui.text(Slot::Mono, &theme);
        if font != self.font {
            self.font = font.clone();
            if let Some(table) = &self.table {
                table.update(cx, |table, cx| {
                    let grid = table.delegate_mut();
                    grid.font = font;
                    grid.reorder();
                    table.refresh(cx);
                });
            }
        }

        let (Some(table), None) = (&self.table, &self.problem) else {
            return div()
                .p_3()
                .track_focus(&self.focus)
                .text_color(theme.danger)
                .child(self.problem.clone().unwrap_or_default())
                .into_any_element();
        };
        let (shown, total, columns) = {
            let grid = table.read(cx).delegate();
            (
                grid.order.len(),
                grid.sheet.rows.len().saturating_sub(grid.header_rows),
                grid.sheet.width(),
            )
        };
        let weak = cx.entity().downgrade();
        let weak_tables = weak.clone();
        let truncated = self.sqlite.as_ref().is_some_and(|d| d.truncated);
        let header_rows = self.header_rows;
        let delimiter = self.delimiter;

        let toolbar = h_flex()
            .gap_1()
            .p_1()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .child(Input::new(&self.filter).small().cleanable(true)),
            )
            .child(div().text_color(muted).child("Header rows"))
            .child(
                Button::new("table-header-minus")
                    .icon(IconName::Minus)
                    .ghost()
                    .small()
                    .disabled(header_rows == 0)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let rows = this.header_rows.saturating_sub(1);
                        this.set_header_rows(rows, cx)
                    })),
            )
            .child(
                div()
                    .w(px(18.))
                    .text_center()
                    .child(header_rows.to_string()),
            )
            .child(
                Button::new("table-header-plus")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        let rows = this.header_rows + 1;
                        this.set_header_rows(rows, cx)
                    })),
            )
            .when_some(
                self.sqlite
                    .as_ref()
                    .map(|d| (d.tables.clone(), d.shown.clone().unwrap_or_default())),
                |bar, (tables, shown)| {
                    let weak = weak_tables.clone();
                    bar.child(
                        Button::new("table-picker")
                            .ghost()
                            .small()
                            .label(format!("Table: {shown}"))
                            .tooltip("Which table of this database to show")
                            .dropdown_menu(move |mut menu, _window, _cx| {
                                for t in &tables {
                                    let (weak, name) = (weak.clone(), t.name.clone());
                                    menu =
                                        menu.item(
                                            PopupMenuItem::new(format!(
                                                "{} — {}",
                                                t.name,
                                                t.rows_label()
                                            ))
                                            .checked(t.name == shown)
                                            .on_click(move |_, window, cx| {
                                                let name = name.clone();
                                                let _ = weak.update(cx, |panel, cx| {
                                                    if let Some(d) = &mut panel.sqlite {
                                                        d.shown = Some(name);
                                                    }
                                                    panel.show_table(window, cx);
                                                });
                                            }),
                                        );
                                }
                                menu
                            }),
                    )
                },
            )
            .when(self.sqlite.is_none(), |bar| {
                bar.child(
                    Button::new("table-delimiter")
                        .ghost()
                        .small()
                        .label(format!(
                            "{}{}",
                            delimiter.label(),
                            if self.auto { " (detected)" } else { "" }
                        ))
                        .tooltip("Field delimiter")
                        .dropdown_menu(move |mut menu, _window, _cx| {
                            for choice in Delimiter::ALL {
                                let weak = weak.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(choice.label())
                                        .checked(choice == delimiter)
                                        .on_click(move |_, window, cx| {
                                            let _ = weak.update(cx, |panel, cx| {
                                                panel.delimiter = choice;
                                                panel.auto = false;
                                                panel.reparse(window, cx);
                                            });
                                        }),
                                );
                            }
                            menu
                        }),
                )
            });

        v_flex()
            .size_full()
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(DataTable::new(table).stripe(true).small()),
            )
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_3()
                    .text_color(muted)
                    .child(format!("{shown} of {total} rows × {columns} columns"))
                    .when(truncated, |bar| {
                        bar.child(format!(
                            "only the first {} rows were read",
                            reactor_db::browse::MAX_ROWS
                        ))
                    }),
            )
            .into_any_element()
    }
}
