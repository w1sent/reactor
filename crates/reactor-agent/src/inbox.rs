//! Input that is waiting for the model, kept in the session log
//! ([ADR-0061](../../../docs/adr/0061-typed-input-is-a-fact-in-the-log-before-it-is-sent.md)).
//!
//! A person types while the agent works. That text is admitted as an entry, so a crash does not
//! lose it and every frontend reads the same truth; it is delivered later as an ordinary user
//! message, and a second entry says so. Cancelling it is a third entry, never a deletion.
//!
//! Two classes say *when*:
//! - **steer**: at the next step boundary of the running turn, after the tool call in flight has
//!   settled and before the next request. The turn is not interrupted and no work is lost.
//! - **queue**: when the turn is over, as a prompt of its own.
//!
//! Both are [`Kind::Custom`] entries (`inbox`, `inbox_end`), so the projection skips them, an
//! older build reads the log unchanged, and a reduction treats them like any other state entry.
//! The item id is the caller's; admitting one id twice on a branch keeps the first.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::entry::{EntryId, Kind};
use crate::error::Result;
use crate::store::Store;

pub const KEY_INBOX: &str = "inbox";
pub const KEY_INBOX_END: &str = "inbox_end";

/// When pending input reaches the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// At the next step boundary of the turn that is running.
    Steer,
    /// When the turn is over.
    Queue,
}

/// How an item left the inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// It was sent; the entry is the user message it became.
    Delivered(EntryId),
    /// The person took it back.
    Cancelled,
}

/// Input admitted and neither delivered nor cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub item: String,
    pub class: Class,
    pub text: String,
}

fn admitted(kind: &Kind) -> Option<Pending> {
    let Kind::Custom { key, data } = kind else {
        return None;
    };
    if key != KEY_INBOX {
        return None;
    }
    Some(Pending {
        item: data.get("item")?.as_str()?.to_string(),
        class: serde_json::from_value(data.get("class")?.clone()).ok()?,
        text: data.get("text")?.as_str()?.to_string(),
    })
}

fn ended(kind: &Kind) -> Option<&str> {
    let Kind::Custom { key, data } = kind else {
        return None;
    };
    (key == KEY_INBOX_END)
        .then(|| data.get("item").and_then(Value::as_str))
        .flatten()
}

/// What is waiting on this branch, oldest first.
pub fn pending(store: &Store) -> Vec<Pending> {
    let mut out: Vec<Pending> = Vec::new();
    for e in store.branch() {
        if let Some(p) = admitted(&e.kind) {
            if !out.iter().any(|q| q.item == p.item) {
                out.push(p);
            }
        } else if let Some(item) = ended(&e.kind) {
            out.retain(|p| p.item != item);
        }
    }
    out
}

/// Whether the branch has admitted `item` at any time, delivered or not.
fn seen(store: &Store, item: &str) -> bool {
    store
        .branch()
        .iter()
        .any(|e| admitted(&e.kind).is_some_and(|p| p.item == item))
}

/// Admit input. `false` when `item` was admitted on this branch already: the first admission
/// wins, so a repeated submit is not a second message.
pub fn admit(store: &mut Store, item: &str, class: Class, text: &str) -> Result<bool> {
    if seen(store, item) {
        return Ok(false);
    }
    store.append(Kind::Custom {
        key: KEY_INBOX.into(),
        data: json!({ "item": item, "class": class, "text": text }),
    })?;
    Ok(true)
}

/// Record how `item` left.
pub fn finish(store: &mut Store, item: &str, how: End) -> Result<EntryId> {
    let data = match how {
        End::Delivered(entry) => json!({ "item": item, "how": "delivered", "entry": entry }),
        End::Cancelled => json!({ "item": item, "how": "cancelled" }),
    };
    store.append(Kind::Custom {
        key: KEY_INBOX_END.into(),
        data,
    })
}
