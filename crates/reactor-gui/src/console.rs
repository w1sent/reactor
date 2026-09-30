//! The console's process plumbing: run a command on a pty, stream what it
//! draws, type back into it.
//!
//! This is a general command runner, not an installer. `reactor install <id>`
//! from the Tools panel is one producer of commands; anything the user types
//! is another. Both land in the same session, which keeps the interactive
//! half honest — an installer that stops to ask "Proceed? [y/N]" is answered
//! by the same input box that runs `ls`.
//!
//! **Why a pty and not pipes** (SPEC.md §9, v0.2): a program asks
//! `isatty()` before it decides how to behave. On a pipe, `sudo` refuses to
//! prompt at all (`no tty present and no askpass program specified`), which
//! took out every Linux distro manager — `tools.toml` marks `pacman`, `apt`,
//! `dnf`, `zypper`, `apk` and `port` as needing root. On a pty they prompt,
//! and the answer typed here reaches them.
//!
//! The cost of that is on the way back: a program talking to a terminal
//! emits escape sequences. [`AnsiReader`] handles the ones that carry
//! meaning for a log — colour, and the line-rewriting a progress bar does —
//! and drops the rest rather than printing it. What it deliberately does not
//! do is emulate a screen: no cursor addressing, no scroll regions, no
//! alternate screen, so a program that paints a full UI (`vim`, `htop`) will
//! not render. That is v0.3's grid, and it is a different component.
//!
//! No async runtime: a reader thread and a channel, drained by the GUI on the tick
//! it already runs.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// How many lines the pane keeps. A long `brew install` is loud; the cap
/// keeps the buffer bounded without the user ever noticing it.
const MAX_LINES: usize = 5_000;

/// The terminal size claimed to the program. Nothing re-flows on resize
/// here — the pane wraps long lines itself — but the value is not
/// arbitrary: it is what a program asks for when it decides how wide to
/// draw a table or a progress bar, and 80 would make most output cramped.
const PTY_SIZE: PtySize = PtySize {
    rows: 40,
    cols: 120,
    pixel_width: 0,
    pixel_height: 0,
};

/// One thing a running command can tell us.
#[derive(Debug)]
enum Chunk {
    /// Bytes as they arrived — may end mid-line, or mid-escape.
    Bytes(String),
    /// The pty reached EOF.
    ReaderDone,
}

/// A command running (or finished) on a pty.
pub struct ConsoleSession {
    child: Box<dyn Child + Send + Sync>,
    writer: Option<Box<dyn Write + Send>>,
    /// Kept alive for the lifetime of the session: dropping the master
    /// closes the pty, and the child would lose its terminal mid-run.
    _master: Box<dyn MasterPty + Send>,
    chunks: mpsc::Receiver<Chunk>,
    reader_done: bool,
    exit: Option<Option<i32>>,
}

