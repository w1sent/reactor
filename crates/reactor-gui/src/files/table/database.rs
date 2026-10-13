//! The table viewer on a SQLite file ([ADR-0050]): a rail of its tables and views with their
//! columns, and tabs that browse a table or run a query. Everything here is reading; the
//! file is never written.
//!
//! A table tab is a [`SqlView`] over the one shared connection, fetched a page at a time. A
//! query tab has an editor and its own connection (opened from the first one, so a long
//! statement does not hold up browsing), and the rows stream into its grid in chunks from a
//! worker. Stop interrupts the statement; sorting the result runs it again ordered.
//!
//! [ADR-0050]: ../../../../../docs/adr/0050-the-table-viewer-is-a-grid-over-sources.md

use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui_kit::KeyBinding;
use gpui_kit::actions;
use gpui_kit::component::input::{
    Editor, EditorState, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle,
};
use gpui_kit::component::resizable::{resizable_panel, v_resizable};

use reactor_db::browse::{
    self, Budget, Database, Event, Finished, InterruptHandle, OpenMode, QueryError, Stop,
    TableInfo, TableSchema, TextTable,
};

use super::*;
use crate::files::table_export::{self, Format};
use crate::files::table_sql::{
    history, line_col, line_range, plan_lines, remember_statement, statement_label,
};
use std::time::Duration;

actions!(
    reactor_table,
    [
        /// Ctrl+Enter / Cmd+Enter in a query: run it.
        RunQuery
    ]
);

const CONTEXT: &str = "ReactorQuery";

/// How long a click on a name in the rail waits for a second one, when a query could take
/// the name: a double click inserts it, a single click opens the table.
const DOUBLE_CLICK: Duration = Duration::from_millis(300);

/// What goes in front of a statement to ask SQLite how it would run it.
const EXPLAIN: &str = "EXPLAIN QUERY PLAN ";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("secondary-enter", RunQuery, Some(CONTEXT))]);
}

/// How a run of a statement is going.
enum Run {
    /// Nothing run yet.
    Idle,
    Running {
        rows: u64,
        stop: Arc<AtomicBool>,
    },
    Done(Finished),
    Failed {
        error: QueryError,
        /// Line and column in the statement that was run, when SQLite says where.
        at: Option<(usize, usize)>,
    },
}

struct Query {
    /// "Query 1", for the tab.
    n: u32,
    editor: Entity<EditorState>,
    grid: Entity<TableState<Grid>>,
    /// This tab's own connection, opened when it first runs.
    db: Option<Arc<Mutex<Database>>>,
    interrupt: Option<Arc<InterruptHandle>>,
    /// The statement of the last run, which a sort runs again.
    ran: String,
    /// The last run was a sort, whose offsets are into a statement that was wrapped.
    sorted: bool,
    /// The last run asked for the plan, not the rows: a result that is not sorted again.
    explain: bool,
    /// Bytes in front of the person's text in the statement that ran, to find a mistake.
    prefix: usize,
    /// How deep each step of a plan is, so the steps can be indented under their parents.
    depths: HashMap<i64, usize>,
    /// Paints the line a mistake is on.
    mark: RangeDecorationCollection,
    run: Run,
    /// Bumped for each run, so a late message of an old one is dropped.
    generation: u64,
}

impl Query {
    /// Stop what is running: the receiver is told to break, and the statement is
    /// interrupted in case it is not producing rows.
    fn cancel(&mut self) {
        if let Run::Running { stop, .. } = &self.run {
            stop.store(true, Ordering::Relaxed);
            if let Some(handle) = &self.interrupt {
                handle.interrupt();
            }
        }
    }
}

enum Kind {
    /// A delimited file's own grid, which the query tabs sit beside.
    Sheet {
        grid: Entity<TableState<Grid>>,
        filter: String,
    },
    Browse {
        table: String,
        grid: Entity<TableState<Grid>>,
        /// What was typed in the filter box while this tab was shown.
        filter: String,
    },
    Query(Box<Query>),
}

struct Tab {
    id: u64,
    kind: Kind,
}

/// What a reload brings back: the tabs that were open.
pub(super) enum Remembered {
    Table(String),
    Query(String),
}

/// What the database is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    /// A SQLite file the person opened.
    Sqlite,
    /// A delimited file's rows, loaded into a temporary database to be queried.
    Text,
}

/// An open database: its connection, the rail, and the tabs.
pub(super) struct Open {
    source: Source,
    db: Arc<Mutex<Database>>,
    mode: OpenMode,
    tables: Vec<TableInfo>,
    /// Columns and keys, read when a table is expanded or searched.
    schemas: HashMap<String, TableSchema>,
    expanded: HashSet<String>,
    rail_filter: Entity<InputState>,
    tabs: Vec<Tab>,
    active: Option<u64>,
    next_id: u64,
    queries: u32,
    /// What the statements run on this database are kept under.
    history_key: String,
    /// The query tab shown last, which a name double-clicked in the rail goes into.
    last_query: Option<u64>,
    /// Counts the clicks on the rail, so a pending open can tell it was overtaken.
    click_token: u64,
    /// Something went wrong that is not a tab's: shown under the strip.
    notice: Option<String>,
}

impl Open {
    fn tab(&self, id: u64) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    fn tab_mut(&mut self, id: u64) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.tab(id))
    }

    pub(super) fn active_grid(&self) -> Option<Entity<TableState<Grid>>> {
        self.active_tab().map(|t| match &t.kind {
            Kind::Sheet { grid, .. } | Kind::Browse { grid, .. } => grid.clone(),
            Kind::Query(q) => q.grid.clone(),
        })
    }

    /// Whether this is a delimited file's rows rather than a SQLite file.
    pub(super) fn is_text(&self) -> bool {
        self.source == Source::Text
    }

    /// Every grid, for the ones that need telling the font changed.
    pub(super) fn grids(&self) -> Vec<Entity<TableState<Grid>>> {
        self.tabs
            .iter()
            .map(|t| match &t.kind {
                Kind::Sheet { grid, .. } | Kind::Browse { grid, .. } => grid.clone(),
                Kind::Query(q) => q.grid.clone(),
            })
            .collect()
    }

    /// The filter box changed: it belongs to the table tab that is shown.
    pub(super) fn set_filter(&mut self, text: String) {
        if let Some(id) = self.active
            && let Some(tab) = self.tab_mut(id)
            && let Kind::Sheet { filter, .. } | Kind::Browse { filter, .. } = &mut tab.kind
        {
            *filter = text;
        }
    }

    fn take_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// The tabs as they are now, to be opened again after the file is read again.
    pub(super) fn remember(&self, cx: &App) -> Vec<Remembered> {
        self.tabs
            .iter()
            .filter_map(|t| match &t.kind {
                Kind::Sheet { .. } => None,
                Kind::Browse { table, .. } => Some(Remembered::Table(table.clone())),
                Kind::Query(q) => Some(Remembered::Query(q.editor.read(cx).value().to_string())),
            })
            .collect()
    }
}

