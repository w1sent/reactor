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

/// What makes a child. Implemented by the agent that owns the tool, which knows the model, the
/// summarizer, the tools and the configuration a child is made from.
pub trait Children: Send + Sync {
    /// Run `task` in a child restricted to the tools named (all of the parent's but this one if
    /// `tools` is `None`) and return its report.
    fn run<'a>(
        &'a self,
        task: String,
        tools: Option<Vec<String>>,
        ctx: &'a ToolCtx,
    ) -> BoxFut<'a, ToolOutput>;
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
                    "tools": { "type": "array", "items": { "type": "string" }, "description": "Names of the tools it gets (default: yours)." }
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
            children.run(task.to_string(), tools, ctx).await
        })
    }
}