impl ConsoleSession {
    /// Run `command` through the user's shell, so pipes, `&&` and globs work
    /// the way they do in a terminal.
    pub fn shell(command: &str, cwd: Option<PathBuf>) -> anyhow::Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        let mut build = CommandBuilder::new(shell);
        build.arg("-c");
        build.arg(command);
        Self::spawn(build, cwd)
    }

    /// Run one program directly, without a shell between — for commands the
    /// GUI builds itself (`reactor install <id>`), where the arguments are
    /// already separated and must not be re-split by shell quoting rules.
    pub fn program(program: &str, args: &[String], cwd: Option<PathBuf>) -> anyhow::Result<Self> {
        let mut build = CommandBuilder::new(program);
        for arg in args {
            build.arg(arg);
        }
        Self::spawn(build, cwd)
    }

    fn spawn(mut build: CommandBuilder, cwd: Option<PathBuf>) -> anyhow::Result<Self> {
        if let Some(cwd) = cwd {
            build.cwd(cwd);
        }
        // Tell the program what it is talking to. Without this it either
        // assumes something ancient or gives up on colour entirely.
        build.env("TERM", "xterm-256color");

        let pair = native_pty_system().openpty(PTY_SIZE)?;
        let child = pair.slave.spawn_command(build)?;
        // The slave is the child's end; holding it open would keep the pty
        // from ever reporting EOF once the child exits.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (tx, chunks) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        let text = String::from_utf8_lossy(&buf[..read]).into_owned();
                        if tx.send(Chunk::Bytes(text)).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = tx.send(Chunk::ReaderDone);
        });

        Ok(Self {
            child,
            writer: Some(writer),
            _master: pair.master,
            chunks,
            reader_done: false,
            exit: None,
        })
    }

    /// Answer a prompt: one line, newline included.
    ///
    /// No local echo is wanted alongside this — the pty's line discipline
    /// echoes what is written to it, so the typed answer appears in the
    /// output on its own, exactly where the program asked for it.
    pub fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the command is not accepting input",
            ));
        };
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()
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
                Ok(Chunk::Bytes(text)) => sink.push(&text),
                Ok(Chunk::ReaderDone) => self.reader_done = true,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        if self.exit.is_none()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.exit = Some(Some(status.exit_code() as i32));
        }
        // Both halves have to be true: the process is gone and nothing is
        // still in flight behind it.
        match (self.exit, disconnected || self.reader_done) {
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

/// One run of text sharing a colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    /// An ANSI colour index (0-15), resolved against the theme by the panel
    /// so this module stays independent of it.
    pub color: Option<u8>,
    pub bold: bool,
}

/// One rendered line: the spans it is made of.
pub type Line = Vec<Span>;

/// The pane's text: whole lines of coloured spans, with the last line still
/// open for the next chunk to extend (that is how a prompt with no trailing
/// newline shows up before it is answered).
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleBuffer {
    lines: Vec<Line>,
    reader: AnsiReader,
    /// Whether the last line is still open for a `write` to extend.
    ///
    /// `push_line` closes it: without this, the first byte of a command's
    /// own output landed on the *same* line as the GUI's own `$ command`
    /// echo above it (`write` always extends `self.lines.last_mut()`,
    /// oblivious to whether that line was a finished GUI-authored line or a
    /// still-open one from a previous chunk) — `whoami` then `user` arrived
    /// as one line, `whoamiuser`.
    open_for_write: bool,
}

impl Default for ConsoleBuffer {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            reader: AnsiReader::default(),
            open_for_write: true,
        }
    }
}

impl ConsoleBuffer {
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.reader = AnsiReader::default();
        self.open_for_write = true;
    }

    /// A whole line of the GUI's own (the echoed command, an exit notice),
    /// in the terminal's default colour.
    ///
    /// Always its own line, sealed on arrival: the next `write` (a command's
    /// first byte of output) starts a fresh line rather than extending this
    /// one, however this line ends up sharing its colour and weight.
    pub fn push_line(&mut self, line: impl Into<String>) {
        self.lines.push(vec![Span {
            text: line.into(),
            color: None,
            bold: false,
        }]);
        self.open_for_write = false;
        self.trim();
    }

    /// Append output as it arrived, reading escapes out of it.
    pub fn push(&mut self, chunk: &str) {
        let mut reader = std::mem::take(&mut self.reader);
        reader.feed(chunk, self);
        self.reader = reader;
        self.trim();
    }

    /// The plain text of each line — what a test asserts on, and what a
    /// future "copy output" would put on the clipboard.
    pub fn plain_lines(&self) -> Vec<String> {
        self.lines
            .iter()
            .map(|line| line.iter().map(|span| span.text.as_str()).collect())
            .collect()
    }

    fn write(&mut self, text: &str, color: Option<u8>, bold: bool) {
        if text.is_empty() {
            return;
        }
        if self.lines.is_empty() || !self.open_for_write {
            self.lines.push(Vec::new());
            self.open_for_write = true;
        }
        let line = self.lines.last_mut().expect("a line exists");
        match line.last_mut() {
            // Growing the run in place keeps one span per colour change
            // rather than one per read, which is what makes rendering cheap.
            Some(span) if span.color == color && span.bold == bold => span.text.push_str(text),
            _ => line.push(Span {
                text: text.to_owned(),
                color,
                bold,
            }),
        }
    }

    fn newline(&mut self) {
        self.lines.push(Vec::new());
        self.open_for_write = true;
    }

    /// Throw away the current line's content, keeping the line itself — what
    /// a carriage return or an erase-line means for a log that has no cursor
    /// to move.
    fn erase_line(&mut self) {
        if let Some(line) = self.lines.last_mut() {
            line.clear();
        }
    }

    fn trim(&mut self) {
        if self.lines.len() > MAX_LINES {
            let excess = self.lines.len() - MAX_LINES;
            self.lines.drain(..excess);
        }
    }
}

