//! Byte-pattern search in a file — what the hex viewer's search box runs.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use memchr::memmem;

use crate::vfs::ReadAt;

const CHUNK: usize = 1 << 20;

/// Where a pattern was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub offset: u64,
    /// The search ran off the end (or the start) and came round again.
    pub wrapped: bool,
}

/// `"de ad BE EF"`, `"0xdeadbeef"` and `"DEADBEEF"` are the same four bytes. None for odd
/// digit counts, anything that is not hex, or nothing at all.
pub fn parse_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    let text = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    let digits: Vec<u8> = text
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .map(|b| (b as char).to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()?;
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return None;
    }
    Some(
        digits
            .chunks(2)
            .map(|pair| pair[0] << 4 | pair[1])
            .collect(),
    )
}

fn read_exact_at(file: &dyn ReadAt, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    file.read_range(offset, len)
}

/// The first occurrence of `pattern` starting at or after `from`, in `[from, end)`.
fn scan_forward(
    file: &dyn ReadAt,
    pattern: &[u8],
    from: u64,
    end: u64,
    cancel: &AtomicBool,
) -> io::Result<Option<u64>> {
    let finder = memmem::Finder::new(pattern);
    let overlap = pattern.len() as u64 - 1;
    let mut at = from;
    while at < end {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let want = (CHUNK as u64).min(end - at + overlap).min(file.len() - at);
        let block = read_exact_at(file, at, want as usize)?;
        if let Some(i) = finder.find(&block) {
            let found = at + i as u64;
            if found < end {
                return Ok(Some(found));
            }
        }
        if at + want >= end.min(file.len()) {
            break;
        }
        at += (CHUNK as u64).min(want.saturating_sub(overlap)).max(1);
    }
    Ok(None)
}

/// The last occurrence that *starts* before `before`, searched back to the file's start.
fn scan_backward(
    file: &dyn ReadAt,
    pattern: &[u8],
    before: u64,
    cancel: &AtomicBool,
) -> io::Result<Option<u64>> {
    let finder = memmem::FinderRev::new(pattern);
    let overlap = pattern.len() as u64 - 1;
    // Occurrences must start below `before`, so they may reach up to `overlap` bytes past it.
    let mut top = (before + overlap).min(file.len());
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let bottom = top.saturating_sub(CHUNK as u64);
        let block = read_exact_at(file, bottom, (top - bottom) as usize)?;
        let mut hay: &[u8] = &block;
        // Skip candidates that start at or after `before`.
        while let Some(i) = finder.rfind(hay) {
            let start = bottom + i as u64;
            if start < before {
                return Ok(Some(start));
            }
            hay = &hay[..i + pattern.len() - 1];
            if hay.len() < pattern.len() {
                break;
            }
        }
        if bottom == 0 {
            return Ok(None);
        }
        top = bottom + overlap;
    }
}

/// Every offset where `pattern` starts (overlapping matches included), in order, and whether
/// the search stopped at `limit` matches with more left.
pub fn find_all(
    file: &dyn ReadAt,
    pattern: &[u8],
    limit: usize,
    cancel: &AtomicBool,
) -> io::Result<(Vec<u64>, bool)> {
    let mut found = Vec::new();
    if pattern.is_empty() || file.len() < pattern.len() as u64 {
        return Ok((found, false));
    }
    let finder = memmem::Finder::new(pattern);
    let overlap = pattern.len() as u64 - 1;
    let mut at = 0u64;
    while at < file.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let block = file.read_range(at, CHUNK + overlap as usize)?;
        // A match belongs to the chunk it starts in; the overlap only lets it finish.
        let mut from = 0;
        while let Some(i) = finder.find(&block[from..]) {
            let start = from + i;
            if start >= CHUNK {
                break;
            }
            if found.len() >= limit {
                return Ok((found, true));
            }
            found.push(at + start as u64);
            from = start + 1;
        }
        at += CHUNK as u64;
    }
    Ok((found, false))
}

