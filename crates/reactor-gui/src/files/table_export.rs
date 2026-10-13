//! A query result as text another program reads: CSV, TSV, JSON or a Markdown table. No gpui
//! in here ([ADR-0050]).
//!
//! A cell keeps its type where the format has one: a NULL is `null` in JSON and an empty
//! field in CSV, a BLOB is its bytes in hex (all of them, not the preview the grid shows).
//!
//! [ADR-0050]: ../../../../docs/adr/0050-the-table-viewer-is-a-grid-over-sources.md

use reactor_db::browse::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Tsv,
    Json,
    Markdown,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Csv, Format::Tsv, Format::Json, Format::Markdown];

    pub fn label(self) -> &'static str {
        match self {
            Format::Csv => "CSV",
            Format::Tsv => "TSV",
            Format::Json => "JSON",
            Format::Markdown => "Markdown table",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Tsv => "tsv",
            Format::Json => "json",
            Format::Markdown => "md",
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A cell as plain text, for the delimited and Markdown forms.
fn plain(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => f.to_string(),
        Value::Text(t) => t.clone(),
        Value::Blob(b) => hex(b),
    }
}

/// Names a JSON object can hold: a second `id` becomes `id:1`, as SQLite names it in a
/// subquery, so no column is lost.
fn unique(columns: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(columns.len());
    for name in columns {
        let mut candidate = name.clone();
        let mut n = 0;
        while out.contains(&candidate) {
            n += 1;
            candidate = format!("{name}:{n}");
        }
        out.push(candidate);
    }
    out
}

fn json_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Integer(i) => i.to_string(),
        // JSON has no NaN or infinity.
        Value::Real(f) if f.is_finite() => {
            serde_json::Number::from_f64(*f).map_or_else(|| "null".to_string(), |n| n.to_string())
        }
        Value::Real(f) => serde_json::to_string(&f.to_string()).unwrap_or_default(),
        Value::Text(t) => serde_json::to_string(t).unwrap_or_default(),
        Value::Blob(b) => serde_json::to_string(&hex(b)).unwrap_or_default(),
    }
}

fn delimited(columns: &[String], rows: &[Vec<Value>], delimiter: u8) -> String {
    let mut out = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .from_writer(Vec::new());
    let _ = out.write_record(columns);
    for row in rows {
        let _ = out.write_record(row.iter().map(plain));
    }
    String::from_utf8(out.into_inner().unwrap_or_default()).unwrap_or_default()
}

fn json(columns: &[String], rows: &[Vec<Value>]) -> String {
    let keys: Vec<String> = unique(columns)
        .iter()
        .map(|k| serde_json::to_string(k).unwrap_or_default())
        .collect();
    let objects: Vec<String> = rows
        .iter()
        .map(|row| {
            let fields: Vec<String> = keys
                .iter()
                .zip(row)
                .map(|(key, value)| format!("{key}: {}", json_value(value)))
                .collect();
            format!("  {{{}}}", fields.join(", "))
        })
        .collect();
    if objects.is_empty() {
        "[]\n".to_string()
    } else {
        format!("[\n{}\n]\n", objects.join(",\n"))
    }
}

fn markdown(columns: &[String], rows: &[Vec<Value>]) -> String {
    let cell = |text: String| text.replace('|', "\\|").replace(['\r', '\n'], " ");
    let line = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));
    let mut out = line(columns.iter().map(|c| cell(c.clone())).collect());
    out.push_str(&line(columns.iter().map(|_| "---".to_string()).collect()));
    for row in rows {
        out.push_str(&line(
            row.iter()
                .map(|v| match v {
                    // Markdown has no null, and an empty cell would pass for an empty string.
                    Value::Null => "NULL".to_string(),
                    other => cell(plain(other)),
                })
                .collect(),
        ));
    }
    out
}

/// The whole result in `format`.
pub fn render(format: Format, columns: &[String], rows: &[Vec<Value>]) -> String {
    match format {
        Format::Csv => delimited(columns, rows, b','),
        Format::Tsv => delimited(columns, rows, b'\t'),
        Format::Json => json(columns, rows),
        Format::Markdown => markdown(columns, rows),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn sample() -> (Vec<String>, Vec<Vec<Value>>) {
        (
            cols(&["id", "name", "ratio", "raw"]),
            vec![
                vec![
                    Value::Integer(1),
                    Value::Text("a, \"quoted\" b".into()),
                    Value::Real(0.5),
                    Value::Blob(vec![0xde, 0xad]),
                ],
                vec![
                    Value::Integer(2),
                    Value::Null,
                    Value::Real(1.0),
                    Value::Blob(vec![]),
                ],
            ],
        )
    }

    #[test]
    fn csv_quotes_what_needs_it_and_leaves_a_null_empty() {
        let (c, r) = sample();
        assert_eq!(
            render(Format::Csv, &c, &r),
            "id,name,ratio,raw\n1,\"a, \"\"quoted\"\" b\",0.5,dead\n2,,1,\n"
        );
    }

    #[test]
    fn tsv_uses_tabs() {
        let (c, r) = sample();
        let out = render(Format::Tsv, &c, &r);
        assert!(out.starts_with("id\tname\tratio\traw\n1\t"), "{out}");
    }

    #[test]
    fn json_keeps_types_and_every_column() {
        let (c, r) = sample();
        assert_eq!(
            render(Format::Json, &c, &r),
            "[\n  {\"id\": 1, \"name\": \"a, \\\"quoted\\\" b\", \"ratio\": 0.5, \"raw\": \"dead\"},\n  {\"id\": 2, \"name\": null, \"ratio\": 1.0, \"raw\": \"\"}\n]\n"
        );
        // Two columns of one name are both there.
        let dup = render(
            Format::Json,
            &cols(&["id", "id"]),
            &[vec![Value::Integer(1), Value::Integer(2)]],
        );
        assert!(dup.contains("\"id\": 1, \"id:1\": 2"), "{dup}");
        assert_eq!(render(Format::Json, &cols(&["a"]), &[]), "[]\n");
        // JSON has no NaN.
        let nan = render(Format::Json, &cols(&["x"]), &[vec![Value::Real(f64::NAN)]]);
        assert!(nan.contains("\"NaN\""), "{nan}");
    }

    #[test]
    fn markdown_escapes_pipes_and_says_null() {
        let out = render(
            Format::Markdown,
            &cols(&["a|b", "c"]),
            &[vec![Value::Text("x|y\nz".into()), Value::Null]],
        );
        assert_eq!(out, "| a\\|b | c |\n| --- | --- |\n| x\\|y z | NULL |\n");
    }
}