/// A name as SQL writes it: bare when it can be, quoted when it cannot.
fn ident(name: &str) -> String {
    let bare = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if bare {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// What a query tab sends the panel while it runs.
enum Message {
    Event(Event),
    Done(Result<Finished, QueryError>),
}

impl TablePanel {
    /// A SQLite file: open it, list its tables, and show the tabs that were open before
    /// (after a reload), or the first table.
    pub(super) fn load_database(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.file.fs.local_path(&self.file.path) else {
            self.problem = Some(format!(
                "{} is a SQLite database, which can only be read from the local disk",
                self.file.path
            ));
            self.table = None;
            return;
        };
        let remembered = self
            .sqlite
            .as_ref()
            .filter(|open| !open.is_text())
            .map(|open| open.remember(cx))
            .unwrap_or_default();
        let opened = Database::open(&path).and_then(|db| {
            let tables = db.tables()?;
            Ok((db, tables))
        });
        let (db, tables) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                self.problem = Some(e.to_string());
                self.table = None;
                return;
            }
        };
        let rail_filter = self.rail_filter_input(window, cx);
        self.problem = None;
        self.table = None;
        self.sqlite = Some(Open {
            history_key: path.to_string_lossy().into_owned(),
            source: Source::Sqlite,
            mode: db.mode(),
            db: Arc::new(Mutex::new(db)),
            tables,
            schemas: HashMap::new(),
            expanded: HashSet::new(),
            rail_filter,
            tabs: Vec::new(),
            active: None,
            next_id: 0,
            queries: 0,
            last_query: None,
            click_token: 0,
            notice: None,
        });
        for item in remembered {
            match item {
                Remembered::Table(name) => {
                    let known = self
                        .sqlite
                        .as_ref()
                        .is_some_and(|o| o.tables.iter().any(|t| t.name == name));
                    if known {
                        self.open_table(&name, window, cx);
                    }
                }
                Remembered::Query(sql) => self.new_query(sql, window, cx),
            }
        }
        let empty = self.sqlite.as_ref().is_some_and(|o| o.tabs.is_empty());
        if empty {
            let first = self
                .sqlite
                .as_ref()
                .and_then(|o| o.tables.first().map(|t| t.name.clone()));
            if let Some(first) = first {
                self.open_table(&first, window, cx);
            }
        }
        cx.notify();
    }

    /// The find box over the rail.
    fn rail_filter_input(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find a table or column…"));
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                this.load_all_schemas();
                cx.notify();
            }
        })
        .detach();
        input
    }

    /// The tabs of a delimited file's database, when it has one: to open again after the
    /// file or its settings changed.
    pub(super) fn text_queries(&self, cx: &App) -> Vec<Remembered> {
        self.sqlite
            .as_ref()
            .filter(|open| open.is_text())
            .map(|open| open.remember(cx))
            .unwrap_or_default()
    }

    /// The delimiter or the header rows changed, so what was loaded for querying is out of
    /// date: load it again, with the tabs that were open.
    pub(super) fn requery_after_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let remembered = self.text_queries(cx);
        if remembered.is_empty() {
            return;
        }
        self.sqlite = None;
        self.pending = remembered;
        self.query_text(window, cx);
    }

    /// *Query…* on a delimited file: put its rows in a temporary database on a worker, then
    /// show them beside the sheet with a query tab. When it is loaded already, one more
    /// query tab.
    pub(super) fn query_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = &self.sqlite {
            let table = open.tables.first().map(|t| t.name.clone());
            if let Some(table) = table {
                let sql = format!("SELECT * FROM {} LIMIT 100", ident(&table));
                self.new_query(sql, window, cx);
                self.run_active(cx);
            }
            return;
        }
        if self.loading_query {
            return;
        }
        let Some(grid) = &self.table else { return };
        let (sheet, header_rows) = {
            let delegate = grid.read(cx).delegate();
            match &delegate.backing {
                Backing::Text { sheet, .. } => (sheet.clone(), delegate.header_rows),
                _ => return,
            }
        };
        let columns: Vec<String> = (0..sheet.width())
            .map(|c| super::model::column_name(&sheet, header_rows, c))
            .collect();
        let name = {
            let file = self.file.name();
            let stem = file.rsplit_once('.').map_or(file.as_str(), |(s, _)| s);
            if stem.is_empty() { "data" } else { stem }.to_string()
        };
        self.loading_query = true;
        self.text_notice = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    let body = sheet.rows.get(header_rows..).unwrap_or(&[]);
                    browse::load_text_table(&name, &columns, body)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| this.text_loaded(loaded, window, cx));
        })
        .detach();
    }

    /// The sheet is in its database: the sheet and the queries on it become tabs.
    fn text_loaded(
        &mut self,
        loaded: reactor_db::Result<TextTable>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.loading_query = false;
        let pending = std::mem::take(&mut self.pending);
        let loaded = match loaded {
            Ok(loaded) => loaded,
            Err(e) => {
                self.text_notice = Some(format!("could not load the rows for querying: {e}"));
                cx.notify();
                return;
            }
        };
        let Some(sheet_grid) = self.table.clone() else {
            return;
        };
        let TextTable {
            db, table, rows, ..
        } = loaded;
        let schema = db.schema(&table).ok();
        let rail_filter = self.rail_filter_input(window, cx);
        let filter = self.filter.read(cx).value().to_string();
        let history_key = self.file.fs.local_path(&self.file.path).map_or_else(
            || self.file.path.to_string(),
            |p| p.to_string_lossy().into_owned(),
        );
        let mut open = Open {
            history_key,
            source: Source::Text,
            mode: db.mode(),
            db: Arc::new(Mutex::new(db)),
            tables: vec![TableInfo {
                name: table.clone(),
                is_view: false,
                rows: Some(rows),
            }],
            schemas: HashMap::new(),
            expanded: HashSet::from([table.clone()]),
            rail_filter,
            tabs: Vec::new(),
            active: None,
            next_id: 0,
            queries: 0,
            last_query: None,
            click_token: 0,
            notice: None,
        };
        if let Some(schema) = schema {
            open.schemas.insert(table.clone(), schema);
        }
        let id = open.take_id();
        open.tabs.push(Tab {
            id,
            kind: Kind::Sheet {
                grid: sheet_grid,
                filter,
            },
        });
        open.active = Some(id);
        self.sqlite = Some(open);

        let had_queries = pending.iter().any(|p| matches!(p, Remembered::Query(_)));
        for item in pending {
            match item {
                Remembered::Table(name) if name == table => self.open_table(&name, window, cx),
                Remembered::Table(_) => {}
                Remembered::Query(sql) => self.new_query(sql, window, cx),
            }
        }
        if !had_queries {
            let sql = format!("SELECT * FROM {} LIMIT 100", ident(&table));
            self.new_query(sql, window, cx);
            self.run_active(cx);
        }
        cx.notify();
    }

    /// Columns and keys of every table, so the rail can be searched by column.
    fn load_all_schemas(&mut self) {
        let Some(open) = &mut self.sqlite else { return };
        let db = open.db.clone();
        let db = db.lock().unwrap_or_else(|e| e.into_inner());
        for table in &open.tables {
            if !open.schemas.contains_key(&table.name)
                && let Ok(schema) = db.schema(&table.name)
            {
                open.schemas.insert(table.name.clone(), schema);
            }
        }
    }

    fn toggle_expanded(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(open) = &mut self.sqlite else { return };
        if !open.expanded.remove(name) {
            open.expanded.insert(name.to_string());
            if !open.schemas.contains_key(name) {
                let schema = open
                    .db
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .schema(name);
                match schema {
                    Ok(schema) => {
                        open.schemas.insert(name.to_string(), schema);
                    }
                    Err(e) => open.notice = Some(e.to_string()),
                }
            }
        }
        cx.notify();
    }

    /// Show a table in a tab: the one already open for it, or a new one.
    pub(super) fn open_table(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let font = self.font.clone();
        let Some(open) = &mut self.sqlite else { return };
        let existing = open.tabs.iter().find_map(|t| match &t.kind {
            Kind::Browse { table, .. } if table == name => Some(t.id),
            _ => None,
        });
        if let Some(id) = existing {
            self.activate(id, window, cx);
            return;
        }
        let schema = open
            .db
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .schema(name);
        let schema = match schema {
            Ok(schema) => schema,
            Err(e) => {
                open.notice = Some(e.to_string());
                cx.notify();
                return;
            }
        };
        open.notice = None;
        let (columns, types) = schema
            .columns
            .iter()
            .filter(|c| !c.hidden)
            .map(|c| (c.name.clone(), c.declared_type.clone()))
            .unzip();
        open.schemas.insert(name.to_string(), schema);
        let view = SqlView::new(open.db.clone(), name.to_string(), columns, types);
        let grid = cx.new(|cx| TableState::new(Grid::sql(view, font), window, cx));
        grid.update(cx, |table, cx| table.delegate_mut().reload(cx));
        let id = open.take_id();
        open.tabs.push(Tab {
            id,
            kind: Kind::Browse {
                table: name.to_string(),
                grid,
                filter: String::new(),
            },
        });
        self.activate(id, window, cx);
    }

    /// A new query tab, with `sql` in its editor.
    pub(super) fn new_query(&mut self, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        let font = self.font.clone();
        let panel = cx.entity().downgrade();
        let Some(open) = &mut self.sqlite else { return };
        open.queries += 1;
        let n = open.queries;
        let id = open.take_id();
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .line_number(true)
                .soft_wrap(false)
        });
        editor.update(cx, |editor, cx| editor.set_value(sql, window, cx));
        let mark = editor.update(cx, |editor, cx| {
            editor.create_range_decorations_collection(Vec::new(), cx)
        });
        // Fixing the statement takes the mark off it.
        cx.subscribe_in(
            &editor,
            window,
            move |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.clear_mark(id, cx);
                }
            },
        )
        .detach();
        let grid = cx.new(|cx| TableState::new(Grid::results(font, panel, id), window, cx));
        open.tabs.push(Tab {
            id,
            kind: Kind::Query(Box::new(Query {
                n,
                editor,
                grid,
                db: None,
                interrupt: None,
                ran: String::new(),
                sorted: false,
                explain: false,
                prefix: 0,
                depths: HashMap::new(),
                mark,
                run: Run::Idle,
                generation: 0,
            })),
        });
        self.activate(id, window, cx);
    }

    fn activate(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &mut self.sqlite else { return };
        open.active = Some(id);
        if matches!(open.tab(id).map(|t| &t.kind), Some(Kind::Query(_))) {
            open.last_query = Some(id);
        }
        // The one filter box shows what was typed for this tab.
        let filter = match open.tab(id).map(|t| &t.kind) {
            Some(Kind::Sheet { filter, .. } | Kind::Browse { filter, .. }) => filter.clone(),
            _ => String::new(),
        };
        self.filter
            .update(cx, |input, cx| input.set_value(filter, window, cx));
        cx.notify();
    }

    fn close_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &mut self.sqlite else { return };
        let Some(ix) = open.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        if matches!(open.tabs[ix].kind, Kind::Sheet { .. }) {
            return;
        }
        if let Kind::Query(q) = &mut open.tabs[ix].kind {
            q.cancel();
        }
        open.tabs.remove(ix);
        if open.active == Some(id) {
            let next = open
                .tabs
                .get(ix)
                .or_else(|| ix.checked_sub(1).and_then(|i| open.tabs.get(i)))
                .map(|t| t.id);
            open.active = next;
            match next {
                Some(next) => self.activate(next, window, cx),
                None => {
                    self.filter
                        .update(cx, |input, cx| input.set_value(String::new(), window, cx));
                    cx.notify();
                }
            }
        } else {
            cx.notify();
        }
    }

    /// Run the statement in query tab `id`, or — with `sort` — the one it last ran, ordered
    /// by a result column.
    fn start(&mut self, id: u64, sort: Option<(usize, bool)>, cx: &mut Context<Self>) {
        self.start_with(id, sort, false, cx);
    }

    /// [`start`](Self::start), or — with `explain` — asking for the plan of the statement in
    /// the editor instead of its rows.
    fn start_with(
        &mut self,
        id: u64,
        sort: Option<(usize, bool)>,
        explain: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &mut self.sqlite else { return };
        let shared = open.db.clone();
        let history_key = open.history_key.clone();
        let Some(Tab {
            kind: Kind::Query(q),
            ..
        }) = open.tab_mut(id)
        else {
            return;
        };
        q.cancel();
        q.mark.clear(cx);
        q.sorted = sort.is_some();
        q.explain = explain;
        q.prefix = if explain { EXPLAIN.len() } else { 0 };
        q.depths.clear();
        let sql = if sort.is_some() {
            q.ran.clone()
        } else {
            let text = q.editor.read(cx).value().to_string();
            q.ran = text.clone();
            text
        };
        if sql.trim().is_empty() {
            q.run = Run::Failed {
                error: QueryError {
                    message: "there is nothing to run".into(),
                    offset: None,
                    refused: false,
                },
                at: None,
            };
            cx.notify();
            return;
        }
        if sort.is_none() {
            remember_statement(&history_key, &sql);
        }
        let statement = match sort {
            _ if explain => format!("{EXPLAIN}{sql}"),
            Some((column, ascending)) => browse::ordered_by(&sql, column + 1, ascending),
            None => sql.clone(),
        };
        // Its own connection, so a long statement does not hold up the tables.
        let db = match &q.db {
            Some(db) => db.clone(),
            None => {
                let opened = shared.lock().unwrap_or_else(|e| e.into_inner()).reopen();
                match opened {
                    Ok(db) => {
                        q.interrupt = Some(Arc::new(db.interrupt_handle()));
                        let db = Arc::new(Mutex::new(db));
                        q.db = Some(db.clone());
                        db
                    }
                    Err(e) => {
                        q.run = Run::Failed {
                            error: QueryError {
                                message: e.to_string(),
                                offset: None,
                                refused: false,
                            },
                            at: None,
                        };
                        cx.notify();
                        return;
                    }
                }
            }
        };
        q.generation += 1;
        let generation = q.generation;
        let stop = Arc::new(AtomicBool::new(false));
        q.run = Run::Running {
            rows: 0,
            stop: stop.clone(),
        };
        // The sort arrow stays on the column it was chosen on; a new run from the editor
        // starts unsorted.
        q.grid
            .update(cx, |table, _| table.delegate_mut().sort = sort);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
        cx.background_executor()
            .spawn(async move {
                let db = db.lock().unwrap_or_else(|e| e.into_inner());
                let result = db.run(&statement, &Budget::default(), |event| {
                    if stop.load(Ordering::Relaxed) {
                        return ControlFlow::Break(());
                    }
                    let _ = tx.send(Message::Event(event));
                    ControlFlow::Continue(())
                });
                let _ = tx.send(Message::Done(result));
            })
            .detach();
        cx.spawn(async move |panel, cx| {
            while let Some(message) = rx.recv().await {
                let last = matches!(message, Message::Done(_));
                let alive = panel
                    .update(cx, |panel, cx| {
                        panel.query_message(id, generation, message, cx)
                    })
                    .is_ok();
                if !alive || last {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// A result's sort was chosen: run the statement again, ordered.
    pub(super) fn rerun(&mut self, id: u64, sort: Option<(usize, bool)>, cx: &mut Context<Self>) {
        // A plan is not rows to be ordered: running it again ordered would ask SQLite for
        // `SELECT * FROM (EXPLAIN …)`, which it does not have.
        let explaining = self
            .sqlite
            .as_ref()
            .and_then(|o| o.tab(id))
            .is_some_and(|t| matches!(&t.kind, Kind::Query(q) if q.explain));
        if explaining {
            return;
        }
        self.start(id, sort, cx);
    }

    /// Put a statement from the history in the editor of query tab `id`.
    fn use_statement(&mut self, id: u64, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.sqlite.as_ref().and_then(|o| match o.tab(id) {
            Some(Tab {
                kind: Kind::Query(q),
                ..
            }) => Some(q.editor.clone()),
            _ => None,
        });
        if let Some(editor) = editor {
            editor.update(cx, |editor, cx| {
                editor.set_value(sql, window, cx);
                editor.focus(window, cx);
            });
            self.clear_mark(id, cx);
        }
    }

    fn explain_active(&mut self, cx: &mut Context<Self>) {
        let id = self.sqlite.as_ref().and_then(|o| match o.active_tab() {
            Some(Tab {
                id,
                kind: Kind::Query(_),
            }) => Some(*id),
            _ => None,
        });
        if let Some(id) = id {
            self.start_with(id, None, true, cx);
        }
    }

    fn stop_query(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(open) = &mut self.sqlite
            && let Some(Tab {
                kind: Kind::Query(q),
                ..
            }) = open.tab_mut(id)
        {
            q.cancel();
        }
        cx.notify();
    }

    fn query_message(
        &mut self,
        id: u64,
        generation: u64,
        message: Message,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &mut self.sqlite else { return };
        let Some(Tab {
            kind: Kind::Query(q),
            ..
        }) = open.tab_mut(id)
        else {
            return;
        };
        if q.generation != generation {
            return;
        }
        match message {
            Message::Event(Event::Columns(columns)) => {
                // A plan is one column: the outline.
                let columns = if q.explain {
                    vec!["plan".to_string()]
                } else {
                    columns
                };
                q.grid.update(cx, |table, cx| {
                    table.delegate_mut().start_result(columns);
                    table.refresh(cx);
                });
            }
            Message::Event(Event::Rows(chunk)) => {
                let chunk = if q.explain {
                    plan_lines(&chunk, &mut q.depths)
                        .into_iter()
                        .map(|line| vec![browse::Value::Text(line)])
                        .collect()
                } else {
                    chunk
                };
                if let Run::Running { rows, .. } = &mut q.run {
                    *rows += chunk.len() as u64;
                }
                q.grid.update(cx, |table, cx| {
                    table.delegate_mut().push_result(chunk);
                    table.refresh(cx);
                });
            }
            Message::Done(Ok(finished)) => q.run = Run::Done(finished),
            Message::Done(Err(error)) => {
                // A sort's statement was wrapped, so its offsets are not the editor's.
                let offset = error
                    .offset
                    .filter(|_| !q.sorted)
                    .and_then(|offset| offset.checked_sub(q.prefix));
                let at = offset.map(|offset| line_col(&q.ran, offset));
                if let Some(offset) = offset {
                    let mark = RangeDecoration::new(line_range(&q.ran, offset))
                        .with_style(RangeDecorationStyle::Fill)
                        .with_color(cx.theme().danger.opacity(0.28));
                    q.mark.set(vec![mark], cx);
                }
                q.run = Run::Failed { error, at };
            }
        }
        cx.notify();
    }

    fn clear_mark(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(open) = &mut self.sqlite
            && let Some(Tab {
                kind: Kind::Query(q),
                ..
            }) = open.tab_mut(id)
        {
            q.mark.clear(cx);
        }
    }

    /// A click on a table's name in the rail. One click opens the table; two insert its name
    /// into the query that was being written. When there is such a query, the single click
    /// waits a moment to see whether a second one follows, so a double click does not leave
    /// a table tab behind.
    fn rail_click(
        &mut self,
        name: String,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &mut self.sqlite else { return };
        open.click_token += 1;
        let token = open.click_token;
        if clicks >= 2 {
            self.insert_name(&ident(&name), window, cx);
            return;
        }
        let has_query = open
            .last_query
            .is_some_and(|id| matches!(open.tab(id).map(|t| &t.kind), Some(Kind::Query(_))));
        if !has_query {
            self.open_table(&name, window, cx);
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(DOUBLE_CLICK).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this
                    .sqlite
                    .as_ref()
                    .is_some_and(|open| open.click_token == token)
                {
                    this.open_table(&name, window, cx);
                }
            });
        })
        .detach();
    }

    /// A click on a column's name in the rail: two insert it.
    fn column_click(
        &mut self,
        name: String,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(open) = &mut self.sqlite {
            open.click_token += 1;
        }
        if clicks >= 2 {
            self.insert_name(&ident(&name), window, cx);
        }
    }

    /// Put `text` at the cursor of the query tab that was shown last, and show that tab.
    fn insert_name(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.sqlite.as_ref().and_then(|open| {
            let id = open.last_query?;
            match open.tab(id) {
                Some(Tab {
                    kind: Kind::Query(q),
                    ..
                }) => Some((id, q.editor.clone())),
                _ => None,
            }
        });
        let Some((id, editor)) = target else { return };
        self.activate(id, window, cx);
        let text = text.to_string();
        editor.update(cx, |editor, cx| {
            // Not run into the word before it.
            let before = editor
                .value()
                .get(..editor.cursor())
                .and_then(|text| text.chars().last());
            let spaced =
                before.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '"' | ')'));
            let text = if spaced { format!(" {text}") } else { text };
            editor.insert(text, window, cx);
            editor.focus(window, cx);
        });
    }

    fn run_active(&mut self, cx: &mut Context<Self>) {
        let id = self.sqlite.as_ref().and_then(|o| match o.active_tab() {
            Some(Tab {
                id,
                kind: Kind::Query(_),
            }) => Some(*id),
            _ => None,
        });
        if let Some(id) = id {
            self.start(id, None, cx);
        }
    }

    /// The result of query tab `id` as text, in `format`.
    fn result_text(&self, id: u64, format: Format, cx: &App) -> Option<String> {
        let open = self.sqlite.as_ref()?;
        let Some(Tab {
            kind: Kind::Query(q),
            ..
        }) = open.tab(id)
        else {
            return None;
        };
        let grid = q.grid.read(cx).delegate();
        let (columns, rows) = grid.result()?;
        Some(table_export::render(format, columns, rows))
    }

    fn copy_result(&mut self, id: u64, format: Format, cx: &mut Context<Self>) {
        if let Some(text) = self.result_text(id, format, cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Write the result as CSV to a file the person names.
    fn save_result(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.result_text(id, Format::Csv, cx) else {
            return;
        };
        let stem = self.file.name();
        let stem = stem.rsplit_once('.').map_or(stem.as_str(), |(s, _)| s);
        let name = format!("{stem}-result.csv");
        let directory = std::env::current_dir().unwrap_or_default();
        let receiver = cx.prompt_for_new_path(&directory, Some(&name));
        window
            .spawn(cx, async move |_| {
                if let Ok(Ok(Some(path))) = receiver.await {
                    let _ = std::fs::write(path, text);
                }
            })
            .detach();
    }

    /// What the bar under a query's grid says.
    fn query_status(q: &Query, columns: usize) -> String {
        match &q.run {
            Run::Idle => "Ctrl+Enter runs the statement. Only reads are allowed.".into(),
            Run::Running { rows, .. } => format!("Running… {rows} rows so far"),
            Run::Done(done) => {
                let ms = done.elapsed.as_millis();
                let mut line = if q.explain {
                    format!("Query plan · {} steps · {ms} ms", done.rows)
                } else {
                    format!("{} rows × {columns} columns · {ms} ms", done.rows)
                };
                match done.stop {
                    Stop::Finished => {}
                    Stop::Stopped => line.push_str(" · stopped"),
                    Stop::Budget => line.push_str(
                        " · stopped at the memory budget; add a LIMIT or narrow the WHERE",
                    ),
                }
                line
            }
            Run::Failed { error, at } => {
                let place = at.map_or(String::new(), |(l, c)| format!(" (line {l}, column {c})"));
                format!("{error}{place}")
            }
        }
    }

    // --- test hooks (tests/table_sqlite.rs) ------------------------------------------------

    /// Open a table in a tab, as a click on the rail does.
    #[doc(hidden)]
    pub fn open_table_for_test(
        &mut self,
        table: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_table(table, window, cx);
    }

    /// Open a query tab with `sql` and run it, as Ctrl+Enter does.
    #[doc(hidden)]
    pub fn query_for_test(&mut self, sql: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.new_query(sql.to_string(), window, cx);
        self.run_active(cx);
    }

    /// Ask for the plan of the active query's statement, as *Explain* does.
    #[doc(hidden)]
    pub fn explain_for_test(&mut self, cx: &mut Context<Self>) {
        self.explain_active(cx);
    }

    /// The statements run on this database, newest first.
    #[doc(hidden)]
    pub fn history_for_test(&self) -> Vec<String> {
        self.sqlite
            .as_ref()
            .map(|o| history(&o.history_key))
            .unwrap_or_default()
    }

    /// Choose the statement at `ix` of the history, as its menu entry does.
    #[doc(hidden)]
    pub fn use_history_for_test(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.sqlite.as_ref().and_then(|o| {
            let sql = history(&o.history_key).get(ix)?.clone();
            Some((o.active?, sql))
        });
        if let Some((id, sql)) = target {
            self.use_statement(id, sql, window, cx);
        }
    }

    /// Run the active query tab again, with the statement as it is in its editor.
    #[doc(hidden)]
    pub fn run_for_test(&mut self, cx: &mut Context<Self>) {
        self.run_active(cx);
    }

    /// Stop the active query tab's statement.
    #[doc(hidden)]
    pub fn stop_for_test(&mut self, cx: &mut Context<Self>) {
        let id = self.sqlite.as_ref().and_then(|o| o.active);
        if let Some(id) = id {
            self.stop_query(id, cx);
        }
    }

    /// A click on a table's name in the rail; `clicks` is 2 for a double click.
    #[doc(hidden)]
    pub fn rail_click_for_test(
        &mut self,
        name: &str,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rail_click(name.to_string(), clicks, window, cx);
    }

    /// A click on a column's name in the rail.
    #[doc(hidden)]
    pub fn column_click_for_test(
        &mut self,
        name: &str,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.column_click(name.to_string(), clicks, window, cx);
    }

    /// The active query's statement as it is in its editor.
    #[doc(hidden)]
    pub fn editor_text_for_test(&self, cx: &App) -> Option<String> {
        match self.sqlite.as_ref()?.active_tab()? {
            Tab {
                kind: Kind::Query(q),
                ..
            } => Some(q.editor.read(cx).value().to_string()),
            _ => None,
        }
    }

    /// The byte ranges marked in the active query's editor as where it went wrong.
    #[doc(hidden)]
    pub fn error_mark_for_test(&self, cx: &App) -> Vec<std::ops::Range<usize>> {
        match self.sqlite.as_ref().and_then(|o| o.active_tab()) {
            Some(Tab {
                kind: Kind::Query(q),
                ..
            }) => q.mark.get_ranges(cx),
            _ => Vec::new(),
        }
    }

    /// Show the tab at `ix`, as a click on it does.
    #[doc(hidden)]
    pub fn activate_for_test(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let id = self
            .sqlite
            .as_ref()
            .and_then(|o| o.tabs.get(ix).map(|t| t.id));
        if let Some(id) = id {
            self.activate(id, window, cx);
        }
    }

    /// Close the tab at `ix`.
    #[doc(hidden)]
    pub fn close_tab_for_test(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let id = self
            .sqlite
            .as_ref()
            .and_then(|o| o.tabs.get(ix).map(|t| t.id));
        if let Some(id) = id {
            self.close_tab(id, window, cx);
        }
    }

    /// Close the active tab, as its × does.
    #[doc(hidden)]
    pub fn close_active_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.sqlite.as_ref().and_then(|o| o.active) {
            self.close_tab(id, window, cx);
        }
    }

    /// Read the file again, as the reload button does.
    #[doc(hidden)]
    pub fn reload_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load(window, cx);
    }

    /// The tab labels, and which is active.
    #[doc(hidden)]
    pub fn tabs_for_test(&self) -> (Vec<String>, Option<usize>) {
        let Some(open) = &self.sqlite else {
            return (Vec::new(), None);
        };
        let labels = open
            .tabs
            .iter()
            .map(|t| match &t.kind {
                Kind::Sheet { .. } => "Sheet".to_string(),
                Kind::Browse { table, .. } => table.clone(),
                Kind::Query(q) => format!("Query {}", q.n),
            })
            .collect();
        let active = open
            .active
            .and_then(|id| open.tabs.iter().position(|t| t.id == id));
        (labels, active)
    }

    /// The line under the active query's grid.
    #[doc(hidden)]
    pub fn status_for_test(&self, cx: &App) -> String {
        let Some(open) = &self.sqlite else {
            return String::new();
        };
        match open.active_tab() {
            Some(Tab {
                kind: Kind::Query(q),
                ..
            }) => Self::query_status(q, q.grid.read(cx).delegate().width()),
            _ => String::new(),
        }
    }

    /// The active query's result as text.
    #[doc(hidden)]
    pub fn export_for_test(&self, format: &str, cx: &App) -> Option<String> {
        let format = Format::ALL.into_iter().find(|f| f.extension() == format)?;
        let id = self.sqlite.as_ref()?.active?;
        self.result_text(id, format, cx)
    }

    /// Whether a statement is running in the active query tab.
    #[doc(hidden)]
    pub fn running_for_test(&self) -> bool {
        self.sqlite
            .as_ref()
            .and_then(|o| o.active_tab())
            .is_some_and(
                |t| matches!(&t.kind, Kind::Query(q) if matches!(q.run, Run::Running { .. })),
            )
    }

    /// Put text in the active query's editor.
    #[doc(hidden)]
    pub fn editor_for_test(&mut self, sql: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.sqlite.as_ref().and_then(|o| match o.active_tab() {
            Some(Tab {
                kind: Kind::Query(q),
                ..
            }) => Some(q.editor.clone()),
            _ => None,
        });
        if let Some(editor) = editor {
            editor.update(cx, |editor, cx| {
                editor.set_value(sql.to_string(), window, cx)
            });
        }
    }

    // --- drawing ---------------------------------------------------------------------------

    pub(super) fn render_database(
        &mut self,
        font: &(SharedString, Pixels),
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let Some(open) = &self.sqlite else {
            return div().into_any_element();
        };

        // The strip: what this file is, and how it was opened.
        let tables = open.tables.iter().filter(|t| !t.is_view).count();
        let views = open.tables.len() - tables;
        let strip = h_flex()
            .px_2()
            .py_1()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .text_color(muted)
            .child(div().text_color(theme.primary).child(if open.is_text() {
                "Delimited text"
            } else {
                "SQLite"
            }))
            .child("read-only")
            .child(if open.is_text() {
                "its rows are in a temporary database; the file is not changed".to_string()
            } else {
                format!(
                    "{tables} table{} · {views} view{}",
                    if tables == 1 { "" } else { "s" },
                    if views == 1 { "" } else { "s" }
                )
            })
            .when(open.mode == OpenMode::Copy, |bar| {
                bar.child("read from a copy of the file and its -wal")
            })
            .when(open.mode == OpenMode::Immutable, |bar| {
                bar.child("opened immutable: nothing was written beside it")
            })
            .when_some(open.notice.clone(), |bar, notice| {
                bar.child(div().text_color(theme.danger).child(notice))
            });

        let rail = self.render_rail(cx);
        let tabs = self.render_tabs(cx);
        let body = self.render_body(font, cx);

        v_flex()
            .size_full()
            .child(strip)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(rail)
                    .child(v_flex().flex_1().min_w_0().h_full().child(tabs).child(body)),
            )
            .into_any_element()
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let open = self.sqlite.as_ref().expect("a database is open");
        let needle = open.rail_filter.read(cx).value().trim().to_lowercase();
        let shown_table = open.active_tab().and_then(|t| match &t.kind {
            Kind::Browse { table, .. } => Some(table.clone()),
            _ => None,
        });

        let mut tables = Vec::new();
        let mut views = Vec::new();
        for info in &open.tables {
            let columns: Vec<&browse::ColumnInfo> = open
                .schemas
                .get(&info.name)
                .map(|s| s.columns.iter().filter(|c| !c.hidden).collect())
                .unwrap_or_default();
            let by_name = info.name.to_lowercase().contains(&needle);
            let by_column = columns
                .iter()
                .any(|c| c.name.to_lowercase().contains(&needle));
            if !needle.is_empty() && !by_name && !by_column {
                continue;
            }
            // Searching by a column opens the table to show it.
            let expanded = open.expanded.contains(&info.name) || (!needle.is_empty() && by_column);
            let name = info.name.clone();
            let ix = tables.len() + views.len();
            let toggle_name = name.clone();
            let open_name = name.clone();
            let selected = shown_table.as_deref() == Some(info.name.as_str());
            let mut item = v_flex().child(
                h_flex()
                    .h(px(28.))
                    .px_1()
                    .gap_1()
                    .items_center()
                    .when(selected, |row| row.bg(theme.list_active))
                    .child(
                        Button::new(("rail-chevron", ix))
                            .icon(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_expanded(&toggle_name, cx)
                            })),
                    )
                    .child(
                        div()
                            .id(("rail-table", ix))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .hover(|row| row.bg(theme.list_hover))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(info.name.clone())
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                this.rail_click(open_name.clone(), event.click_count(), window, cx)
                            })),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(muted)
                            .child(match info.rows {
                                Some(_) => {
                                    info.rows_label().replace(" rows", "").replace(" row", "")
                                }
                                None => "view".to_string(),
                            }),
                    ),
            );
            if expanded {
                for (cn, column) in columns.iter().enumerate() {
                    let column_name = column.name.clone();
                    item = item.child(
                        h_flex()
                            .id(("rail-column", ix * 4096 + cn))
                            .cursor_pointer()
                            .hover(|row| row.bg(theme.list_hover))
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                this.column_click(
                                    column_name.clone(),
                                    event.click_count(),
                                    window,
                                    cx,
                                )
                            }))
                            .h(px(22.))
                            .pl(px(34.))
                            .pr_2()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .font_family(self.font.0.clone())
                                    .text_size(px(12.))
                                    .when(
                                        !needle.is_empty()
                                            && column.name.to_lowercase().contains(&needle),
                                        |c| c.text_color(theme.info),
                                    )
                                    .child(column.name.clone()),
                            )
                            .when(column.primary_key > 0, |row| {
                                row.child(div().text_xs().text_color(theme.primary).child("pk"))
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(column.declared_type.clone()),
                            ),
                    );
                }
            }
            if info.is_view {
                views.push(item.into_any_element());
            } else {
                tables.push(item.into_any_element());
            }
        }

        let heading = |text: String| {
            div()
                .px_2()
                .pt_2()
                .pb_1()
                .text_xs()
                .text_color(muted)
                .child(text)
        };
        v_flex()
            .w(px(248.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                div()
                    .p_1()
                    .child(Input::new(&open.rail_filter).small().cleanable(true)),
            )
            .child(
                v_flex()
                    .id("rail-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(!tables.is_empty(), |list| {
                        list.child(heading(format!("TABLES · {}", tables.len())))
                            .children(tables)
                    })
                    .when(!views.is_empty(), |list| {
                        list.child(heading(format!("VIEWS · {}", views.len())))
                            .children(views)
                    }),
            )
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let open = self.sqlite.as_ref().expect("a database is open");
        let mut strip = h_flex()
            .h(px(32.))
            .flex_none()
            .px_1()
            .gap_1()
            .items_center()
            .border_b_1()
            .border_color(theme.border);
        for (ix, tab) in open.tabs.iter().enumerate() {
            let id = tab.id;
            let active = open.active == Some(id);
            let (label, is_query) = match &tab.kind {
                Kind::Sheet { .. } => ("Sheet".to_string(), false),
                Kind::Browse { table, .. } => (table.clone(), false),
                Kind::Query(q) => (format!("Query {}", q.n), true),
            };
            let closable = !matches!(tab.kind, Kind::Sheet { .. });
            strip = strip.child(
                h_flex()
                    .h(px(28.))
                    .items_center()
                    .when(active, |t| t.bg(theme.list_active))
                    .child(
                        div()
                            .id(("tab", ix))
                            .px_2()
                            .h_full()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .hover(|t| t.bg(theme.list_hover))
                            .when(is_query, |t| t.italic())
                            .child(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.activate(id, window, cx)
                            })),
                    )
                    .when(closable, |tab| {
                        tab.child(
                            Button::new(("tab-close", ix))
                                .icon(IconName::X)
                                .ghost()
                                .xsmall()
                                .tooltip("Close this tab")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.close_tab(id, window, cx)
                                })),
                        )
                    }),
            );
        }
        strip
            .child(
                Button::new("tab-new-query")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .tooltip("A new query")
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.new_query(String::new(), window, cx)
                        }),
                    ),
            )
            .into_any_element()
    }

    fn render_body(&mut self, font: &(SharedString, Pixels), cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let open = self.sqlite.as_ref().expect("a database is open");
        let Some(tab) = open.active_tab() else {
            return div()
                .flex_1()
                .p_4()
                .text_color(muted)
                .child("Pick a table in the list, or press + for a query.")
                .into_any_element();
        };
        let id = tab.id;
        match &tab.kind {
            Kind::Sheet { .. } => self.render_text(cx),
            Kind::Browse { table, grid, .. } => {
                let (shown, columns, at_least, error, sort) = {
                    let delegate = grid.read(cx).delegate();
                    let at_least = match &delegate.backing {
                        Backing::Sql(view) => view.total.is_some_and(|c| !c.exact),
                        _ => false,
                    };
                    (
                        delegate.shown(),
                        delegate.width(),
                        at_least,
                        delegate.error.clone(),
                        delegate.sort,
                    )
                };
                let table_name = table.clone();
                let sort_name = sort.and_then(|(col, asc)| {
                    open.schemas.get(&table_name).and_then(|s| {
                        s.columns
                            .iter()
                            .filter(|c| !c.hidden)
                            .nth(col)
                            .map(|c| (c.name.clone(), asc))
                    })
                });
                let as_query = {
                    let mut sql = format!("SELECT * FROM {}", ident(&table_name));
                    if let Some((name, asc)) = &sort_name {
                        sql.push_str(&format!(
                            "\nORDER BY {} {}",
                            ident(name),
                            if *asc { "ASC" } else { "DESC" }
                        ));
                    }
                    sql
                };
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        h_flex()
                            .gap_1()
                            .p_1()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.filter).small().cleanable(true)),
                            )
                            .child(
                                Button::new("table-open-as-query")
                                    .ghost()
                                    .small()
                                    .label("Open as query")
                                    .tooltip("The same rows as a statement you can change")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.new_query(as_query.clone(), window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(DataTable::new(grid).stripe(true).small()),
                    )
                    .child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .gap_3()
                            .text_color(muted)
                            .child(format!(
                                "{shown}{} rows × {columns} columns",
                                if at_least { "+" } else { "" }
                            ))
                            .when_some(error, |bar, message| {
                                bar.child(div().text_color(theme.danger).child(message))
                            }),
                    )
                    .into_any_element()
            }
            Kind::Query(q) => {
                let (columns, has_rows) = {
                    let delegate = q.grid.read(cx).delegate();
                    (delegate.width(), delegate.shown() > 0)
                };
                let running = matches!(q.run, Run::Running { .. });
                let failed = matches!(q.run, Run::Failed { .. });
                let status = Self::query_status(q, columns);
                let weak = cx.entity().downgrade();
                let copy_id = id;
                let statements = history(&open.history_key);
                let weak_history = weak.clone();
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .key_context(CONTEXT)
                    .on_action(cx.listener(|this, _: &RunQuery, _, cx| this.run_active(cx)))
                    .child(
                        h_flex()
                            .gap_1()
                            .p_1()
                            .items_center()
                            .child(
                                Button::new("query-run")
                                    .primary()
                                    .small()
                                    .icon(IconName::Play)
                                    .label("Run")
                                    .tooltip("Run the statement (Ctrl+Enter, Cmd+Enter)")
                                    .disabled(running)
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.start(id, None, cx)),
                                    ),
                            )
                            .child(
                                Button::new("query-stop")
                                    .ghost()
                                    .small()
                                    .label("Stop")
                                    .disabled(!running)
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.stop_query(id, cx)),
                                    ),
                            )
                            .child(
                                Button::new("query-explain")
                                    .ghost()
                                    .small()
                                    .label("Explain")
                                    .tooltip("How SQLite would run this statement, instead of running it")
                                    .disabled(running)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start_with(id, None, true, cx)
                                    })),
                            )
                            .child(
                                Button::new("query-history")
                                    .ghost()
                                    .small()
                                    .label("History")
                                    .tooltip("Statements run on this database, newest first")
                                    .disabled(statements.is_empty())
                                    .dropdown_menu(move |mut menu, _window, _cx| {
                                        for sql in &statements {
                                            let (weak, text) = (weak_history.clone(), sql.clone());
                                            menu = menu.item(
                                                PopupMenuItem::new(statement_label(sql)).on_click(
                                                    move |_, window, cx| {
                                                        let text = text.clone();
                                                        let _ = weak.update(cx, |panel, cx| {
                                                            panel.use_statement(id, text, window, cx)
                                                        });
                                                    },
                                                ),
                                            );
                                        }
                                        menu
                                    }),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("query-copy")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Copy)
                                    .label("Copy as…")
                                    .disabled(!has_rows)
                                    .dropdown_menu(move |mut menu, _window, _cx| {
                                        for format in Format::ALL {
                                            let weak = weak.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(format.label()).on_click(
                                                    move |_, _, cx| {
                                                        let _ = weak.update(cx, |panel, cx| {
                                                            panel.copy_result(copy_id, format, cx)
                                                        });
                                                    },
                                                ),
                                            );
                                        }
                                        menu
                                    }),
                            )
                            .child(
                                Button::new("query-save")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Save)
                                    .label("Save as CSV…")
                                    .disabled(!has_rows)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.save_result(id, window, cx)
                                    })),
                            ),
                    )
                    .child(
                        // The editor and the result share the height; the divider between
                        // them is dragged. (The size is kept while the window is open.)
                        div()
                            .flex_1()
                            .min_h_0()
                            .border_t_1()
                            .border_color(theme.border)
                            .child(
                                v_resizable(("query-split", id as usize))
                                    .child(
                                        resizable_panel()
                                            .size(px(180.))
                                            .size_range(px(60.)..px(900.))
                                            .child(
                                                div().size_full().child(
                                                    Editor::new(&q.editor)
                                                        .bordered(false)
                                                        .font_family(font.0.clone())
                                                        .text_size(font.1)
                                                        .size_full(),
                                                ),
                                            ),
                                    )
                                    .child(
                                        resizable_panel()
                                            .size_range(px(80.)..px(4000.))
                                            .child(
                                                div().size_full().child(
                                                    DataTable::new(&q.grid).stripe(true).small(),
                                                ),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .gap_3()
                            .text_color(if failed { theme.danger } else { muted })
                            .child(status),
                    )
                    .into_any_element()
            }
        }
    }
}
