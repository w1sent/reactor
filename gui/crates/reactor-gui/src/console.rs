//! The console's process plumbing: run a command, stream its output, type
//! back into it.
//!
//! This is a general command runner, not an installer. `reactor install <id>`
//! from the Tools panel is one producer of commands; anything the user types
//! is another. Both land in the same session, which keeps the interactive
//! half honest — an installer that stops to ask "Proceed? [y/N]" is answered
//! by the same input box that runs `ls`.
//!
//! Streaming rules, and why:
//!
//! - **Bytes, not lines.** A prompt arrives *without* a trailing newline
//!   (`reactor install` ends its plan with a bare question), so a
//!   line-buffered reader would hold it back until after the user had
//!   already had to answer it. Chunks are forwarded the moment they arrive.
//! - **Carriage returns overwrite.** Progress bars (`brew`, `pip`) redraw a
//!   line with `\r`; treating it as text would fill the pane with hundreds
//!   of near-identical lines.
//! - **No async runtime.** Reader threads and a channel, the same shape
//!   `reactor-rpc` uses for pi's stdout (gui/SPEC.md §2) — the GUI drains
//!   the channel on the tick it already runs.
//!
//! What this deliberately is *not*: a terminal emulator. There is no pty, so
//! a program that demands a tty (`sudo`'s password prompt, a full-screen
//! curses UI) will not behave. Pipe-level interaction — the y/N class of
//! question, which is what the install path actually asks — does work. A pty
//! is the upgrade path if that ceases to be enough.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;

/// How many lines the pane keeps. A long `brew install` is loud; the cap
/// keeps the buffer bounded without the user ever noticing it.
const MAX_LINES: usize = 5_000;

/// One thing a running command can tell us.
#[derive(Debug)]
enum Chunk {
    /// Output bytes, as they arrived — may end mid-line.
    Text(String),
    /// One of the two readers reached EOF.
    ReaderDone,
}

/// A command running (or finished) under the console.
pub struct ConsoleSession {
    child: Child,
    stdin: Option<ChildStdin>,
    chunks: mpsc::Receiver<Chunk>,
    readers_done: usize,
    exit: Option<Option<i32>>,
}

impl ConsoleSession {
    /// Run `command` through the user's shell, so pipes, `&&` and globs work
    /// the way they do in a terminal.
    pub fn shell(command: &str, cwd: Option<PathBuf>) -> std::io::Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        let mut spawn = Command::new(shell);
        spawn.arg("-c").arg(command);
        Self::spawn(spawn, cwd)
    }

    /// Run one program directly, without a shell between — for commands the
    /// GUI builds itself (`reactor install <id>`), where the arguments are
    /// already separated and must not be re-split by shell quoting rules.
    pub fn program(program: &str, args: &[String], cwd: Option<PathBuf>) -> std::io::Result<Self> {
        let mut spawn = Command::new(program);
        spawn.args(args);
        Self::spawn(spawn, cwd)
    }

    fn spawn(mut spawn: Command, cwd: Option<PathBuf>) -> std::io::Result<Self> {
        if let Some(cwd) = cwd {
            spawn.current_dir(cwd);
        }
        let mut child = spawn
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let (tx, chunks) = mpsc::channel();
        // stdout and stderr both stream into one channel: the pane shows the
        // command's output as the terminal would, interleaved in arrival
        // order rather than split into two panes.
        if let Some(stdout) = child.stdout.take() {
            pump_reader(stdout, tx.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            pump_reader(stderr, tx.clone());
        }
        drop(tx);

        Ok(Self {
            stdin: child.stdin.take(),
            child,
            chunks,
            readers_done: 0,
            exit: None,
        })
    }

    /// Answer a prompt: one line, newline included, straight to the
    /// command's stdin.
    pub fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the command is not accepting input",
            ));
        };
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    /// Close the command's stdin — an EOF, for a program reading until one.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// Drain whatever has arrived since the last call into `sink`, and
    /// report the exit code once the command is finished *and* its output is
    /// fully drained — never before, or the pane would cut off the last
    /// lines of a command that exits promptly.
    pub fn drain(&mut self, sink: &mut ConsoleBuffer) -> Option<Option<i32>> {
        let mut disconnected = false;
        loop {
            match self.chunks.try_recv() {
                Ok(Chunk::Text(text)) => sink.push(&text),
                Ok(Chunk::ReaderDone) => self.readers_done += 1,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        if self.exit.is_none() {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.exit = Some(status.code());
            }
        }
        // Both halves have to be true: the process is gone and nothing is
        // still in flight behind it.
        match (self.exit, disconnected || self.readers_done >= 2) {
            (Some(code), true) => Some(code),
            _ => None,
        }
    }
}

