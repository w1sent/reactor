//! The table viewer on a SQLite file, in a real (headless) window: that rows arrive a page
//! at a time as the grid asks for them, and that sorting and filtering are the database's.
#![recursion_limit = "256"]

use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext, Bounds, Entity, Point, TestAppContext, WeakEntity, WindowBounds, WindowOptions, px,
    size,
};
use reactor_fs::{FileRef, LocalFs, VPath, Vfs};
use reactor_gui::files::table::TablePanel;

const ROWS: i64 = 1_000;

fn database(dir: &std::path::Path) {
    let c = rusqlite::Connection::open(dir.join("big.sqlite")).unwrap();
    c.execute_batch(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL, note TEXT);
         CREATE VIEW low AS SELECT * FROM t WHERE id <= 3;",
    )
    .unwrap();
    let mut insert = c.prepare("INSERT INTO t VALUES (?1, ?2, ?3, ?4)").unwrap();
    for i in 1..=ROWS {
        let note = if i % 10 == 0 {
            None
        } else {
            Some(format!("n{i}"))
        };
        insert
            .execute(rusqlite::params![
                i,
                format!("host-{i:04}"),
                i as f64 / 4.0,
                note
            ])
            .unwrap();
    }
}

fn open(
    cx: &mut TestAppContext,
    dir: &std::path::Path,
    name: &str,
) -> (gpui_kit::AnyWindowHandle, Entity<TablePanel>) {
    cx.update(gpui_kit::init);
    let fs: Arc<dyn Vfs> = Arc::new(LocalFs::new(dir, "test"));
    let file = FileRef::new(fs, VPath::new(&format!("/{name}")));
    let (window, panel) = cx.update(|cx| {
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(900.), px(500.)),
        };
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| TablePanel::new(WeakEntity::new_invalid(), file, window, cx)),
        )
        .expect("open test window")
    });
    settle(cx, window);
    (window, panel)
}

/// Draw, let the workers answer, and draw what they sent.
fn settle(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
}

fn first_cells(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    panel: &Entity<TablePanel>,
    n: usize,
) -> (usize, Vec<Vec<String>>) {
    cx.update_window(window, |_, _, cx| panel.read(cx).cells_for_test(0..n, cx))
        .unwrap()
}

#[gpui_kit::test]
async fn a_table_shows_its_rows_without_being_read_whole(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let (window, panel) = open(cx, dir.path(), "big.sqlite");

    // The first table by name is the view; the picker is how the table is chosen, so ask
    // for it directly.
    let (total, rows) = first_cells(cx, window, &panel, 3);
    assert_eq!(total, 3, "the first table listed is the view `low`");
    assert_eq!(rows[0], ["1", "host-0001", "0.25", "n1"]);

    cx.update_window(window, |_, window, cx| {
        panel.update(cx, |p, cx| p.open_table_for_test("t", window, cx));
    })
    .unwrap();
    settle(cx, window);
    let (total, rows) = first_cells(cx, window, &panel, 3);
    assert_eq!(total as i64, ROWS);
    assert_eq!(rows[0], ["1", "host-0001", "0.25", "n1"]);
    assert_eq!(rows[2][1], "host-0003");
    // Only the pages near the view were read: a row of the first page is there, one far
    // down the table has not been asked for.
    let held = |cx: &mut TestAppContext, row: usize| {
        cx.update_window(window, |_, _, cx| {
            panel.read(cx).cells_for_test(row..row + 1, cx).1[0][0].clone()
        })
        .unwrap()
    };
    assert_eq!(held(cx, 150), "151");
    assert_eq!(held(cx, 900), "", "page 4 has not been read");
    // A NULL reads as NULL, and is not an empty cell.
    let (_, tenth) = cx
        .update_window(window, |_, _, cx| panel.read(cx).cells_for_test(9..10, cx))
        .unwrap();
    assert_eq!(tenth[0][3], "NULL");
    assert_eq!(
        cx.update_window(window, |_, _, cx| panel.read(cx).error_for_test(cx))
            .unwrap(),
        None
    );
}

