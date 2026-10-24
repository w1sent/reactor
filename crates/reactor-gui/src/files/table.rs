//! The table viewer: delimited text (CSV, TSV, …) and SQLite tables in gpui-kit's data table,
//! with sorting, a filter box and, for text, a delimiter picker and a user-chosen number of
//! header rows. Any number of them can be open ([ADR-0050]).
//!
//! The grid draws one of two things ([`Backing`]):
//!
//! - A delimited file: a [`Sheet`] from [`super::table_model`], read whole, sorted and
//!   filtered in memory.
//! - A SQLite table or view: [`super::table_sql::SqlView`]. Only the rows near the ones in
//!   view are held, fetched a page at a time on a worker through `reactor_db::browse`, and
//!   the filter and sort are part of the SELECT. The file is opened read-only and never
//!   changed — the agent's `reactor_data.sqlite` is read here.
//!
//! [ADR-0050]: ../../../../docs/adr/0050-the-table-viewer-is-a-grid-over-sources.md

mod database;

use database::Open;
pub use database::bind_keys;

use std::ops::Range;
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

use reactor_db::browse::{self, Count, Page, Value};
use reactor_fs::FileRef;

use super::table_model::{self as model, Delimiter, Sheet};
use super::table_sql::SqlView;
use crate::app::ReactorApp;
use crate::panels::{close_button, closer, panel_title_closing};
use crate::settings::Slot;

/// Larger files are not parsed into a grid.
const MAX_BYTES: u64 = 64 << 20;

