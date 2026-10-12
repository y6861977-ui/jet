//! Install orchestration — a port of installer.ts.
//! resolve -> download tarballs -> unpack into the store -> lay out with hardlinks
//! -> lock -> history. Nested layout (like npm@2), dedup by ancestor visibility.
//!
//! Auto registry switching: both resolution and tarball downloads run against
//! ONE registry (the tarball URL comes from the packument, i.e. it points at the
//! host of the registry we resolved against). So an attempt is a "resolve +
//! download" pair: if the registry turned out unreachable at either stage, the
//! whole pair is retried against the next live mirror.

use crate::lockfile::{build_lock, read_lock, write_lock};
use crate::lockresolve::resolve_from_lock;
use crate::registry::looks_unreachable;
use crate::resolver::{ResolveResult, ResolvedNode, resolve};
use crate::storage::get_tarball;
use crate::store::{has_in_store, link_package, populate_store, store_path};
use crate::{config, history, mirrors, platform, registry};
use std::collections::HashMap;
use std::path::Path;

/// Whether the package is needed on THIS machine. Allowed by the tree is not yet
/// installed: platform variants for other systems exist in the tree and the lock,
/// but there's no point downloading and laying them out.
fn wanted_here(node: &ResolvedNode) -> bool {
    platform::matches(&node.os, &node.cpu)
}

/// A warning about a package that doesn't fit this machine.
///
/// We stay quiet about optional ones: of eight platform variants, seven will
/// ALWAYS not fit, and mentioning it only clutters the output. But a required
/// package that isn't for this platform is a real problem worth reporting.
fn platform_warning(node: &ResolvedNode) -> Option<String> {
    if node.optional || wanted_here(node) {
        return None;
    }
    Some(format!(
        "{}@{} is not meant for this platform ({}); skipped",
        node.name,
        node.version,
        platform::describe(&node.os, &node.cpu)
    ))
}

/// A note that a package has an install script.
///
/// We don't run them on principle — it's the main vector of supply-chain attacks,
/// and jet doesn't have it by design. But we can't stay silent: some tools place
/// their native binary into position precisely in an install script, and without
/// it the package installs but won't work.
fn install_script_warning(node: &ResolvedNode) -> Option<String> {
    if !node.has_install_script {
        return None;
    }
    Some(format!(
        "{}@{} expects an install script — jet doesn't run them (that's safer). \
         If the package doesn't work, this is why.",
        node.name, node.version
    ))
}

/// How an attempt to install everything from one specific registry ended.
enum AttemptError {
    /// The registry is unreachable (network/DNS/timeout) — worth trying a mirror.
    Unreachable(String),
    /// A non-network error (no such package, integrity mismatch) — a mirror won't help.
    Fatal(String),
}

/// A successful attempt: the tree is resolved and all tarballs are in the store.
struct Attempt {
    resolution: ResolveResult,
    downloaded: usize,
    cached: usize,
    from_lock: bool,
}

/// One attempt against a specific registry: resolve + download all tarballs.
///
/// `allow_lock` is turned off when retrying on a mirror: the lock file holds
/// tarball URLs from the old (failed) host, so a fresh resolution is needed there.
fn attempt_with_registry(
    root: &Path,
    deps: &[(String, String)],
    registry_url: &str,
    allow_lock: bool,
    log: &mut dyn FnMut(&str),
) -> Result<Attempt, AttemptError> {
    // 1. Resolution: the lock first (if allowed and it fits), otherwise the network.
    let mut from_lock = false;
    let resolution = match allow_lock
        .then(|| read_lock(root).and_then(|lock| resolve_from_lock(deps, &lock)))
        .flatten()
    {
        Some(res) => {
            from_lock = true;
            log("Using versions from the lock file.");
            res
        }
        None => {
            log("Resolving dependencies...");
            resolve(deps, |n| registry::fetch_packument_from(n, registry_url))
        }
    };

    // The root failed over the network — a mirror may help. Failed with a 404 — it can't.
    if let Some(f) = resolution
        .failed_roots
        .iter()
        .find(|f| looks_unreachable(&f.message))
    {
        return Err(AttemptError::Unreachable(format!(
            "{} — {}",
            f.name, f.message
        )));
    }
    if resolution.roots.is_empty() {
        if let Some(f) = resolution.failed_roots.first() {
            return Err(AttemptError::Fatal(f.message.clone()));
        }
    }

    // 2. Fill the store: download+unpack each unique version exactly once.
    // We skip foreign platforms BEFORE downloading — that's the whole point:
    // claude-code has eight platform variants, and exactly one needs downloading.
    log("Downloading and unpacking...");
    let mut downloaded = 0;
    let mut cached = 0;
    for node in resolution.flat.values() {
        if !wanted_here(node) {
            continue;
        }
        if has_in_store(&node.name, &node.version) {
            continue;
        }
        let tb = match get_tarball(
            &node.name,
            &node.version,
            &node.tarball,
            node.integrity.as_deref(),
        ) {
            Ok(tb) => tb,
            Err(e) if looks_unreachable(&e) => {
                // The tarball won't download from this registry — also a reason to switch mirror.
                return Err(AttemptError::Unreachable(format!(
                    "{}@{} — {e}",
                    node.name, node.version
                )));
            }
            Err(e) => return Err(AttemptError::Fatal(e)),
        };
        if tb.from_cache {
            cached += 1;
        } else {
            downloaded += 1;
        }
        if let Err(e) = populate_store(&node.name, &node.version, &tb.path) {
            return Err(AttemptError::Fatal(e));
        }
    }

    Ok(Attempt {
        resolution,
        downloaded,
        cached,
        from_lock,
    })
}

