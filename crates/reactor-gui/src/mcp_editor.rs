//! The MCP server editor: a window of its own for adding a server or changing one. Opened from
//! Settings › MCP servers.
//!
//! A server is either a *local command* (REactor starts it and speaks to it over its standard
//! input and output) or a *remote URL* (streamable HTTP). Values may refer to the environment
//! as `${NAME}`, so a token need not be written into the file. The fields are text -- one line
//! of command, `KEY=VALUE` lines for the environment, `Name: value` lines for headers -- and
//! the functions that turn them into a [`Server`] and back are plain functions, tested below.

use std::collections::BTreeMap;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Root, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, SharedString, Window, div, px, size};

use reactor_core::mcp::{self, Server, Transport};

use crate::app::ReactorApp;
use crate::hints;

/// What the editor opens on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    New,
    /// A saved server, to change (and perhaps rename).
    Edit(String),
}

// -- text <-> server ------------------------------------------------------------------------

/// Split a command line into words: spaces separate, `'…'` and `"…"` group, `\` escapes.
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => match chars.next() {
                Some(next) => {
                    word.push(next);
                    started = true;
                }
                None => return Err("the command line ends with a `\\`".into()),
            },
            (Some(_), c) => word.push(c),
            (None, '\'') | (None, '"') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("a quote is opened and never closed".into());
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

/// The inverse of [`split_command`]: quote only the words that need it.
pub fn join_command(words: &[String]) -> String {
    words
        .iter()
        .map(|w| {
            if !w.is_empty() && !w.chars().any(|c| c.is_whitespace() || "'\"\\".contains(c)) {
                w.clone()
            } else {
                format!("\"{}\"", w.replace('\\', "\\\\").replace('"', "\\\""))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `KEY=VALUE` lines; blank lines and `#` comments are skipped.
pub fn parse_env(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!("environment line {}: expected KEY=VALUE", n + 1));
        };
        let k = k.trim();
        if k.is_empty() || k.chars().any(char::is_whitespace) {
            return Err(format!(
                "environment line {}: `{k}` is not a variable name",
                n + 1
            ));
        }
        out.insert(k.to_string(), v.trim().to_string());
    }
    Ok(out)
}

pub fn format_env(env: &BTreeMap<String, String>) -> String {
    env.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
}

/// `Name: value` lines.
pub fn parse_headers(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            return Err(format!("header line {}: expected `Name: value`", n + 1));
        };
        let k = k.trim();
        if k.is_empty() || k.chars().any(char::is_whitespace) {
            return Err(format!("header line {}: `{k}` is not a header name", n + 1));
        }
        out.insert(k.to_string(), v.trim().to_string());
    }
    Ok(out)
}

pub fn format_headers(headers: &BTreeMap<String, String>) -> String {
    headers.iter().map(|(k, v)| format!("{k}: {v}\n")).collect()
}

/// Tags, separated by commas or spaces.
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in text.split(|c: char| c == ',' || c.is_whitespace()) {
        if !t.is_empty() && !out.iter().any(|o| o == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// What the fields say, as a server -- or why they do not make one.
pub struct Fields<'a> {
    pub name: &'a str,
    pub remote: bool,
    pub command: &'a str,
    pub env: &'a str,
    pub url: &'a str,
    pub headers: &'a str,
    pub tags: &'a str,
    pub enabled: bool,
}

pub fn build(f: &Fields) -> Result<Server, String> {
    let name = f.name.trim();
    if !mcp::valid_name(name) {
        return Err(
            "the name is up to 24 letters, digits and `-`, not starting with `-`: it is part of the name the model calls each tool by".into(),
        );
    }
    let tags = parse_tags(f.tags);
    if let Some(bad) = tags.iter().find(|t| !mcp::valid_tag(t)) {
        return Err(format!("tag `{bad}`: no spaces or commas"));
    }
    let transport = if f.remote {
        let url = f.url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("the URL starts with http:// or https://".into());
        }
        Transport::Http {
            url: url.to_string(),
            headers: parse_headers(f.headers)?,
        }
    } else {
        let mut words = split_command(f.command)?;
        if words.is_empty() {
            return Err("give the command that starts the server".into());
        }
        let command = words.remove(0);
        Transport::Stdio {
            command,
            args: words,
            env: parse_env(f.env)?,
        }
    };
    Ok(Server {
        name: name.to_string(),
        transport,
        tags,
        enabled: f.enabled,
    })
}

// -- the window -----------------------------------------------------------------------------

pub fn open(cx: &mut App, app: Entity<ReactorApp>, start: Start) {
    let existing = match &start {
        Start::Edit(name) => app
            .read(cx)
            .mcp_rows()
            .ok()
            .and_then(|rows| rows.into_iter().find(|r| &r.server.name == name))
            .map(|r| r.server),
        Start::New => None,
    };
    let bounds = gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::centered(
        None,
        size(px(720.), px(640.)),
        cx,
    ));
    let opened = cx.open_window(crate::chrome::window_options(bounds), move |window, cx| {
        crate::theme::install_ayu_dark(cx);
        let view = cx.new(|cx| Editor::new(app, existing, window, cx));
        let root: gpui_kit::AnyView = view.into();
        cx.new(|cx| Root::new(root, window, cx))
    });
    if opened.is_ok() {
        cx.activate(true);
    }
}

