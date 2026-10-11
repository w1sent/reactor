//! The graph tools, called the way the loop calls them (ADR-0047).

use std::sync::{Arc, Mutex};

use reactor_agent::context::{KEY_MEMORY, save_state};
use reactor_agent::store::Store;
use reactor_agent::tools::{ToolCtx, ToolOutput, Tools};
use reactor_core::Paths;
use reactor_core::paths::Shipped;
use reactor_db::Case;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

struct Rig {
    dir: tempfile::TempDir,
    ctx: ToolCtx,
    tools: Tools,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(work.join("samples")).unwrap();
    std::fs::write(work.join("samples/dropper.bin"), b"MZ payload").unwrap();
    let case = Case::create(&work, None).unwrap();
    let store = Store::create(&case, "s1").unwrap();
    let ctx = ToolCtx {
        cwd: work.clone(),
        cancel: CancellationToken::new(),
        emit: Arc::new(|_: &str| {}),
        store: Arc::new(Mutex::new(store)),
        paths: Paths::new(dir.path().join("cfg"), Shipped::Embedded),
        scenarios_dir: dir.path().join("scenarios"),
        model: "test/model".into(),
        embedder: None,
    };
    Rig {
        dir,
        ctx,
        tools: Tools::standard(work),
    }
}

impl Rig {
    async fn call(&self, tool: &str, args: Value) -> ToolOutput {
        self.tools.find(tool).unwrap().call(args, &self.ctx).await
    }

    async fn ok(&self, tool: &str, args: Value) -> String {
        let out = self.call(tool, args).await;
        assert!(!out.is_error, "{tool}: {}", out.text);
        out.text
    }

    fn id_in(text: &str, prefix: &str) -> String {
        text.split_whitespace()
            .find(|w| w.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix} id in: {text}"))
            .to_string()
    }
}

#[tokio::test]
async fn a_finding_goes_in_with_its_source_gets_judged_and_can_be_read_back() {
    let r = rig();
    let file = r
        .ok(
            "artifact_register",
            json!({ "path": "samples/dropper.bin", "kind": "sample" }),
        )
        .await;
    assert!(
        file.starts_with("samples/dropper.bin sample 10B\nsha256 "),
        "{file}"
    );

    let added = r
        .ok(
            "graph_add",
            json!({ "elements": [{
                "kind": "observation",
                "type": "malware.c2_config",
                "value": "config decrypted with RC4",
                "links": [
                    { "role": "subject", "to": { "type": "file.name", "value": "dropper.bin" } },
                    { "role": "object", "to": { "type": "domain", "value": "C2.Example.Net." } }
                ],
                "source": { "uri": "samples/dropper.bin", "locator": "addr=0x401000" }
            }]}),
        )
        .await;
    assert!(
        added.contains("malware.c2_config") && added.contains("change #"),
        "{added}"
    );
    let claim = Rig::id_in(&added, "observation--");

    // The same thing again is not a second claim -- but a claim typed by hand has no
    // row to be recognised by, so it is: the model is told to look first.
    let rel = r.ok("graph_query", json!({ "what": "relations" })).await;
    assert_eq!(
        rel,
        format!(
            "file.name:dropper.bin --malware.c2_config--> net.domain:c2.example.net  ({claim})"
        )
    );

    let hyp = r
        .ok(
            "graph_annotate",
            json!({ "type": "hypothesis", "text": "c2.example.net is the operator's C2" }),
        )
        .await;
    let hypothesis = Rig::id_in(&hyp, "annotation--");
    let judged = r
        .ok(
            "graph_annotate",
            json!({
                "type": "judgement",
                "text": "the config is live: the domain resolves",
                "confidence": "likely",
                "about": [claim],
                "supports": [hypothesis]
            }),
        )
        .await;
    assert!(
        judged.contains("-about->") && judged.contains("judged its version 1"),
        "{judged}"
    );
    assert!(judged.contains("by agent:reactor@test/model"), "{judged}");

    // An entity by what it is: where else does it show up?
    let domain = r
        .ok(
            "graph_query",
            json!({ "what": "element", "id": { "type": "net.domain", "value": "c2.example.net" } }),
        )
        .await;
    assert!(domain.contains("<-object- malware.c2_config"), "{domain}");
    let support = r
        .ok(
            "graph_query",
            json!({ "what": "element", "id": hypothesis }),
        )
        .await;
    assert!(
        support.contains("<-supports- analysis.judgement"),
        "{support}"
    );
    assert!(
        support.contains("props {\"hypothesis_status\":\"open\"}"),
        "{support}"
    );

    // Revising the claim flags the judgement that was about its earlier version.
    let changed = r
        .ok(
            "graph_annotate",
            json!({ "change": { "id": claim, "status": "conflicting", "reason": "the sandbox run shows another domain" } }),
        )
        .await;
    assert!(
        changed.contains("[conflicting]") && changed.contains("now at version 2"),
        "{changed}"
    );
    let stale = r.ok("graph_query", json!({ "what": "stale" })).await;
    assert!(
        stale.contains("judged version 1") && stale.contains("now conflicting"),
        "{stale}"
    );

    let found = r
        .ok(
            "graph_query",
            json!({ "what": "find", "type": "analysis." }),
        )
        .await;
    assert_eq!(
        found.lines().count(),
        3,
        "hypothesis, judgement, and the note with the reason"
    );
    let files = r.ok("graph_query", json!({ "what": "files" })).await;
    assert!(
        files.starts_with("samples/dropper.bin sample sha256:"),
        "{files}"
    );
}

