//! `tool_search`: load the tools of a deferred group
//! ([ADR-0055](../../../../docs/adr/0055-large-tool-groups-are-loaded-on-demand.md)).
//!
//! The definition the model sees is made by [`crate::deferral::spec`], which lists what can
//! be loaded; this is the part that runs. A load is a state entry like any other, so it
//! follows the branch, survives a resume, and is not undone by a context reduction.

use serde_json::{Value, json};

use super::{BoxFut, Tool, ToolCtx, ToolOutput, str_arg, usize_arg};
use crate::context::{KEY_TOOLS, SessionState, save_state};
use crate::deferral::{self, TOOL};
use crate::llm::ToolSpec;

const DEFAULT_RESULTS: usize = 5;

pub struct ToolSearch;

impl Tool for ToolSearch {
    fn spec(&self) -> ToolSpec {
        deferral::spec(&[])
    }

    /// Advertised by the plan, which knows what there is to load.
    fn available(&self, _state: &SessionState) -> bool {
        false
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(query) = str_arg(&args, "query").filter(|q| !q.trim().is_empty()) else {
                return ToolOutput::err(format!("{TOOL} needs a `query`"));
            };
            if ctx.deferred.is_empty() {
                return ToolOutput::ok("No tools are waiting to be loaded.");
            }
            let max = usize_arg(&args, "max_results")
                .unwrap_or(DEFAULT_RESULTS)
                .max(1);
            let found = deferral::find(&ctx.deferred, query, max);
            if found.is_empty() {
                let names: Vec<&str> = ctx.deferred.iter().map(|t| t.name.as_str()).collect();
                return ToolOutput::err(format!(
                    "No tool matches `{}`. Not loaded: {}.",
                    query.trim(),
                    names.join(", ")
                ));
            }
            let mut store = ctx.store.lock().unwrap();
            let mut loaded = SessionState::load(&store).loaded;
            let (mut new, mut had) = (Vec::new(), Vec::new());
            for t in found {
                if loaded.insert(t.name.clone()) {
                    new.push(t.name.as_str());
                } else {
                    had.push(t.name.as_str());
                }
            }
            if !new.is_empty()
                && let Err(e) = save_state(&mut store, KEY_TOOLS, json!({ "loaded": loaded }))
            {
                return ToolOutput::err(format!("could not record the load: {e}"));
            }
            let mut out = Vec::new();
            if !new.is_empty() {
                out.push(format!(
                    "Loaded: {}. In your tool list from your next step.",
                    new.join(", ")
                ));
            }
            if !had.is_empty() {
                out.push(format!("Already loaded: {}.", had.join(", ")));
            }
            ToolOutput::ok(out.join("\n"))
        })
    }
}
