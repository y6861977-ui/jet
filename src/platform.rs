//! Selecting packages by platform — the `os` and `cpu` fields from the manifest.
//!
//! Why this is needed. Native tools (esbuild, rollup, swc, claude-code) publish
//! the binary not inside the main package but as separate packages per system:
//! `@esbuild/win32-x64`, `@esbuild/darwin-arm64`, and so on. The main package
//! lists them all in `optionalDependencies`, and each one declares who it's for:
//!
//! ```json
//! "os":  ["win32"],
//! "cpu": ["x64"]
//! ```
//!
//! Without this selection the choice is bad: either pull all eight variants (for
//! claude-code that's over a gigabyte — a death sentence on a slow network), or
//! install none and get a non-working tool.
//!
//! The list syntax is npm's:
//!   - an empty list (or its absence) imposes no constraints;
//!   - an entry `!win32` is a denial, "everywhere except Windows";
//!   - `any` allows everything.
//! The values come from the Node.js world (`process.platform` / `process.arch`),
//! not the Rust world, so we translate the names.

/// Our system in Node.js `process.platform` terms.
pub fn current_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else if cfg!(target_os = "openbsd") {
        "openbsd"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        // It's more honest to call an unknown system unknown than to pretend it's
        // linux: a package with a constraint simply won't match, and we'll warn.
        "unknown"
    }
}

/// Our architecture in Node.js `process.arch` terms.
pub fn current_cpu() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "ia32"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else if cfg!(target_arch = "powerpc64") {
        "ppc64"
    } else if cfg!(target_arch = "riscv64") {
        "riscv64"
    } else if cfg!(target_arch = "s390x") {
        "s390x"
    } else if cfg!(target_arch = "loongarch64") {
        "loong64"
    } else {
        "unknown"
    }
}

/// Whether a value passes a constraint list (shared logic for os and cpu).
///
/// The rules are exactly npm's: an explicit denial outweighs everything; if there
/// are any allowing entries, the value must match at least one; a list of only
/// denials lets through anything not listed in it.
fn passes(value: &str, list: &[String]) -> bool {
    if list.is_empty() {
        return true;
    }
    let mut negations = 0;
    let mut allowed = false;
    for entry in list {
        match entry.strip_prefix('!') {
            Some(denied) => {
                if denied == value {
                    return false;
                }
                negations += 1;
            }
            None => {
                if entry == value || entry == "any" {
                    allowed = true;
                }
            }
        }
    }
    // No allowing entries at all — the list was all denials, we didn't hit any of
    // them, so we pass.
    allowed || negations == list.len()
}

/// Whether a package with these constraints fits this machine.
pub fn matches(os: &[String], cpu: &[String]) -> bool {
    passes(current_os(), os) && passes(current_cpu(), cpu)
}

/// A human-readable description of the constraints — for warnings.
/// An empty list is printed as `any`, so as not to show empty brackets.
pub fn describe(os: &[String], cpu: &[String]) -> String {
    fn one(list: &[String]) -> String {
        if list.is_empty() {
            "any".to_string()
        } else {
            list.join(", ")
        }
    }
    format!("os: {}, cpu: {}", one(os), one(cpu))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_list_allows_everything() {
        assert!(passes("win32", &[]));
        assert!(passes("anything", &[]));
    }

    #[test]
    fn positive_list_requires_a_hit() {
        assert!(passes("win32", &list(&["win32"])));
        assert!(passes("darwin", &list(&["darwin", "linux"])));
        assert!(!passes("win32", &list(&["darwin", "linux"])));
    }

    #[test]
    fn negation_denies_and_lets_the_rest_through() {
        assert!(!passes("win32", &list(&["!win32"])));
        assert!(
            passes("linux", &list(&["!win32"])),
            "a list of only denials lets the rest through"
        );
        assert!(!passes("darwin", &list(&["!win32", "!darwin"])));
    }

    #[test]
    fn explicit_denial_beats_permission() {
        // Mixed list: a denial is stronger than a permission.
        assert!(!passes("win32", &list(&["win32", "!win32"])));
    }

    #[test]
    fn any_allows_everything() {
        assert!(passes("loong64", &list(&["any"])));
    }

    #[test]
    fn matches_checks_both_axes() {
        // A package for our system but a foreign architecture — doesn't fit.
        let our_os = list(&[current_os()]);
        assert!(matches(&our_os, &[]));
        assert!(!matches(&our_os, &list(&["nonexistent-arch"])));
        assert!(matches(&[], &[]), "a package with no constraints fits everyone");
    }

    #[test]
    fn current_platform_is_recognized() {
        // On a supported machine we must know our own name — otherwise platform
        // packages silently stop installing.
        assert_ne!(current_os(), "unknown");
        assert_ne!(current_cpu(), "unknown");
    }

    #[test]
    fn describe_is_readable() {
        assert_eq!(
            describe(&list(&["win32"]), &list(&["x64"])),
            "os: win32, cpu: x64"
        );
        assert_eq!(describe(&[], &[]), "os: any, cpu: any");
    }
}
