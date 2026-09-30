//! Deterministic text. The registry block is injected into the system prompt,
//! so it is rendered here — in one language, under test — rather than by any
//! other component: same machine state → byte-identical string. Nothing time-derived
//! may enter [`render_registry`]'s output (ADR-0006).

use crate::catalogue::DetectKind;
use crate::model::ToolEntry;
use crate::probe::{ServiceState, Status};
use crate::util::ljust;

pub const REGISTRY_HEADING: &str = "## Available RE tools (this machine)";
pub const REGISTRY_FOOTER: &str = "\
`<tool> --help` or `man <tool>` is the documentation -- read it rather than guessing flags.
`reactor tools show <id>` for detail, `reactor doctor` for what else exists and how to install it.
Anything not listed here is either absent or deactivated; prefer these over writing a parser by hand.";

pub fn glyph(status: Status) -> &'static str {
    match status {
        Status::Present => "[+]",
        Status::Absent => "[-]",
        Status::Unknown => "[?]",
    }
}

pub fn service_glyph(state: ServiceState) -> &'static str {
    match state {
        ServiceState::Up => "[^]",
        ServiceState::Down => "[v]",
        ServiceState::Unknown => "[?]",
    }
}

/// The block injected into the system prompt.
pub fn render_registry(entries: &[ToolEntry]) -> String {
    let listed: Vec<&ToolEntry> = entries.iter().filter(|e| e.active && e.status == Status::Present).collect();
    if listed.is_empty() {
        return format!(
            "{REGISTRY_HEADING}\n\n\
             None. No catalogued RE tool was detected on this machine -- run \
             `reactor doctor` to see what is missing and how to install it."
        );
    }

    let rows: Vec<(String, &str, String)> = listed.iter().map(|e| (label(e), e.desc.as_str(), annotation(e))).collect();
    let w_label = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(0);
    let w_desc = rows.iter().filter(|r| !r.2.is_empty()).map(|r| r.1.chars().count()).max().unwrap_or(0);

    let mut lines = Vec::new();
    for (label, desc, ann) in &rows {
        let mut line = format!("{}  {desc}", ljust(label, w_label));
        if !ann.is_empty() {
            line = format!("{}  {ann}", ljust(&line, w_label + 2 + w_desc));
        }
        lines.push(line.trim_end().to_string());
    }
    format!("{REGISTRY_HEADING}\n\n{}\n\n{REGISTRY_FOOTER}", lines.join("\n"))
}

/// The first column: what you type. A multi-word `invoke` is a usage example,
/// not a command name, so python libraries show their module.
fn label(e: &ToolEntry) -> String {
    if e.invoke.split_whitespace().count() == 1 {
        e.invoke.clone()
    } else {
        e.detect.value.clone()
    }
}

fn annotation(e: &ToolEntry) -> String {
    if let Some(svc) = &e.service {
        let label = svc.label.as_deref().unwrap_or("service");
        return match svc.state {
            ServiceState::Up => match svc.detail.as_deref().filter(|d| !d.is_empty()) {
                Some(d) => format!("[{label}: {d}]"),
                None => format!("[{label}: up]"),
            },
            ServiceState::Down => format!("[{label}: down]"),
            ServiceState::Unknown => format!("[{label}: unknown]"),
        };
    }
    if let Some(v) = e.version.as_deref().filter(|v| !v.is_empty()) {
        return v.to_string();
    }
    if e.detect.kind == DetectKind::PythonModule {
        return "(python module)".into();
    }
    String::new()
}

/// Left-aligned columns, two spaces apart, trailing space trimmed.
pub fn table(rows: &[Vec<String>]) -> String {
    let Some(first) = rows.first() else { return String::new() };
    let widths: Vec<usize> = (0..first.len())
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    rows.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(i, cell)| ljust(cell, widths[i]))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_tool_list(entries: &[ToolEntry]) -> String {
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            vec![
                glyph(e.status).to_string(),
                e.id.clone(),
                e.version.clone().unwrap_or_default(),
                if e.active { String::new() } else { "(off)".into() },
                e.desc.clone(),
            ]
        })
        .collect();
    table(&rows)
}

pub fn render_tool_show(e: &ToolEntry) -> String {
    let title = if e.name == e.id { e.id.clone() } else { format!("{} -- {}", e.id, e.name) };
    let mut out = vec![title, String::new(), format!("  {}", e.desc), String::new()];
    out.push(format!(
        "  status    {}{}",
        e.status.as_str(),
        e.path.as_ref().map(|p| format!(" ({p})")).unwrap_or_default()
    ));
    out.push(format!("  invoke    {}", e.invoke));
    if let Some(v) = e.version.as_deref().filter(|v| !v.is_empty()) {
        out.push(format!("  version   {v}"));
    }
    out.push(format!("  active    {}", if e.active { "yes" } else { "no" }));
    out.push(format!("  tags      {}", if e.tags.is_empty() { "-".to_string() } else { e.tags.join(", ") }));
    out.push(format!("  detect    {} = {}", e.detect.kind.as_str(), e.detect.value));
    if let Some(src) = &e.source {
        out.push(format!("  source    {src}"));
    }
    if let Some(svc) = &e.service {
        let detail = svc.detail.as_ref().map(|d| format!(" ({d})")).unwrap_or_default();
        out.push(format!(
            "  service   {}: {}{detail}",
            svc.label.as_deref().unwrap_or("service"),
            svc.state.as_str()
        ));
    }
    if let Some(sk) = &e.skill {
        let where_ = if sk.fetched { "fetched" } else { "NOT FETCHED" };
        out.push(format!(
            "  skill     {where_} -- {} @ {}",
            sk.source.as_deref().unwrap_or("None"),
            sk.git_ref.as_deref().unwrap_or("HEAD")
        ));
    }
    if let Some(inst) = &e.install {
        out.push(String::new());
        if let Some(rec) = &inst.recommended {
            out.push(format!("  install   {}", rec.text()));
            for c in inst.candidates.iter().skip(1) {
                out.push(format!("            {}", c.text()));
            }
        }
        for (key, text) in inst.notes.iter() {
            out.push(format!("  {} {text}", ljust(key, 9)));
        }
    }
    out.join("\n")
}
