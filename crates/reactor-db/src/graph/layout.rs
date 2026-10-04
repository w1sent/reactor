//! Where the drawn graph's nodes are, kept with the case so that the picture a person has
//! arranged is there when they come back.
//!
//! This is how the graph is *drawn*, not what it holds: it has no versions and no row in the
//! change log, a position is overwritten by the next, and an element the graph no longer
//! shows keeps its row (it comes back where it was if it returns). A case export carries it
//! with the rest of the database; a STIX bundle does not.

use std::collections::HashMap;

use rusqlite::params;

use crate::db::Db;
use crate::error::Result;
use crate::time::now_rfc3339;

/// Every stored position, by element id.
pub fn load(db: &Db) -> Result<HashMap<String, (f32, f32)>> {
    db.read(|conn| {
        let mut stmt = conn.prepare("SELECT element_id, x, y FROM graph_layout")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (r.get::<_, f64>(1)? as f32, r.get::<_, f64>(2)? as f32),
                ))
            })?
            .collect::<std::result::Result<HashMap<_, _>, _>>()?;
        Ok(rows)
    })
}

/// Store these positions (replacing earlier ones), together or not at all. A position that
/// is not a finite number is not stored.
pub fn save(db: &Db, positions: &[(String, (f32, f32))]) -> Result<()> {
    let now = now_rfc3339();
    let rows: Vec<(String, f64, f64)> = positions
        .iter()
        .filter(|(_, (x, y))| x.is_finite() && y.is_finite())
        .map(|(id, (x, y))| (id.clone(), f64::from(*x), f64::from(*y)))
        .collect();
    if rows.is_empty() {
        return Ok(());
    }
    db.write(move |tx| {
        let mut stmt = tx.prepare(
            "INSERT INTO graph_layout(element_id, x, y, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(element_id) DO UPDATE SET x = ?2, y = ?3, updated_at = ?4",
        )?;
        for (id, x, y) in &rows {
            stmt.execute(params![id, x, y, now])?;
        }
        Ok(())
    })
}

/// Forget every stored position: the next layout is made afresh.
pub fn clear(db: &Db) -> Result<()> {
    db.write(|tx| {
        tx.execute("DELETE FROM graph_layout", [])?;
        Ok(())
    })
}
