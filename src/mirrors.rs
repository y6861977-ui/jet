//! Registry mirrors + live availability check — a port of mirrors.ts.

use std::process::Command;
use std::time::Instant;

pub struct Mirror {
    /// A short name for commands (e.g. `jet registry use yarn`).
    pub key: &'static str,
    pub name: &'static str,
    /// The URL is always without a trailing slash — consistent with the rest of the code.
    pub url: &'static str,
    /// A note for humans: how this mirror differs.
    pub note: &'static str,
}

/// The canonical list of mirrors. The order = the numbering in `jet registry mirrors`.
pub const MIRRORS: &[Mirror] = &[
    Mirror {
        key: "npm",
        name: "npm Official",
        url: "https://registry.npmjs.org",
        note: "The official registry",
    },
    Mirror {
        key: "npmmirror",
        name: "npmmirror / CNPM",
        url: "https://registry.npmmirror.com",
        note: "The main public npm mirror",
    },
    Mirror {
        key: "tencent",
        name: "Tencent Cloud",
        url: "https://mirrors.cloud.tencent.com/npm",
        note: "Tencent's HTTPS mirror",
    },
    Mirror {
        key: "tencent-http",
        name: "Tencent Alternative",
        url: "http://mirrors.tencent.com/npm",
        note: "Officially listed by Tencent, but HTTP only",
    },
    Mirror {
        key: "huawei",
        name: "Huawei Cloud",
        url: "https://repo.huaweicloud.com/repository/npm",
        note: "Huawei's mirror",
    },
    Mirror {
        key: "yarn",
        name: "Yarn Classic",
        url: "https://registry.yarnpkg.com",
        note: "An npm proxy, not a separate package database",
    },
];

/// Find a mirror by short name or by number (1..N).
pub fn resolve_input(input: &str) -> Option<&'static Mirror> {
    if let Ok(n) = input.parse::<usize>() {
        return MIRRORS.get(n.checked_sub(1)?);
    }
    let key = input.to_lowercase();
    MIRRORS.iter().find(|m| m.key == key)
}

/// Whether the connection is unencrypted (plain HTTP).
pub fn is_insecure(url: &str) -> bool {
    url.trim().starts_with("http://")
}

/// A warning for HTTP mirrors. `None` for https.
pub fn insecure_warning(url: &str) -> Option<String> {
    if !is_insecure(url) {
        return None;
    }
    Some(format!(
        "Warning: {url} works over HTTP only — the connection is unencrypted, \
         your provider sees and can modify the traffic. Tarball integrity is still \
         checked against the sha512 from the packument, so tampering with the package contents will be caught."
    ))
}

pub struct CheckResult {
    pub name: String,
    pub url: String,
    pub ok: bool,
    pub ms: u128,
}

/// Check a registry: pull a tiny real package through curl with a timeout.
pub fn check_registry(url: &str, name: &str) -> CheckResult {
    let clean = url.trim_end_matches('/');
    let probe = format!("{clean}/is-number");
    let null_dev = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let start = Instant::now();
    let out = Command::new("curl")
        .args([
            "-sS",
            "-o",
            null_dev,
            "-w",
            "%{http_code}",
            "--max-time",
            "8",
        ])
        .arg(&probe)
        .output();
    let ms = start.elapsed().as_millis();
    let ok = matches!(out, Ok(o) if o.status.success() && {
        let code = String::from_utf8_lossy(&o.stdout);
        code.starts_with('2') || code.starts_with('3')
    });
    CheckResult {
        name: name.to_string(),
        url: clean.to_string(),
        ok,
        ms,
    }
}

/// Check the current registry and all mirrors (current first, no duplicates).
pub fn check_all(current: &str) -> Vec<CheckResult> {
    let clean_current = current.trim_end_matches('/');
    let mut results = Vec::new();
    let known = MIRRORS
        .iter()
        .find(|m| m.url.trim_end_matches('/') == clean_current);
    let cur_name = known
        .map(|m| format!("{} (current)", m.name))
        .unwrap_or_else(|| "current".to_string());
    results.push(check_registry(clean_current, &cur_name));
    for m in MIRRORS {
        if m.url.trim_end_matches('/') == clean_current {
            continue;
        }
        results.push(check_registry(m.url, m.name));
    }
    results
}

