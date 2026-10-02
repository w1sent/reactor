//! The data behind the table viewer: parsing delimited text, naming columns from header
//! rows, and the filtered and sorted order of the body. No gpui in here.
//!
//! The model is a list of [`Sheet`]s so a format with several (a spreadsheet) can arrive
//! later as another parser feeding the same viewer.

use std::cmp::Ordering;

/// One grid of text cells. Rows may be ragged; a missing cell reads as empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

impl Sheet {
    pub fn width(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }

    pub fn cell(&self, row: usize, col: usize) -> &str {
        self.rows
            .get(row)
            .and_then(|r| r.get(col))
            .map_or("", String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    Comma,
    Tab,
    Semicolon,
    Pipe,
}

impl Delimiter {
    pub const ALL: [Delimiter; 4] = [
        Delimiter::Comma,
        Delimiter::Tab,
        Delimiter::Semicolon,
        Delimiter::Pipe,
    ];

    pub fn byte(self) -> u8 {
        match self {
            Delimiter::Comma => b',',
            Delimiter::Tab => b'\t',
            Delimiter::Semicolon => b';',
            Delimiter::Pipe => b'|',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Delimiter::Comma => "Comma",
            Delimiter::Tab => "Tab",
            Delimiter::Semicolon => "Semicolon",
            Delimiter::Pipe => "Pipe",
        }
    }

    /// What the file name implies, before looking inside.
    pub fn from_extension(ext: &str) -> Option<Delimiter> {
        match ext {
            "csv" => Some(Delimiter::Comma),
            "tsv" | "tab" => Some(Delimiter::Tab),
            "psv" => Some(Delimiter::Pipe),
            _ => None,
        }
    }
}

/// The delimiter whose count per line is the most consistent across the first lines — a
/// comma inside prose does not beat a tab that appears exactly three times on every row.
pub fn detect_delimiter(text: &str) -> Delimiter {
    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).take(20).collect();
    let mut best = (Delimiter::Comma, 0usize);
    for delimiter in Delimiter::ALL {
        let byte = delimiter.byte() as char;
        let counts: Vec<usize> = lines.iter().map(|l| l.matches(byte).count()).collect();
        let Some(&first) = counts.first() else {
            continue;
        };
        if first == 0 {
            continue;
        }
        let consistent = counts.iter().filter(|c| **c == first).count();
        // Consistency first, then more columns.
        let score = consistent * 1000 + first.min(999);
        if score > best.1 {
            best = (delimiter, score);
        }
    }
    best.0
}

pub fn parse(text: &str, delimiter: Delimiter) -> Result<Sheet, String> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter.byte())
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| e.to_string())?;
        rows.push(record.iter().map(str::to_owned).collect());
    }
    Ok(Sheet {
        name: "Sheet 1".into(),
        rows,
    })
}

/// A column's name: its header cells, top to bottom, joined; "Column N" when there are none.
pub fn column_name(sheet: &Sheet, header_rows: usize, col: usize) -> String {
    let parts: Vec<&str> = (0..header_rows)
        .map(|row| sheet.cell(row, col).trim())
        .filter(|cell| !cell.is_empty())
        .collect();
    if parts.is_empty() {
        format!("Column {}", col + 1)
    } else {
        parts.join(" ")
    }
}

/// Numbers sort as numbers, everything else as case-insensitive text; empty cells last.
pub fn compare_cells(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim(), b.trim());
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

/// The body rows (indices into `sheet.rows`, header rows excluded) that contain `filter`
/// in any cell, in `sort` order — `(column, ascending)` — or file order.
pub fn order(
    sheet: &Sheet,
    header_rows: usize,
    filter: &str,
    sort: Option<(usize, bool)>,
) -> Vec<usize> {
    let needle = filter.trim().to_lowercase();
    let mut rows: Vec<usize> = (header_rows.min(sheet.rows.len())..sheet.rows.len())
        .filter(|&row| {
            needle.is_empty()
                || sheet.rows[row]
                    .iter()
                    .any(|cell| cell.to_lowercase().contains(&needle))
        })
        .collect();
    if let Some((col, ascending)) = sort {
        // Stable, so equal cells keep their file order either way round; empty cells stay last.
        rows.sort_by(|&a, &b| {
            let (x, y) = (sheet.cell(a, col), sheet.cell(b, col));
            let ordering = compare_cells(x, y);
            if ascending || x.trim().is_empty() || y.trim().is_empty() {
                ordering
            } else {
                ordering.reverse()
            }
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(text: &str) -> Sheet {
        parse(text, detect_delimiter(text)).unwrap()
    }

    #[test]
    fn detects_the_consistent_delimiter() {
        assert_eq!(detect_delimiter("a,b,c\n1,2,3\n"), Delimiter::Comma);
        assert_eq!(detect_delimiter("a\tb\tc\n1\t2\t3\n"), Delimiter::Tab);
        assert_eq!(detect_delimiter("a;b;c\n1;2;3\n"), Delimiter::Semicolon);
        // commas inside the text, tabs structural
        assert_eq!(
            detect_delimiter("n\tnote\nx\thello, world\ny\tfoo, bar, baz\n"),
            Delimiter::Tab
        );
        assert_eq!(detect_delimiter("no delimiters here"), Delimiter::Comma);
    }

    #[test]
    fn quoted_fields_and_ragged_rows() {
        let s = sheet("a,b\n\"x,y\",\"q\"\"r\"\nlone\n");
        assert_eq!(s.rows[1], ["x,y", "q\"r"]);
        assert_eq!(s.cell(2, 1), "");
        assert_eq!(s.width(), 2);
    }

    #[test]
    fn column_names_from_header_rows() {
        let s = sheet("Group,Group\nName,Age\nann,3\n");
        assert_eq!(column_name(&s, 2, 1), "Group Age");
        assert_eq!(column_name(&s, 1, 0), "Group");
        assert_eq!(column_name(&s, 0, 0), "Column 1");
        assert_eq!(column_name(&s, 1, 5), "Column 6");
    }

    #[test]
    fn numbers_sort_as_numbers_and_empties_last() {
        let s = sheet("n\n10\n9\n\n100\nabc\n");
        let asc = order(&s, 1, "", Some((0, true)));
        let cells: Vec<&str> = asc.iter().map(|&r| s.cell(r, 0)).collect();
        assert_eq!(&cells[..3], ["9", "10", "100"]);
        let desc = order(&s, 1, "", Some((0, false)));
        let cells: Vec<&str> = desc.iter().map(|&r| s.cell(r, 0)).collect();
        assert_eq!(cells[0], "abc");
        assert_eq!(cells[1], "100");
    }

    #[test]
    fn filter_matches_any_cell_case_insensitively_and_skips_headers() {
        let s = sheet("name,city\nAnn,Oslo\nBob,Rome\n");
        assert_eq!(order(&s, 1, "oSl", None), [1]);
        // The header row is not a body row, however many there are.
        assert_eq!(order(&s, 1, "city", None), Vec::<usize>::new());
        assert_eq!(order(&s, 0, "city", None), [0]);
        assert_eq!(order(&s, 9, "", None), Vec::<usize>::new());
    }
}
