use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::vfs::ReadAt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Md5,
    Sha1,
    Sha256,
    Crc32,
}

impl Algorithm {
    pub const ALL: [Algorithm; 4] = [
        Algorithm::Md5,
        Algorithm::Sha1,
        Algorithm::Sha256,
        Algorithm::Crc32,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Algorithm::Md5 => "MD5",
            Algorithm::Sha1 => "SHA-1",
            Algorithm::Sha256 => "SHA-256",
            Algorithm::Crc32 => "CRC-32",
        }
    }
}

enum State {
    Md5(Md5),
    Sha1(Sha1),
    Sha256(Sha256),
    Crc32(crc32fast::Hasher),
}

impl State {
    fn new(algorithm: Algorithm) -> Self {
        match algorithm {
            Algorithm::Md5 => State::Md5(Md5::new()),
            Algorithm::Sha1 => State::Sha1(Sha1::new()),
            Algorithm::Sha256 => State::Sha256(Sha256::new()),
            Algorithm::Crc32 => State::Crc32(crc32fast::Hasher::new()),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            State::Md5(h) => h.update(data),
            State::Sha1(h) => h.update(data),
            State::Sha256(h) => h.update(data),
            State::Crc32(h) => h.update(data),
        }
    }

    fn finish(self) -> String {
        fn hex(bytes: &[u8]) -> String {
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        }
        match self {
            State::Md5(h) => hex(&h.finalize()),
            State::Sha1(h) => hex(&h.finalize()),
            State::Sha256(h) => hex(&h.finalize()),
            State::Crc32(h) => format!("{:08x}", h.finalize()),
        }
    }
}

/// Hashes the whole file with every requested algorithm in one pass. `cancel` is checked
/// between blocks; a cancelled run is an `Interrupted` error, never a partial digest.
pub fn hash_file(
    file: &dyn ReadAt,
    algorithms: &[Algorithm],
    cancel: &AtomicBool,
) -> io::Result<Vec<(Algorithm, String)>> {
    let mut states: Vec<_> = algorithms.iter().map(|a| (*a, State::new(*a))).collect();
    let mut buf = vec![0u8; 1 << 20];
    let mut offset = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = file.read_at(offset, &mut buf)?;
        if n == 0 {
            break;
        }
        for (_, state) in &mut states {
            state.update(&buf[..n]);
        }
        offset += n as u64;
    }
    Ok(states.into_iter().map(|(a, s)| (a, s.finish())).collect())
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

    #[test]
    fn known_digests_of_abc() {
        let sums = hash_file(
            &Bytes(b"abc".to_vec()),
            &Algorithm::ALL,
            &AtomicBool::new(false),
        )
        .unwrap();
        let get = |a| sums.iter().find(|(x, _)| *x == a).unwrap().1.clone();
        assert_eq!(get(Algorithm::Md5), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            get(Algorithm::Sha1),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            get(Algorithm::Sha256),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(get(Algorithm::Crc32), "352441c2");
    }

    #[test]
    fn cancel_is_an_error_not_a_digest() {
        let err = hash_file(
            &Bytes(vec![0; 10]),
            &[Algorithm::Md5],
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
    }
}
