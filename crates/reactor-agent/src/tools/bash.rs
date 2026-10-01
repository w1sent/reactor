//! `bash`: the tool an RE agent lives in.
//!
//! **Persistent sessions.** A named shell stays alive between calls, so `cd`, exports,
//! a half-built pipeline of variables and an attached `gdb`-in-a-fifo survive from one
//! call to the next — reverse engineering is stateful in exactly the way a fresh
//! `bash -c` per call is not. `session` picks one (default `main`); `restart: true`
//! starts it over.
//!
//! **Streaming.** Output reaches `ctx.emit` line by line while the command runs, so a
//! frontend shows a long `strings` or a build as it happens.
//!
//! **Bounded, and lossless.** Memory holds at most a few MiB. Past that the whole
//! output spills to a file (which the loop keeps as the entry's blob) and only the
//! head and tail are held for the model — see [`crate::truncate`].
//!
//! A command that outlives its timeout, or a cancelled turn, kills the shell's whole
//! process group and drops the session: a hung child cannot be interrupted out of a
//! shell that is still reading its stdin, so the honest options are to wait or to
//! restart, and the result says which happened and what was lost.

use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::{BoxFut, Tool, ToolCtx, ToolOutput, str_arg, usize_arg};
use crate::llm::ToolSpec;
use crate::truncate::{CUT, HEAD_BYTES, TAIL_BYTES};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 3_600;
/// Hold this much in memory before spilling to disk.
const SPILL_AT: usize = 4 * 1024 * 1024;

pub struct Bash {
    cwd: PathBuf,
    sessions: tokio::sync::Mutex<HashMap<String, Shell>>,
    counter: AtomicU64,
}

impl Bash {
    pub fn new(cwd: PathBuf) -> Self {
        Bash {
            cwd,
            sessions: tokio::sync::Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
        }
    }
}

struct Shell {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Shell {
    async fn spawn(cwd: &Path) -> std::io::Result<Shell> {
        let mut cmd = Command::new("bash");
        cmd.args(["--noprofile", "--norc"])
            .current_dir(cwd)
            .env("TERM", "dumb")
            .env("PAGER", "cat")
            .env("GIT_PAGER", "cat")
            .env("PS1", "")
            .env("PS2", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn()?;
        let mut stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        // One stream for the model: stderr joins stdout for good, no job-control chatter.
        stdin
            .write_all(b"exec 2>&1\nset +m\nunset HISTFILE\n")
            .await?;
        Ok(Shell {
            child,
            stdin,
            stdout,
        })
    }

    fn kill_group(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            // SAFETY: signalling a process group we created; no memory is touched.
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
        let _ = self.child.start_kill();
    }
}

/// Head, tail and (past a point) the whole of a command's output.
struct Capture {
    mem: Vec<u8>,
    total: usize,
    spilled: Option<(std::fs::File, PathBuf)>,
    head: Vec<u8>,
    tail: VecDeque<u8>,
}

impl Capture {
    fn new() -> Self {
        Capture {
            mem: Vec::new(),
            total: 0,
            spilled: None,
            head: Vec::new(),
            tail: VecDeque::new(),
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        if let Some((file, _)) = &mut self.spilled {
            let _ = file.write_all(bytes);
            self.tail.extend(bytes.iter().copied());
            let excess = self.tail.len().saturating_sub(2 * TAIL_BYTES);
            self.tail.drain(..excess);
            return;
        }
        self.mem.extend_from_slice(bytes);
        if self.mem.len() > SPILL_AT {
            let path = std::env::temp_dir().join(format!(
                "reactor-out-{}-{}.txt",
                std::process::id(),
                unique()
            ));
            if let Ok(mut file) = std::fs::File::create(&path) {
                let _ = file.write_all(&self.mem);
                self.head = self.mem[..HEAD_BYTES.min(self.mem.len())].to_vec();
                let start = self.mem.len().saturating_sub(2 * TAIL_BYTES);
                self.tail = self.mem[start..].iter().copied().collect();
                self.mem = Vec::new();
                self.spilled = Some((file, path));
            }
        }
    }

    /// The text for the loop and the file holding all of it, if it was too big.
    fn finish(mut self) -> (String, Option<PathBuf>) {
        match self.spilled.take() {
            None => (String::from_utf8_lossy(&self.mem).into_owned(), None),
            Some((mut file, path)) => {
                let _ = file.flush();
                let tail: Vec<u8> = self.tail.iter().copied().collect();
                let tail = &tail[tail.len().saturating_sub(TAIL_BYTES)..];
                let text = format!(
                    "{}{CUT}{}",
                    String::from_utf8_lossy(&self.head),
                    String::from_utf8_lossy(tail)
                );
                (text, Some(path))
            }
        }
    }
}

fn unique() -> u64 {
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

enum Ended {
    Exit(i32),
    TimedOut,
    Cancelled,
    /// The shell went away (killed, `exit`, a syntax error in a non-interactive shell).
    Died,
}

async fn run_in(
    shell: &mut Shell,
    command: &str,
    marker: &str,
    timeout: Duration,
    ctx: &ToolCtx,
    cap: &mut Capture,
) -> Ended {
    // Group, not subshell, so `cd` and `export` persist; stdin from /dev/null so a
    // command that reads it cannot swallow the lines that follow.
    let script =
        format!("{{ {command}\n}} </dev/null\n__rc=$?\nprintf '\\n%s%d\\n' '{marker}' \"$__rc\"\n");
    if shell.stdin.write_all(script.as_bytes()).await.is_err() || shell.stdin.flush().await.is_err()
    {
        return Ended::Died;
    }
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut line = Vec::new();
    loop {
        line.clear();
        tokio::select! {
            _ = ctx.cancel.cancelled() => return Ended::Cancelled,
            _ = &mut deadline => return Ended::TimedOut,
            read = shell.stdout.read_until(b'\n', &mut line) => match read {
                Ok(0) | Err(_) => return Ended::Died,
                Ok(_) => {
                    if let Some(rest) = line.strip_prefix(marker.as_bytes()) {
                        let code = String::from_utf8_lossy(rest).trim().parse().unwrap_or(-1);
                        return Ended::Exit(code);
                    }
                    (ctx.emit)(&String::from_utf8_lossy(&line));
                    cap.push(&line);
                }
            },
        }
    }
}

impl Tool for Bash {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".into(),
            description: format!(
                "Run a shell command and return its combined stdout and stderr. The shell is persistent: cd, exports and variables survive between calls (the `session` name picks one; `restart: true` starts it over). \
                 Long output is cut to its head and tail with a note naming the entry that holds all of it -- read the rest with history_read, or search it with history_search. \
                 A command gets {DEFAULT_TIMEOUT_SECS}s unless `timeout_secs` says otherwise (max {MAX_TIMEOUT_SECS}); one that outlives it is killed along with the shell session, so run anything long-lived in the background (`cmd > out.log 2>&1 &`) and poll. Commands cannot read stdin."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command line to run." },
                    "session": { "type": "string", "description": "Shell session name (default \"main\")." },
                    "timeout_secs": { "type": "integer", "description": "Kill the command after this many seconds." },
                    "restart": { "type": "boolean", "description": "Start this session over first." }
                },
                "required": ["command"]
            }),
        }
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(command) = str_arg(&args, "command") else {
                return ToolOutput::err("bash needs a `command` string");
            };
            let session = str_arg(&args, "session").unwrap_or("main").to_string();
            let timeout = Duration::from_secs(
                usize_arg(&args, "timeout_secs")
                    .map(|s| s as u64)
                    .unwrap_or(DEFAULT_TIMEOUT_SECS)
                    .clamp(1, MAX_TIMEOUT_SECS),
            );
            let restart = args
                .get("restart")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let marker = format!(
                "__REACTOR_DONE_{}_{}__",
                std::process::id(),
                self.counter.fetch_add(1, Ordering::Relaxed)
            );