impl Drop for ConsoleSession {
    /// A command outliving the window it was started from would keep running
    /// unattended, with nothing left to show its output.
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn pump_reader(mut source: impl Read + Send + 'static, tx: mpsc::Sender<Chunk>) {
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match source.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let text = String::from_utf8_lossy(&buf[..read]).into_owned();
                    if tx.send(Chunk::Text(text)).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(Chunk::ReaderDone);
    });
}

/// The pane's text: whole lines, with the last one still open for the next
/// chunk to extend (that is how a prompt with no trailing newline shows up
/// before it is answered).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ConsoleBuffer {
    lines: Vec<String>,
}

impl ConsoleBuffer {
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// A whole line of the GUI's own (the echoed command, an exit notice).
    pub fn push_line(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
        self.trim();
    }

    /// Append output as it arrived, honouring newlines and carriage returns.
    pub fn push(&mut self, chunk: &str) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        for (at, piece) in chunk.split('\n').enumerate() {
            if at > 0 {
                self.lines.push(String::new());
            }
            let current = self
                .lines
                .last_mut()
                .expect("a line exists: one was pushed above");
            match piece.rsplit_once('\r') {
                // A carriage return redraws the line: keep only what follows
                // the last one, the way a terminal would.
                Some((_, after)) => {
                    current.clear();
                    current.push_str(after);
                }
                None => current.push_str(piece),
            }
        }
        self.trim();
    }

    fn trim(&mut self) {
        if self.lines.len() > MAX_LINES {
            let excess = self.lines.len() - MAX_LINES;
            self.lines.drain(..excess);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_assemble_into_lines_and_leave_the_last_one_open() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push("one\ntw");
        assert_eq!(buffer.lines(), ["one", "tw"]);
        // The open line keeps growing — a prompt with no trailing newline
        // shows up as soon as it arrives, not after the answer.
        buffer.push("o\n");
        assert_eq!(buffer.lines(), ["one", "two", ""]);
        buffer.push("Proceed? [y/N] ");
        assert_eq!(buffer.lines(), ["one", "two", "Proceed? [y/N] "]);
    }

    #[test]
    fn a_carriage_return_redraws_the_line_instead_of_stacking_up() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push("downloading  1%");
        buffer.push("\rdownloading 50%");
        buffer.push("\rdownloading 100%");
        assert_eq!(buffer.lines(), ["downloading 100%"]);
    }

    #[test]
    fn the_buffer_stays_bounded() {
        let mut buffer = ConsoleBuffer::default();
        for line in 0..MAX_LINES + 100 {
            buffer.push_line(format!("line {line}"));
        }
        assert_eq!(buffer.lines().len(), MAX_LINES);
        assert_eq!(buffer.lines()[0], format!("line {}", 100));
    }

    /// The whole loop against a real child: output streams out, the exit
    /// code arrives only once the output has been drained.
    #[test]
    fn a_command_streams_its_output_and_then_reports_its_exit() {
        let mut session = ConsoleSession::shell("printf 'a\\nb'; exit 3", None).unwrap();
        let mut buffer = ConsoleBuffer::default();
        let mut exit = None;
        for _ in 0..200 {
            if let Some(code) = session.drain(&mut buffer) {
                exit = Some(code);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(exit, Some(Some(3)), "the exit code reaches the caller");
        assert_eq!(buffer.lines(), ["a", "b"]);
    }

    /// The interactive half: what the user types reaches the command's stdin.
    #[test]
    fn typed_input_reaches_the_command() {
        let mut session = ConsoleSession::shell("read answer; echo \"got:$answer\"", None).unwrap();
        session.write_line("yes").unwrap();
        let mut buffer = ConsoleBuffer::default();
        for _ in 0..200 {
            if session.drain(&mut buffer).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            buffer.lines().iter().any(|line| line == "got:yes"),
            "the command saw the typed line, got {:?}",
            buffer.lines()
        );
    }
}
