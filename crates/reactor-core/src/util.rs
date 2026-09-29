//! Subprocesses, PATH lookup and the small helpers everything else leans on.

use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// What `_run` answered: did it finish, with what code, and what it printed.
#[derive(Debug, Clone)]
pub struct Run {
    /// False only on timeout.
    pub completed: bool,
    pub code: Option<i32>,
    /// stdout and stderr, interleaved as the child wrote them.
    pub output: String,
}

impl Run {
    pub fn ok(&self) -> bool {
        self.completed && self.code == Some(0)
    }
}

/// Run `argv`, merged stdout+stderr, killed after `timeout`. Never fails: a
/// missing binary is exit 127 and a timeout is `completed == false`.
pub fn run(argv: &[String], timeout: Duration) -> Run {
    run_in(argv, None, timeout)
}

pub fn run_in(argv: &[String], cwd: Option<&Path>, timeout: Duration) -> Run {
    let not_found = Run { completed: true, code: Some(127), output: String::new() };
    let Some((program, rest)) = argv.split_first() else {
        return not_found;
    };
    let Ok((mut reader, writer)) = std::io::pipe() else {
        return not_found;
    };
    let Ok(writer2) = writer.try_clone() else {
        return not_found;
    };
    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::from(writer2));
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let Ok(mut child) = cmd.spawn() else {
        return not_found;
    };
    // The Command still holds the parent's copies of the write end; until they
    // are dropped the reader never sees EOF.
    drop(cmd);

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // Not joining the reader: a grandchild can hold the pipe open
            // long after its parent is dead, and a timed-out probe must not
            // wait for it.
            return Run { completed: false, code: None, output: String::new() };
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let bytes = rx.recv().unwrap_or_default();
    Run {
        completed: true,
        code: status.map(|s| s.code().unwrap_or(-1)),
        output: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// First non-blank line, trimmed.
pub fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// `shutil.which`: the first executable regular file named `name` on `PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    fn executable(p: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    if name.is_empty() {
        return None;
    }
    if name.contains('/') {
        let p = PathBuf::from(name);
        return executable(&p).then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(name))
        .find(|p| executable(p))
}

pub fn on_path(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == dir))
        .unwrap_or(false)
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(unix)]
pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
pub fn is_root() -> bool {
    false
}

pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

/// `pool.map` with at most `max` workers, results in input order.
pub fn par_map<T: Sync, R: Send>(items: &[T], max: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = max.min(items.len()).max(1);
    let next = AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<R>>> =
        items.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    *slots[i].lock().unwrap() = Some(f(item));
                }
            });
        }
    });
    slots.into_iter().map(|m| m.into_inner().unwrap().unwrap()).collect()
}

pub fn now_secs_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Right-pad to `width` *characters* (Python's `ljust`, which counts code points).
pub fn ljust(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - n))
    }
}