/// Searches for `pattern` forward from `from` (a match *at* `from` counts) or backward from
/// just before it (a match starting at `from` does not), wrapping once. The caller passes one
/// past the current match to find the next, and the current match's offset to find the
/// previous.
pub fn find_bytes(
    file: &dyn ReadAt,
    pattern: &[u8],
    from: u64,
    forward: bool,
    cancel: &AtomicBool,
) -> io::Result<Option<Found>> {
    if pattern.is_empty() || file.len() < pattern.len() as u64 {
        return Ok(None);
    }
    let len = file.len();
    if forward {
        let from = from.min(len);
        if let Some(offset) = scan_forward(file, pattern, from, len, cancel)? {
            return Ok(Some(Found {
                offset,
                wrapped: false,
            }));
        }
        // Round again: matches that start before `from`.
        let offset = scan_forward(file, pattern, 0, from.min(len), cancel)?;
        Ok(offset.map(|offset| Found {
            offset,
            wrapped: true,
        }))
    } else {
        if let Some(offset) = scan_backward(file, pattern, from.min(len), cancel)? {
            return Ok(Some(Found {
                offset,
                wrapped: false,
            }));
        }
        let offset = scan_backward(file, pattern, len, cancel)?.filter(|o| *o >= from.min(len));
        Ok(offset.map(|offset| Found {
            offset,
            wrapped: true,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bytes(Vec<u8>);
    impl ReadAt for Bytes {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            let from = (offset as usize).min(self.0.len());
            let n = buf.len().min(self.0.len() - from);
            buf[..n].copy_from_slice(&self.0[from..from + n]);
            Ok(n)
        }
    }

    fn find(data: &[u8], pat: &[u8], from: u64, fwd: bool) -> Option<(u64, bool)> {
        find_bytes(
            &Bytes(data.to_vec()),
            pat,
            from,
            fwd,
            &AtomicBool::new(false),
        )
        .unwrap()
        .map(|f| (f.offset, f.wrapped))
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("de ad BE ef"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(parse_hex("0xDEAD"), Some(vec![0xde, 0xad]));
        assert_eq!(parse_hex("abc"), None);
        assert_eq!(parse_hex("zz"), None);
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn forward_next_and_wrap() {
        let data = b"xxabxxabxx";
        assert_eq!(find(data, b"ab", 0, true), Some((2, false)));
        assert_eq!(find(data, b"ab", 3, true), Some((6, false)));
        assert_eq!(find(data, b"ab", 7, true), Some((2, true)));
        assert_eq!(find(data, b"zz", 0, true), None);
    }

    #[test]
    fn backward_previous_and_wrap() {
        let data = b"xxabxxabxx";
        assert_eq!(find(data, b"ab", 6, false), Some((2, false)));
        assert_eq!(find(data, b"ab", 2, false), Some((6, true)));
        assert_eq!(find(data, b"ab", 10, false), Some((6, false)));
    }

    #[test]
    fn matches_across_chunk_boundaries() {
        let mut data = vec![0u8; CHUNK * 2 + 10];
        let at = CHUNK - 2;
        data[at..at + 4].copy_from_slice(b"ABCD");
        assert_eq!(find(&data, b"ABCD", 0, true), Some((at as u64, false)));
        assert_eq!(
            find(&data, b"ABCD", data.len() as u64, false),
            Some((at as u64, false))
        );
    }

    #[test]
    fn find_all_lists_every_start_once_across_chunks() {
        let mut data = vec![0u8; CHUNK * 2 + 10];
        for at in [5, CHUNK - 2, CHUNK + 10, CHUNK * 2 + 6] {
            data[at..at + 4].copy_from_slice(b"ABCD");
        }
        let (all, more) = find_all(&Bytes(data), b"ABCD", 100, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            all,
            [5, CHUNK as u64 - 2, CHUNK as u64 + 10, CHUNK as u64 * 2 + 6]
        );
        assert!(!more);
        let (all, more) =
            find_all(&Bytes(b"aaaa".to_vec()), b"aa", 2, &AtomicBool::new(false)).unwrap();
        assert_eq!((all, more), (vec![0, 1], true));
    }

    #[test]
    fn overlapping_matches_are_all_reachable() {
        let data = b"aaaa";
        assert_eq!(find(data, b"aa", 1, true), Some((1, false)));
        assert_eq!(find(data, b"aa", 3, false), Some((2, false)));
    }
}
