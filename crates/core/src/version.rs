//! Semantic versions as our release tags use them (`v1.2.3`, `1.2.3-rc.1`, `0.10`), compared with
//! semver precedence: a pre-release sorts before its release, build metadata is ignored.
use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<Ident>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ident {
    Num(u64),
    Alpha(String),
}

impl Version {
    /// `v1.2.3`, `1.2`, `V2`, `1.2.3-beta.2+build.5`. Leading zeros, empty parts and anything
    /// else non-numeric in the core are rejected.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let num = |p: &str| -> Option<u64> {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) || (p.len() > 1 && p.starts_with('0')) {
                return None;
            }
            p.parse().ok()
        };
        let mut parts = core.split('.');
        let major = num(parts.next()?)?;
        let minor = parts.next().map_or(Some(0), num)?;
        let patch = parts.next().map_or(Some(0), num)?;
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(p) => p
                .split('.')
                .map(|id| {
                    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                        None
                    } else if id.bytes().all(|b| b.is_ascii_digit()) {
                        id.parse().ok().map(Ident::Num)
                    } else {
                        Some(Ident::Alpha(id.to_string()))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Version { major, minor, patch, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Ident::Num(a), Ident::Num(b)) => a.cmp(b),
            (Ident::Num(_), Ident::Alpha(_)) => Ordering::Less,
            (Ident::Alpha(_), Ident::Num(_)) => Ordering::Greater,
            (Ident::Alpha(a), Ident::Alpha(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => self.pre.cmp(&other.pre),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (i, id) in self.pre.iter().enumerate() {
            f.write_str(if i == 0 { "-" } else { "." })?;
            match id {
                Ident::Num(n) => write!(f, "{n}")?,
                Ident::Alpha(s) => f.write_str(s)?,
            }
        }
        Ok(())
    }
}

/// Whether `candidate` (a tag like `v0.2.0`) is newer than `current` (`0.1.0`). Unparseable
/// versions are never newer, so a strange tag can't trigger an update loop.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((Version::parse(candidate), Version::parse(current)), (Some(a), Some(b)) if a > b)
}

/// `v0.2.0` → `0.2.0` for display; unparseable input is returned trimmed.
pub fn display(tag: &str) -> String {
    Version::parse(tag).map(|v| v.to_string()).unwrap_or_else(|| tag.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("parse {s}"))
    }

    #[test]
    fn parses() {
        assert_eq!(v("v1.2.3"), Version { major: 1, minor: 2, patch: 3, pre: vec![] });
        assert_eq!(v("0.10").to_string(), "0.10.0");
        assert_eq!(v("V2").to_string(), "2.0.0");
        assert_eq!(v("1.2.3-rc.1+build.7").to_string(), "1.2.3-rc.1");
        assert_eq!(v(" v0.1.0 ").to_string(), "0.1.0");
        for bad in ["", "v", "nightly", "1.2.3.4", "01.2.3", "1..3", "1.2.3-", "1.2.3-a..b", "1.x"] {
            assert!(Version::parse(bad).is_none(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn precedence() {
        // the order from semver.org §11
        let chain = ["1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1", "1.1.0", "2.0.0"];
        for w in chain.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
        assert_eq!(v("1.0.0+a").cmp(&v("1.0.0+b")), Ordering::Equal);
        assert!(v("0.10.0") > v("0.9.9"));
    }

    #[test]
    fn newer() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.0"));
        assert!(is_newer("v1.0.0", "1.0.0-rc.2"));
        assert!(is_newer("v1.0.0-rc.2", "1.0.0-rc.1"));
        assert!(!is_newer("v1.0.0-rc.1", "1.0.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("v0.2.0", "garbage"));
        assert_eq!(display("v0.3.0"), "0.3.0");
        assert_eq!(display("latest"), "latest");
    }
}