/// Reads the escape sequences a program writes to a terminal, keeping the
/// few that mean something to a log and dropping the rest.
///
/// Kept minimal on purpose (SPEC.md §9): colour (SGR), the line-rewriting
/// of a progress bar (`\r` and erase-line), and nothing else. Anything that
/// moves a cursor around a screen is consumed and ignored rather than
/// printed — dropping it silently is what keeps output readable without
/// pretending to be a terminal.
///
/// Carries its state across chunks, because a sequence can be split by a
/// read boundary.
#[derive(Debug, Default, Clone, PartialEq)]
struct AnsiReader {
    state: State,
    /// Parameter bytes of the sequence being read.
    params: String,
    color: Option<u8>,
    bold: bool,
    /// A carriage return was seen and nothing has been written since: the
    /// next text starts the line over. Deferred rather than applied at once
    /// so a trailing `\r\n` ends the line instead of blanking it.
    overwrite: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Text,
    /// Saw `ESC`, waiting to learn what kind of sequence this is.
    Escape,
    /// Inside `ESC [ … final`, collecting parameters.
    Csi,
    /// Inside `ESC ] … BEL|ST` — a window title and the like. Skipped whole.
    Osc,
}

impl AnsiReader {
    fn feed(&mut self, chunk: &str, out: &mut ConsoleBuffer) {
        let mut text = String::new();
        for ch in chunk.chars() {
            match self.state {
                State::Text => match ch {
                    '\x1b' => {
                        self.flush(&mut text, out);
                        self.state = State::Escape;
                    }
                    '\n' => {
                        self.flush(&mut text, out);
                        out.newline();
                        self.overwrite = false;
                    }
                    '\r' => {
                        self.flush(&mut text, out);
                        self.overwrite = true;
                    }
                    // Other C0 controls (bell, backspace, …) carry no
                    // meaning for a log and would render as boxes.
                    c if (c as u32) < 0x20 && c != '\t' => {}
                    c => text.push(c),
                },
                State::Escape => {
                    self.params.clear();
                    match ch {
                        '[' => self.state = State::Csi,
                        ']' => self.state = State::Osc,
                        // A two-byte escape (charset selection, keypad
                        // mode): one byte and it is over.
                        _ => self.state = State::Text,
                    }
                }
                State::Csi => {
                    if ('\x40'..='\x7e').contains(&ch) {
                        self.apply_csi(ch, out);
                        self.state = State::Text;
                        self.params.clear();
                    } else {
                        self.params.push(ch);
                    }
                }
                State::Osc => {
                    // Terminated by BEL, or by ST (`ESC \`) whose ESC lands
                    // us back here on the next byte.
                    if ch == '\x07' || ch == '\x1b' {
                        self.state = State::Text;
                    }
                }
            }
        }
        self.flush(&mut text, out);
    }

    /// Commit pending text, honouring a deferred carriage return.
    fn flush(&mut self, text: &mut String, out: &mut ConsoleBuffer) {
        if text.is_empty() {
            return;
        }
        if self.overwrite {
            out.erase_line();
            self.overwrite = false;
        }
        out.write(text, self.color, self.bold);
        text.clear();
    }

    fn apply_csi(&mut self, final_byte: char, out: &mut ConsoleBuffer) {
        match final_byte {
            'm' => self.apply_sgr(),
            // Erase in line. A progress bar writes `\r`, erases, redraws;
            // with no cursor to be left of, every variant means the same
            // thing here.
            'K' => {
                out.erase_line();
                self.overwrite = false;
            }
            // Cursor moves, erase-in-display, scroll regions, mode set:
            // consumed so they do not print, ignored because honouring them
            // needs a screen (v0.3).
            _ => {}
        }
    }

