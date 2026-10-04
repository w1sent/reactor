//! Elements and edges as CSV.

use crate::db::Db;
use crate::error::{Error, Result};
use crate::graph::query;

fn csv_of(rows: Vec<Vec<String>>) -> Result<String> {
    let mut w = csv::Writer::from_writer(Vec::new());
    for row in rows {
        w.write_record(&row).map_err(|e| Error::Io(e.to_string()))?;
    }
    let bytes = w.into_inner().map_err(|e| Error::Io(e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| Error::Io(e.to_string()))
}

/// Every element as it stands, one row each.
pub fn elements(db: &Db, include_deleted: bool) -> Result<String> {
    let (elements, _) = query::snapshot(db, include_deleted)?;
    let mut rows = vec![
        [
            "id",
            "kind",
            "type",
            "value",
            "status",
            "confidence",
            "tags",
            "occurred_at",
            "valid_from",
            "valid_to",
            "time_as_written",
            "zone",
            "version",
            "recorded_at",
            "author",
            "props",
        ]
        .map(String::from)
        .to_vec(),
    ];
    for e in &elements {
        let c = &e.content;
        rows.push(vec![
            e.id.clone(),
            e.kind.as_str().to_string(),
            e.type_.clone(),
            c.value.clone().unwrap_or_default(),
            c.status.clone(),
            c.confidence.clone().unwrap_or_default(),
            c.tags.join(";"),
            c.time.occurred_at.clone().unwrap_or_default(),
            c.time.valid_from.clone().unwrap_or_default(),
            c.time.valid_to.clone().unwrap_or_default(),
            c.time.raw.clone().unwrap_or_default(),
            c.time.tz_certainty.clone().unwrap_or_default(),
            e.ver.to_string(),
            e.recorded_at.clone(),
            e.author.clone(),
            if c.props.is_empty() {
                String::new()
            } else {
                serde_json::Value::Object(c.props.clone()).to_string()
            },
        ]);
    }
    csv_of(rows)
}

/// Every edge as it stands, with what its two ends are.
pub fn edges(db: &Db, include_deleted: bool) -> Result<String> {
    let (elements, edges) = query::snapshot(db, include_deleted)?;
    let by_id: std::collections::HashMap<&str, &query::Element> =
        elements.iter().map(|e| (e.id.as_str(), e)).collect();
    let label = |id: &str| -> (String, String) {
        by_id.get(id).map_or((String::new(), String::new()), |e| {
            (e.type_.clone(), e.content.value.clone().unwrap_or_default())
        })
    };
    let mut rows = vec![
        [
            "id",
            "from_id",
            "from_type",
            "from_value",
            "role",
            "to_id",
            "to_type",
            "to_value",
            "pinned_version",
            "locator",
            "fingerprint",
        ]
        .map(String::from)
        .to_vec(),
    ];
    for g in &edges {
        let (ft, fv) = label(&g.from_id);
        let (tt, tv) = label(&g.to_id);
        rows.push(vec![
            g.id.clone(),
            g.from_id.clone(),
            ft,
            fv,
            g.role.clone(),
            g.to_id.clone(),
            tt,
            tv,
            g.pinned_ver.map(|v| v.to_string()).unwrap_or_default(),
            g.locator.clone().unwrap_or_default(),
            g.fingerprint.clone().unwrap_or_default(),
        ]);
    }
    csv_of(rows)
}