#[gpui_kit::test]
async fn sorting_and_filtering_are_done_by_the_database(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let (window, panel) = open(cx, dir.path(), "big.sqlite");
    cx.update_window(window, |_, window, cx| {
        panel.update(cx, |p, cx| p.open_table_for_test("t", window, cx));
    })
    .unwrap();
    settle(cx, window);

    // Descending by id puts the last row of a thousand first, though only a page was read.
    cx.update_window(window, |_, _, cx| {
        panel.update(cx, |p, cx| p.sort_for_test(0, false, cx));
    })
    .unwrap();
    settle(cx, window);
    let (total, rows) = first_cells(cx, window, &panel, 2);
    assert_eq!(total as i64, ROWS);
    assert_eq!(rows[0][0], "1000");
    assert_eq!(rows[1][0], "999");

    // A filter narrows the count, and the order holds.
    cx.update_window(window, |_, window, cx| {
        panel.update(cx, |p, cx| p.filter_for_test("host-099", window, cx));
    })
    .unwrap();
    settle(cx, window);
    let (total, rows) = first_cells(cx, window, &panel, 2);
    assert_eq!(total, 10, "host-0990 to host-0999");
    assert_eq!(rows[0][1], "host-0999");

    // Nothing matches, and it says zero rather than keeping the old rows.
    cx.update_window(window, |_, window, cx| {
        panel.update(cx, |p, cx| p.filter_for_test("no such host", window, cx));
    })
    .unwrap();
    settle(cx, window);
    assert_eq!(first_cells(cx, window, &panel, 1).0, 0);
}

#[gpui_kit::test]
async fn delimited_text_is_unchanged(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("hosts.csv"),
        "name,port\nws-042,3389\nws-043,22\nsrv-db1,5432\n",
    )
    .unwrap();
    let (window, panel) = open(cx, dir.path(), "hosts.csv");
    let (total, rows) = first_cells(cx, window, &panel, 3);
    assert_eq!(total, 3, "the header row is not a body row");
    assert_eq!(rows[0], ["ws-042", "3389"]);

    cx.update_window(window, |_, _, cx| {
        panel.update(cx, |p, cx| p.sort_for_test(1, true, cx));
    })
    .unwrap();
    settle(cx, window);
    let (_, rows) = first_cells(cx, window, &panel, 3);
    assert_eq!(
        rows.iter().map(|r| r[1].as_str()).collect::<Vec<_>>(),
        ["22", "3389", "5432"],
        "numbers sort as numbers"
    );
}

fn panel_update(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    panel: &Entity<TablePanel>,
    f: impl FnOnce(&mut TablePanel, &mut gpui_kit::Window, &mut gpui_kit::Context<TablePanel>),
) {
    cx.update_window(window, |_, window, cx| {
        panel.update(cx, |p, cx| f(p, window, cx))
    })
    .unwrap();
    settle(cx, window);
}

fn status(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    panel: &Entity<TablePanel>,
) -> String {
    cx.update_window(window, |_, _, cx| panel.read(cx).status_for_test(cx))
        .unwrap()
}

#[gpui_kit::test]
async fn a_query_streams_its_rows_into_the_grid(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let (window, panel) = open(cx, dir.path(), "big.sqlite");

    panel_update(cx, window, &panel, |p, w, cx| {
        p.query_for_test("SELECT id, name FROM t WHERE id <= 5 ORDER BY id", w, cx)
    });
    let (total, rows) = first_cells(cx, window, &panel, 5);
    assert_eq!(total, 5);
    assert_eq!(rows[0], ["1", "host-0001"]);
    assert_eq!(rows[4], ["5", "host-0005"]);
    let line = status(cx, window, &panel);
    assert!(line.starts_with("5 rows × 2 columns"), "{line}");

    // More than a chunk: every row arrives, in order, without a limit on the count.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.editor_for_test("SELECT id FROM t", w, cx);
        p.run_for_test(cx)
    });
    let (total, _) = first_cells(cx, window, &panel, 1);
    assert_eq!(total as i64, ROWS);
    assert!(status(cx, window, &panel).starts_with("1000 rows × 1 columns"));
}

