//! What the table viewer holds for a SQLite table: the rows near the ones in view, in
//! pages, and the filter and sort that decide which rows those are ([ADR-0050]). No gpui in
//! here; `table.rs` fetches the pages on a worker and draws them.
//!
//! The table is never read whole. The grid asks for the rows it shows, [`Pages::want`] says
//! which pages are missing, and [`Pages::put`] keeps the ones nearest to where it is.
//!
//! [ADR-0050]: ../../../../docs/adr/0050-the-table-viewer-is-a-grid-over-sources.md

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use reactor_db::browse::{Count, Database, Value, WindowSpec};

/// Rows in a page.
pub const PAGE_ROWS: usize = 200;

/// Pages held at once; the ones furthest from the view go first.
const KEEP_PAGES: usize = 48;

/// Pages of rows, by page number.
#[derive(Default)]
pub struct Pages {
    held: HashMap<usize, Arc<Vec<Vec<Value>>>>,
    asked: HashSet<usize>,
}

impl Pages {
    pub fn clear(&mut self) {
        self.held.clear();
        self.asked.clear();
    }

    /// The cell of body row `row`, when its page has arrived.
    pub fn cell(&self, row: usize, col: usize) -> Option<&Value> {
        self.held
            .get(&(row / PAGE_ROWS))?
            .get(row % PAGE_ROWS)?
            .get(col)
    }

    /// Whether any page is held.
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// The pages to fetch for these rows: those under them and one more either side, that
    /// are neither held nor already asked for. They are marked as asked.
    pub fn want(&mut self, rows: Range<usize>, total: usize) -> Vec<usize> {
        if total == 0 {
            return Vec::new();
        }
        let last = (total - 1) / PAGE_ROWS;
        let first = (rows.start / PAGE_ROWS).saturating_sub(1).min(last);
        let end = (rows.end.saturating_sub(1) / PAGE_ROWS + 1).min(last);
        let mut out = Vec::new();
        for page in first..=end {
            if !self.held.contains_key(&page) && self.asked.insert(page) {
                out.push(page);
            }
        }
        out
    }

    /// A page arrived. Pages far from `near` (a body row) are dropped to stay in bounds.
    pub fn put(&mut self, page: usize, rows: Vec<Vec<Value>>, near: usize) {
        self.asked.remove(&page);
        self.held.insert(page, Arc::new(rows));
        if self.held.len() > KEEP_PAGES {
            let centre = near / PAGE_ROWS;
            let mut by_distance: Vec<usize> = self.held.keys().copied().collect();
            by_distance.sort_by_key(|p| p.abs_diff(centre));
            for far in by_distance.split_off(KEEP_PAGES) {
                self.held.remove(&far);
            }
        }
    }

    /// The rows of the first page held, for sizing columns.
    pub fn sample(&self) -> impl Iterator<Item = &Vec<Value>> {
        let first = self.held.keys().min().copied();
        first
            .and_then(|p| self.held.get(&p))
            .into_iter()
            .flat_map(|rows| rows.iter().take(200))
    }
}

/// One table or view of an open database, as the grid sees it.
pub struct SqlView {
    pub db: Arc<Mutex<Database>>,
    pub table: String,
    /// The columns `SELECT *` gives, and what they were declared as.
    pub columns: Vec<String>,
    pub types: Vec<String>,
    pub filter: String,
    /// A column position and whether ascending.
    pub sort: Option<(usize, bool)>,
    /// Rows that match the filter; `None` until they are counted.
    pub total: Option<Count>,
    pub pages: Pages,
    /// Bumped whenever the filter or sort changes, so a late answer to an old question is
    /// not taken for the answer to this one.
    pub generation: u64,
}

impl SqlView {
    pub fn new(
        db: Arc<Mutex<Database>>,
        table: String,
        columns: Vec<String>,
        types: Vec<String>,
    ) -> SqlView {
        SqlView {
            db,
            table,
            columns,
            types,
            filter: String::new(),
            sort: None,
            total: None,
            pages: Pages::default(),
            generation: 0,
        }
    }

    /// The question for one page.
    pub fn spec(&self, page: usize) -> WindowSpec {
        WindowSpec {
            filter: self.filter.clone(),
            sort: self
                .sort
                .and_then(|(col, ascending)| Some((self.columns.get(col)?.clone(), ascending))),
            offset: (page * PAGE_ROWS) as u64,
            limit: PAGE_ROWS as u64,
        }
    }

    /// Rows to show, as far as they are known.
    pub fn rows(&self) -> usize {
        self.total.map_or(0, |c| c.rows as usize)
    }
}

/// Statements run on a database, most recent first.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct QueryHistory {
    items: Vec<String>,
}

/// Statements kept for one database.
const HISTORY_KEEP: usize = 50;

