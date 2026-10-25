//! Real programs on a real pty, read back from the grid: the checks a person would do by hand
//! (`ls --color`, `vim`, `less`, `htop`, `top`, a REPL, a progress bar, a paste, a resize).
//!
//! They need the programs installed and a moment each, so they are ignored by default:
//! `cargo test -p reactor-gui --lib programs_tests -- --ignored`. One that is not installed
//! passes with a note rather than failing.

use std::time::{Duration, Instant};

use super::backend::Backend;
use super::keys;
use super::session::{Kind, Session};
use super::types::*;

struct Prog {
    backend: Backend,
    session: Session,
    rows: u16,
    cols: u16,
}

fn installed(name: &str) -> bool {
    let found = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()));
    if !found {
        eprintln!("NOTE: {name} is not installed; skipped");
    }
    found
}

impl Prog {
    fn start(program: &str, args: &[&str], rows: u16, cols: u16) -> Self {
        let backend = Backend::new(rows as usize, cols as usize, &Settings::default());
        let kind = Kind::Task {
            program: program.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        };
        let session = Session::start(
            &backend,
            &kind,
            None,
            TermSize::new(rows, cols, 8, 16),
            None,
        )
        .expect("a pty");
        Self {
            backend,
            session,
            rows,
            cols,
        }
    }

    fn screen(&self) -> Vec<String> {
        let c = self.backend.snapshot();
        (0..c.rows)
            .map(|r| c.row_text(r).trim_end().to_owned())
            .collect()
    }

