//! Versionsnummern nach Semantic Versioning 2.0.0 (nur so viel, wie das Update braucht).
//!
//! Angeboten werden nur reguläre Versionen `X.Y.Z` (Tag `vX.Y.Z`); Pre-Releases (`-…`) und ungültige Nummern
//! (führende Nullen, fehlende Stellen) werden verworfen. Build-Metadaten (`+…`) zählen nicht für die Rangfolge.

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Pre-Release-Kennung (leer = reguläre Version). Nur für die eigene Version relevant (z. B. ein Testbau `2.1.0-dev`).
    pub pre: String,
}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self { major, minor, patch, pre: String::new() }
    }

    /// `X.Y.Z[-pre][+build]` streng nach SemVer (keine führenden Nullen, genau drei Stellen).
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let core_pre = s.split_once('+').map_or(s, |(a, _)| a);
        let (core, pre) = match core_pre.split_once('-') {
            Some((c, p)) if !p.is_empty() && p.split('.').all(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) => (c, p),
            Some(_) => return None,
            None => (core_pre, ""),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        let num = |p: &str| -> Option<u64> {
            if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) || (p.len() > 1 && p.starts_with('0')) {
                return None;
            }
            p.parse().ok()
        };
        Some(Version { major: num(parts[0])?, minor: num(parts[1])?, patch: num(parts[2])?, pre: pre.to_string() })
    }

    /// Reguläre Version aus einem Release-Tag `vX.Y.Z` (ein Pre-Release liefert `None`).
    pub fn from_tag(tag: &str) -> Option<Version> {
        let v = Version::parse(tag.strip_prefix('v')?)?;
        v.pre.is_empty().then_some(v)
    }

    /// Version dieses Programms.
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(0, 0, 0))
    }

    /// Sprung in eine neue Hauptversion (2.x → 3.0): deutliche Warnung und Datenbank-Sicherung vor dem Update.
    pub fn is_major_jump(&self, to: &Version) -> bool {
        to.major > self.major
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(o.major, o.minor, o.patch)).then_with(|| match (self.pre.is_empty(), o.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            // Eine Pre-Release-Version liegt vor der regulären gleichen Nummer.
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => cmp_pre(&self.pre, &o.pre),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Rangfolge der Pre-Release-Kennungen (SemVer 11.4): numerisch < alphanumerisch, Teil für Teil.
fn cmp_pre(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.split('.'), b.split('.'));
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) => {
                let o = match (p.parse::<u64>(), q.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => p.cmp(q),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parse_strict() {
        assert_eq!(v("2.1.0"), Version::new(2, 1, 0));
        assert_eq!(v("10.0.3+build.7"), Version::new(10, 0, 3));
        assert_eq!(v("1.0.0-rc.1").pre, "rc.1");
        for bad in ["", "1", "1.2", "1.2.3.4", "01.2.3", "1.02.3", "1.2.03", "a.b.c", "1.2.3-", "1.2.3-a..b", "-1.2.3", "1.2.x", " 1. 2.3"] {
            assert!(Version::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn tags() {
        assert_eq!(Version::from_tag("v2.1.0"), Some(Version::new(2, 1, 0)));
        assert_eq!(Version::from_tag("2.1.0"), None, "Tags tragen immer ein v");
        assert_eq!(Version::from_tag("v2.1.0-rc.1"), None, "Pre-Releases werden nie angeboten");
        assert_eq!(Version::from_tag("vX"), None);
    }

    #[test]
    fn ordering() {
        assert!(v("0.10.0") > v("0.9.0"));
        assert!(v("2.0.0") > v("1.99.99"));
        assert!(v("2.1.1") > v("2.1.0"));
        assert!(v("2.1.0") > v("2.1.0-rc.1"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.10") > v("1.0.0-rc.9"));
        assert!(v("1.0.0-beta") > v("1.0.0-alpha.1"));
        assert!(v("1.0.0-alpha.1") > v("1.0.0-alpha"));
        assert!(v("1.0.0-alpha.beta") > v("1.0.0-alpha.1"));
        assert_eq!(v("1.2.3+a").cmp(&v("1.2.3+b")), Ordering::Equal);
    }

    #[test]
    fn major_jump() {
        assert!(v("2.4.1").is_major_jump(&v("3.0.0")));
        assert!(!v("2.0.0").is_major_jump(&v("2.9.0")));
        assert!(!v("3.0.0").is_major_jump(&v("2.9.0")));
    }

    #[test]
    fn current_parses() {
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }
}
