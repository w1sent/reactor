//! The graph panel alone in a window, on a small made-up case: for looking at how it draws.
//!
//!     cargo run -p reactor-gui --example graph_demo -- [stage ...]
//!
//! Each argument is a stage (`GraphPanel::stage_for_test`): `find:beacon`, `legend`,
//! `size:l`, `layers`, `xl`, `pick`.

use std::sync::Arc;

use gpui_kit::component::Root;
use gpui_kit::prelude::*;
use gpui_kit::{Bounds, WeakEntity, WindowBounds, WindowOptions, px, size};
use reactor_db::graph::{Batch, Content, Kind, Link, Put, Target, apply, vocab};
use reactor_db::manifest::Author;
use reactor_gui::graph::panel::GraphPanel;

const PATH: &str = "C:\\Users\\jdoe\\AppData\\Local\\Microsoft\\Office\\16.0\\OfficeFileCache\\FSD-7C1E0A52-9B3D-4F6A-A1C8-2E5D90B7F314\\update_helper.dll";
const NOTE: &str = "Ask IT whether ws-042 was reimaged in March. The EDR gap would explain the missing process events.\n\n## Checked so far\n- EDR has no process events for ws-042 from 1 to 19 March.\n- The VPN log has jdoe logging on from ws-042 on 12 March, so the host was in use.\n- Asset inventory lists a new disk serial from 20 March.\n\nIf it was reimaged, update_helper.dll cannot have been written before the 20th, and the foothold hypothesis needs another first host. ws-017 shares the VPN pool and is the next one to look at.";