/// Resolve + download with auto failover to a mirror. We ask the user nothing:
/// if it failed over the network, we find a live mirror ourselves and retry.
fn attempt_with_failover(
    root: &Path,
    deps: &[(String, String)],
    opts: &InstallOptions,
    log: &mut dyn FnMut(&str),
) -> Result<Attempt, String> {
    let current = config::get_registry();
    let first = match attempt_with_registry(root, deps, &current, opts.use_lock, log) {
        Ok(a) => return Ok(a),
        Err(AttemptError::Fatal(m)) => return Err(m),
        Err(AttemptError::Unreachable(m)) => m,
    };

    log(&format!("Registry {current} is not responding ({first})."));
    log("Looking for a working mirror (checking all, picking the fastest)...");

    let candidates = mirrors::candidates_by_speed(&current);
    if candidates.is_empty() {
        return Err(format!(
            "Registry {current} is unreachable ({first}), and none of the {} mirrors answered. \
             Looks like a network problem. List: jet registry mirrors",
            mirrors::MIRRORS.len()
        ));
    }

    let mut tried = vec![format!("{current} — {first}")];
    for c in &candidates {
        log(&format!(
            "Trying mirror: {} ({}) — {} ms",
            c.name, c.url, c.ms
        ));
        match attempt_with_registry(root, deps, &c.url, false, log) {
            Ok(a) => {
                // Remember the working mirror so the next install goes straight to it.
                if let Err(e) = config::set_registry(&c.url) {
                    log(&format!("Failed to save the registry choice: {e}"));
                }
                log(&format!("Switched to mirror: {} ({})", c.name, c.url));
                if let Some(w) = mirrors::insecure_warning(&c.url) {
                    log(&w);
                }
                return Ok(a);
            }
            Err(AttemptError::Fatal(m)) => return Err(m),
            Err(AttemptError::Unreachable(m)) => {
                log(&format!("Mirror {} didn't work out: {m}", c.name));
                tried.push(format!("{} — {m}", c.url));
            }
        }
    }

    Err(format!(
        "Couldn't install from any registry. Tried:\n  {}\nCheck connectivity: jet registry check",
        tried.join("\n  ")
    ))
}

pub struct InstallReport {
    pub installed: usize,
    pub deduped: usize,
    pub downloaded: usize,
    pub cached: usize,
    pub from_lock: bool,
    pub warnings: Vec<String>,
    pub roots: Vec<ResolvedNode>,
    pub failed_roots: Vec<crate::resolver::FailedRoot>,
}

#[derive(Default)]
pub struct InstallOptions {
    pub use_lock: bool,
    pub record_history: bool,
}

