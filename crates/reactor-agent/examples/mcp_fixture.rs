//! A tiny MCP server over stdio, for the tests of the MCP client (`tests/mcp.rs`).
//!
//! Newline-delimited JSON-RPC, by hand, so the test does not depend on the server half of the
//! `rmcp` SDK it is testing the client half of. Tools:
//!
//! - `echo`   — `{"text"}` back as `echo: …`
//! - `fail`   — an error result
//! - `image`  — a result with an image block
//! - `env`    — the value of the environment variable named by `{"name"}`
//! - `slow`   — never answers (to test cancelling a call)
//!
//! Say `--die` and it exits after writing to stderr, as a server that cannot start does.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn main() {
    if std::env::args().any(|a| a == "--die") {
        eprintln!("fixture: refusing to start: no such backend");
        std::process::exit(3);
    }
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (Some(id), Some(method)) = (msg.get("id").cloned(), msg["method"].as_str()) else {
            continue; // a notification
        };
        let params = &msg["params"];
        let result = match method {
            "initialize" => json!({
                "protocolVersion": params["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fixture", "version": "0"},
            }),
            "tools/list" => json!({"tools": [
                {"name": "echo", "description": "Say the text back.\nSecond line.",
                 "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
                {"name": "fail", "description": "Always fails.", "inputSchema": {"type": "object"}},
                {"name": "image", "description": "Returns a picture.", "inputSchema": {"type": "object"}},
                {"name": "env", "description": "Reads an environment variable.",
                 "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}}},
                {"name": "slow", "description": "Never answers.", "inputSchema": {"type": "object"}},
                {"name": "odd name.v2", "description": "A name a model API would refuse.", "inputSchema": {"type": "object"}},
            ]}),
            "tools/call" => {
                let args = &params["arguments"];
                match params["name"].as_str().unwrap_or("") {
                    "echo" => {
                        json!({"content": [{"type": "text", "text": format!("echo: {}", args["text"].as_str().unwrap_or(""))}]})
                    }
                    "fail" => {
                        json!({"isError": true, "content": [{"type": "text", "text": "it broke"}]})
                    }
                    "image" => json!({"content": [
                        {"type": "text", "text": "here"},
                        {"type": "image", "data": "AAAAAAAA", "mimeType": "image/png"}]}),
                    "env" => {
                        json!({"content": [{"type": "text", "text": std::env::var(args["name"].as_str().unwrap_or("")).unwrap_or_default()}]})
                    }
                    "slow" => continue,
                    _ => {
                        writeln!(out, "{}", json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "unknown tool"}})).unwrap();
                        out.flush().unwrap();
                        continue;
                    }
                }
            }
            _ => json!({}),
        };
        writeln!(
            out,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        )
        .unwrap();
        out.flush().unwrap();
    }
}
