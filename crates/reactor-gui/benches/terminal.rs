//! What a terminal frame works out: copying the visible rows out of the engine, turning them into
//! what is drawn, and how fast the engine parses a stream. The painting itself needs a window.
//!
//! ```text
//! cargo bench -p reactor-gui --bench terminal
//! REACTOR_BENCH_SIZES=24x80,60x240 cargo bench -p reactor-gui --bench terminal -- snapshot
//! ```
//!
//! A frame at 60 a second has about 16 ms; a snapshot and a build of every row should be a small
//! fraction of that on the largest sizes.

use reactor_db::bench::Bench;
use reactor_gui::terminal::backend::Backend;
use reactor_gui::terminal::palette::Palette;
use reactor_gui::terminal::rows;
use reactor_gui::terminal::types::Settings;

/// Lines of coloured text, as `ls --color`, a build log or a diff would write them.
fn coloured(lines: usize, width: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..lines {
        let colour = 31 + (i % 7);
        let text: String = (0..width.saturating_sub(12))
            .map(|c| (b'a' + ((i + c) % 26) as u8) as char)
            .collect();
        out.extend_from_slice(format!("\x1b[{colour}mline {i:04}\x1b[0m {text}\r\n").as_bytes());
    }
    out
}

fn palette() -> Palette {
    let mut ansi = [[0u8; 3]; 16];
    for (i, c) in ansi.iter_mut().enumerate() {
        *c = [(i * 16) as u8, 128, 255 - (i * 16) as u8];
    }
    Palette {
        ansi,
        fg: [200; 3],
        bg: [10; 3],
        cursor: [255, 200, 0],
        selection: [0; 3],
        min_contrast: 3.0,
    }
}

fn main() {
    let mut bench = Bench::new("gui-terminal");
    let sizes = std::env::var("REACTOR_BENCH_SIZES")
        .ok()
        .map(|v| {
            v.split(',')
                .filter_map(|s| {
                    let (r, c) = s.trim().split_once('x')?;
                    Some((r.parse().ok()?, c.parse().ok()?))
                })
                .collect::<Vec<(usize, usize)>>()
        })
        .unwrap_or_else(|| vec![(24, 80), (40, 120), (60, 240)]);
    let palette = palette();

    for (rows_n, cols) in sizes {
        println!("-- {rows_n} rows x {cols} columns");
        let backend = Backend::new(rows_n, cols, &Settings::default());
        backend.feed(&coloured(rows_n * 3, cols));
        let cells = rows_n * cols;

        bench.run("snapshot", cells, || backend.snapshot());

        let content = backend.snapshot();
        bench.run("rows::build, every row", cells, || {
            let changed = |i: usize| content.palette_at(i);
            (0..content.rows)
                .map(|r| {
                    rows::build(content.row(r), &palette, &changed, None)
                        .runs
                        .len()
                })
                .sum::<usize>()
        });

        let stream = coloured(2_000, cols);
        let mb = stream.len();
        bench.run("parse a stream of coloured lines (bytes)", mb, || {
            backend.feed(&stream);
        });
    }
}
