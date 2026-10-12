//! Our own semver in Rust — a port of the TypeScript version (jet: src/core/semver.ts).
//! No external crates, std only. Parsing versions, comparison, and ranges
//! (`^ ~ x * >= <= - ||`), picking the highest matching version.

use std::cmp::Ordering;

/// A prerelease identifier: a number or a string (a number is always "lower" than a string).
#[derive(Clone, Debug, PartialEq, Eq)]
enum PreId {
    Num(u64),
    Str(String),
}

/// A parsed version. Build metadata (+...) is discarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pre: Vec<PreId>,
}

impl Version {
    /// A normalized version string without build metadata.
    pub fn raw(&self) -> String {
        if self.pre.is_empty() {
            format!("{}.{}.{}", self.major, self.minor, self.patch)
        } else {
            let pre: Vec<String> = self
                .pre
                .iter()
                .map(|p| match p {
                    PreId::Num(n) => n.to_string(),
                    PreId::Str(s) => s.clone(),
                })
                .collect();
            format!(
                "{}.{}.{}-{}",
                self.major,
                self.minor,
                self.patch,
                pre.join(".")
            )
        }
    }
}

/// Parse a version string. Err — on an invalid format.
pub fn parse(input: &str) -> Result<Version, String> {
    let v = input.trim().strip_prefix('v').unwrap_or(input.trim());
    // Separate out build metadata — it doesn't affect comparison.
    let core = v.split('+').next().unwrap_or("");
    let (main_part, pre_part) = match core.split_once('-') {
        Some((m, p)) => (m, Some(p)),
        None => (core, None),
    };

    let nums: Vec<&str> = main_part.split('.').collect();
    if nums.len() != 3 {
        return Err(format!(
            "Invalid version: \"{input}\" (expected major.minor.patch)"
        ));
    }
    let parse_num = |s: &str| -> Result<u64, String> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!(
                "Invalid version: \"{input}\" (non-numeric component \"{s}\")"
            ));
        }
        s.parse::<u64>()
            .map_err(|_| format!("Invalid version: \"{input}\""))
    };
    let major = parse_num(nums[0])?;
    let minor = parse_num(nums[1])?;
    let patch = parse_num(nums[2])?;

    let pre = match pre_part {
        None | Some("") => Vec::new(),
        Some(p) => p
            .split('.')
            .map(|id| {
                if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                    PreId::Num(id.parse().unwrap_or(0))
                } else {
                    PreId::Str(id.to_string())
                }
            })
            .collect(),
    };

    Ok(Version {
        major,
        minor,
        patch,
        pre,
    })
}

/// A safe parse: None instead of an error.
pub fn try_parse(input: &str) -> Option<Version> {
    parse(input).ok()
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then_with(|| compare_pre(&self.pre, &other.pre))
    }
}

/// A version without a prerelease is higher than one with a prerelease at equal core.
fn compare_pre(a: &[PreId], b: &[PreId]) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater, // 1.0.0 > 1.0.0-beta (no prerelease is higher)
        (false, true) => return Ordering::Less,
        _ => {}
    }
    for (x, y) in a.iter().zip(b.iter()) {
        let ord = match (x, y) {
            (PreId::Num(m), PreId::Num(n)) => m.cmp(n),
            (PreId::Num(_), PreId::Str(_)) => Ordering::Less, // a number is lower than a string
            (PreId::Str(_), PreId::Num(_)) => Ordering::Greater,
            (PreId::Str(m), PreId::Str(n)) => m.cmp(n),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a.len().cmp(&b.len())
}

// ---------------------------------------------------------------------------
// Ranges
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Lt,
    Lte,
    Gt,
    Gte,
    Eq,
}

#[derive(Clone, Debug)]
struct Comparator {
    op: Op,
    version: Version,
}

/// A parsed range: a list of alternatives (OR), each a set of constraints (AND).
#[derive(Clone, Debug)]
pub struct Range {
    alternatives: Vec<Vec<Comparator>>,
}

/// Parse a range string. "" and "*"/"x"/"latest" mean "any version".
pub fn parse_range(input: &str) -> Range {
    let alternatives = input
        .split("||")
        .map(|part| parse_comparator_set(part.trim()))
        .collect();
    Range { alternatives }
}