#[tokio::test]
async fn a_conclusion_is_not_accepted_as_an_observation() {
    let r = rig();
    let out = r
        .call(
            "graph_add",
            json!({ "elements": [{
                "kind": "observation", "type": "net.resolution",
                "links": [{ "role": "query", "to": { "type": "net.domain", "value": "x.example" } }]
            }]}),
        )
        .await;
    assert!(
        out.is_error && out.text.contains("needs a source"),
        "{}",
        out.text
    );
    assert!(out.text.contains("annotation"), "{}", out.text);
    let out = r
        .call(
            "graph_add",
            json!({ "elements": [{ "kind": "annotation", "type": "analysis.note" }] }),
        )
        .await;
    assert!(
        out.is_error && out.text.contains("graph_annotate"),
        "{}",
        out.text
    );
    assert_eq!(
        r.ok("graph_query", json!({ "what": "find" })).await,
        "nothing recorded matches"
    );
    // Asking about something that was never recorded says so, without claiming absence.
    let none = r
        .ok(
            "graph_query",
            json!({ "what": "element", "id": { "type": "net.ipv4", "value": "203.0.113.9" } }),
        )
        .await;
    assert!(
        none.contains("not in the graph") && none.contains("not the same as not existing"),
        "{none}"
    );
}

#[tokio::test]
async fn rows_a_script_wrote_are_pulled_and_a_wrong_pull_is_taken_back() {
    let r = rig();
    let work = r.dir.path().join("work");
    std::fs::write(
        work.join("conns.csv"),
        "ts,src,dst\n2026-10-01T03:12:07Z,ws-042,198.51.100.7\n2026-10-01T03:13:00Z,ws-042,198.51.100.7\n",
    )
    .unwrap();
    let query = r#"SELECT 'net.connection' AS "@type", ts AS "@time", _row AS "@row",
                          src AS "host.name@src", dst AS "net.ipv4@dst" FROM src"#;

    let preview = r
        .ok(
            "graph_pull",
            json!({ "source": "conns.csv", "query": query, "preview": true }),
        )
        .await;
    assert!(
        preview.contains("2 row(s)") && preview.contains("would become"),
        "{preview}"
    );
    assert_eq!(
        r.ok("graph_query", json!({ "what": "find" })).await,
        "nothing recorded matches"
    );

    let pulled = r
        .ok(
            "graph_pull",
            json!({ "source": "conns.csv", "query": query }),
        )
        .await;
    assert!(
        pulled.contains("csv:conns.csv: 2 row(s)") && pulled.contains("2 claim(s)"),
        "{pulled}"
    );
    let again = r
        .ok(
            "graph_pull",
            json!({ "source": "conns.csv", "query": query }),
        )
        .await;
    assert!(again.contains("nothing written"), "{again}");
    assert_eq!(
        r.ok("graph_query", json!({ "what": "relations" }))
            .await
            .lines()
            .count(),
        2
    );

    let bad = r
        .call(
            "graph_pull",
            json!({ "source": "conns.csv", "query": "DELETE FROM src" }),
        )
        .await;
    assert!(bad.is_error, "{}", bad.text);

    // The pull was change #1 in this fresh case.
    let seq: i64 = pulled
        .split("change #")
        .nth(1)
        .and_then(|t| t.trim_end_matches(')').split(')').next())
        .and_then(|n| n.trim().parse().ok())
        .unwrap();
    let preview = r
        .ok(
            "graph_annotate",
            json!({ "revert": { "change": seq, "preview": true } }),
        )
        .await;
    assert!(
        preview.starts_with("would take back:") && preview.contains("leaves the graph"),
        "{preview}"
    );
    assert_eq!(
        r.ok("graph_query", json!({ "what": "relations" }))
            .await
            .lines()
            .count(),
        2
    );
    let done = r
        .ok("graph_annotate", json!({ "revert": { "change": seq } }))
        .await;
    assert!(done.starts_with("taken back:"), "{done}");
    assert_eq!(
        r.ok("graph_query", json!({ "what": "find" })).await,
        "nothing recorded matches"
    );
}