fn fill(db: &reactor_db::Db) -> Vec<String> {
    let claim = |kind, type_: &str, links: Vec<Link>| Put {
        links,
        ..Put::new(kind, type_)
    };
    let found = apply(
        db,
        Batch::new(Author::analyst())
            .put(claim(
                Kind::Event,
                "auth.logon",
                vec![
                    Link::entity("actor", "account.user", "jdoe"),
                    Link::entity("target", "host.name", "ws-042"),
                    Link::source("artifact:vpn.csv"),
                ],
            ))
            .put(Put {
                content: Content {
                    confidence: Some("very likely".into()),
                    ..Content::default()
                },
                ..claim(
                    Kind::Event,
                    "net.connection",
                    vec![
                        Link::entity("src", "host.name", "ws-042"),
                        Link::entity("dst", "net.ipv4", "198.51.100.7"),
                        Link::source("artifact:fw.csv"),
                    ],
                )
            })
            .put(claim(
                Kind::Observation,
                "net.resolution",
                vec![
                    Link::entity("query", "net.domain", "cdn-sync.example"),
                    Link::entity("answer", "net.ipv4", "198.51.100.7"),
                    Link::source("artifact:pdns.csv").at("row=1", None),
                ],
            ))
            .put(Put {
                content: Content {
                    status: "conflicting".into(),
                    ..Content::default()
                },
                ..claim(
                    Kind::Observation,
                    "net.resolution",
                    vec![
                        Link::entity("query", "net.domain", "cdn-sync.example"),
                        Link::entity("answer", "net.ipv4", "198.51.100.7"),
                        Link::source("artifact:pdns.csv").at("row=2", None),
                    ],
                )
            })
            .put(claim(
                Kind::Event,
                "file.write",
                vec![
                    Link::entity("actor", "host.name", "ws-042"),
                    Link::entity("target", "file.path", PATH),
                    Link::source("artifact:edr.jsonl"),
                ],
            ))
            .put(claim(
                Kind::Observation,
                "file.digest",
                vec![
                    Link::entity("subject", "file.path", PATH),
                    Link::entity(
                        "object",
                        "file.hash.sha256",
                        "9f2c4e1ab07d53c8e6f1420b9a7d3e5c81f0a2b4c6d8e0f13579bdf2468ace01",
                    ),
                    Link::source("artifact:edr.jsonl"),
                ],
            ))
            .put(Put {
                content: Content {
                    status: "conflicting".into(),
                    confidence: Some("roughly even".into()),
                    ..Content::default()
                },
                ..claim(
                    Kind::Event,
                    "process.start",
                    vec![
                        Link::entity("parent", "process.name", "winword.exe"),
                        Link::entity("child", "process.name", "powershell.exe"),
                        Link::entity("actor", "account.user", "jdoe"),
                        Link::source("artifact:edr.jsonl"),
                    ],
                )
            })
            .put(claim(
                Kind::Observation,
                "malware.variant_of",
                vec![
                    Link::entity("subject", "file.path", PATH),
                    Link::entity("object", "malware.family", "QakBot"),
                    Link::source("artifact:edr.jsonl"),
                ],
            ))
            .put(claim(
                Kind::Observation,
                "attack.observed",
                vec![
                    Link::entity("subject", "process.name", "powershell.exe"),
                    Link::entity("object", "attack.technique", "t1059.001"),
                    Link::source("artifact:edr.jsonl"),
                ],
            ))
            .put(claim(
                Kind::Observation,
                "code.defines",
                vec![
                    Link::entity("subject", "file.path", PATH),
                    Link::entity("object", "code.function", "sub_180004a20 (rc4_init)"),
                    Link::source("artifact:edr.jsonl"),
                ],
            ))
            .put(claim(
                Kind::Observation,
                "hunt.lead_for",
                vec![
                    Link::entity("subject", "hunt.lead", "LEAD-17"),
                    Link::entity("object", "host.name", "ws-042"),
                    Link::source("artifact:edr.jsonl"),
                ],
            )),
    )
    .unwrap()
    .ids;
    let entity = |type_: &str, value: &str| {
        reactor_db::graph::query::entity_id(db, type_, value, None).unwrap()
    };
    let (host, ip) = (
        entity("host.name", "ws-042"),
        entity("net.ipv4", "198.51.100.7"),
    );
    apply(
        db,
        Batch::new(Author::analyst()).change(reactor_db::graph::Change {
            id: ip.clone(),
            add_tags: vec!["c2.beacon".into(), "phase.c2".into()],
            ..Default::default()
        }),
    )
    .unwrap();
    let hypothesis = apply(
        db,
        Batch::new(Author::agent("demo")).put(Put {
            content: Content {
                confidence: Some("likely".into()),
                tags: vec!["phase.c2".into()],
                props: [("hypothesis_status".to_string(), serde_json::json!("open"))]
                    .into_iter()
                    .collect(),
                ..Content::value(
                    "ws-042 was the initial foothold; update_helper.dll is a second-stage loader fetched over the beacon channel.",
                )
            },
            links: vec![Link::new("about", Target::Id(host.clone()))],
            ..Put::new(Kind::Annotation, "analysis.hypothesis")
        }),
    )
    .unwrap()
    .ids
    .remove(0);
    apply(
        db,
        Batch::new(Author::agent("demo")).put(Put {
            content: Content {
                confidence: Some("very likely".into()),
                ..Content::value(
                    "Resolution overlaps the beacon window (02:14 to 02:19 UTC) on 5 of 5 polls.",
                )
            },
            links: vec![
                Link::new("about", Target::Id(ip.clone())),
                Link::new("supports", Target::Id(hypothesis.clone())),
            ],
            ..Put::new(Kind::Annotation, "analysis.judgement")
        }),
    )
    .unwrap();
    let note = apply(
        db,
        Batch::new(Author::analyst()).put(Put {
            content: Content {
                tags: vec!["needs-it".into()],
                ..Content::value(NOTE)
            },
            links: vec![Link::new("about", Target::Id(host.clone()))],
            ..Put::new(Kind::Annotation, "analysis.note")
        }),
    )
    .unwrap()
    .ids
    .remove(0);
    let _ = found;
    vec![host, ip, hypothesis, note]
}