fn parse_comparator_set(part: &str) -> Vec<Comparator> {
    if matches!(part, "" | "*" | "x" | "X" | "latest") {
        return Vec::new(); // matches any non-prerelease version
    }

    // Hyphen range: "1.2.3 - 2.3.4".
    if let Some((from, to)) = split_hyphen(part) {
        let mut set = lower_bound(&from);
        set.extend(upper_bound_inclusive(&to));
        return set;
    }

    let tokens = join_dangling_operators(part.split_whitespace().collect());
    let mut comparators = Vec::new();
    for token in tokens {
        comparators.extend(parse_token(&token));
    }
    comparators
}

/// Split "a - b" (a hyphen surrounded by spaces) into bounds.
fn split_hyphen(part: &str) -> Option<(String, String)> {
    let toks: Vec<&str> = part.split_whitespace().collect();
    if toks.len() == 3 && toks[1] == "-" {
        Some((toks[0].to_string(), toks[2].to_string()))
    } else {
        None
    }
}

const BARE_OPS: [&str; 7] = [">=", "<=", ">", "<", "=", "^", "~"];

/// ">=" "2.1.2" -> ">=2.1.2" (npm allows a space between the operator and the version).
fn join_dangling_operators(tokens: Vec<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        if BARE_OPS.contains(&t) && i + 1 < tokens.len() {
            out.push(format!("{}{}", t, tokens[i + 1]));
            i += 2;
        } else {
            out.push(t.to_string());
            i += 1;
        }
    }
    out
}

fn parse_token(token: &str) -> Vec<Comparator> {
    let (prefix, rest) = split_prefix(token);
    match prefix {
        "^" => caret(rest),
        "~" => tilde(rest),
        ">=" | "<=" | ">" | "<" => {
            let op = match prefix {
                ">=" => Op::Gte,
                "<=" => Op::Lte,
                ">" => Op::Gt,
                _ => Op::Lt,
            };
            match coerce(rest) {
                Some(version) => vec![Comparator { op, version }],
                None => Vec::new(),
            }
        }
        _ => wildcard_or_exact(rest), // "" or "="
    }
}

fn split_prefix(token: &str) -> (&str, &str) {
    for p in [">=", "<=", ">", "<", "=", "^", "~"] {
        if let Some(rest) = token.strip_prefix(p) {
            return (p, rest.trim_start());
        }
    }
    ("", token)
}

/// Pad an incomplete version with zeros, KEEPING the prerelease: "1" -> 1.0.0,
/// "1.2" -> 1.2.0, "1.2.3-beta.1" -> 1.2.3-beta.1.
fn coerce(v: &str) -> Option<Version> {
    let clean = v.strip_prefix('v').unwrap_or(v);
    let (main, pre) = match clean.split_once('-') {
        Some((m, p)) => (m, Some(p)),
        None => (clean, None),
    };
    let mut parts: Vec<String> = main
        .split('.')
        .filter(|p| !p.is_empty() && !matches!(*p, "x" | "X" | "*"))
        .map(|s| s.to_string())
        .collect();
    while parts.len() < 3 {
        parts.push("0".to_string());
    }
    let base = parts[..3].join(".");
    let full = match pre {
        Some(p) if !p.is_empty() => format!("{base}-{p}"),
        _ => base,
    };
    parse(&full).ok()
}

fn wildcard_or_exact(v: &str) -> Vec<Comparator> {
    let clean = v.strip_prefix('v').unwrap_or(v);
    let base = clean.split('-').next().unwrap_or("");
    let parts: Vec<&str> = base.split('.').collect();
    let is_wild = |s: &str| matches!(s, "x" | "X" | "*" | "");

    if parts.len() == 3 && !parts.iter().any(|p| is_wild(p)) {
        return match parse(clean) {
            Ok(version) => vec![Comparator {
                op: Op::Eq,
                version,
            }],
            Err(_) => Vec::new(),
        };
    }

    let major = parts
        .first()
        .filter(|p| !is_wild(p))
        .and_then(|p| p.parse::<u64>().ok());
    let Some(major) = major else {
        return Vec::new(); // "*" — any version
    };
    let minor = parts
        .get(1)
        .filter(|p| !is_wild(p))
        .and_then(|p| p.parse::<u64>().ok());

    match minor {
        None => bound_pair(major, 0, major + 1, 0), // "1.x" -> >=1.0.0 <2.0.0
        Some(minor) => bound_pair(major, minor, major, minor + 1), // "1.2.x" -> >=1.2.0 <1.3.0
    }
}

fn bound_pair(lo_maj: u64, lo_min: u64, hi_maj: u64, hi_min: u64) -> Vec<Comparator> {
    vec![
        Comparator {
            op: Op::Gte,
            version: v3(lo_maj, lo_min, 0),
        },
        Comparator {
            op: Op::Lt,
            version: v3(hi_maj, hi_min, 0),
        },
    ]
}

