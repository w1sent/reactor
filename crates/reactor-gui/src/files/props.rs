//! The properties popup: what a file is. Common metadata, the filesystem's own extras, what
//! `file(1)` and the magic numbers say it is, and the hashes the user asked for.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use gpui_kit::base::{SelectableText, h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{ClipboardItem, Context, Render, Window, div, px};

use reactor_fs::{Algorithm, FileRef, Identity, Kind, Stat, hash_file, identify_file};

use super::human_size;

enum Hash {
    Running,
    Done(String),
    Failed(String),
}

pub struct PropertiesView {
    file: FileRef,
    stat: Result<Stat, String>,
    identity: Option<Result<Identity, String>>,
    hashes: Vec<(Algorithm, Hash)>,
    cancel: Arc<AtomicBool>,
}

impl Drop for PropertiesView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn time(t: Option<SystemTime>) -> Option<String> {
    let t: chrono::DateTime<chrono::Local> = t?.into();
    Some(t.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// `-rwxr-xr-x (0755)`.
fn mode_text(mode: u32, kind: Option<Kind>) -> String {
    let mut out = String::from(if kind == Some(Kind::Dir) { "d" } else { "-" });
    for shift in [6, 3, 0] {
        let bits = mode >> shift;
        out.push(if bits & 4 != 0 { 'r' } else { '-' });
        out.push(if bits & 2 != 0 { 'w' } else { '-' });
        out.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    format!("{out} ({:04o})", mode & 0o7777)
}

impl PropertiesView {
    /// `hash` are computed straight away, so a menu's "SHA-256" opens the popup already
    /// working on it.
    pub fn new(file: FileRef, hash: &[Algorithm], cx: &mut Context<Self>) -> Self {
        let stat = file.fs.stat(&file.path).map_err(|e| e.to_string());
        let is_file = matches!(&stat, Ok(s) if s.kind == Some(Kind::File));
        let mut this = Self {
            file,
            stat,
            identity: None,
            hashes: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        if is_file {
            this.identify(cx);
            if !hash.is_empty() {
                this.hash(hash, cx);
            }
        }
        this
    }

    fn identify(&mut self, cx: &mut Context<Self>) {
        let file = self.file.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let reader = file.open().map_err(|e| e.to_string())?;
                    identify_file(reader.as_ref()).map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.identity = Some(result);
                cx.notify();
            });
        })
        .detach();
    }

