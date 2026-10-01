#![allow(dead_code)]

use reactor_agent::entry::{Block, EntryId, Kind};
use reactor_agent::store::Store;
use serde_json::json;
use tempfile::TempDir;

pub fn store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::create(dir.path().join("s1"), "s1", dir.path()).unwrap();
    (dir, s)
}

pub fn user(s: &mut Store, text: &str) -> EntryId {
    s.append(Kind::User { text: text.into() }).unwrap()
}

pub fn assistant(s: &mut Store, text: &str) -> EntryId {
    s.append(Kind::Assistant {
        blocks: vec![Block::Text { text: text.into() }],
        model: None,
        usage: None,
        stop: None,
    })
    .unwrap()
}

/// An assistant message that calls one tool, and that tool's result.
pub fn call(
    s: &mut Store,
    id: &str,
    name: &str,
    args: serde_json::Value,
    result: &str,
) -> (EntryId, EntryId) {
    let a = s
        .append(Kind::Assistant {
            blocks: vec![Block::ToolCall {
                id: id.into(),
                name: name.into(),
                arguments: args,
            }],
            model: None,
            usage: None,
            stop: Some("tool_use".into()),
        })
        .unwrap();
    let r = s
        .append(Kind::ToolResult {
            call_id: id.into(),
            name: name.into(),
            content: result.into(),
            is_error: false,
            blob: None,
        })
        .unwrap();
    (a, r)
}

pub fn bash(s: &mut Store, id: &str, cmd: &str, result: &str) -> (EntryId, EntryId) {
    call(s, id, "bash", json!({ "command": cmd }), result)
}