fn v3(major: u64, minor: u64, patch: u64) -> Version {
    Version {
        major,
        minor,
        patch,
        pre: Vec::new(),
    }
}

/// caret: allows changes that don't alter the leftmost non-zero component.
fn caret(v: &str) -> Vec<Comparator> {
    let Some(ver) = coerce(v) else {
        return Vec::new();
    };
    let upper = if ver.major > 0 {
        v3(ver.major + 1, 0, 0)
    } else if ver.minor > 0 {
        v3(0, ver.minor + 1, 0)
    } else {
        v3(0, 0, ver.patch + 1)
    };
    vec![
        Comparator {
            op: Op::Gte,
            version: ver,
        },
        Comparator {
            op: Op::Lt,
            version: upper,
        },
    ]
}

/// tilde: patch-level changes (if a minor is given) or minor-level changes (if only major).
fn tilde(v: &str) -> Vec<Comparator> {
    let clean = v.strip_prefix('v').unwrap_or(v);
    let base = clean.split('-').next().unwrap_or("");
    let has_minor = base.split('.').count() >= 2;
    let Some(ver) = coerce(v) else {
        return Vec::new();
    };
    let upper = if has_minor {
        v3(ver.major, ver.minor + 1, 0)
    } else {
        v3(ver.major + 1, 0, 0)
    };
    vec![
        Comparator {
            op: Op::Gte,
            version: ver,
        },
        Comparator {
            op: Op::Lt,
            version: upper,
        },
    ]
}

fn lower_bound(v: &str) -> Vec<Comparator> {
    match coerce(v) {
        Some(version) => vec![Comparator {
            op: Op::Gte,
            version,
        }],
        None => Vec::new(),
    }
}

/// The upper bound of a hyphen range: an incomplete version expands upward.
fn upper_bound_inclusive(v: &str) -> Vec<Comparator> {
    let clean = v.strip_prefix('v').unwrap_or(v);
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() < 3 {
        let major = parts
            .first()
            .and_then(|p| p.parse::<u64>().ok())
            .unwrap_or(0);
        match parts.get(1).and_then(|p| p.parse::<u64>().ok()) {
            None => vec![Comparator {
                op: Op::Lt,
                version: v3(major + 1, 0, 0),
            }],
            Some(minor) => vec![Comparator {
                op: Op::Lt,
                version: v3(major, minor + 1, 0),
            }],
        }
    } else {
        match coerce(v) {
            Some(version) => vec![Comparator {
                op: Op::Lte,
                version,
            }],
            None => Vec::new(),
        }
    }
}

fn satisfies_comparator(version: &Version, c: &Comparator) -> bool {
    let ord = version.cmp(&c.version);
    match c.op {
        Op::Lt => ord == Ordering::Less,
        Op::Lte => ord != Ordering::Greater,
        Op::Gt => ord == Ordering::Greater,
        Op::Gte => ord != Ordering::Less,
        Op::Eq => ord == Ordering::Equal,
    }
}

/// Whether a version satisfies a range.
pub fn satisfies(version: &Version, range: &Range) -> bool {
    range
        .alternatives
        .iter()
        .any(|set| satisfies_set(version, set))
}

fn satisfies_set(version: &Version, set: &[Comparator]) -> bool {
    if !version.pre.is_empty() {
        // A prerelease passes only if the set has a comparator with the same core and its own prerelease.
        let allowed = set.iter().any(|c| {
            !c.version.pre.is_empty()
                && c.version.major == version.major
                && c.version.minor == version.minor
                && c.version.patch == version.patch
        });
        if !allowed {
            return false;
        }
    }
    set.iter().all(|c| satisfies_comparator(version, c))
}

/// The highest of the versions satisfying the range. None — if none fit.
pub fn max_satisfying(versions: &[&str], range: &Range) -> Option<String> {
    let mut best: Option<Version> = None;
    for raw in versions {
        let Some(v) = try_parse(raw) else { continue };
        if !satisfies(&v, range) {
            continue;
        }
        if best.as_ref().is_none_or(|b| v > *b) {
            best = Some(v);
        }
    }
    best.map(|v| v.raw())
}