#[tokio::test]
async fn a_detached_session_has_no_graph_tools_and_a_stale_call_is_told_why() {
    let r = rig();
    r.ok(
        "graph_annotate",
        json!({ "type": "note", "text": "from the attached session" }),
    )
    .await;
    let names = |r: &Rig| -> Vec<String> {
        // What the loop does: the session's layer over the default.
        let mut state = r.ctx.state();
        let global = reactor_context::settings::Settings::load(&r.ctx.paths).memory;
        state.memory = state.memory.over(&global);
        r.tools.specs(&state).into_iter().map(|t| t.name).collect()
    };
    assert!(names(&r).contains(&"graph_add".to_string()));

    save_state(
        &mut r.ctx.store.lock().unwrap(),
        KEY_MEMORY,
        json!({ "caseGraph": false }),
    )
    .unwrap();
    let left = names(&r);
    assert!(
        !left
            .iter()
            .any(|n| n.starts_with("graph_") || n == "artifact_register"),
        "{left:?}"
    );
    assert!(left.contains(&"bash".to_string()) && left.contains(&"history_search".to_string()));

    // It neither reads nor writes.
    for (tool, args) in [
        ("graph_query", json!({ "what": "find" })),
        (
            "graph_annotate",
            json!({ "type": "note", "text": "should not land" }),
        ),
        (
            "graph_add",
            json!({ "elements": [{ "kind": "entity", "type": "net.domain", "value": "x.example" }] }),
        ),
        (
            "graph_pull",
            json!({ "source": "conns.csv", "query": "select 1" }),
        ),
        (
            "artifact_register",
            json!({ "path": "samples/dropper.bin" }),
        ),
    ] {
        let out = r.call(tool, args).await;
        assert!(
            out.is_error && out.text.contains("detached from the case's graph"),
            "{tool}: {}",
            out.text
        );
        assert!(!out.text.contains("from the attached session"));
    }
    save_state(
        &mut r.ctx.store.lock().unwrap(),
        KEY_MEMORY,
        json!({ "caseGraph": true }),
    )
    .unwrap();
    let all = r.ok("graph_query", json!({ "what": "find" })).await;
    assert_eq!(
        all.lines().count(),
        1,
        "only what was written while attached: {all}"
    );
}

#[test]
fn the_agent_is_told_to_write_markdown_only_for_a_long_text() {
    let tools = Tools::standard(std::path::PathBuf::from("."));
    let spec = tools.find("graph_annotate").unwrap().spec();
    let said = spec.parameters["properties"]["text"]["description"]
        .as_str()
        .unwrap();
    assert!(said.contains("Markdown") && said.contains("long"), "{said}");
    assert!(said.contains("Plain text"), "{said}");
}

#[tokio::test]
async fn the_vocabulary_is_listed_by_namespace_and_a_type_of_ones_own_is_defined_and_drawn() {
    let r = rig();
    let all = r.ok("graph_query", json!({ "what": "vocabulary" })).await;
    for line in [
        "net -- Network: addresses, names and traffic.",
        "  net.ipv4 (entity) An IPv4 address.",
        "malware -- ",
        "  code.function (entity)",
        "  attack.technique (entity)",
    ] {
        assert!(all.contains(line), "no `{line}` in:\n{all}");
    }
    let hashes = r
        .ok(
            "graph_query",
            json!({ "what": "vocabulary", "type": "file.hash" }),
        )
        .await;
    assert!(
        hashes.contains("file.hash.imphash") && !hashes.contains("file.path"),
        "{hashes}"
    );

    // A new type needs a kind; then only what is given changes.
    let refused = r
        .call(
            "graph_annotate",
            json!({ "define_type": { "type": "ics.plc" } }),
        )
        .await;
    assert!(
        refused.is_error && refused.text.contains("kind"),
        "{}",
        refused.text
    );
    let made = r
        .ok(
            "graph_annotate",
            json!({ "define_type": { "type": "ics.plc", "kind": "entity",
                "definition": "A programmable logic controller.", "icon": "cpu", "colour": "teal" } }),
        )
        .await;
    assert!(
        made.contains("ics.plc (entity)") && made.contains("icon cpu, colour teal"),
        "{made}"
    );
    let red = r
        .call(
            "graph_annotate",
            json!({ "define_type": { "type": "ics.plc", "colour": "red" } }),
        )
        .await;
    assert!(
        red.is_error && red.text.contains("disputed"),
        "{}",
        red.text
    );
    // A known type's look falls back to its namespace where it says nothing itself.
    let known = r
        .ok(
            "graph_annotate",
            json!({ "define_type": { "type": "net.cidr", "definition": "A range." } }),
        )
        .await;
    assert!(known.contains("icon waypoints, colour blue"), "{known}");
    let listed = r
        .ok(
            "graph_query",
            json!({ "what": "vocabulary", "type": "ics" }),
        )
        .await;
    assert!(
        listed.contains("ics.plc (entity) A programmable logic controller."),
        "{listed}"
    );
}
