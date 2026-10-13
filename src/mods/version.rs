//! Mod versions, the requirements naming them (`api`, `dependencies`), and
//! `drh` tag sets, as the game document defines them.

use std::cmp::Ordering;
use std::fmt;

/// `MAJOR.MINOR.PATCH`, digits only.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let [major, minor, patch] = numbers::<3>(text)?;
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// `"x.y"` or `"x.y.z"`: the same major, and not older. With major `0`, the
/// minor is the major.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Requirement {
    lowest: Version,
}

impl Requirement {
    pub fn parse(text: &str) -> Option<Self> {
        let lowest = match text.split('.').count() {
            2 => {
                let [major, minor] = numbers::<2>(text)?;
                Version {
                    major,
                    minor,
                    patch: 0,
                }
            }
            3 => Version::parse(text)?,
            _ => return None,
        };
        Some(Self { lowest })
    }

    pub fn matches(&self, version: Version) -> bool {
        if version.cmp(&self.lowest) == Ordering::Less || version.major != self.lowest.major {
            return false;
        }
        self.lowest.major != 0 || version.minor == self.lowest.minor
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.lowest)
    }
}

/// A `drh` value: `"20"`, `"20,21"` or `"20-22"` (closed, inclusive).
pub fn parse_tags(text: &str) -> Option<Vec<u32>> {
    let mut tags = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        match part.split_once('-') {
            None => tags.push(whole_number(part)?),
            Some((from, to)) => {
                let (from, to): (u32, u32) = (whole_number(from)?, whole_number(to)?);
                if from > to {
                    return None;
                }
                tags.extend(from..=to);
            }
        }
    }
    Some(tags)
}

fn numbers<const N: usize>(text: &str) -> Option<[u64; N]> {
    let mut out = [0; N];
    let mut parts = text.split('.');
    for slot in &mut out {
        *slot = whole_number(parts.next()?)?;
    }
    parts.next().is_none().then_some(out)
}

/// Digits only, no sign, no leading zero.
fn whole_number<T: std::str::FromStr>(text: &str) -> Option<T> {
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if !digits || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn parses_versions_digits_only() {
        assert_eq!(version("1.20.3").to_string(), "1.20.3");
        for bad in ["1.2", "1.2.3.4", "1.2.x", "v1.2.3", "01.2.3", "1.2.-3", ""] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn requirement_keeps_the_major_and_refuses_older() {
        let requirement = Requirement::parse("1.2").unwrap();
        assert!(requirement.matches(version("1.2.0")));
        assert!(requirement.matches(version("1.9.4")));
        assert!(!requirement.matches(version("1.1.9")));
        assert!(!requirement.matches(version("2.0.0")));

        let patch = Requirement::parse("1.2.3").unwrap();
        assert!(!patch.matches(version("1.2.2")));
        assert!(patch.matches(version("1.2.3")));
    }

    #[test]
    fn requirement_with_major_zero_keeps_the_minor() {
        let requirement = Requirement::parse("0.3").unwrap();
        assert!(requirement.matches(version("0.3.7")));
        assert!(!requirement.matches(version("0.4.0")));
        assert!(!requirement.matches(version("0.2.9")));
    }

    #[test]
    fn requirement_syntax() {
        for bad in ["1", "1.2.3.4", "^1.2", ">=1.2", "1.x"] {
            assert!(Requirement::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn parses_closed_tag_sets() {
        assert_eq!(parse_tags("20"), Some(vec![20]));
        assert_eq!(parse_tags("20, 22"), Some(vec![20, 22]));
        assert_eq!(parse_tags("20-22"), Some(vec![20, 21, 22]));
        for bad in ["", "20,", ">=20", "V20", "22-20", "20-21-22", "20-"] {
            assert_eq!(parse_tags(bad), None, "{bad}");
        }
    }
}