/// What a grid cell holds, as far as it is known.
enum Cell<'a> {
    Text(&'a str),
    Value(&'a Value),
    /// A SQLite row whose page has not arrived.
    Pending,
}

/// Where the grid's rows come from.
enum Backing {
    Text {
        sheet: Arc<Sheet>,
        /// Body rows (indices into the sheet) that pass the filter, in sort order.
        order: Vec<usize>,
    },
    Sql(Box<SqlView>),
    /// The rows of a query, as they arrive. The database ordered them, so a sort is a
    /// re-run of the statement (`rerun`).
    Rows {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
    },
}

/// What the data table asks for.
struct Grid {
    backing: Backing,
    /// Text only: how many leading rows name the columns.
    header_rows: usize,
    filter: String,
    sort: Option<(usize, bool)>,
    widths: Vec<f32>,
    font: (SharedString, Pixels),
    /// The body rows in view, as the table last said.
    visible: Range<usize>,
    /// Why a page could not be read.
    error: Option<String>,
    /// For a query result: the panel and the tab that re-run the statement to sort it.
    rerun: Option<(gpui_kit::WeakEntity<TablePanel>, u64)>,
}

impl Grid {
    fn text(sheet: Arc<Sheet>, font: (SharedString, Pixels)) -> Self {
        let mut grid = Self {
            backing: Backing::Text {
                sheet,
                order: Vec::new(),
            },
            header_rows: 1,
            filter: String::new(),
            sort: None,
            widths: Vec::new(),
            font,
            visible: 0..50,
            error: None,
            rerun: None,
        };
        grid.reorder();
        grid
    }

    fn sql(view: SqlView, font: (SharedString, Pixels)) -> Self {
        let mut grid = Self {
            backing: Backing::Sql(Box::new(view)),
            header_rows: 0,
            filter: String::new(),
            sort: None,
            widths: Vec::new(),
            font,
            visible: 0..50,
            error: None,
            rerun: None,
        };
        grid.reorder();
        grid
    }

    /// The grid of a query tab: empty until its first rows arrive.
    fn results(
        font: (SharedString, Pixels),
        panel: gpui_kit::WeakEntity<TablePanel>,
        tab: u64,
    ) -> Self {
        Self {
            backing: Backing::Rows {
                columns: Vec::new(),
                rows: Vec::new(),
            },
            header_rows: 0,
            filter: String::new(),
            sort: None,
            widths: Vec::new(),
            font,
            visible: 0..50,
            error: None,
            rerun: Some((panel, tab)),
        }
    }

    fn width(&self) -> usize {
        match &self.backing {
            Backing::Text { sheet, .. } => sheet.width(),
            Backing::Sql(view) => view.columns.len(),
            Backing::Rows { columns, .. } => columns.len(),
        }
    }

    fn column_name(&self, col: usize) -> String {
        match &self.backing {
            Backing::Text { sheet, .. } => model::column_name(sheet, self.header_rows, col),
            Backing::Sql(view) => view.columns.get(col).cloned().unwrap_or_default(),
            Backing::Rows { columns, .. } => columns.get(col).cloned().unwrap_or_default(),
        }
    }

    /// Rows that pass the filter.
    fn shown(&self) -> usize {
        match &self.backing {
            Backing::Text { order, .. } => order.len(),
            Backing::Sql(view) => view.rows(),
            Backing::Rows { rows, .. } => rows.len(),
        }
    }

    fn cell(&self, row: usize, col: usize) -> Cell<'_> {
        match &self.backing {
            Backing::Text { sheet, order } => {
                Cell::Text(order.get(row).map_or("", |&r| sheet.cell(r, col)))
            }
            Backing::Sql(view) => view.pages.cell(row, col).map_or(Cell::Pending, Cell::Value),
            Backing::Rows { rows, .. } => rows
                .get(row)
                .and_then(|r| r.get(col))
                .map_or(Cell::Pending, Cell::Value),
        }
    }

    /// Text: put the rows in order and size the columns. SQLite: size the columns (the
    /// order is the database's, and arrives with the pages).
    fn reorder(&mut self) {
        if let Backing::Text { sheet, order } = &mut self.backing {
            *order = model::order(sheet, self.header_rows, &self.filter, self.sort);
        }
        self.measure();
    }

    /// Column widths from the first rows, so a column fits what is in it.
    fn measure(&mut self) {
        let size = self.font.1.as_f32();
        let fit = |chars: usize| (chars as f32 * size * 0.62 + 24.).clamp(70., 420.);
        self.widths = match &self.backing {
            Backing::Text { sheet, .. } => (0..sheet.width())
                .map(|col| {
                    let chars = (0..sheet.rows.len().min(200))
                        .map(|row| sheet.cell(row, col).chars().count())
                        .chain([model::column_name(sheet, self.header_rows, col)
                            .chars()
                            .count()
                            + 3])
                        .max()
                        .unwrap_or(4);
                    fit(chars)
                })
                .collect(),
            Backing::Sql(view) => {
                let mut chars: Vec<usize> =
                    view.columns.iter().map(|c| c.chars().count() + 3).collect();
                for row in view.pages.sample() {
                    for (col, value) in row.iter().enumerate().take(chars.len()) {
                        chars[col] = chars[col].max(value.display().chars().count());
                    }
                }
                chars.into_iter().map(fit).collect()
            }
            Backing::Rows { columns, rows } => {
                let mut chars: Vec<usize> = columns.iter().map(|c| c.chars().count() + 3).collect();
                for row in rows.iter().take(200) {
                    for (col, value) in row.iter().enumerate().take(chars.len()) {
                        chars[col] = chars[col].max(value.display().chars().count());
                    }
                }
                chars.into_iter().map(fit).collect()
            }
        };
    }

    /// SQLite: ask again, because the filter or the sort changed. Rows already on screen
    /// stay until the answer arrives.
    fn reload(&mut self, cx: &mut Context<TableState<Self>>) {
        let (filter, sort) = (self.filter.clone(), self.sort);
        self.error = None;
        let Backing::Sql(view) = &mut self.backing else {
            return;
        };
        view.filter = filter.clone();
        view.sort = sort;
        view.generation += 1;
        view.pages.clear();
        let (db, table, generation) = (view.db.clone(), view.table.clone(), view.generation);
        cx.spawn(async move |state, cx| {
            let counted = cx
                .background_executor()
                .spawn(async move {
                    db.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .count(&table, &filter)
                })
                .await;
            let _ = state.update(cx, |state, cx| {
                let pages = state.delegate_mut().counted(generation, counted);
                state.delegate_mut().fetch(pages, cx);
                state.refresh(cx);
            });
        })
        .detach();
    }

    /// The count of an earlier [`Grid::reload`] arrived: the pages to ask for.
    fn counted(&mut self, generation: u64, counted: reactor_db::Result<Count>) -> Vec<usize> {
        let Backing::Sql(view) = &mut self.backing else {
            return Vec::new();
        };
        if view.generation != generation {
            return Vec::new();
        }
        match counted {
            Ok(count) => {
                view.total = Some(count);
                view.pages.want(self.visible.clone(), count.rows as usize)
            }
            Err(e) => {
                view.total = Some(Count {
                    rows: 0,
                    exact: true,
                });
                self.error = Some(e.to_string());
                Vec::new()
            }
        }
    }

    /// SQLite: read these pages on a worker.
    fn fetch(&mut self, pages: Vec<usize>, cx: &mut Context<TableState<Self>>) {
        let Backing::Sql(view) = &self.backing else {
            return;
        };
        if pages.is_empty() {
            return;
        }
        let (db, table, generation) = (view.db.clone(), view.table.clone(), view.generation);
        let asked: Vec<_> = pages.into_iter().map(|p| (p, view.spec(p))).collect();
        cx.spawn(async move |state, cx| {
            let answers = cx
                .background_executor()
                .spawn(async move {
                    let db = db.lock().unwrap_or_else(|e| e.into_inner());
                    asked
                        .into_iter()
                        .map(|(page, spec)| (page, db.window(&table, &spec)))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = state.update(cx, |state, cx| {
                if state.delegate_mut().received(generation, answers) {
                    state.refresh(cx);
                } else {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Pages arrived. Whether the columns need sizing again, because these are the first.
    fn received(
        &mut self,
        generation: u64,
        answers: Vec<(usize, reactor_db::Result<Page>)>,
    ) -> bool {
        let near = self.visible.start;
        let Backing::Sql(view) = &mut self.backing else {
            return false;
        };
        if view.generation != generation {
            return false;
        }
        let first = view.pages.is_empty();
        for (page, answer) in answers {
            match answer {
                Ok(read) => view.pages.put(page, read.rows, near),
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        if first {
            self.measure();
        }
        first
    }

    /// The filter or the sort changed.
    fn apply(&mut self, cx: &mut Context<TableState<Self>>) {
        match self.backing {
            Backing::Text { .. } => self.reorder(),
            Backing::Sql(_) => self.reload(cx),
            // A result is filtered by the statement, and sorted by `sorted`.
            Backing::Rows { .. } => {}
        }
    }

    /// The sort was chosen. A result is ordered by running the statement again, which the
    /// panel does; this is called inside an update of the table, so it is asked for after.
    fn sorted(&mut self, cx: &mut Context<TableState<Self>>) {
        match (&self.backing, self.rerun.clone()) {
            (Backing::Rows { .. }, Some((panel, tab))) => {
                let sort = self.sort;
                cx.spawn(async move |_, cx| {
                    let _ = panel.update(cx, |panel, cx| panel.rerun(tab, sort, cx));
                })
                .detach();
            }
            _ => self.apply(cx),
        }
    }

    /// A query started: these are its columns, and nothing has arrived yet.
    fn start_result(&mut self, columns: Vec<String>) {
        if let Backing::Rows {
            columns: held,
            rows,
        } = &mut self.backing
        {
            *held = columns;
            rows.clear();
        }
        self.error = None;
        self.measure();
    }

    /// Rows of the running query arrived. Whether the columns need sizing again.
    fn push_result(&mut self, chunk: Vec<Vec<Value>>) -> bool {
        let Backing::Rows { rows, .. } = &mut self.backing else {
            return false;
        };
        let first = rows.is_empty();
        rows.extend(chunk);
        if first {
            self.measure();
        }
        first
    }

    /// A query result's columns and rows, for copying out.
    fn result(&self) -> Option<(&[String], &[Vec<Value>])> {
        match &self.backing {
            Backing::Rows { columns, rows } => Some((columns, rows)),
            _ => None,
        }
    }
}

impl TableDelegate for Grid {
    fn columns_count(&self, _: &App) -> usize {
        self.width()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.shown()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let sort = match self.sort {
            Some((col, true)) if col == col_ix => ColumnSort::Ascending,
            Some((col, false)) if col == col_ix => ColumnSort::Descending,
            _ => ColumnSort::Default,
        };
        Column::new(format!("col-{col_ix}"), self.column_name(col_ix))
            .width(px(self.widths.get(col_ix).copied().unwrap_or(120.)))
            .sort(sort)
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.sort = match sort {
            ColumnSort::Default => None,
            ColumnSort::Ascending => Some((col_ix, true)),
            ColumnSort::Descending => Some((col_ix, false)),
        };
        self.sorted(cx);
    }

    fn visible_rows_changed(
        &mut self,
        visible_range: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.visible = visible_range.clone();
        let pages = match &mut self.backing {
            Backing::Sql(view) => {
                let total = view.rows();
                view.pages.want(visible_range, total)
            }
            Backing::Text { .. } | Backing::Rows { .. } => return,
        };
        self.fetch(pages, cx);
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let base = div()
            .size_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .font_family(self.font.0.clone())
            .text_size(self.font.1);
        match self.cell(row_ix, col_ix) {
            Cell::Text(text) => base.child(text.to_owned()),
            Cell::Pending => base,
            Cell::Value(value) => match value {
                // A NULL is not an empty string, so it does not look like one.
                Value::Null => base
                    .italic()
                    .text_color(theme.muted_foreground)
                    .child("NULL"),
                Value::Integer(_) | Value::Real(_) => base.text_right().child(value.display()),
                Value::Blob(_) => base.text_color(theme.info).child(value.display()),
                Value::Text(text) => base.child(text.clone()),
            },
        }
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        match self.cell(row_ix, col_ix) {
            Cell::Text(text) => text.to_owned(),
            Cell::Value(value) => value.display(),
            Cell::Pending => String::new(),
        }
    }
}

pub struct TablePanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    file: FileRef,
    text: Arc<String>,
    delimiter: Delimiter,
    /// Whether the delimiter was worked out rather than chosen.
    auto: bool,
    /// The grid of a delimited file.
    table: Option<Entity<TableState<Grid>>>,
    /// The filter box: a delimited file's, or the shown table's of a database.
    filter: Entity<InputState>,
    header_rows: usize,
    problem: Option<String>,
    font: (SharedString, Pixels),
    focus: FocusHandle,
    /// A SQLite file, or a delimited file's rows loaded to be queried: opened, with its
    /// rail and its tabs.
    sqlite: Option<Open>,
    /// A delimited file's rows are being loaded into a database.
    loading_query: bool,
    /// The tabs to open once they are.
    pending: Vec<database::Remembered>,
    /// Why that did not work.
    text_notice: Option<String>,
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
            loading_query: false,
            pending: Vec::new(),
            text_notice: None,
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
        if browse::is_sqlite(&head) {
            self.load_database(window, cx);
            return;
        }
        // Reading a delimited file again keeps the queries that were open on it.
        let remembered = self.text_queries(cx);
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
        if !remembered.is_empty() {
            self.pending = remembered;
            self.query_text(window, cx);
        }
    }

    /// Select row `row` of a delimited file, counted as RFC 7111 counts them: from 1, the
    /// header rows among them. Where a reference to a row of the file lands (ADR-0066).
    pub fn goto_row(&mut self, row: u64, cx: &mut Context<Self>) {
        let Some(grid) = self.active_grid() else {
            return;
        };
        let ix = (row.saturating_sub(1) as usize).saturating_sub(self.header_rows);
        grid.update(cx, |table, cx| {
            table.scroll_to_row(ix, cx);
            table.set_selected_row(ix, cx);
        });
    }

    /// Parse the text again with the current delimiter and rebuild the table.
    fn reparse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match model::parse(&self.text, self.delimiter) {
            Ok(sheet) => {
                self.problem = None;
                let mut grid = Grid::text(Arc::new(sheet), self.font.clone());
                grid.header_rows = self.header_rows;
                grid.filter = self.filter.read(cx).value().to_string();
                grid.reorder();
                self.table = Some(cx.new(|cx| TableState::new(grid, window, cx)));
                // What was loaded for querying was loaded with the old delimiter.
                self.requery_after_change(window, cx);
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

    /// The grid being looked at: a delimited file's, or the active tab's.
    fn active_grid(&self) -> Option<Entity<TableState<Grid>>> {
        match &self.sqlite {
            Some(open) => open.active_grid(),
            None => self.table.clone(),
        }
    }

    /// Push the header count and filter into the grids.
    fn apply(&mut self, cx: &mut Context<Self>) {
        let (header_rows, filter) = (self.header_rows, self.filter.read(cx).value().to_string());
        // A database's filter belongs to the tab that is shown.
        if let Some(open) = &mut self.sqlite {
            open.set_filter(filter.clone());
        }
        // The header rows are the sheet's, whichever tab is shown.
        if let Some(sheet) = self.table.clone() {
            sheet.update(cx, |table, cx| {
                let grid = table.delegate_mut();
                if matches!(grid.backing, Backing::Text { .. }) && grid.header_rows != header_rows {
                    grid.header_rows = header_rows;
                    grid.reorder();
                    table.refresh(cx);
                }
            });
        }
        if let Some(table) = self.active_grid() {
            table.update(cx, |table, cx| {
                let grid = table.delegate_mut();
                grid.filter = filter;
                grid.apply(cx);
                table.refresh(cx);
            });
        }
        cx.notify();
    }

    fn set_header_rows(&mut self, rows: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.header_rows = rows;
        self.apply(cx);
        self.requery_after_change(window, cx);
    }

    /// The rows the grid reports and the cells it has for these rows (empty where a page has
    /// not arrived), for the tests that drive the real panel in a headless window
    /// (`tests/table_sqlite.rs`).
    #[doc(hidden)]
    pub fn cells_for_test(&self, rows: Range<usize>, cx: &App) -> (usize, Vec<Vec<String>>) {
        let Some(table) = self.active_grid() else {
            return (0, Vec::new());
        };
        let grid = table.read(cx).delegate();
        let cells = rows
            .map(|row| {
                (0..grid.width())
                    .map(|col| match grid.cell(row, col) {
                        Cell::Text(t) => t.to_owned(),
                        Cell::Value(v) => v.display(),
                        Cell::Pending => String::new(),
                    })
                    .collect()
            })
            .collect();
        (grid.shown(), cells)
    }

    /// The column names of the grid being looked at.
    #[doc(hidden)]
    pub fn columns_for_test(&self, cx: &App) -> Vec<String> {
        self.active_grid()
            .map(|t| {
                let grid = t.read(cx).delegate();
                (0..grid.width()).map(|c| grid.column_name(c)).collect()
            })
            .unwrap_or_default()
    }

    /// Sort by a column, as a click on its header does.
    #[doc(hidden)]
    pub fn sort_for_test(&mut self, col: usize, ascending: bool, cx: &mut Context<Self>) {
        if let Some(table) = self.active_grid() {
            table.update(cx, |table, cx| {
                let grid = table.delegate_mut();
                grid.sort = Some((col, ascending));
                grid.sorted(cx);
                table.refresh(cx);
            });
        }
    }

    /// Type into the filter box. Typing emits a change; setting the value does not, so this
    /// applies it as that event's handler does.
    #[doc(hidden)]
    pub fn filter_for_test(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |input, cx| {
            input.set_value(text.to_string(), window, cx)
        });
        self.apply(cx);
    }

    /// The message under the grid, if a page could not be read.
    #[doc(hidden)]
    pub fn error_for_test(&self, cx: &App) -> Option<String> {
        self.active_grid()
            .and_then(|t| t.read(cx).delegate().error.clone())
            .or_else(|| self.problem.clone())
    }
}

impl Focusable for TablePanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.active_grid() {
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        let ui = self
            .app
            .upgrade()
            .map(|a| a.read(cx).ui.clone())
            .unwrap_or_default();
        let font = ui.text(Slot::Mono, &theme);
        if font != self.font {
            self.font = font.clone();
            let grids = match &self.sqlite {
                Some(open) => open.grids(),
                None => self.table.iter().cloned().collect(),
            };
            for table in grids {
                let font = font.clone();
                table.update(cx, |table, cx| {
                    let grid = table.delegate_mut();
                    grid.font = font;
                    grid.reorder();
                    table.refresh(cx);
                });
            }
        }

        if self.sqlite.is_some() && self.problem.is_none() {
            return self.render_database(&font, window, cx);
        }
        self.render_text(cx)
    }
}

impl TablePanel {
    /// A delimited file: the filter, the header rows, the delimiter and the query button
    /// over its grid. Also the *Sheet* tab of a file that has been loaded for querying.
    fn render_text(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;

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
            let total = match &grid.backing {
                Backing::Text { sheet, .. } => sheet.rows.len().saturating_sub(grid.header_rows),
                _ => grid.shown(),
            };
            (grid.shown(), total, grid.width())
        };
        let weak = cx.entity().downgrade();
        let header_rows = self.header_rows;
        let delimiter = self.delimiter;
        let queryable = self.sqlite.is_none() && !self.loading_query;
        let loaded = self.sqlite.is_some();

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
                    .on_click(cx.listener(|this, _, window, cx| {
                        let rows = this.header_rows.saturating_sub(1);
                        this.set_header_rows(rows, window, cx)
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
                    .on_click(cx.listener(|this, _, window, cx| {
                        let rows = this.header_rows + 1;
                        this.set_header_rows(rows, window, cx)
                    })),
            )
            .child(
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
            // Not while it is already a database: its tabs are beside the sheet.
            .when(!loaded, |bar| {
                bar.child(
                    Button::new("table-query")
                        .ghost()
                        .small()
                        .label(if self.loading_query {
                            "Loading…"
                        } else {
                            "Query…"
                        })
                        .tooltip("Put these rows in a temporary database and ask it with SQL")
                        .disabled(!queryable)
                        .on_click(cx.listener(|this, _, window, cx| this.query_text(window, cx))),
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
                    .when_some(self.text_notice.clone(), |bar, message| {
                        bar.child(div().text_color(theme.danger).child(message))
                    }),
            )
            .into_any_element()
    }

    /// *Query…* as a click does it, for the tests (`tests/table_sqlite.rs`).
    #[doc(hidden)]
    pub fn query_text_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query_text(window, cx);
    }

    /// Set the number of header rows, as the + and − buttons do.
    #[doc(hidden)]
    pub fn header_rows_for_test(
        &mut self,
        rows: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_header_rows(rows, window, cx);
    }

    /// Whether a delimited file's rows are being loaded for querying.
    #[doc(hidden)]
    pub fn loading_for_test(&self) -> bool {
        self.loading_query
    }

    /// Why loading the rows failed, if it did.
    #[doc(hidden)]
    pub fn notice_for_test(&self) -> Option<String> {
        self.text_notice.clone()
    }
}
