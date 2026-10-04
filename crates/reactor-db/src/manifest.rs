//! The case's goal and guidelines
//! ([ADR-0045](../../../docs/adr/0045-a-case-is-the-working-directory.md)): the strategy
//! of an investigation, the same in every session of it. Each change is a new version
//! with its author and time, so the history of the strategy is part of the case.

use rusqlite::{OptionalExtension, Transaction, params};

use crate::db::Db;
use crate::error::Result;
use crate::time::now_rfc3339;

/// Who or what wrote something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    /// `analyst`, `tool` or `agent`.
    pub kind: String,
    pub identity: String,
    /// A tool's version, an agent's model. Empty when there is none.
    pub version: String,
}

impl Author {
    pub fn analyst() -> Author {
        Author {
            kind: "analyst".into(),
            identity: "user".into(),
            version: String::new(),
        }
    }

    pub fn agent(model: &str) -> Author {
        Author {
            kind: "agent".into(),
            identity: "reactor".into(),
            version: model.to_string(),
        }
    }
}

/// The registry row for `author`, made on first use.
pub fn author_id(tx: &Transaction, author: &Author) -> Result<i64> {
    tx.execute(
        "INSERT OR IGNORE INTO author(kind, identity, version) VALUES (?1, ?2, ?3)",
        params![author.kind, author.identity, author.version],
    )?;
    Ok(tx.query_row(
        "SELECT id FROM author WHERE kind = ?1 AND identity = ?2 AND version = ?3",
        params![author.kind, author.identity, author.version],
        |r| r.get(0),
    )?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Goal,
    Guidelines,
}

impl Field {
    fn name(self) -> &'static str {
        match self {
            Field::Goal => "goal",
            Field::Guidelines => "guidelines",
        }
    }
}

/// The case's goal and guidelines as they stand. Empty text reads as unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaseManifest {
    pub goal: Option<String>,
    pub guidelines: Option<String>,
}

pub fn current(db: &Db) -> Result<CaseManifest> {
    db.read(|conn| {
        let get = |field: Field| -> Result<Option<String>> {
            Ok(conn
                .query_row(
                    "SELECT value FROM case_manifest WHERE field = ?1 ORDER BY ver DESC LIMIT 1",
                    [field.name()],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .filter(|v| !v.is_empty()))
        };
        Ok(CaseManifest {
            goal: get(Field::Goal)?,
            guidelines: get(Field::Guidelines)?,
        })
    })
}

/// Record a new value. `None` clears the field. Setting what is already there is not a
/// change and writes nothing; the answer says whether a version was written.
pub fn set(
    db: &Db,
    field: Field,
    value: Option<&str>,
    author: &Author,
    session: Option<&str>,
) -> Result<bool> {
    let (value, author) = (value.unwrap_or("").to_string(), author.clone());
    let session = session.map(str::to_string);
    db.write(move |tx| {
        let last: Option<(i64, String)> = tx
            .query_row(
                "SELECT ver, value FROM case_manifest WHERE field = ?1 ORDER BY ver DESC LIMIT 1",
                [field.name()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (ver, unchanged) = match &last {
            Some((ver, old)) => (ver + 1, *old == value),
            None => (1, value.is_empty()),
        };
        if unchanged {
            return Ok(false);
        }
        let author_id = author_id(tx, &author)?;
        let now = now_rfc3339();
        tx.execute(
            "INSERT INTO event(id, ts, session_id, author_id, type, payload)
             VALUES (?1, ?2, ?3, ?4, 'manifest', json_object('field', ?5))",
            params![
                uuid::Uuid::now_v7().to_string(),
                now,
                session,
                author_id,
                field.name()
            ],
        )?;
        let event = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO case_manifest(field, ver, value, recorded_at, author_id, event_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![field.name(), ver, value, now, author_id, event],
        )?;
        Ok(true)
    })
}