#[gpui_kit::test]
async fn sorting_a_result_runs_the_statement_again_ordered(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let (window, panel) = open(cx, dir.path(), "big.sqlite");
    panel_update(cx, window, &panel, |p, w, cx| {
        p.query_for_test("SELECT id, name FROM t -- every host\n", w, cx)
    });
    assert_eq!(first_cells(cx, window, &panel, 1).1[0][0], "1");

    panel_update(cx, window, &panel, |p, _, cx| p.sort_for_test(0, false, cx));
    let (total, rows) = first_cells(cx, window, &panel, 2);
    assert_eq!(total as i64, ROWS, "the sort covers every row");
    assert_eq!(rows[0][0], "1000");
    assert_eq!(rows[1][0], "999");

    // Editing the statement and running it starts unsorted again.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.editor_for_test("SELECT id FROM t WHERE id < 4", w, cx);
        p.run_for_test(cx)
    });
    let (total, rows) = first_cells(cx, window, &panel, 3);
    assert_eq!((total, rows[0][0].as_str()), (3, "1"));
}

#[gpui_kit::test]
async fn a_query_that_would_write_or_is_wrong_says_so_and_changes_nothing(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let before = std::fs::read(dir.path().join("big.sqlite")).unwrap();
    let (window, panel) = open(cx, dir.path(), "big.sqlite");

    panel_update(cx, window, &panel, |p, w, cx| {
        p.query_for_test("UPDATE t SET note = 'x'", w, cx)
    });
    let line = status(cx, window, &panel);
    assert!(line.contains("only reads"), "{line}");

    panel_update(cx, window, &panel, |p, w, cx| {
        p.editor_for_test("SELECT 1\nFORM t", w, cx);
        p.run_for_test(cx)
    });
    let line = status(cx, window, &panel);
    assert!(
        line.contains("syntax error") && line.contains("line 2"),
        "{line}"
    );

    // The tab still works afterwards.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.editor_for_test("SELECT count(*) FROM t", w, cx);
        p.run_for_test(cx)
    });
    assert_eq!(first_cells(cx, window, &panel, 1).1[0][0], "1000");
    assert_eq!(
        std::fs::read(dir.path().join("big.sqlite")).unwrap(),
        before
    );
}

#[gpui_kit::test]
async fn a_result_is_copied_out_in_the_formats_offered(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let (window, panel) = open(cx, dir.path(), "big.sqlite");
    panel_update(cx, window, &panel, |p, w, cx| {
        p.query_for_test(
            "SELECT id, note FROM t WHERE id IN (9, 10) ORDER BY id",
            w,
            cx,
        )
    });
    let get = |cx: &mut TestAppContext, format: &str| {
        cx.update_window(window, |_, _, cx| {
            panel.read(cx).export_for_test(format, cx)
        })
        .unwrap()
        .unwrap()
    };
    assert_eq!(get(cx, "csv"), "id,note\n9,n9\n10,\n");
    assert_eq!(get(cx, "tsv"), "id\tnote\n9\tn9\n10\t\n");
    assert_eq!(
        get(cx, "json"),
        "[\n  {\"id\": 9, \"note\": \"n9\"},\n  {\"id\": 10, \"note\": null}\n]\n"
    );
    assert_eq!(
        get(cx, "md"),
        "| id | note |\n| --- | --- |\n| 9 | n9 |\n| 10 | NULL |\n"
    );
}

#[gpui_kit::test]
async fn tabs_open_once_close_and_survive_a_reload(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let (window, panel) = open(cx, dir.path(), "big.sqlite");
    let tabs = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, _, cx| panel.read(cx).tabs_for_test())
            .unwrap()
    };
    // The first table listed opens by itself.
    assert_eq!(tabs(cx), (vec!["low".to_string()], Some(0)));

    panel_update(cx, window, &panel, |p, w, cx| {
        p.open_table_for_test("t", w, cx)
    });
    panel_update(cx, window, &panel, |p, w, cx| {
        p.open_table_for_test("low", w, cx)
    });
    assert_eq!(
        tabs(cx),
        (vec!["low".to_string(), "t".to_string()], Some(0)),
        "a table already open is shown, not opened again"
    );

    panel_update(cx, window, &panel, |p, w, cx| {
        p.query_for_test("SELECT 1 AS one", w, cx)
    });
    assert_eq!(tabs(cx).1, Some(2));
    assert_eq!(first_cells(cx, window, &panel, 1).1[0][0], "1");

    // Reading the file again keeps the tabs.
    panel_update(cx, window, &panel, |p, w, cx| p.reload_for_test(w, cx));
    let (labels, _) = tabs(cx);
    assert_eq!(labels, ["low", "t", "Query 1"]);

    // Closing the active tab shows its neighbour.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.close_active_for_test(w, cx)
    });
    let (labels, active) = tabs(cx);
    assert_eq!(labels, ["low", "t"]);
    assert_eq!(active, Some(1));
    assert_eq!(first_cells(cx, window, &panel, 1).0 as i64, ROWS);
}