    fn apply_sgr(&mut self) {
        // A bare `ESC[m` is a reset, same as `ESC[0m`.
        if self.params.is_empty() {
            self.color = None;
            self.bold = false;
            return;
        }
        // Private-parameter sequences (`ESC[?…m`) are not colour.
        if self.params.starts_with('?') {
            return;
        }
        let mut params = self.params.split(';').peekable();
        while let Some(param) = params.next() {
            match param.trim().parse::<u16>() {
                Ok(0) => {
                    self.color = None;
                    self.bold = false;
                }
                Ok(1) => self.bold = true,
                Ok(22) => self.bold = false,
                Ok(39) => self.color = None,
                // The eight normal and eight bright foreground colours.
                Ok(code @ 30..=37) => self.color = Some((code - 30) as u8),
                Ok(code @ 90..=97) => self.color = Some((code - 90 + 8) as u8),
                // 256-colour and truecolour: `38;5;n` and `38;2;r;g;b`.
                // The palette below 16 maps to the theme; anything richer is
                // flattened to the default rather than guessed at.
                Ok(38) => {
                    match params
                        .next()
                        .and_then(|kind| kind.trim().parse::<u16>().ok())
                    {
                        Some(5) => {
                            let index = params.next().and_then(|n| n.trim().parse::<u16>().ok());
                            self.color = index.filter(|n| *n < 16).map(|n| n as u8);
                        }
                        Some(2) => {
                            // r, g, b — consumed, not rendered.
                            for _ in 0..3 {
                                params.next();
                            }
                            self.color = None;
                        }
                        _ => self.color = None,
                    }
                }
                // Background colours and everything else: no effect on a
                // pane that paints its own background.
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(buffer: &ConsoleBuffer) -> Vec<String> {
        buffer.plain_lines()
    }

    #[test]
    fn chunks_assemble_into_lines_and_leave_the_last_one_open() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push("one\ntw");
        assert_eq!(plain(&buffer), ["one", "tw"]);
        buffer.push("o\n");
        assert_eq!(plain(&buffer), ["one", "two", ""]);
        // A prompt with no trailing newline shows up as soon as it arrives,
        // not after the answer.
        buffer.push("Proceed? [y/N] ");
        assert_eq!(plain(&buffer), ["one", "two", "Proceed? [y/N] "]);
    }

    #[test]
    fn a_carriage_return_redraws_the_line_instead_of_stacking_up() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push("downloading  1%");
        buffer.push("\rdownloading 50%");
        buffer.push("\rdownloading 100%");
        assert_eq!(plain(&buffer), ["downloading 100%"]);
    }

    /// The `\r\n` pairing must end the line, not blank it — which is why the
    /// carriage return is deferred until something is actually written.
    #[test]
    fn a_carriage_return_before_a_newline_keeps_the_line() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push("done\r\nnext");
        assert_eq!(plain(&buffer), ["done", "next"]);
    }