struct Editor {
    app: Entity<ReactorApp>,
    /// The saved server being changed, by the name it has now.
    editing: Option<String>,
    remote: bool,
    enabled: bool,
    name: Entity<InputState>,
    command: Entity<InputState>,
    env: Entity<TextareaState>,
    url: Entity<InputState>,
    headers: Entity<TextareaState>,
    tags: Entity<InputState>,
    error: Option<String>,
}

fn input(
    placeholder: &'static str,
    value: &str,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Entity<InputState> {
    cx.new(|cx| {
        let mut s = InputState::new(window, cx).placeholder(placeholder);
        s.set_value(value, window, cx);
        s
    })
}

fn area(
    placeholder: &'static str,
    value: &str,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Entity<TextareaState> {
    cx.new(|cx| {
        let mut s = TextareaState::new(window, cx).placeholder(placeholder);
        s.set_value(value, window, cx);
        s
    })
}

impl Editor {
    fn new(
        app: Entity<ReactorApp>,
        existing: Option<Server>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (mut command, mut env, mut url, mut headers) =
            (String::new(), String::new(), String::new(), String::new());
        let (mut remote, mut enabled, mut tags, mut name) =
            (false, true, String::new(), String::new());
        if let Some(s) = &existing {
            name = s.name.clone();
            tags = s.tags.join(", ");
            enabled = s.enabled;
            match &s.transport {
                Transport::Stdio {
                    command: c,
                    args,
                    env: e,
                } => {
                    let mut words = vec![c.clone()];
                    words.extend(args.iter().cloned());
                    command = join_command(&words);
                    env = format_env(e);
                }
                Transport::Http { url: u, headers: h } => {
                    remote = true;
                    url = u.clone();
                    headers = format_headers(h);
                }
            }
        }
        let name_input = input("name, e.g. ghidra", &name, window, cx);
        let focus = name_input.clone();
        let focus_new = existing.is_none();
        window.defer(cx, move |window, cx| {
            if focus_new {
                focus.update(cx, |s, cx| s.focus(window, cx));
            }
        });
        Editor {
            app,
            editing: existing.map(|s| s.name),
            remote,
            enabled,
            name: name_input,
            command: input("npx -y @scope/some-mcp-server --flag", &command, window, cx),
            env: area("API_TOKEN=${MY_API_TOKEN}", &env, window, cx),
            url: input("https://example.com/mcp", &url, window, cx),
            headers: area(
                "Authorization: Bearer ${MY_API_TOKEN}",
                &headers,
                window,
                cx,
            ),
            tags: input("tags, e.g. static, research", &tags, window, cx),
            error: None,
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = |s: &Entity<InputState>, cx: &App| s.read(cx).value().to_string();
        let area = |s: &Entity<TextareaState>, cx: &App| s.read(cx).value().to_string();
        let (name, command, url, tags) = (
            value(&self.name, cx),
            value(&self.command, cx),
            value(&self.url, cx),
            value(&self.tags, cx),
        );
        let (env, headers) = (area(&self.env, cx), area(&self.headers, cx));
        let server = match build(&Fields {
            name: &name,
            remote: self.remote,
            command: &command,
            env: &env,
            url: &url,
            headers: &headers,
            tags: &tags,
            enabled: self.enabled,
        }) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let old = self.editing.clone();
        let result = self
            .app
            .update(cx, |app, cx| app.mcp_save(old.as_deref(), server, cx));
        match result {
            Ok(()) => window.remove_window(),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let heading = match &self.editing {
            Some(name) => format!("MCP server: {name}"),
            None => "A new MCP server".to_string(),
        };
        let row = |label: &'static str, field: AnyElement| {
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .pt_1()
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(field))
        };
        let remote = self.remote;
        v_flex()
            .size_full()
            .child(crate::chrome::title_bar("REactor — MCP server", None, None, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(div().text_color(theme.foreground).child(heading))
                            .child(hints::info_button(
                                "mcp-editor-info",
                                "An MCP server gives the agent tools of its own. Its tools are listed in the Tools panel, off until you turn them on: each active tool puts its whole definition into every request, which the panel shows as a cost. A tag you give the server is on every one of its tools, so a toolset with that tag includes them. `${NAME}` in a value is the environment variable NAME — keep tokens there rather than in the file. REactor runs a local server as you, with no sandbox: only add ones you trust.",
                            )),
                    )
                    .child(row(
                        "Name",
                        div().w(px(280.)).child(Input::new(&self.name)).into_any_element(),
                    ))
                    .child(row(
                        "Kind",
                        TabBar::new("mcp-kind")
                            .segmented()
                            .selected_index(usize::from(remote))
                            .children([Tab::new().label("Local command"), Tab::new().label("Remote URL")])
                            .on_click(cx.listener(|this, ix: &usize, _w, cx| {
                                this.remote = *ix == 1;
                                cx.notify();
                            }))
                            .into_any_element(),
                    ))
                    .when(!remote, |el| {
                        el.child(row(
                            "Command",
                            Input::new(&self.command).into_any_element(),
                        ))
                        .child(row(
                            "Environment",
                            Textarea::new(&self.env).h(px(110.)).into_any_element(),
                        ))
                    })
                    .when(remote, |el| {
                        el.child(row("URL", Input::new(&self.url).into_any_element()))
                            .child(row(
                                "Headers",
                                Textarea::new(&self.headers).h(px(110.)).into_any_element(),
                            ))
                    })
                    .child(row(
                        "Tags",
                        Input::new(&self.tags).into_any_element(),
                    ))
                    .child(row(
                        "",
                        Switch::new("mcp-enabled")
                            .small()
                            .checked(self.enabled)
                            .label("Connect to this server")
                            .on_click(cx.listener(|this, next: &bool, _w, cx| {
                                this.enabled = *next;
                                cx.notify();
                            }))
                            .into_any_element(),
                    ))
                    .children(self.error.clone().map(|e| {
                        div().text_color(theme.danger).child(SharedString::from(e))
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .p_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("mcp-cancel")
                            .label("Cancel")
                            .ghost()
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        Button::new("mcp-save")
                            .label("Save")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        split_command(line).unwrap()
    }

    #[test]
    fn a_command_line_splits_into_words() {
        assert_eq!(
            words("npx -y @scope/srv  --flag"),
            ["npx", "-y", "@scope/srv", "--flag"]
        );
        assert_eq!(
            words(r#"python "my server.py" 'a b'"#),
            ["python", "my server.py", "a b"]
        );
        assert_eq!(words(r"a\ b c"), ["a b", "c"]);
        assert_eq!(words(r#"x "" y"#), ["x", "", "y"]);
        assert_eq!(words("   "), Vec::<String>::new());
        assert!(split_command("a 'b").is_err());
        assert!(split_command("a\\").is_err());
    }

    #[test]
    fn joining_is_the_inverse_of_splitting() {
        for line in [
            vec!["npx", "-y", "srv"],
            vec!["python", "my server.py", "say \"hi\"", "back\\slash", ""],
        ] {
            let line: Vec<String> = line.into_iter().map(String::from).collect();
            assert_eq!(split_command(&join_command(&line)).unwrap(), line);
        }
    }

    #[test]
    fn environment_and_header_lines() {
        let env = parse_env("# a comment\nTOKEN=${MY}\n\nA = b=c \n").unwrap();
        assert_eq!(env["TOKEN"], "${MY}");
        assert_eq!(env["A"], "b=c");
        assert_eq!(parse_env(&format_env(&env)).unwrap(), env);
        assert!(parse_env("no equals").is_err());
        assert!(parse_env("two words=x").is_err());

        let h = parse_headers("Authorization: Bearer ${T}\nX-A:b: c").unwrap();
        assert_eq!(h["Authorization"], "Bearer ${T}");
        assert_eq!(h["X-A"], "b: c");
        assert_eq!(parse_headers(&format_headers(&h)).unwrap(), h);
        assert!(parse_headers("nocolon").is_err());
    }

    #[test]
    fn tags_are_split_on_commas_and_spaces_without_repeats() {
        assert_eq!(
            parse_tags("static, research  static,x"),
            ["static", "research", "x"]
        );
        assert!(parse_tags("  ").is_empty());
    }

    fn fields() -> Fields<'static> {
        Fields {
            name: " docs ",
            remote: false,
            command: "npx -y srv",
            env: "K=v",
            url: "",
            headers: "",
            tags: "a, b",
            enabled: true,
        }
    }

    #[test]
    fn the_fields_make_a_local_or_a_remote_server() {
        let s = build(&fields()).unwrap();
        assert_eq!(s.name, "docs");
        assert_eq!(s.tags, ["a", "b"]);
        let Transport::Stdio { command, args, env } = s.transport else {
            panic!()
        };
        assert_eq!(
            (command.as_str(), args),
            ("npx", vec!["-y".to_string(), "srv".to_string()])
        );
        assert_eq!(env["K"], "v");

        let r = build(&Fields {
            remote: true,
            url: "https://example.com/mcp",
            headers: "A: b",
            ..fields()
        })
        .unwrap();
        assert!(r.is_http());
    }

    #[test]
    fn what_is_wrong_is_said() {
        for (f, says) in [
            (
                Fields {
                    name: "a_b",
                    ..fields()
                },
                "name",
            ),
            (
                Fields {
                    name: "",
                    ..fields()
                },
                "name",
            ),
            (
                Fields {
                    command: "  ",
                    ..fields()
                },
                "command",
            ),
            (
                Fields {
                    remote: true,
                    url: "ftp://x",
                    ..fields()
                },
                "URL",
            ),
            (
                Fields {
                    env: "oops",
                    ..fields()
                },
                "environment",
            ),
        ] {
            let e = build(&f).err().unwrap_or_default();
            assert!(e.contains(says), "{e}");
        }
    }
}