/// Candidates for auto-switching: only live ones, without the current (failed)
/// registry, fastest first. A pure function — tested without the network.
pub fn rank_candidates(results: Vec<CheckResult>, current: &str) -> Vec<CheckResult> {
    let clean_current = current.trim_end_matches('/');
    let mut out: Vec<CheckResult> = results
        .into_iter()
        .filter(|r| r.ok && r.url.trim_end_matches('/') != clean_current)
        .collect();
    out.sort_by_key(|r| r.ms);
    out
}

/// Live mirrors except the current one, by ascending response time.
pub fn candidates_by_speed(current: &str) -> Vec<CheckResult> {
    rank_candidates(check_all(current), current)
}

/// A human-readable mirror name by URL (for logs).
pub fn name_for_url(url: &str) -> String {
    let clean = url.trim_end_matches('/');
    MIRRORS
        .iter()
        .find(|m| m.url.trim_end_matches('/') == clean)
        .map(|m| m.name.to_string())
        .unwrap_or_else(|| clean.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cr(url: &str, ok: bool, ms: u128) -> CheckResult {
        CheckResult {
            name: url.to_string(),
            url: url.to_string(),
            ok,
            ms,
        }
    }

    #[test]
    fn canonical_list() {
        assert_eq!(
            MIRRORS.len(),
            6,
            "exactly six mirrors from the canonical list"
        );
        let keys: Vec<&str> = MIRRORS.iter().map(|m| m.key).collect();
        assert_eq!(
            keys,
            [
                "npm",
                "npmmirror",
                "tencent",
                "tencent-http",
                "huawei",
                "yarn"
            ]
        );
        for m in MIRRORS {
            assert!(
                !m.url.ends_with('/'),
                "URL without a trailing slash: {}",
                m.url
            );
            assert!(!m.note.is_empty(), "every mirror has a note");
            assert!(m.url.starts_with("http://") || m.url.starts_with("https://"));
        }
    }

    #[test]
    fn lookup_by_key_and_number() {
        assert_eq!(
            resolve_input("yarn").map(|m| m.url),
            Some("https://registry.yarnpkg.com")
        );
        assert_eq!(
            resolve_input("TENCENT-HTTP").map(|m| m.url),
            Some("http://mirrors.tencent.com/npm")
        );
        assert_eq!(resolve_input("1").map(|m| m.key), Some("npm"));
        assert_eq!(resolve_input("6").map(|m| m.key), Some("yarn"));
        assert!(resolve_input("7").is_none());
        assert!(resolve_input("0").is_none());
        assert!(resolve_input("nosuchthing").is_none());
    }

    #[test]
    fn http_mirror_is_flagged_insecure() {
        assert!(is_insecure("http://mirrors.tencent.com/npm"));
        assert!(!is_insecure("https://registry.npmjs.org"));
        let w = insecure_warning("http://mirrors.tencent.com/npm")
            .expect("http has a warning");
        assert!(w.contains("unencrypted"));
        assert!(
            w.contains("sha512"),
            "we mention that integrity still protects"
        );
        assert!(insecure_warning("https://registry.npmjs.org").is_none());
    }

    #[test]
    fn candidates_are_filtered_and_sorted() {
        let results = vec![
            cr("https://registry.npmjs.org", false, 8000), // current, failed
            cr("https://registry.npmmirror.com", true, 300),
            cr("https://repo.huaweicloud.com/repository/npm", false, 8000), // unreachable
            cr("http://mirrors.tencent.com/npm", true, 120),
            cr("https://registry.yarnpkg.com", true, 900),
        ];
        let ranked = rank_candidates(results, "https://registry.npmjs.org");
        let urls: Vec<&str> = ranked.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "http://mirrors.tencent.com/npm", // 120 ms — fastest of all
                "https://registry.npmmirror.com", // 300 ms
                "https://registry.yarnpkg.com",   // 900 ms
            ],
            "only live ones, without the current, fastest first"
        );
    }

    #[test]
    fn current_excluded_even_with_trailing_slash() {
        let results = vec![
            cr("https://registry.npmjs.org", true, 100),
            cr("https://registry.npmmirror.com", true, 200),
        ];
        let ranked = rank_candidates(results, "https://registry.npmjs.org/");
        assert_eq!(ranked.len(), 1, "we don't retry the failed current one");
        assert_eq!(ranked[0].url, "https://registry.npmmirror.com");
    }

    #[test]
    fn name_lookup_for_logs() {
        assert_eq!(
            name_for_url("https://registry.npmmirror.com/"),
            "npmmirror / CNPM"
        );
        assert_eq!(
            name_for_url("https://example.invalid"),
            "https://example.invalid"
        );
    }
}