pub fn install(
    root: &Path,
    deps: &[(String, String)],
    opts: &InstallOptions,
    log: &mut dyn FnMut(&str),
) -> Result<InstallReport, String> {
    // 1-2. Resolution and download (with auto registry switching on network failure).
    let Attempt {
        resolution,
        downloaded,
        cached,
        from_lock,
    } = attempt_with_failover(root, deps, opts, log)?;

    // 3. Lay out into node_modules with hardlinks, dedup along the ancestor chain.
    let mut installed = 0;
    let mut deduped = 0;
    layout(
        &resolution.roots,
        &resolution.flat,
        &root.join("node_modules"),
        &HashMap::new(),
        &mut installed,
        &mut deduped,
    );

    // 4. Lock.
    write_lock(root, &build_lock(&resolution.flat))?;

    // 5. The roots, for the report.
    let roots: Vec<ResolvedNode> = resolution
        .roots
        .iter()
        .filter_map(|k| resolution.flat.get(k).cloned())
        .collect();

    // 6. History.
    if opts.record_history {
        history::record_installs(&roots, &history::default_dir());
    }

    // 7. Warnings. We mention install scripts only for the ROOTS: for a package
    // deep in the tree it's usually not the user's concern, but a tool requested
    // by hand that lacks its script may not work.
    let mut warnings = resolution.warnings;
    // Packages for foreign platforms are collapsed into ONE line with a count: a
    // tool like TypeScript 7 has a couple dozen, and a line-by-line list is noise
    // the user can't do anything about anyway.
    let skipped_foreign = resolution
        .flat
        .values()
        .filter(|n| platform_warning(n).is_some())
        .count();
    if skipped_foreign > 0 {
        warnings.push(format!(
            "Skipped packages for other platforms: {skipped_foreign} (this machine doesn't need them)"
        ));
    }
    // A package with no checksum is downloaded unverified — we don't stay silent about it.
    // The official registry always returns integrity, so normally there's no line;
    // its appearance is a sign of a nonstandard/tampered mirror.
    let no_integrity = resolution
        .flat
        .values()
        .filter(|n| wanted_here(n) && n.integrity.is_none())
        .count();
    if no_integrity > 0 {
        warnings.push(format!(
            "Packages without integrity: {no_integrity} — integrity not verified (usually a sign of a nonstandard mirror)"
        ));
    }
    warnings.extend(roots.iter().filter_map(install_script_warning));
    if resolution.optional_unreachable {
        warnings.push(
            "Some optional dependencies didn't download due to the network — among them can be \
             native binaries. Repeat the install when your connection is better."
                .to_string(),
        );
    }

    Ok(InstallReport {
        installed,
        deduped,
        downloaded,
        cached,
        from_lock,
        warnings,
        roots,
        failed_roots: resolution.failed_roots,
    })
}

fn layout(
    keys: &[String],
    flat: &HashMap<String, ResolvedNode>,
    target_nm: &Path,
    ancestors: &HashMap<String, String>,
    installed: &mut usize,
    deduped: &mut usize,
) {
    // At this level all nodes are mutually visible (and to their descendants).
    let mut level_map = ancestors.clone();
    for key in keys {
        if let Some(n) = flat.get(key) {
            level_map.insert(n.name.clone(), n.version.clone());
        }
    }
    for key in keys {
        let Some(node) = flat.get(key) else { continue };
        if !wanted_here(node) {
            continue; // foreign platform: neither it nor its subtree should be here
        }
        if ancestors.get(&node.name) == Some(&node.version) {
            *deduped += 1; // this version is visible higher up the chain
            continue;
        }
        let dest = target_nm.join(&node.name);
        if link_package(&store_path(&node.name, &node.version), &dest).is_ok() {
            *installed += 1;
        }
        if !node.dependencies.is_empty() {
            layout(
                &node.dependencies,
                flat,
                &dest.join("node_modules"),
                &level_map,
                installed,
                deduped,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, os: &[&str], cpu: &[&str]) -> ResolvedNode {
        ResolvedNode {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            os: os.iter().map(|s| s.to_string()).collect(),
            cpu: cpu.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn foreign_platform_packages_are_skipped() {
        // The very case: eight platform variants, exactly one to download.
        assert!(!wanted_here(&node(
            "@esbuild/darwin-arm64",
            &["darwin"],
            &["arm64"]
        )));
        assert!(
            wanted_here(&node("plain-js-package", &[], &[])),
            "no constraints — needed by everyone"
        );
        let ours = node(
            "ours",
            &[platform::current_os()],
            &[platform::current_cpu()],
        );
        assert!(wanted_here(&ours), "a package exactly for our platform");
    }

    #[test]
    fn platform_mismatch_is_only_reported_for_required_packages() {
        // A foreign platform variant among the optionals is normal — stay quiet.
        let mut opt = node("@esbuild/darwin-arm64", &["darwin"], &["arm64"]);
        opt.optional = true;
        assert!(platform_warning(&opt).is_none());

        // But a required package not for this platform is worth saying out loud.
        let required = node("@esbuild/darwin-arm64", &["darwin"], &["arm64"]);
        let warning =
            platform_warning(&required).expect("a required package should produce a warning");
        assert!(warning.contains("@esbuild/darwin-arm64"));
        assert!(
            warning.contains("darwin"),
            "the text shows who the package is meant for"
        );
    }

    #[test]
    fn install_script_notice_names_the_package() {
        let mut n = node("@anthropic-ai/claude-code", &[], &[]);
        n.has_install_script = true;
        let notice =
            install_script_warning(&n).expect("a package with an install script should produce a note");
        assert!(notice.contains("@anthropic-ai/claude-code"));
        assert!(!install_script_warning(&node("plain", &[], &[])).is_some());
    }
}