fn tabs_of(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    panel: &Entity<TablePanel>,
) -> (Vec<String>, Option<usize>) {
    cx.update_window(window, |_, _, cx| panel.read(cx).tabs_for_test())
        .unwrap()
}

#[gpui_kit::test]
async fn a_delimited_file_is_queried_on_request_and_only_then(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("hosts.csv"),
        "name,port,zip\nws-042,3389,007\nws-043,22,010\nsrv-db1,5432,011\n",
    )
    .unwrap();
    let (window, panel) = open(cx, dir.path(), "hosts.csv");
    // Opening it is as before: no tabs, nothing loaded.
    assert_eq!(tabs_of(cx, window, &panel), (Vec::<String>::new(), None));
    assert_eq!(first_cells(cx, window, &panel, 1).0, 3);

    panel_update(cx, window, &panel, |p, w, cx| p.query_text_for_test(w, cx));
    assert_eq!(
        tabs_of(cx, window, &panel),
        (vec!["Sheet".to_string(), "Query 1".to_string()], Some(1))
    );
    // The statement it starts with has run: the three rows, in the columns' own types.
    let (total, rows) = first_cells(cx, window, &panel, 3);
    assert_eq!(total, 3);
    assert_eq!(rows[0], ["ws-042", "3389", "007"]);

    // The port column is a number and the zip code is text, so sums work and `007` stays.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.editor_for_test("SELECT sum(port), min(zip) FROM hosts", w, cx);
        p.run_for_test(cx)
    });
    assert_eq!(first_cells(cx, window, &panel, 1).1[0], ["8843", "007"]);

    // Nothing was written beside the file.
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "only hosts.csv is in its folder"
    );

    // The sheet is still there, with its own filter.
    panel_update(cx, window, &panel, |p, w, cx| p.activate_for_test(0, w, cx));
    assert_eq!(tabs_of(cx, window, &panel).1, Some(0));
    panel_update(cx, window, &panel, |p, w, cx| {
        p.filter_for_test("ws-04", w, cx)
    });
    assert_eq!(first_cells(cx, window, &panel, 1).0, 2);

    // Asking again adds a query tab, it does not load the rows again.
    panel_update(cx, window, &panel, |p, w, cx| p.query_text_for_test(w, cx));
    assert_eq!(
        tabs_of(cx, window, &panel).0,
        ["Sheet", "Query 1", "Query 2"]
    );
}

#[gpui_kit::test]
async fn changing_the_header_rows_loads_the_rows_again_and_keeps_the_queries(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("hosts.csv"),
        "name,port\nws-042,3389\nws-043,22\n",
    )
    .unwrap();
    let (window, panel) = open(cx, dir.path(), "hosts.csv");
    panel_update(cx, window, &panel, |p, w, cx| p.query_text_for_test(w, cx));
    panel_update(cx, window, &panel, |p, w, cx| {
        p.editor_for_test("SELECT * FROM hosts", w, cx);
        p.run_for_test(cx)
    });
    let columns = cx
        .update_window(window, |_, _, cx| panel.read(cx).columns_for_test(cx))
        .unwrap();
    assert_eq!(columns, ["name", "port"]);

    // No header row: the first row is data now, and the columns have no names of their own.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.header_rows_for_test(0, w, cx)
    });
    assert_eq!(
        tabs_of(cx, window, &panel).0,
        ["Sheet", "Query 1"],
        "the query came back with the rows"
    );
    panel_update(cx, window, &panel, |p, _, cx| p.run_for_test(cx));
    let (total, rows) = first_cells(cx, window, &panel, 1);
    assert_eq!(total, 3);
    assert_eq!(rows[0], ["name", "port"]);
    let columns = cx
        .update_window(window, |_, _, cx| panel.read(cx).columns_for_test(cx))
        .unwrap();
    assert_eq!(columns, ["Column 1", "Column 2"]);

    // Reading the file again keeps the tabs too.
    panel_update(cx, window, &panel, |p, w, cx| p.reload_for_test(w, cx));
    assert_eq!(tabs_of(cx, window, &panel).0, ["Sheet", "Query 1"]);
}