fn main() {
    let mut stages: Vec<String> = std::env::args().skip(1).collect();
    // `shot=<file.png>`: draw, save what was drawn, and quit.
    let shot = stages
        .iter()
        .position(|s| s.starts_with("shot="))
        .map(|i| stages.remove(i)["shot=".len()..].to_string());
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    // SAFETY: set before any thread that reads the environment exists.
    unsafe { std::env::set_var("REACTOR_CONFIG_DIR", root.path().join("cfg")) };
    let backend = Arc::new(
        reactor_gui::backend::Backend::start(reactor_gui::backend::StartOptions {
            cwd: work,
            ..Default::default()
        })
        .expect("a backend"),
    );
    let ids = fill(backend.case.db());
    if stages.iter().any(|s| s == "pick") {
        vocab::set_look(
            backend.case.db(),
            "hunt.lead",
            Kind::Entity,
            Some("flag"),
            Some("magenta"),
        )
        .unwrap();
    }
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            reactor_gui::theme::install_ayu_dark(cx);
            reactor_gui::app::bind_keys(cx);
            let bounds = Bounds::centered(None, size(px(1500.), px(940.)), cx);
            let mut made = None;
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |window, cx| {
                        reactor_gui::theme::install_ayu_dark(cx);
                        let panel =
                            cx.new(|cx| GraphPanel::new(WeakEntity::new_invalid(), window, cx));
                        made = Some(panel.clone());
                        panel.update(cx, |p, cx| {
                            p.set_backend(Some(backend.clone()), cx);
                            p.relayout_for_test(cx);
                        });
                        let view: gpui_kit::AnyView = panel.into();
                        cx.new(|cx| Root::new(view, window, cx))
                    },
                )
                .unwrap();
            cx.activate(true);
            {
                let panel = made.clone().unwrap();
                let fit = !stages.iter().any(|s| s == "nofit");
                cx.spawn(async move |cx| {
                    // Once the window knows its size, bring everything into view.
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(600))
                        .await;
                    // The stages go on once the first frame has measured the fonts, as they
                    // would by hand.
                    let _ = cx.update_window(window.into(), |_, window, cx| {
                        panel.update(cx, |p, cx| {
                            for stage in &stages {
                                match stage.as_str() {
                                    "layers" => {
                                        p.add_layer_for_test("-status:deleted", "only", window, cx);
                                        p.add_layer_for_test("type:process.", "dim", window, cx);
                                        p.add_layer_for_test(
                                            "conf:>=likely",
                                            "highlight",
                                            window,
                                            cx,
                                        );
                                    }
                                    "xl" => {
                                        p.stage_for_test(&format!("own:{}:xl", ids[3]), window, cx)
                                    }
                                    "pick" => {
                                        p.stage_for_test(&format!("pick:{}", ids[0]), window, cx)
                                    }
                                    "select" => {
                                        p.stage_for_test(&format!("select:{}", ids[1]), window, cx)
                                    }
                                    "nofit" => {}
                                    "goto-note" => {
                                        p.stage_for_test(&format!("goto:{}", ids[3]), window, cx)
                                    }
                                    "goto-host" => {
                                        p.stage_for_test(&format!("goto:{}", ids[0]), window, cx)
                                    }
                                    "select-note" => {
                                        p.stage_for_test(&format!("select:{}", ids[3]), window, cx)
                                    }
                                    other => p.stage_for_test(other, window, cx),
                                }
                            }
                            if fit {
                                p.stage_for_test("fit", window, cx);
                            }
                        });
                    });
                    let Some(shot) = shot else {
                        return;
                    };
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1200))
                        .await;
                    let _ = cx.update_window(window.into(), |_, window, _| {
                        match window.render_to_image() {
                            Ok(image) => image.save(&shot).expect("save the picture"),
                            Err(e) => eprintln!("no picture: {e}"),
                        }
                    });
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
        });
}