impl QueryHistory {
    /// Keep a statement that was run. One already there moves to the front, so it is not
    /// listed twice; an empty one is not kept.
    pub fn remember(&mut self, sql: &str) {
        let sql = sql.trim();
        if sql.is_empty() {
            return;
        }
        self.items.retain(|s| s != sql);
        self.items.insert(0, sql.to_string());
        self.items.truncate(HISTORY_KEEP);
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }
}

/// A statement as one line of a menu: its first line with anything on it, cut short, and an
/// ellipsis when there is more.
pub fn statement_label(sql: &str) -> String {
    const WIDTH: usize = 72;
    let mut lines = sql.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or_default();
    let more = lines.next().is_some();
    let mut label: String = first.chars().take(WIDTH).collect();
    if more || first.chars().count() > WIDTH {
        label.push('…');
    }
    label
}

static HISTORIES: std::sync::OnceLock<Mutex<HashMap<String, QueryHistory>>> =
    std::sync::OnceLock::new();

/// The statements run on the database `key` names, most recent first. They are kept while
/// the app runs, so closing a viewer and opening the file again finds them.
pub fn history(key: &str) -> Vec<String> {
    HISTORIES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(key)
        .map(|h| h.items().to_vec())
        .unwrap_or_default()
}

/// Note a statement run on the database `key` names.
pub fn remember_statement(key: &str, sql: &str) {
    HISTORIES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key.to_string())
        .or_default()
        .remember(sql);
}

/// `EXPLAIN QUERY PLAN` gives rows of `(id, parent, notused, detail)`. This is each one as a
/// line of an outline: its step, indented under the step it belongs to. `depths` holds the
/// depth of each step seen so far, so a plan can arrive in chunks.
pub fn plan_lines(rows: &[Vec<Value>], depths: &mut HashMap<i64, usize>) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let number = |i: usize| match row.get(i) {
                Some(Value::Integer(n)) => Some(*n),
                _ => None,
            };
            let detail = row.get(3).map(Value::display).unwrap_or_default();
            let depth = number(1)
                .filter(|parent| *parent != 0)
                .map_or(0, |parent| depths.get(&parent).map_or(0, |d| d + 1));
            if let Some(id) = number(0) {
                depths.insert(id, depth);
            }
            if depth == 0 {
                detail
            } else {
                format!("{}└─ {detail}", "   ".repeat(depth - 1))
            }
        })
        .collect()
}

/// Where a byte offset into a statement is, as a 1-based line and column (in characters).
pub fn line_col(sql: &str, offset: usize) -> (usize, usize) {
    let mut end = offset.min(sql.len());
    while !sql.is_char_boundary(end) {
        end -= 1;
    }
    let before = &sql[..end];
    let line = before.matches('\n').count() + 1;
    let col = before
        .rsplit('\n')
        .next()
        .map_or(0, |last| last.chars().count())
        + 1;
    (line, col)
}