            let mut sessions = self.sessions.lock().await;
            if restart && let Some(mut old) = sessions.remove(&session) {
                old.kill_group();
            }
            if !sessions.contains_key(&session) {
                match Shell::spawn(&self.cwd).await {
                    Ok(s) => {
                        sessions.insert(session.clone(), s);
                    }
                    Err(e) => return ToolOutput::err(format!("could not start bash: {e}")),
                }
            }
            let shell = sessions.get_mut(&session).expect("just ensured");

            let mut cap = Capture::new();
            let ended = run_in(shell, command, &marker, timeout, ctx, &mut cap).await;

            let (mut notes, mut is_error) = (String::new(), false);
            match ended {
                Ended::Exit(0) => {}
                Ended::Exit(code) => notes = format!("[exit code {code}]"),
                Ended::TimedOut | Ended::Cancelled | Ended::Died => {
                    let why = match ended {
                        Ended::TimedOut => format!("timed out after {}s", timeout.as_secs()),
                        Ended::Cancelled => "cancelled".to_string(),
                        _ => {
                            // `exit` inside a command ends the persistent shell itself.
                            let status = match sessions.get_mut(&session) {
                                Some(s) => {
                                    tokio::time::timeout(Duration::from_millis(300), s.child.wait())
                                        .await
                                        .ok()
                                        .and_then(|r| r.ok())
                                }
                                None => None,
                            };
                            match status.and_then(|s| s.code()) {
                                Some(code) => format!("the shell exited (code {code})"),
                                None => "the shell exited".to_string(),
                            }
                        }
                    };
                    if let Some(mut dead) = sessions.remove(&session) {
                        dead.kill_group();
                    }
                    notes = format!(
                        "[{why}; session \"{session}\" was reset -- its working directory, variables and background jobs are gone]"
                    );
                    is_error = true;
                }
            }
            drop(sessions);

            let (mut text, full) = cap.finish();
            // The marker's own leading newline is not the command's output, and a
            // trailing newline is only the last line's terminator.
            if !text.contains(CUT) {
                text.truncate(text.trim_end_matches('\n').len());
            }
            if !notes.is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&notes);
            }
            ToolOutput {
                text,
                is_error,
                full,
            }
        })
    }
}
