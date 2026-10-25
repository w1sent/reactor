//! `agent`: hand a bounded subtask to a child agent
//! ([ADR-0062](../../../../docs/adr/0062-a-subtask-goes-to-a-child-agent-and-only-its-report-comes-back.md)).
//!
//! The child runs its own loop on a session of its own in the same case, with its own context and
//! the tools it was given. Everything it reads stays in its log, which is inspectable afterwards;
//! only its final report returns to the parent, as an ordinary tool result. What the child may use
//! is what it is *told* it has: it is not walled off from `bash`
//! ([ADR-0007](../../../../docs/adr/0007-deactivation-is-soft.md)).

use serde_json::{Value, json};

use super::{BoxFut, Tool, ToolCtx, ToolOutput, str_arg};
use crate::llm::ToolSpec;

/// Name of the tool.
pub const TOOL: &str = "agent";

/// A request to start a child.
#[derive(Debug, Clone, Default)]
pub struct ChildRequest {
    pub task: String,
    /// The tools it gets: all of the parent's but `agent` and `wait` if `None`.
    pub tools: Option<Vec<String>>,
    /// Return at once with the child's session; collect its report with `wait`.
    pub background: bool,
}

/// A child that is running.
#[derive(Debug, Clone)]
pub struct ChildInfo {
    pub session: String,
    /// What it was asked, as given.
    pub task: String,
    /// Started in the background (the parent's call has returned).
    pub background: bool,
    pub started: std::time::Instant,
}

/// What makes a child. Implemented by the agent that owns the tool, which knows the model, the
/// summarizer, the tools and the configuration a child is made from.
pub trait Children: Send + Sync {
    /// Run a child and return its report, or, in the background, where to wait for it.
    fn run<'a>(&'a self, request: ChildRequest, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput>;

    /// The children running now, oldest first.
    fn list(&self) -> Vec<ChildInfo>;

    /// Stop the child of `session`, or all of them: they end with what they have, and their
    /// parents hear that they were cancelled. How many were stopped.
    fn stop(&self, session: Option<&str>) -> usize;

    /// The report of a child started in the background: the named one, or the next to finish.
    fn wait<'a>(&'a self, session: Option<String>, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput>;
}

pub struct AgentTool;

impl Tool for AgentTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: TOOL.into(),
            description: "Hand a self-contained subtask to a fresh agent with its own context. Only its final report comes back.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "What to do and what to report. It does not see this conversation." },
                    "tools": { "type": "array", "items": { "type": "string" }, "description": "Names of the tools it gets (default: yours)." },
                    "background": { "type": "boolean", "description": "Return at once with its session; collect the report with `wait`." }
                },
                "required": ["task"]
            }),
        }
    }

    fn guidelines(&self) -> &'static [&'static str] {
        &[
            "Give `agent` bulk or output-heavy work and everything it needs in `task`; read its report, not its transcript.",
        ]
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(task) = str_arg(&args, "task").filter(|t| !t.trim().is_empty()) else {
                return ToolOutput::err("agent needs a `task`");
            };
            let Some(children) = ctx.children.as_ref() else {
                return ToolOutput::err("this session cannot start another agent");
            };
            let tools = args.get("tools").and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            });
            let background = args
                .get("background")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            children
                .run(
                    ChildRequest {
                        task: task.to_string(),
                        tools,
                        background,
                    },
                    ctx,
                )
                .await
        })
    }
}

/// Name of the waiting tool.
pub const WAIT: &str = "wait";

pub struct WaitTool;

impl Tool for WaitTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: WAIT.into(),
            description: "Wait for an agent started with `background` and return its report: the one named, or the next to finish.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "The session `agent` returned. Omit it to take the next to finish." }
                }
            }),
        }
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(children) = ctx.children.as_ref() else {
                return ToolOutput::err("this session cannot start another agent");
            };
            let session = str_arg(&args, "session")
                .map(|s| s.trim().trim_start_matches('#').to_string())
                .filter(|s| !s.is_empty());
            children.wait(session, ctx).await
        })
    }
}