#[gpui_kit::test]
async fn a_double_click_in_the_rail_puts_the_name_in_the_query(cx: &mut TestAppContext) {
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let (window, panel) = open(cx, dir.path(), "big.sqlite");

    // With no query to put a name in, a click opens the table at once.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.rail_click_for_test("t", 1, w, cx)
    });
    assert_eq!(tabs_of(cx, window, &panel).0, ["low", "t"]);

    // With one, a single click waits for a possible second.
    panel_update(cx, window, &panel, |p, w, cx| p.query_for_test("", w, cx));
    assert_eq!(tabs_of(cx, window, &panel).0, ["low", "t", "Query 1"]);
    panel_update(cx, window, &panel, |p, w, cx| {
        p.close_tab_for_test(1, w, cx)
    });
    assert_eq!(tabs_of(cx, window, &panel).0, ["low", "Query 1"]);

    // A double click inserts the name, and no table tab is left behind.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.rail_click_for_test("t", 1, w, cx)
    });
    panel_update(cx, window, &panel, |p, w, cx| {
        p.rail_click_for_test("t", 2, w, cx)
    });
    cx.executor().advance_clock(Duration::from_millis(600));
    settle(cx, window);
    assert_eq!(tabs_of(cx, window, &panel).0, ["low", "Query 1"]);
    let text = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, _, cx| panel.read(cx).editor_text_for_test(cx))
            .unwrap()
    };
    assert_eq!(text(cx).as_deref(), Some("t"));

    // A column goes in at the cursor, set off from the word before it.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.column_click_for_test("name", 1, w, cx)
    });
    assert_eq!(
        text(cx).as_deref(),
        Some("t"),
        "one click on a column does nothing"
    );
    panel_update(cx, window, &panel, |p, w, cx| {
        p.column_click_for_test("name", 2, w, cx)
    });
    assert_eq!(text(cx).as_deref(), Some("t name"));
    // A name that needs quoting is quoted.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.column_click_for_test("first name", 2, w, cx)
    });
    assert_eq!(text(cx).as_deref(), Some("t name \"first name\""));

    // A single click on its own opens the table, after the wait.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.rail_click_for_test("t", 1, w, cx)
    });
    assert_eq!(tabs_of(cx, window, &panel).0, ["low", "Query 1"], "not yet");
    cx.executor().advance_clock(Duration::from_millis(600));
    settle(cx, window);
    assert_eq!(tabs_of(cx, window, &panel).0, ["low", "Query 1", "t"]);
}

#[gpui_kit::test]
async fn the_line_a_mistake_is_on_is_marked_in_the_editor(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let (window, panel) = open(cx, dir.path(), "big.sqlite");
    let mark = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, _, cx| panel.read(cx).error_mark_for_test(cx))
            .unwrap()
    };

    // `FORM` is the second line: bytes 9 to 15.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.query_for_test("SELECT 1\nFORM t", w, cx)
    });
    let marked = mark(cx);
    assert_eq!(marked.len(), 1);
    assert_eq!(marked[0], 9..15);

    // Running it again, fixed, takes the mark off.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.editor_for_test("SELECT 1\nFROM t LIMIT 1", w, cx);
        p.run_for_test(cx)
    });
    assert!(mark(cx).is_empty());

    // A write is refused with nothing to point at, so nothing is marked.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.editor_for_test("DELETE FROM t", w, cx);
        p.run_for_test(cx)
    });
    assert!(mark(cx).is_empty());
}