    /// One pass over the file for however many algorithms are asked for.
    pub fn hash(&mut self, algorithms: &[Algorithm], cx: &mut Context<Self>) {
        let wanted: Vec<Algorithm> = algorithms
            .iter()
            .copied()
            .filter(|a| !self.hashes.iter().any(|(have, _)| have == a))
            .collect();
        if wanted.is_empty() {
            return;
        }
        for algorithm in &wanted {
            self.hashes.push((*algorithm, Hash::Running));
        }
        let file = self.file.clone();
        let cancel = self.cancel.clone();
        let for_task = wanted.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let reader = file.open()?;
                    hash_file(reader.as_ref(), &for_task, &cancel)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for (algorithm, state) in &mut this.hashes {
                    if !wanted.contains(algorithm) {
                        continue;
                    }
                    *state = match &result {
                        Ok(sums) => sums
                            .iter()
                            .find(|(a, _)| a == algorithm)
                            .map(|(_, sum)| Hash::Done(sum.clone()))
                            .unwrap_or_else(|| Hash::Failed("no result".into())),
                        Err(e) => Hash::Failed(e.to_string()),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = vec![
            ("Name", self.file.name()),
            ("Path", self.file.path.to_string()),
        ];
        if let Some(native) = self.file.fs.local_path(&self.file.path) {
            rows.push(("On disk", native.display().to_string()));
        }
        match &self.stat {
            Err(e) => rows.push(("Error", e.clone())),
            Ok(stat) => {
                rows.push((
                    "Type",
                    match stat.kind {
                        Some(Kind::Dir) => "Folder",
                        Some(Kind::File) => "File",
                        _ => "Special file",
                    }
                    .into(),
                ));
                if stat.kind == Some(Kind::File) {
                    rows.push((
                        "Size",
                        format!("{} ({} bytes)", human_size(stat.size), stat.size),
                    ));
                }
                if let Some(target) = &stat.symlink_target {
                    rows.push(("Link to", target.clone()));
                }
                for (label, value) in [
                    ("Modified", time(stat.modified)),
                    ("Accessed", time(stat.accessed)),
                    ("Created", time(stat.created)),
                    ("Mode", stat.mode.map(|m| mode_text(m, stat.kind))),
                    ("Owner", stat.uid.map(|uid| format!("uid {uid}"))),
                    ("Group", stat.gid.map(|gid| format!("gid {gid}"))),
                ] {
                    if let Some(value) = value {
                        rows.push((label, value));
                    }
                }
                if stat.readonly {
                    rows.push(("Read-only", "yes".into()));
                }
            }
        }
        rows
    }
}

impl Render for PropertiesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let row = |label: &'static str, value: String| {
            h_flex()
                .gap_3()
                .items_start()
                .child(div().w(px(84.)).flex_none().text_color(muted).child(label))
                .child(div().flex_1().min_w_0().child(SelectableText::new(
                    SharedString::from(format!("prop-{label}")),
                    value,
                )))
        };
        let mut body = v_flex().gap_1().w_full();
        for (label, value) in self.rows() {
            body = body.child(row(label, value));
        }
        if let Ok(stat) = &self.stat {
            for (label, value) in &stat.extra {
                body = body.child(
                    h_flex()
                        .gap_3()
                        .child(
                            div()
                                .w(px(84.))
                                .flex_none()
                                .text_color(muted)
                                .child(label.clone()),
                        )
                        .child(div().child(value.clone())),
                );
            }
        }
        match &self.identity {
            None if matches!(&self.stat, Ok(s) if s.kind == Some(Kind::File)) => {
                body = body.child(row("Identified as", "…".into()));
            }
            Some(Ok(identity)) => {
                body = body
                    .child(row("Identified as", identity.description.clone()))
                    .child(row("MIME type", identity.mime.clone()));
            }
            Some(Err(e)) => body = body.child(row("Identified as", format!("unreadable: {e}"))),
            None => {}
        }

        let is_file = matches!(&self.stat, Ok(s) if s.kind == Some(Kind::File));
        if is_file {
            body = body.child(div().h(px(6.)));
            for algorithm in Algorithm::ALL {
                let state = self
                    .hashes
                    .iter()
                    .find(|(a, _)| *a == algorithm)
                    .map(|(_, s)| s);
                let value: AnyElement = match state {
                    None => Button::new(("hash", algorithm as usize))
                        .label("Compute")
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| this.hash(&[algorithm], cx)))
                        .into_any_element(),
                    Some(Hash::Running) => div()
                        .text_color(muted)
                        .child("computing…")
                        .into_any_element(),
                    Some(Hash::Failed(e)) => div()
                        .text_color(theme.danger)
                        .child(e.clone())
                        .into_any_element(),
                    Some(Hash::Done(sum)) => {
                        let copy = sum.clone();
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(SelectableText::new(
                                ("hash-value", algorithm as usize),
                                sum.clone(),
                            ))
                            .child(
                                Button::new(("hash-copy", algorithm as usize))
                                    .icon(gpui_kit::assets::IconName::Copy)
                                    .ghost()
                                    .small()
                                    .tooltip("Copy")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    }),
                            )
                            .into_any_element()
                    }
                };
                body = body.child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            div()
                                .w(px(84.))
                                .flex_none()
                                .text_color(muted)
                                .child(algorithm.label()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(theme.mono_font_family.clone())
                                .child(value),
                        ),
                );
            }
            body = body.child(
                h_flex().justify_end().pt_2().child(
                    Button::new("hash-all")
                        .label("Compute all")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.hash(&Algorithm::ALL, cx))),
                ),
            );
        }
        body
    }
}
