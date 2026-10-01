use std::io::Write;
use std::process::{Command, Stdio};

use crate::vfs::ReadAt;

/// What a file's leading bytes say it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// `file(1)`'s description when that program exists ("ELF 64-bit LSB executable, …"),
    /// else a built-in one.
    pub description: String,
    pub mime: String,
    /// Whether the bytes look like text — what decides editor versus hex viewer.
    pub is_text: bool,
}

/// How much of a file identification reads.
pub const HEAD_BYTES: usize = 64 * 1024;

/// Text if there is no NUL and the head is UTF-8 (a cut multi-byte tail excused), or has a
/// UTF-16 byte-order mark.
fn looks_like_text(head: &[u8]) -> bool {
    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        return true;
    }
    if head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && e.valid_up_to() + 4 > head.len(),
    }
}

/// The built-in identification: magic numbers through `infer`, otherwise text or data.
pub fn identify(head: &[u8]) -> Identity {
    let is_text = looks_like_text(head);
    if let Some(kind) = infer::get(head) {
        return Identity {
            description: format!("{} ({})", kind.mime_type(), kind.extension()),
            mime: kind.mime_type().to_owned(),
            is_text: is_text && kind.matcher_type() == infer::MatcherType::Text,
        };
    }
    if head.is_empty() {
        return Identity {
            description: "empty".into(),
            mime: "inode/x-empty".into(),
            is_text: true,
        };
    }
    if is_text {
        Identity {
            description: "text".into(),
            mime: "text/plain".into(),
            is_text,
        }
    } else {
        Identity {
            description: "data".into(),
            mime: "application/octet-stream".into(),
            is_text,
        }
    }
}

/// Feeds the head to `file -b -`, which reads standard input — so a file that exists only in
/// a virtual filesystem is described the same way a local one is.
fn run_file(head: &[u8], mime: bool) -> Option<String> {
    let mut command = Command::new("file");
    command.arg("-b");
    if mime {
        command.arg("--mime-type");
    }
    let mut child = command
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // A write error is `file` having read enough and exited; the output is still good.
    let _ = child.stdin.take()?.write_all(head);
    let output = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}

/// [`identify`] on the file's head, with `file(1)`'s wording when it is installed.
pub fn identify_file(file: &dyn ReadAt) -> std::io::Result<Identity> {
    let head = file.read_range(0, HEAD_BYTES)?;
    let mut identity = identify(&head);
    if let Some(description) = run_file(&head, false) {
        identity.description = description;
        if let Some(mime) = run_file(&head, true) {
            identity.mime = mime;
        }
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_and_elf_by_magic() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
        assert_eq!(identify(&png).mime, "image/png");
        assert!(!identify(&png).is_text);
        let mut elf = [0u8; 64];
        elf[..8].copy_from_slice(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0]);
        elf[16] = 2;
        assert!(
            identify(&elf).mime.starts_with("application/x-"),
            "{:?}",
            identify(&elf)
        );
    }

    #[test]
    fn text_is_text_and_nul_is_data() {
        assert!(identify(b"fn main() {}\n").is_text);
        assert!(identify("héllo".as_bytes()).is_text);
        assert!(!identify(&[1, 2, 0, 3]).is_text);
        assert!(identify(b"").is_text);
        // a multi-byte character cut by the head limit is still text
        assert!(identify(&"é".as_bytes()[..1]).is_text);
    }
}