/// The line of a statement that a byte offset is on, as a byte range without its newline.
/// An offset past the end, or on an empty line, gives the last line that has text.
pub fn line_range(sql: &str, offset: usize) -> Range<usize> {
    let mut at = offset.min(sql.len());
    while !sql.is_char_boundary(at) {
        at -= 1;
    }
    let line = |at: usize| {
        let start = sql[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = sql[at..].find('\n').map_or(sql.len(), |i| at + i);
        start..end
    };
    let mut range = line(at);
    while range.is_empty() && range.start > 0 {
        range = line(range.start - 1);
    }
    range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_the_latest_first_without_repeats_and_within_bounds() {
        let mut h = QueryHistory::default();
        h.remember("SELECT 1");
        h.remember("  SELECT 2  ");
        h.remember("   ");
        h.remember("SELECT 1");
        assert_eq!(h.items(), ["SELECT 1", "SELECT 2"]);
        for n in 0..80 {
            h.remember(&format!("SELECT {n}"));
        }
        assert_eq!(h.items().len(), 50);
        assert_eq!(h.items()[0], "SELECT 79");
    }

    #[test]
    fn a_statement_is_one_line_of_a_menu() {
        assert_eq!(statement_label("SELECT 1"), "SELECT 1");
        assert_eq!(statement_label("\n  SELECT a\nFROM t"), "SELECT a…");
        let long = format!("SELECT {}", "x".repeat(100));
        let label = statement_label(&long);
        assert_eq!(label.chars().count(), 73);
        assert!(label.ends_with('…'));
        assert_eq!(statement_label(""), "");
    }

    #[test]
    fn a_plan_becomes_an_outline() {
        let row = |id, parent, detail: &str| {
            vec![
                Value::Integer(id),
                Value::Integer(parent),
                Value::Integer(0),
                Value::Text(detail.into()),
            ]
        };
        let mut depths = HashMap::new();
        // Arriving in two chunks changes nothing.
        let mut lines = plan_lines(&[row(2, 0, "SCAN a"), row(5, 2, "SCAN b")], &mut depths);
        lines.extend(plan_lines(
            &[row(7, 5, "USE TEMP B-TREE"), row(9, 0, "SCAN c")],
            &mut depths,
        ));
        assert_eq!(
            lines,
            ["SCAN a", "└─ SCAN b", "   └─ USE TEMP B-TREE", "SCAN c"]
        );
        // A parent that was never seen is a top-level step, not a panic.
        let mut none = HashMap::new();
        assert_eq!(
            plan_lines(&[row(3, 99, "SEARCH t")], &mut none),
            ["SEARCH t"]
        );
    }

    #[test]
    fn the_line_an_offset_is_on() {
        let sql = "SELECT 1\nFROM t\nWHERE x FORM y";
        assert_eq!(line_range(sql, 0), 0..8);
        assert_eq!(line_range(sql, 8), 0..8, "on the newline: the line it ends");
        assert_eq!(line_range(sql, sql.find("FROM").unwrap()), 9..15);
        assert_eq!(line_range(sql, sql.find("FORM").unwrap()), 16..sql.len());
        assert_eq!(line_range(sql, 999), 16..sql.len());
        // Text that ends with a newline: the last line with something on it.
        assert_eq!(line_range("SELECT\n", 7), 0..6);
        assert_eq!(line_range("", 0), 0..0);
        assert_eq!(line_range("é\nb", 1), 0..2, "inside a character: its line");
    }

    #[test]
    fn an_offset_is_a_line_and_a_column() {
        let sql = "SELECT 1\nFROM t\nWHERE x FORM y";
        assert_eq!(line_col(sql, 0), (1, 1));
        assert_eq!(line_col(sql, sql.find("FROM").unwrap()), (2, 1));
        assert_eq!(line_col(sql, sql.find("FORM").unwrap()), (3, 9));
        // Past the end, and inside a multi-byte character, are clamped.
        assert_eq!(line_col("ab", 99), (1, 3));
        assert_eq!(line_col("é1", 1), (1, 1));
    }

    fn page(n: usize) -> Vec<Vec<Value>> {
        (0..PAGE_ROWS)
            .map(|i| vec![Value::Integer((n * PAGE_ROWS + i) as i64)])
            .collect()
    }

    #[test]
    fn a_row_is_found_in_its_page_once_it_has_arrived() {
        let mut pages = Pages::default();
        assert_eq!(pages.cell(0, 0), None);
        pages.put(1, page(1), 0);
        assert_eq!(pages.cell(PAGE_ROWS + 3, 0), Some(&Value::Integer(203)));
        assert_eq!(pages.cell(3, 0), None, "page 0 has not arrived");
        assert_eq!(pages.cell(PAGE_ROWS + 3, 1), None, "no such column");
    }

    #[test]
    fn what_is_wanted_is_what_is_in_view_and_a_page_either_side() {
        let mut pages = Pages::default();
        let total = 10 * PAGE_ROWS;
        assert_eq!(pages.want(0..30, total), [0, 1]);
        // Asking again for what was asked for is asking for nothing.
        assert_eq!(pages.want(0..30, total), Vec::<usize>::new());
        // Scrolled into the middle of page 4: pages 3, 4 and 5.
        assert_eq!(
            pages.want(4 * PAGE_ROWS + 50..4 * PAGE_ROWS + 80, total),
            [3, 4, 5]
        );
        // Page 5 arrives; scrolling to a row on the boundary needs only what is missing.
        pages.put(5, page(5), 0);
        assert_eq!(pages.want(5 * PAGE_ROWS..5 * PAGE_ROWS + 20, total), [6]);
    }

    #[test]
    fn wanting_stays_inside_the_table() {
        let mut pages = Pages::default();
        assert!(pages.want(0..30, 0).is_empty());
        // Three pages' worth: nothing past the last page, however far the view says.
        assert_eq!(
            pages.want(PAGE_ROWS * 2..PAGE_ROWS * 40, PAGE_ROWS * 3),
            [1, 2]
        );
        // One row: one page.
        let mut small = Pages::default();
        assert_eq!(small.want(0..50, 1), [0]);
    }

    #[test]
    fn the_pages_furthest_from_the_view_are_dropped() {
        let mut pages = Pages::default();
        for n in 0..(KEEP_PAGES + 10) {
            pages.put(n, page(n), 0);
        }
        assert!(pages.cell(0, 0).is_some(), "the nearest pages stay");
        assert!(pages.cell((KEEP_PAGES + 9) * PAGE_ROWS, 0).is_none());
        // Looking somewhere else keeps that instead.
        pages.put(
            KEEP_PAGES + 10,
            page(KEEP_PAGES + 10),
            (KEEP_PAGES + 10) * PAGE_ROWS,
        );
        assert!(pages.cell((KEEP_PAGES + 10) * PAGE_ROWS, 0).is_some());
    }

    #[test]
    fn clearing_forgets_what_was_asked() {
        let mut pages = Pages::default();
        assert_eq!(pages.want(0..10, PAGE_ROWS), [0]);
        pages.clear();
        assert_eq!(pages.want(0..10, PAGE_ROWS), [0]);
    }
}
