//! Everything that touches the outside world during an install: the terminal,
//! child processes, the package manager's index. Behind a trait so the install
//! logic is testable without running `pacman`.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::catalogue::Catalogue;
use crate::recipes::available_managers;
use crate::util::{run, run_in, stdin_is_tty};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// The child writes to our stdout/stderr live.
    Inherit,
    /// stdout+stderr merged and returned (json mode: an installer writing to the
    /// inherited stdout would corrupt the payload).
    Both,
    /// Only stderr returned (pip: its PEP 668 refusal is worth detecting).
    Stderr,
}

#[derive(Debug, Clone)]
pub struct Exec {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub capture: Capture,
}

#[derive(Debug, Clone, Default)]
pub struct ExecOutcome {
    pub code: i32,
    /// Merged output for [`Capture::Both`], stderr for [`Capture::Stderr`].
    pub captured: String,
}

pub trait Host {
    /// Ask a yes/no question. Must be `false` where there is no one to ask.
    fn confirm(&self, prompt: &str) -> bool;
    /// A line to stdout. Only called in text mode.
    fn print(&self, line: &str);
    fn eprint(&self, text: &str);
    fn exec(&self, req: &Exec) -> ExecOutcome;
    /// Declared managers that are actually here.
    fn managers(&self, cat: &Catalogue) -> BTreeMap<String, String> {
        available_managers(cat)
    }
    /// The `python3.x` packages this manager's *own* repos offer.
    fn discover_python(&self, manager: &str) -> Vec<String>;
    fn json(&self) -> bool;
}

/// The real thing.
pub struct SystemHost {
    pub json: bool,
    pub yes: bool,
}

impl Host for SystemHost {
    fn confirm(&self, prompt: &str) -> bool {
        if self.yes {
            return true;
        }
        if self.json || !stdin_is_tty() {
            return false;
        }
        print!("{prompt} [y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => {
                println!();
                false
            }
            Ok(_) => matches!(line.trim().to_lowercase().as_str(), "y" | "yes"),
        }
    }

    fn print(&self, line: &str) {
        println!("{line}");
    }

    fn eprint(&self, text: &str) {
        eprint!("{text}");
    }

    fn exec(&self, req: &Exec) -> ExecOutcome {
        match req.capture {
            Capture::Both => {
                let r = run_in(
                    &req.argv,
                    req.cwd.as_deref(),
                    Duration::from_secs(10 * 365 * 86_400),
                );
                ExecOutcome {
                    code: r.code.unwrap_or(-1),
                    captured: r.output,
                }
            }
            Capture::Inherit | Capture::Stderr => {
                let Some((program, rest)) = req.argv.split_first() else {
                    return ExecOutcome {
                        code: 127,
                        captured: String::new(),
                    };
                };
                let mut cmd = Command::new(program);
                cmd.args(rest);
                if let Some(dir) = &req.cwd {
                    cmd.current_dir(dir);
                }
                if req.capture == Capture::Stderr {
                    cmd.stderr(Stdio::piped());
                }
                match cmd.output_or_status(req.capture) {
                    Ok(o) => o,
                    Err(e) => ExecOutcome {
                        code: 127,
                        captured: format!("{program}: {e}\n"),
                    },
                }
            }
        }
    }

    fn discover_python(&self, manager: &str) -> Vec<String> {
        crate::install::discover_python_packages(manager)
    }

    fn json(&self) -> bool {
        self.json
    }
}

trait CommandExt {
    fn output_or_status(&mut self, capture: Capture) -> std::io::Result<ExecOutcome>;
}

impl CommandExt for Command {
    fn output_or_status(&mut self, capture: Capture) -> std::io::Result<ExecOutcome> {
        if capture == Capture::Stderr {
            let out = self.stdout(Stdio::inherit()).spawn()?.wait_with_output()?;
            Ok(ExecOutcome {
                code: out.status.code().unwrap_or(-1),
                captured: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        } else {
            let status = self.status()?;
            Ok(ExecOutcome {
                code: status.code().unwrap_or(-1),
                captured: String::new(),
            })
        }
    }
}

/// Used by discovery: index queries against a package database, slower than a
/// probe but not worth a config knob for one pseudo-target.
pub(crate) const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn discovery_run(argv: &[&str]) -> crate::util::Run {
    run(
        &argv.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        DISCOVERY_TIMEOUT,
    )
}
