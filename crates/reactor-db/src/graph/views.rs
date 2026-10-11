//! Filter views: how the graph was set up to be looked at, kept by name so that it can be put
//! back in one click -- the filter layers, the size, the level of detail.
//!
//! Like [`super::layout`] this is how the graph is *drawn*, not what it holds: no versions, no
//! row in the change log, and saving under a name that is taken replaces it. A case export
//! carries views with the rest of the database.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::{Error, Result};
use crate::time::now_rfc3339;

/// One filter layer of a view: what was typed, what it does (`only`, `hide`, `dim`,
/// `highlight`) and whether it was on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewLayer {
    pub filter: String,
    pub mode: String,
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedView {
    pub name: String,
    pub layers: Vec<ViewLayer>,
    /// `s`, `m`, `l` or `xl`; none leaves the size as it is.
    pub size: Option<String>,
    /// `basic`, `analysis` or `full`; none is the level that follows the zoom.
    pub detail: Option<String>,
}

/// The longest a name may be.
pub const NAME_MAX: usize = 40;

/// Every saved view, by name.
pub fn list(db: &Db) -> Result<Vec<SavedView>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT name, layers, size, detail FROM graph_view ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let layers: String = r.get(1)?;
                Ok(SavedView {
                    name: r.get(0)?,
                    // A view that cannot be read is one with nothing in it, not an error
                    // that hides the others.
                    layers: serde_json::from_str(&layers).unwrap_or_default(),
                    size: r.get(2)?,
                    detail: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// Keep a view under its name, replacing one of that name (case aside).
pub fn save(db: &Db, view: &SavedView) -> Result<SavedView> {
    let name = view.name.trim().to_string();
    if name.is_empty() {
        return Err(Error::Invalid("a view needs a name".into()));
    }
    if name.chars().count() > NAME_MAX {
        return Err(Error::Invalid(format!(
            "a view's name is at most {NAME_MAX} characters"
        )));
    }
    for size in view.size.iter() {
        if !["s", "m", "l", "xl"].contains(&size.as_str()) {
            return Err(Error::Invalid(format!("`{size}` is not a size")));
        }
    }
    for detail in view.detail.iter() {
        if !["basic", "analysis", "full"].contains(&detail.as_str()) {
            return Err(Error::Invalid(format!(
                "`{detail}` is not a level of detail"
            )));
        }
    }
    let kept = SavedView {
        name: name.clone(),
        ..view.clone()
    };
    let layers = serde_json::to_string(&kept.layers)
        .map_err(|e| Error::Invalid(format!("the layers cannot be kept: {e}")))?;
    let (size, detail, now) = (kept.size.clone(), kept.detail.clone(), now_rfc3339());
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO graph_view(name, layers, size, detail, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(name) DO UPDATE SET name = ?1, layers = ?2, size = ?3, detail = ?4,
                    updated_at = ?5",
            params![name, layers, size, detail, now],
        )?;
        Ok(())
    })?;
    Ok(kept)
}

/// Forget a view. One that is not there is not an error.
pub fn delete(db: &Db, name: &str) -> Result<()> {
    let name = name.trim().to_string();
    db.write(move |tx| {
        tx.execute("DELETE FROM graph_view WHERE name = ?1", [name])?;
        Ok(())
    })
}