#[gpui_kit::test]
async fn explain_shows_the_plan_and_not_the_rows(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let (window, panel) = open(cx, dir.path(), "big.sqlite");
    let columns = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, _, cx| panel.read(cx).columns_for_test(cx))
            .unwrap()
    };

    panel_update(cx, window, &panel, |p, w, cx| {
        p.query_for_test("SELECT * FROM t WHERE id = 5", w, cx)
    });
    assert_eq!(columns(cx), ["id", "name", "score", "note"]);

    panel_update(cx, window, &panel, |p, _, cx| p.explain_for_test(cx));
    assert_eq!(columns(cx), ["plan"]);
    let (steps, rows) = first_cells(cx, window, &panel, 1);
    assert!(steps >= 1);
    assert!(rows[0][0].contains("SEARCH t"), "{}", rows[0][0]);
    let line = status(cx, window, &panel);
    assert!(line.starts_with("Query plan · "), "{line}");

    // A plan is not ordered by running it again: a sort changes nothing and breaks nothing.
    panel_update(cx, window, &panel, |p, _, cx| p.sort_for_test(0, false, cx));
    assert_eq!(columns(cx), ["plan"]);
    assert!(status(cx, window, &panel).starts_with("Query plan · "));

    // The rows are still one Run away.
    panel_update(cx, window, &panel, |p, _, cx| p.run_for_test(cx));
    assert_eq!(columns(cx), ["id", "name", "score", "note"]);
    assert_eq!(first_cells(cx, window, &panel, 1).1[0][0], "5");
}

#[gpui_kit::test]
async fn explain_finds_a_mistake_in_the_persons_text_not_in_its_own(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let (window, panel) = open(cx, dir.path(), "big.sqlite");
    panel_update(cx, window, &panel, |p, w, cx| p.query_for_test("", w, cx));
    panel_update(cx, window, &panel, |p, w, cx| {
        p.editor_for_test("SELECT 1\nFORM t", w, cx);
        p.explain_for_test(cx)
    });
    let line = status(cx, window, &panel);
    assert!(line.contains("line 2, column 6"), "{line}");
    let marked = cx
        .update_window(window, |_, _, cx| panel.read(cx).error_mark_for_test(cx))
        .unwrap();
    assert_eq!(marked.len(), 1);
    assert_eq!(
        marked[0],
        9..15,
        "the line in the editor, not 19 bytes along"
    );

    // A write is refused to be planned as it is to be run.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.editor_for_test("DELETE FROM t", w, cx);
        p.explain_for_test(cx)
    });
    assert!(status(cx, window, &panel).contains("only reads"));
}

#[gpui_kit::test]
async fn the_history_lists_what_was_run_newest_first_and_puts_one_back(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path());
    let (window, panel) = open(cx, dir.path(), "big.sqlite");
    let history = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, _, cx| panel.read(cx).history_for_test())
            .unwrap()
    };
    assert!(history(cx).is_empty());

    panel_update(cx, window, &panel, |p, w, cx| {
        p.query_for_test("SELECT 1", w, cx)
    });
    for sql in ["SELECT 2", "SELEC 3", "SELECT 1"] {
        panel_update(cx, window, &panel, |p, w, cx| {
            p.editor_for_test(sql, w, cx);
            p.run_for_test(cx)
        });
    }
    // A mistake is kept too, so it can be fixed; a repeat moves to the front.
    assert_eq!(history(cx), ["SELECT 1", "SELEC 3", "SELECT 2"]);

    // Sorting runs a statement again; it is not a new one.
    panel_update(cx, window, &panel, |p, _, cx| p.sort_for_test(0, false, cx));
    assert_eq!(history(cx).len(), 3);

    // Choosing one puts it in the editor, and takes any mark off it.
    panel_update(cx, window, &panel, |p, w, cx| {
        p.use_history_for_test(2, w, cx)
    });
    let text = cx
        .update_window(window, |_, _, cx| panel.read(cx).editor_text_for_test(cx))
        .unwrap();
    assert_eq!(text.as_deref(), Some("SELECT 2"));

    // Another database has its own.
    let other = tempfile::tempdir().unwrap();
    database(other.path());
    let (w2, p2) = open(cx, other.path(), "big.sqlite");
    let theirs = cx
        .update_window(w2, |_, _, cx| p2.read(cx).history_for_test())
        .unwrap();
    assert!(theirs.is_empty());
}
