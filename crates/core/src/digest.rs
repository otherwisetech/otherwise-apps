//! SHA-256 checks against the `digest` GitHub publishes for every release asset.
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::path::Path;

pub type Sha = [u8; 32];

/// `sha256:<64 hex>` (GitHub's format; case-insensitive) → bytes. Other algorithms → None.
pub fn parse(s: &str) -> Option<Sha> {
    let hex = s.trim().strip_prefix("sha256:")?;
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

pub fn hex(sha: &Sha) -> String {
    sha.iter().map(|b| format!("{b:02x}")).collect()
}

/// A writer that hashes everything passing through it.
pub struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
    pub written: u64,
}

impl<W: Write> HashingWriter<W> {
    pub fn new(inner: W) -> Self {
        HashingWriter { inner, hasher: Sha256::new(), written: 0 }
    }

    pub fn finish(mut self) -> io::Result<(W, Sha)> {
        self.inner.flush()?;
        Ok((self.inner, self.hasher.finalize().into()))
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub fn file_sha256(path: &Path) -> io::Result<Sha> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    Ok,
    /// No usable digest was published: nothing can be verified, so nothing is installed.
    Missing,
    Mismatch { expected: String, actual: String },
}

/// Compares a file's hash with the published digest.
pub fn check(actual: &Sha, published: Option<&str>) -> Check {
    let Some(expected) = published.and_then(parse) else { return Check::Missing };
    if &expected == actual { Check::Ok } else { Check::Mismatch { expected: hex(&expected), actual: hex(actual) } }
}

#[cfg(test)]
mod tests {
    use super::*;

    // sha256("abc")
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn parse_and_check() {
        let mut w = HashingWriter::new(Vec::new());
        w.write_all(b"abc").unwrap();
        let (buf, sha) = w.finish().unwrap();
        assert_eq!(buf, b"abc");
        assert_eq!(hex(&sha), ABC);
        assert_eq!(parse(&format!("sha256:{ABC}")), Some(sha));
        assert_eq!(parse(&format!("sha256:{}", ABC.to_uppercase())), Some(sha));
        assert_eq!(parse(&format!("sha512:{ABC}")), None);
        assert_eq!(parse("sha256:abc"), None);
        assert_eq!(parse(ABC), None);
        assert_eq!(check(&sha, Some(&format!("sha256:{ABC}"))), Check::Ok);
        assert_eq!(check(&sha, None), Check::Missing);
        assert_eq!(check(&sha, Some("md5:x")), Check::Missing);
        let other = format!("sha256:{}", "0".repeat(64));
        assert_eq!(check(&sha, Some(&other)), Check::Mismatch { expected: "0".repeat(64), actual: ABC.into() });
    }
}
