//! Starting the program a console hosts: which one, with what environment, and how to reach it.
//!
//! A console runs either the person's shell or a task program (`reactor install <id>`,
//! `/console <cmd>`). The platform differences are decided here and nowhere else, so the call
//! sites name a program and its arguments and do not care where they run.

use std::io;
use std::path::{Path, PathBuf};

use super::backend::{Backend, Pty, SpawnOptions};
use super::types::TermSize;

/// What a console hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// The person's shell, which stays until they leave it.
    Shell,
    /// One program, which keeps its output when it exits.
    Task { program: String, args: Vec<String> },
}

impl Kind {
    /// A task. `sh -c <command>`, which is how the app asks for a command line to be run, becomes
    /// the platform's own shell where there is no `sh`.
    pub fn task(program: &str, args: &[String]) -> Self {
        #[cfg(windows)]
        if program == "sh" && args.len() == 2 && args[0] == "-c" {
            let (program, args) = windows_command(&args[1], &WindowsShells::find());
            return Kind::Task { program, args };
        }
        Kind::Task {
            program: program.to_owned(),
            args: args.to_vec(),
        }
    }

    /// The command line as the tab shows it.
    pub fn label(&self) -> Option<String> {
        match self {
            Kind::Shell => None,
            Kind::Task { program, args } => Some(
                std::iter::once(program.as_str())
                    .chain(args.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        }
    }
}

/// A running (or just ended) program on a pty.
pub struct Session {
    pty: Pty,
    size: TermSize,
    alive: bool,
}

impl Session {
    /// Start `kind` on a pty of `size`, its output parsed into `backend`'s grid. `shell` is the
    /// person's own choice of shell, which beats the platform's.
    pub fn start(
        backend: &Backend,
        kind: &Kind,
        cwd: Option<PathBuf>,
        size: TermSize,
        shell: Option<&str>,
    ) -> io::Result<Self> {
        let command = match kind {
            Kind::Shell => default_shell(shell, &WindowsShells::find()),
            Kind::Task { program, args } => Some((program.clone(), args.clone())),
        };
        let inherited: Vec<(String, String)> = std::env::vars().collect();
        let pty = backend.spawn(SpawnOptions {
            command,
            cwd,
            env: child_env(&inherited),
            size,
        })?;
        Ok(Self {
            pty,
            size,
            alive: true,
        })
    }

    pub fn alive(&self) -> bool {
        self.alive
    }

    /// The program ended; nothing more is written, and the window size is no longer sent.
    pub fn ended(&mut self) {
        self.alive = false;
    }

    pub fn write(&self, bytes: impl Into<std::borrow::Cow<'static, [u8]>>) {
        if self.alive {
            self.pty.write(bytes);
        }
    }

    /// Ctrl-C: the line discipline turns it into SIGINT for the foreground group, as in any
    /// terminal.
    pub fn interrupt(&self) {
        self.write(&b"\x03"[..]);
    }

    /// Tell the program the grid changed. True when something was different.
    pub fn resize(&mut self, size: TermSize) -> bool {
        if size == self.size {
            return false;
        }
        self.size = size;
        if self.alive {
            self.pty.resize(size);
        }
        true
    }

    pub fn size(&self) -> TermSize {
        self.size
    }
}

impl Drop for Session {
    /// Closing a console hangs up on its program. The wait for it happens on the thread that owns
    /// the pty, not here.
    fn drop(&mut self) {
        self.pty.shutdown();
    }
}

/// Variables that describe the terminal REactor itself was started from. A child that inherited
/// them would believe it is in that terminal.
const FOREIGN_PREFIXES: [&str; 6] = [
    "ALACRITTY_",
    "KITTY_",
    "WEZTERM_",
    "GHOSTTY_",
    "ITERM_",
    "TMUX",
];
const FOREIGN_NAMES: [&str; 5] = [
    "VTE_VERSION",
    "TERM_SESSION_ID",
    "COLUMNS",
    "LINES",
    "WINDOWID",
];

/// What is added to the child's environment, given what the process has. The engine's crate can
/// add and override variables of a child but not remove them, so a variable that must not be
/// inherited is set empty. Nothing here touches the environment of REactor itself.
pub fn child_env(inherited: &[(String, String)]) -> Vec<(String, String)> {
    let has = |name: &str| inherited.iter().any(|(k, v)| k == name && !v.is_empty());
    let mut env = vec![
        ("TERM".to_owned(), "xterm-256color".to_owned()),
        ("COLORTERM".to_owned(), "truecolor".to_owned()),
        ("TERM_PROGRAM".to_owned(), "REactor".to_owned()),
        (
            "TERM_PROGRAM_VERSION".to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
        ),
    ];
    // A program started from a launcher often has no locale, and then prints `?` for anything
    // that is not ASCII. macOS always has this one; elsewhere the C locale in UTF-8 always exists.
    if !has("LANG") {
        let lang = if cfg!(target_os = "macos") {
            "en_US.UTF-8"
        } else {
            "C.UTF-8"
        };
        env.push(("LANG".to_owned(), lang.to_owned()));
    }
    if cfg!(unix) {
        // A shell adds one to this, so a nested shell starts at 1.
        env.push(("SHLVL".to_owned(), "0".to_owned()));
        // The engine's crate sets these itself, to values that mean nothing here.
        env.push(("ALACRITTY_WINDOW_ID".to_owned(), String::new()));
        env.push(("WINDOWID".to_owned(), String::new()));
        for (name, _) in inherited {
            let foreign = FOREIGN_PREFIXES.iter().any(|p| name.starts_with(p))
                || FOREIGN_NAMES.contains(&name.as_str());
            if foreign && !env.iter().any(|(k, _)| k == name) {
                env.push((name.clone(), String::new()));
            }
        }
    }
    env
}

/// Which PowerShell and cmd are there. Only used on Windows, but decided by a plain function so
/// it can be tested anywhere.
#[derive(Debug, Clone, Default)]
pub struct WindowsShells {
    pub pwsh: bool,
    pub powershell: bool,
    pub comspec: Option<String>,
}

impl WindowsShells {
    pub fn find() -> Self {
        if !cfg!(windows) {
            return Self::default();
        }
        Self {
            pwsh: on_path("pwsh.exe"),
            powershell: on_path("powershell.exe"),
            comspec: std::env::var("COMSPEC").ok().filter(|s| !s.is_empty()),
        }
    }
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| Path::new(&dir).join(name).is_file())
    })
}