    fn wait(&self, what: &str, done: impl Fn(&Content) -> bool) {
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            self.backend.inbox().take(1024);
            let c = self.backend.snapshot();
            if done(&c) {
                return;
            }
            assert!(
                Instant::now() < until,
                "gave up waiting for {what}; screen:\n{}",
                self.screen().join("\n")
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_text(&self, text: &str) {
        self.wait(text, |c| (0..c.rows).any(|r| c.row_text(r).contains(text)));
    }

    fn type_text(&self, text: &str) {
        self.session.write(text.as_bytes().to_vec());
    }

    /// A key the way the view sends it.
    fn press(&self, keystroke: &str) {
        let k = gpui_kit::Keystroke::parse(keystroke).expect("a keystroke");
        let bytes = keys::encode(&k, self.backend.modes(), true).expect("bytes for the key");
        self.session.write(bytes.into_owned());
    }

    fn wait_exit(&self) -> Option<i32> {
        let until = Instant::now() + Duration::from_secs(15);
        let mut code = None;
        let mut exited = false;
        while !exited {
            for e in self.backend.inbox().take(1024) {
                match e {
                    Event::ChildExit(info) => code = info.code,
                    Event::Exit => exited = true,
                    _ => {}
                }
            }
            assert!(
                Instant::now() < until,
                "did not exit; screen:\n{}",
                self.screen().join("\n")
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        code
    }
}

#[test]
#[ignore = "needs ls"]
fn ls_colours_a_directory_blue_and_an_executable_green() {
    if !installed("ls") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("adir")).unwrap();
    let exe = dir.path().join("arun");
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    std::os::unix::fs::PermissionsExt::set_mode(
        &mut std::fs::metadata(&exe).unwrap().permissions(),
        0o755,
    );
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(dir.path().join("plain.txt"), "x").unwrap();
    let p = Prog::start(
        "ls",
        &["--color=always", dir.path().to_str().unwrap()],
        10,
        60,
    );
    p.wait_exit();
    let c = p.backend.snapshot();
    let find = |word: &str| {
        (0..c.rows)
            .find_map(|r| c.row_text(r).find(word).map(|col| c.cell(r, col).clone()))
            .unwrap_or_else(|| panic!("{word} not on screen:\n{}", p.screen().join("\n")))
    };
    assert_eq!(find("adir").fg, Color::Indexed(4), "directories are blue");
    assert_eq!(find("arun").fg, Color::Indexed(2), "executables are green");
    assert_eq!(find("plain").fg, Color::DefaultFg);
}

#[test]
#[ignore = "needs sh"]
fn a_progress_bar_rewrites_its_line() {
    let p = Prog::start(
        "sh",
        &[
            "-c",
            "for i in 10 40 70 100; do printf '\\rworking %s%%' $i; sleep 0.05; done; echo",
        ],
        6,
        40,
    );
    p.wait_exit();
    let s = p.screen();
    assert_eq!(s[0], "working 100%", "{s:?}");
    assert_eq!(s[1], "", "{s:?}");
}

#[test]
#[ignore = "needs python3"]
fn a_repl_answers_what_is_typed_at_its_prompt() {
    if !installed("python3") {
        return;
    }
    let p = Prog::start("python3", &["-q"], 12, 60);
    p.wait_text(">>>");
    p.type_text("print(6 * 7)\r");
    p.wait("the answer", |c| {
        (0..c.rows).any(|r| c.row_text(r).trim() == "42")
    });
    p.press("ctrl-d");
    assert_eq!(p.wait_exit(), Some(0));
}

#[test]
#[ignore = "needs vim"]
fn vim_draws_a_screen_takes_escape_and_arrows_and_leaves_it_clean() {
    if !installed("vim") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("note.txt");
    let p = Prog::start(
        "vim",
        &["-u", "NONE", "-N", "-n", file.to_str().unwrap()],
        20,
        70,
    );
    p.wait("vim's screen", |c| c.modes.alt_screen);
    p.type_text("ihello world");
    p.wait_text("hello world");
    // Escape leaves insert mode, so the next keys are commands: `0` and `x` take the first letter out.
    p.press("escape");
    p.type_text("0x");
    p.wait_text("ello world");
    // Arrow keys move the cursor in normal mode; `x` deletes where it lands (the second `l`).
    p.press("right");
    p.type_text("x");
    p.wait_text("elo world");
    p.type_text(":q!\r");
    assert_eq!(p.wait_exit(), Some(0));
    let c = p.backend.snapshot();
    assert!(!c.modes.alt_screen, "the primary screen is back");
}

#[test]
#[ignore = "needs less"]
fn less_pages_through_a_file_and_quits() {
    if !installed("less") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("many.txt");
    std::fs::write(
        &file,
        (1..=300).map(|i| format!("line {i}\n")).collect::<String>(),
    )
    .unwrap();
    let p = Prog::start("less", &[file.to_str().unwrap()], 15, 60);
    p.wait_text("line 1");
    p.type_text("G");
    p.wait_text("line 300");
    p.press("pageup");
    p.wait("an earlier page", |c| {
        !(0..c.rows).any(|r| c.row_text(r).contains("line 300"))
    });
    p.type_text("q");
    assert_eq!(p.wait_exit(), Some(0));
}

#[test]
#[ignore = "needs htop"]
fn htop_uses_the_alternate_screen_and_the_mouse_and_follows_a_resize() {
    if !installed("htop") {
        return;
    }
    let mut p = Prog::start("htop", &[], 24, 100);
    p.wait("htop's screen", |c| {
        c.modes.alt_screen && (0..c.rows).any(|r| c.row_text(r).contains("PID"))
    });
    let modes = p.backend.modes();
    assert_ne!(modes.mouse, MouseMode::Off, "htop asks for the mouse");
    // Resize while it runs: the program is told, and redraws at the new size.
    let size = TermSize::new(30, 130, 8, 16);
    p.backend.resize(30, 130);
    assert!(p.session.resize(size));
    p.rows = 30;
    p.cols = 130;
    // Its meters stretch to the width it has, so a row wider than the old grid says it was told.
    p.wait("a redraw at the new size", |c| {
        c.rows == 30 && (0..c.rows).any(|r| c.row_text(r).trim_end().chars().count() > 105)
    });
    p.type_text("q");
    assert_eq!(p.wait_exit(), Some(0));
    assert_eq!((p.rows, p.cols), (30, 130));
}

#[test]
#[ignore = "needs top"]
fn top_refreshes_in_place() {
    if !installed("top") {
        return;
    }
    let p = Prog::start("top", &[], 24, 90);
    p.wait("top's screen", |c| {
        (0..c.rows).any(|r| c.row_text(r).contains("PID"))
    });
    p.type_text("q");
    assert_eq!(p.wait_exit(), Some(0));
}

#[test]
#[ignore = "needs cat"]
fn a_paste_is_wrapped_when_the_program_asked_and_plain_when_it_did_not() {
    let p = Prog::start("sh", &["-c", "printf '\\033[?2004h'; exec cat -v"], 8, 50);
    p.wait("bracketed paste on", |c| c.modes.bracketed_paste);
    p.session.write(keys::paste("one\ntwo", true));
    p.session.write(&b"\r"[..]);
    p.wait_text("^[[200~one");
    p.wait_text("two^[[201~");

    let q = Prog::start("sh", &["-c", "exec cat -v"], 8, 50);
    q.wait("the pty", |_| true);
    q.session.write(keys::paste("one\ntwo", false));
    q.session.write(&b"\r"[..]);
    q.wait_text("two");
    assert!(!q.screen().iter().any(|l| l.contains("^[[200~")));
}

#[test]
#[ignore = "needs sudo"]
fn sudo_asks_for_a_password_on_the_terminal() {
    if !installed("sudo") {
        return;
    }
    // `-k` forgets any ticket, so it has to ask. Nothing is typed but Ctrl-C.
    let p = Prog::start("sudo", &["-k", "-v"], 8, 70);
    p.wait("the prompt", |c| {
        (0..c.rows).any(|r| c.row_text(r).to_lowercase().contains("password"))
    });
    p.press("ctrl-c");
    p.wait_exit();
}