    #[test]
    fn colour_becomes_spans_and_escapes_never_reach_the_text() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push("\x1b[32mok\x1b[0m plain");
        assert_eq!(plain(&buffer), ["ok plain"]);
        let line = &buffer.lines()[0];
        assert_eq!(line[0].text, "ok");
        assert_eq!(line[0].color, Some(2), "SGR 32 is colour 2 (green)");
        assert_eq!(line[1].text, " plain");
        assert_eq!(line[1].color, None, "the reset clears it again");
    }

    #[test]
    fn bright_colour_and_bold_are_carried() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push("\x1b[1;91mloud\x1b[m");
        let span = &buffer.lines()[0][0];
        assert_eq!(span.text, "loud");
        assert_eq!(span.color, Some(9), "91 is bright red: 1 + 8");
        assert!(span.bold);
    }

    /// The reader survives a sequence split across reads — a pty hands over
    /// whatever happened to be ready, not whole sequences.
    #[test]
    fn an_escape_split_across_chunks_still_parses() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push("\x1b[3");
        buffer.push("2mgreen");
        assert_eq!(plain(&buffer), ["green"]);
        assert_eq!(buffer.lines()[0][0].color, Some(2));
    }

    /// Cursor moves and screen clears are consumed, not printed. Honouring
    /// them needs a grid, which is v0.3 — but they must never show up as
    /// literal text.
    #[test]
    fn sequences_that_need_a_screen_are_dropped_rather_than_printed() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push("\x1b[2J\x1b[H\x1b[3Aafter\x1b]0;a title\x07!");
        assert_eq!(plain(&buffer), ["after!"]);
    }

    #[test]
    fn erase_line_clears_what_the_progress_bar_drew() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push("a long line of progress");
        buffer.push("\r\x1b[Kshort");
        assert_eq!(plain(&buffer), ["short"]);
    }

    /// The bug: a command's own first line of output landed glued onto the
    /// GUI's `$ command` echo above it, because `write` always extended
    /// `lines.last_mut()` with no regard for whether that line was already
    /// finished. `whoami` then `user\n` must stay two lines, not become the
    /// single line `whoamiuser`.
    #[test]
    fn command_output_never_glues_onto_the_echoed_command_line() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push_line("$ whoami");
        buffer.push("user\n");
        assert_eq!(plain(&buffer), ["$ whoami", "user", ""]);
    }

    /// The same seal applies between two GUI-authored lines and a command's
    /// output that never sends a newline before the command exits.
    #[test]
    fn a_sealed_line_stays_sealed_even_with_no_output_after_it() {
        let mut buffer = ConsoleBuffer::default();
        buffer.push_line("$ true");
        buffer.push_line("[exit 1]");
        assert_eq!(plain(&buffer), ["$ true", "[exit 1]"]);
    }

    #[test]
    fn the_buffer_stays_bounded() {
        let mut buffer = ConsoleBuffer::default();
        for line in 0..MAX_LINES + 100 {
            buffer.push_line(format!("line {line}"));
        }
        assert_eq!(buffer.lines().len(), MAX_LINES);
        assert_eq!(plain(&buffer)[0], "line 100");
    }

    /// The whole loop against a real child on a real pty: output streams
    /// out, the exit code arrives once the output has been drained.
    #[test]
    fn a_command_streams_its_output_and_then_reports_its_exit() {
        let mut session = ConsoleSession::shell("printf 'a\\nb\\n'; exit 3", None).unwrap();
        let mut buffer = ConsoleBuffer::default();
        let mut exit = None;
        for _ in 0..400 {
            if let Some(code) = session.drain(&mut buffer) {
                exit = Some(code);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(exit, Some(Some(3)), "the exit code reaches the caller");
        let text = plain(&buffer);
        assert!(
            text.iter().any(|line| line == "a") && text.iter().any(|line| line == "b"),
            "both lines arrived, got {text:?}"
        );
    }

    /// The point of the pty: a program that checks for a terminal finds one.
    #[test]
    fn the_command_is_given_a_real_terminal() {
        let mut session =
            ConsoleSession::shell("test -t 0 && echo IS_TTY || echo NO_TTY", None).unwrap();
        let mut buffer = ConsoleBuffer::default();
        for _ in 0..400 {
            if session.drain(&mut buffer).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let text = plain(&buffer);
        assert!(
            text.iter().any(|line| line.contains("IS_TTY")),
            "stdin must be a tty, got {text:?}"
        );
    }

    /// The interactive half: what the user types reaches the command, and
    /// the pty echoes it back the way a terminal does.
    #[test]
    fn typed_input_reaches_the_command() {
        let mut session = ConsoleSession::shell("read answer; echo \"got:$answer\"", None).unwrap();
        // Give the shell a moment to be ready for input before answering.
        std::thread::sleep(std::time::Duration::from_millis(200));
        session.write_line("yes").unwrap();
        let mut buffer = ConsoleBuffer::default();
        for _ in 0..400 {
            if session.drain(&mut buffer).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let text = plain(&buffer);
        assert!(
            text.iter().any(|line| line.contains("got:yes")),
            "the command saw the typed line, got {text:?}"
        );
    }
}