/// The shell to run: the person's choice, else on Windows the first of PowerShell 7, Windows
/// PowerShell and the command interpreter, else `None` for the platform's own (`$SHELL`).
pub fn default_shell(
    chosen: Option<&str>,
    windows: &WindowsShells,
) -> Option<(String, Vec<String>)> {
    if let Some(shell) = chosen.map(str::trim).filter(|s| !s.is_empty()) {
        return Some((shell.to_owned(), Vec::new()));
    }
    if !cfg!(windows) {
        return None;
    }
    Some(windows_shell(windows))
}

fn windows_shell(shells: &WindowsShells) -> (String, Vec<String>) {
    if shells.pwsh {
        ("pwsh.exe".to_owned(), vec!["-NoLogo".to_owned()])
    } else if shells.powershell {
        ("powershell.exe".to_owned(), vec!["-NoLogo".to_owned()])
    } else {
        (
            shells
                .comspec
                .clone()
                .unwrap_or_else(|| "cmd.exe".to_owned()),
            Vec::new(),
        )
    }
}

/// A command line run by the platform's shell on Windows.
pub fn windows_command(command: &str, shells: &WindowsShells) -> (String, Vec<String>) {
    if shells.pwsh || shells.powershell {
        let program = if shells.pwsh {
            "pwsh.exe"
        } else {
            "powershell.exe"
        };
        (
            program.to_owned(),
            vec![
                "-NoLogo".to_owned(),
                "-Command".to_owned(),
                command.to_owned(),
            ],
        )
    } else {
        (
            shells
                .comspec
                .clone()
                .unwrap_or_else(|| "cmd.exe".to_owned()),
            vec!["/C".to_owned(), command.to_owned()],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::types::{Content, Event, ExitInfo, Settings};
    use std::time::{Duration, Instant};

    fn env_of(vars: &[(&str, &str)]) -> Vec<(String, String)> {
        let inherited: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        child_env(&inherited)
    }

    fn get<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
        env.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    #[test]
    fn the_child_is_told_what_it_talks_to() {
        let env = env_of(&[]);
        assert_eq!(get(&env, "TERM"), Some("xterm-256color"));
        assert_eq!(get(&env, "COLORTERM"), Some("truecolor"));
        assert_eq!(get(&env, "TERM_PROGRAM"), Some("REactor"));
        assert!(get(&env, "LANG").is_some_and(|l| l.ends_with("UTF-8")));
    }

    #[test]
    fn a_locale_that_is_set_is_left_alone() {
        let env = env_of(&[("LANG", "sv_SE.UTF-8")]);
        assert_eq!(get(&env, "LANG"), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_terminal_it_was_started_from_is_not_inherited() {
        let env = env_of(&[
            ("KITTY_WINDOW_ID", "7"),
            ("TMUX", "/tmp/tmux-1000/default,1,0"),
            ("TMUX_PANE", "%1"),
            ("VTE_VERSION", "7000"),
            ("COLUMNS", "200"),
            ("ALACRITTY_LOG", "x"),
            ("PATH", "/usr/bin"),
        ]);
        for name in [
            "KITTY_WINDOW_ID",
            "TMUX",
            "TMUX_PANE",
            "VTE_VERSION",
            "COLUMNS",
            "ALACRITTY_LOG",
        ] {
            assert_eq!(get(&env, name), Some(""), "{name}");
        }
        assert_eq!(get(&env, "PATH"), None, "the rest is inherited as it is");
        assert_eq!(get(&env, "SHLVL"), Some("0"));
        assert_eq!(get(&env, "ALACRITTY_WINDOW_ID"), Some(""));
    }

    #[test]
    fn a_chosen_shell_beats_the_platform_one() {
        let shells = WindowsShells::default();
        assert_eq!(
            default_shell(Some(" /bin/fish "), &shells),
            Some(("/bin/fish".to_owned(), Vec::new()))
        );
        if cfg!(unix) {
            assert_eq!(default_shell(None, &shells), None);
            assert_eq!(default_shell(Some("  "), &shells), None);
        }
    }

    #[test]
    fn windows_prefers_pwsh_then_powershell_then_the_command_interpreter() {
        let all = WindowsShells {
            pwsh: true,
            powershell: true,
            comspec: Some("C:\\Windows\\System32\\cmd.exe".into()),
        };
        assert_eq!(windows_shell(&all).0, "pwsh.exe");
        let old = WindowsShells {
            pwsh: false,
            ..all.clone()
        };
        assert_eq!(windows_shell(&old).0, "powershell.exe");
        let bare = WindowsShells {
            pwsh: false,
            powershell: false,
            comspec: Some("C:\\Windows\\System32\\cmd.exe".into()),
        };
        assert_eq!(windows_shell(&bare).0, "C:\\Windows\\System32\\cmd.exe");
        assert_eq!(windows_shell(&WindowsShells::default()).0, "cmd.exe");
    }

    #[test]
    fn a_command_line_goes_to_the_platform_shell_whole() {
        let (program, args) = windows_command("dir \"a b\" && echo x", &WindowsShells::default());
        assert_eq!(program, "cmd.exe");
        assert_eq!(args, ["/C", "dir \"a b\" && echo x"]);
        let ps = WindowsShells {
            powershell: true,
            ..Default::default()
        };
        let (program, args) = windows_command("Get-ChildItem", &ps);
        assert_eq!(program, "powershell.exe");
        assert_eq!(args.last().map(String::as_str), Some("Get-ChildItem"));
    }

    #[test]
    fn a_task_names_its_command_line_for_the_tab() {
        let k = Kind::Task {
            program: "reactor".into(),
            args: vec!["install".into(), "nmap".into()],
        };
        assert_eq!(k.label().as_deref(), Some("reactor install nmap"));
        assert_eq!(Kind::Shell.label(), None);
    }

    // ----- a real pty (unix); the program is `sh`, which every unix has -----

    #[cfg(unix)]
    struct Run {
        backend: Backend,
        session: Session,
        events: Vec<Event>,
    }

    #[cfg(unix)]
    impl Run {
        fn task(script: &str, rows: u16, cols: u16) -> Self {
            let size = TermSize::new(rows, cols, 8, 16);
            let backend = Backend::new(rows as usize, cols as usize, &Settings::default());
            let kind = Kind::task("sh", &["-c".into(), script.into()]);
            let session = Session::start(&backend, &kind, None, size, None).expect("a pty");
            Self {
                backend,
                session,
                events: Vec::new(),
            }
        }

        /// Collect events until `done` says so, or fail after a while.
        fn wait(&mut self, what: &str, done: impl Fn(&[Event], &Content) -> bool) {
            let until = Instant::now() + Duration::from_secs(15);
            loop {
                self.events.extend(self.backend.inbox().take(256));
                let content = self.backend.snapshot();
                if done(&self.events, &content) {
                    return;
                }
                assert!(
                    Instant::now() < until,
                    "gave up waiting for {what}; got {:?}; screen {:?}",
                    self.events,
                    (0..content.rows)
                        .map(|r| content.row_text(r))
                        .collect::<Vec<_>>()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn wait_exit(&mut self) -> ExitInfo {
            self.wait("the program to exit", |ev, _| {
                ev.iter().any(|e| matches!(e, Event::Exit))
            });
            self.events
                .iter()
                .find_map(|e| match e {
                    Event::ChildExit(info) => Some(*info),
                    _ => None,
                })
                .expect("a child exit")
        }

        fn row(&self, row: usize) -> String {
            self.backend.snapshot().row_text(row).trim_end().to_owned()
        }
    }

    #[cfg(unix)]
    #[test]
    fn output_lands_on_the_grid_and_the_exit_code_is_reported() {
        let mut run = Run::task("printf 'a\\nb\\n'; exit 3", 10, 40);
        let exit = run.wait_exit();
        assert_eq!(exit.code, Some(3));
        let screen: Vec<String> = (0..4).map(|r| run.row(r)).collect();
        assert_eq!(screen[0], "a", "{screen:?}");
        assert_eq!(screen[1], "b", "{screen:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_program_has_a_terminal() {
        let mut run = Run::task("test -t 0 && test -t 1 && echo IS_TTY", 5, 40);
        run.wait_exit();
        assert_eq!(run.row(0), "IS_TTY");
    }

    #[cfg(unix)]
    #[test]
    fn an_answer_typed_into_the_pane_reaches_the_program() {
        let mut run = Run::task("printf 'Proceed? '; read answer; echo got:$answer", 5, 40);
        run.wait("the question", |_, c| c.row_text(0).starts_with("Proceed?"));
        run.session.write(&b"yes\r"[..]);
        run.wait_exit();
        let screen: Vec<String> = (0..5).map(|r| run.row(r)).collect();
        assert!(screen.iter().any(|r| r == "got:yes"), "{screen:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_program_sees_the_size_it_was_started_with_and_a_resize() {
        let mut run = Run::task("stty size; read x; stty size", 7, 33);
        run.wait("the first size", |_, c| c.row_text(0).starts_with("7 33"));
        assert!(run.session.resize(TermSize::new(12, 50, 8, 16)));
        run.backend.resize(12, 50);
        run.session.write(&b"\r"[..]);
        run.wait_exit();
        let screen: Vec<String> = (0..12).map(|r| run.row(r)).collect();
        assert!(
            screen.iter().any(|r| r == "12 50"),
            "the program was not told: {screen:?}"
        );
        assert!(
            !run.session.resize(TermSize::new(12, 50, 8, 16)),
            "same size is not a change"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_environment_is_the_childs_and_ours_is_untouched() {
        let before = std::env::var("TERM").ok();
        let mut run = Run::task("echo $TERM:$COLORTERM:$TERM_PROGRAM:$SHLVL", 5, 60);
        run.wait_exit();
        assert_eq!(run.row(0), "xterm-256color:truecolor:REactor:0");
        assert_eq!(std::env::var("TERM").ok(), before);
        assert_ne!(
            std::env::var("COLORTERM").ok().as_deref(),
            Some("never-set-by-us")
        );
    }

    #[cfg(unix)]
    #[test]
    fn closing_hangs_up_on_the_program_and_it_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let script = format!("echo $$ > {}; exec sleep 60", pid_file.display());
        let run = Run::task(&script, 5, 40);
        let until = Instant::now() + Duration::from_secs(10);
        let pid: i32 = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|s| s.trim().parse().ok())
            {
                break pid;
            }
            assert!(Instant::now() < until, "the program did not start");
            std::thread::sleep(Duration::from_millis(10));
        };
        let proc = PathBuf::from(format!("/proc/{pid}"));
        let Run {
            backend, session, ..
        } = run;
        drop(session);
        let mut events = Vec::new();
        let until = Instant::now() + Duration::from_secs(10);
        while !events.iter().any(|e| matches!(e, Event::Closed { .. })) {
            events.extend(backend.inbox().take(64));
            assert!(Instant::now() < until, "the loop did not end: {events:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
        let until = Instant::now() + Duration::from_secs(10);
        while proc.exists() {
            assert!(Instant::now() < until, "process {pid} is still there");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, Event::Closed { panicked: true })),
            "the loop ended by itself"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_program_that_is_not_there_is_an_error_not_a_hang() {
        let backend = Backend::new(5, 40, &Settings::default());
        let kind = Kind::Task {
            program: "/definitely/not/a/program".into(),
            args: vec![],
        };
        let err = Session::start(&backend, &kind, None, TermSize::new(5, 40, 8, 16), None)
            .err()
            .expect("an error");
        assert!(
            err.to_string().contains("/definitely/not/a/program"),
            "{err}"
        );
    }

    /// The screen is read (as the panel does on every wakeup) while a program prints its last
    /// line and exits. The crate reads the bytes, finds the grid locked by the reader of the
    /// screen, goes on to the next read, which fails because the child is gone, and returns
    /// without parsing what it already read.
    #[cfg(unix)]
    #[test]
    fn the_last_output_is_not_lost_when_the_screen_is_read_as_the_program_exits() {
        let mut lost = 0;
        let runs = 15;
        for i in 0..runs {
            let mut run = Run::task(&format!("echo last-line-{i}"), 4, 40);
            let backend = run.backend.clone();
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let reader = {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let _ = backend.snapshot();
                    }
                })
            };
            run.wait_exit();
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            reader.join().unwrap();
            if run.row(0) != format!("last-line-{i}") {
                lost += 1;
            }
        }
        eprintln!("tail lost in {lost} of {runs} runs");
        assert_eq!(lost, 0, "the output of {lost} of {runs} programs was lost");
    }

    #[cfg(unix)]
    #[test]
    fn a_flood_of_output_is_parsed_and_the_exit_still_arrives() {
        let mut run = Run::task("yes 0123456789abcdef | head -c 20000000", 24, 80);
        let started = Instant::now();
        let exit = run.wait_exit();
        assert_eq!(exit.code, Some(0));
        eprintln!("20 MB parsed in {:?}", started.elapsed());
        assert!(started.elapsed() < Duration::from_secs(30));
    }
}
