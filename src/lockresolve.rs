//! Reconstructing the tree from the lock without the network — a port of lockresolve.ts.

use crate::lockfile::{LockFile, split_key};
use crate::resolver::{ResolveResult, ResolvedNode};
use crate::semver::{max_satisfying, parse_range, satisfies_str};
use std::collections::HashMap;

/// Find in the lock the highest version of package `name` that satisfies `range`.
fn find_locked_version(lock: &LockFile, name: &str, range: &str) -> Option<String> {
    let candidates: Vec<String> = lock
        .packages
        .keys()
        .filter_map(|k| split_key(k))
        .filter(|(n, _)| n == name)
        .map(|(_, v)| v)
        .collect();
    // A dist-tag latest/exact isn't stored; we rely on the range.
    let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    if let Some(v) = max_satisfying(&refs, &parse_range(range)) {
        return Some(v);
    }
    // A range like "latest" can't be resolved from the lock — take the single version, if there's one.
    if candidates.len() == 1 {
        return Some(candidates[0].clone());
    }
    candidates.into_iter().find(|v| satisfies_str(v, range))
}

/// Build the tree from the lock. None — if the lock doesn't cover the requirement (online needed).
pub fn resolve_from_lock(root_deps: &[(String, String)], lock: &LockFile) -> Option<ResolveResult> {
    let mut flat: HashMap<String, ResolvedNode> = HashMap::new();

    fn build(
        lock: &LockFile,
        name: &str,
        version: &str,
        requested: &str,
        flat: &mut HashMap<String, ResolvedNode>,
    ) -> Option<()> {
        let key = format!("{name}@{version}");
        let entry = lock.packages.get(&key)?;
        if flat.contains_key(&key) {
            return Some(()); // dedup/cycle
        }
        flat.insert(
            key.clone(),
            ResolvedNode {
                name: name.to_string(),
                version: version.to_string(),
                requested: requested.to_string(),
                tarball: entry.resolved.clone(),
                integrity: entry.integrity.clone(),
                unpacked_size: None,
                dependencies: Vec::new(),
                // Platform constraints come from the lock — thanks to them a file
                // built on another OS works correctly here.
                os: entry.os.clone(),
                cpu: entry.cpu.clone(),
                optional: entry.optional,
                // Not in the lock: we don't run install scripts anyway, and the
                // warning was already shown on the first install.
                has_install_script: false,
            },
        );
        // Child order is deterministic.
        let mut deps: Vec<(String, String)> = entry
            .dependencies
            .iter()
            .map(|(n, v)| (n.clone(), v.clone()))
            .collect();
        deps.sort_by(|a, b| a.0.cmp(&b.0));
        let mut child_keys = Vec::new();
        for (dn, dv) in &deps {
            build(lock, dn, dv, dv, flat)?;
            child_keys.push(format!("{dn}@{dv}"));
        }
        flat.get_mut(&key).unwrap().dependencies = child_keys;
        Some(())
    }

    let mut roots = Vec::new();
    for (name, range) in root_deps {
        let version = find_locked_version(lock, name, range)?;
        build(lock, name, &version, range, &mut flat)?;
        roots.push(format!("{name}@{version}"));
    }

    Some(ResolveResult {
        roots,
        flat,
        warnings: Vec::new(),
        failed_roots: Vec::new(),
        optional_unreachable: false,
    })
}