/// A convenience wrapper: check a version string against a range string.
pub fn satisfies_str(version: &str, range: &str) -> bool {
    match try_parse(version) {
        Some(v) => satisfies(&v, &parse_range(range)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing() {
        let v = parse("1.2.3").unwrap();
        assert_eq!((v.major, v.minor, v.patch), (1, 2, 3));
        assert!(v.pre.is_empty());
        assert_eq!(parse("2.0.0-beta.1").unwrap().raw(), "2.0.0-beta.1");
        assert_eq!(parse("v3.4.5").unwrap().major, 3);
        assert_eq!(parse("1.2.3+build.99").unwrap().raw(), "1.2.3");
        assert!(parse("1.2").is_err());
        assert!(parse("a.b.c").is_err());
        assert!(try_parse("nope").is_none());
    }

    #[test]
    fn comparison() {
        assert!(parse("1.0.0").unwrap() < parse("2.0.0").unwrap());
        assert!(parse("1.2.0").unwrap() > parse("1.1.9").unwrap());
        assert!(parse("1.0.0").unwrap() == parse("1.0.0").unwrap());
        assert!(parse("1.0.0").unwrap() > parse("1.0.0-beta").unwrap());
        assert!(parse("1.0.0-alpha").unwrap() < parse("1.0.0-beta").unwrap());
        assert!(parse("1.0.0-alpha.1").unwrap() < parse("1.0.0-alpha.2").unwrap());
        assert!(parse("1.0.0-1").unwrap() < parse("1.0.0-alpha").unwrap());
    }

    #[test]
    fn caret_ranges() {
        assert!(satisfies_str("1.2.3", "^1.2.3"));
        assert!(satisfies_str("1.9.0", "^1.2.3"));
        assert!(!satisfies_str("2.0.0", "^1.2.3"));
        assert!(!satisfies_str("1.2.2", "^1.2.3"));
        assert!(satisfies_str("0.2.9", "^0.2.3"));
        assert!(!satisfies_str("0.3.0", "^0.2.3"));
        assert!(!satisfies_str("0.0.4", "^0.0.3"));
    }

    #[test]
    fn tilde_ranges() {
        assert!(satisfies_str("1.2.9", "~1.2.3"));
        assert!(!satisfies_str("1.3.0", "~1.2.3"));
        assert!(satisfies_str("1.2.0", "~1.2"));
        assert!(!satisfies_str("1.3.0", "~1.2"));
    }

    #[test]
    fn wildcards_and_comparators() {
        assert!(satisfies_str("1.5.0", "1.x"));
        assert!(!satisfies_str("2.0.0", "1.x"));
        assert!(satisfies_str("1.2.7", "1.2.x"));
        assert!(satisfies_str("9.9.9", "*"));
        assert!(satisfies_str("1.5.0", ">=1.2.0 <2.0.0"));
        assert!(!satisfies_str("2.1.0", ">=1.2.0 <2.0.0"));
        assert!(satisfies_str("1.0.0", "1.0.0 || 2.0.0"));
        assert!(satisfies_str("2.0.0", "1.0.0 || 2.0.0"));
        assert!(satisfies_str("1.3.0", "1.2.0 - 1.4.0"));
        assert!(!satisfies_str("1.5.0", "1.2.0 - 1.4.0"));
    }

    #[test]
    fn spaces_after_operators() {
        assert!(satisfies_str("2.1.2", ">= 2.1.2 < 3.0.0"));
        assert!(!satisfies_str("3.0.0", ">= 2.1.2 < 3.0.0"));
        assert!(!satisfies_str("2.0.0", ">= 2.1.2 < 3.0.0"));
        assert!(satisfies_str("1.5.0", "> 1.0.0"));
        assert!(satisfies_str("2.0.0", "<= 2.0.0"));
    }

    #[test]
    fn prerelease_in_ranges() {
        assert!(!satisfies_str("1.2.3-beta", "^1.0.0"));
        assert!(satisfies_str("1.2.3-beta.2", ">=1.2.3-beta.1 <1.3.0"));
    }

    #[test]
    fn max_satisfying_picks() {
        let versions = ["1.0.0", "1.2.0", "1.2.5", "1.3.0", "2.0.0"];
        assert_eq!(
            max_satisfying(&versions, &parse_range("^1.2.0")).as_deref(),
            Some("1.3.0")
        );
        assert_eq!(
            max_satisfying(&versions, &parse_range("~1.2.0")).as_deref(),
            Some("1.2.5")
        );
        assert_eq!(max_satisfying(&versions, &parse_range(">=3.0.0")), None);
        assert_eq!(
            max_satisfying(&versions, &parse_range("*")).as_deref(),
            Some("2.0.0")
        );
    }
}
