//! Content store of unpacked packages + layout with hardlinks — a port of store.ts.
//! Each version is unpacked into ~/.jet/store once; node_modules gets links.

use crate::config::store_dir;
use crate::targz;
use std::fs;
use std::path::{Path, PathBuf};

fn safe_name(name: &str) -> String {
    name.replace(['/', '@'], "_")
}

pub fn store_path(name: &str, version: &str) -> PathBuf {
    store_dir().join(format!("{}@{version}", safe_name(name)))
}

pub fn has_in_store(name: &str, version: &str) -> bool {
    store_path(name, version).exists()
}

/// Unpack a tarball into the store (idempotent, atomic via temp+rename).
pub fn populate_store(name: &str, version: &str, tgz_path: &Path) -> Result<PathBuf, String> {
    let dest = store_path(name, version);
    if dest.exists() {
        return Ok(dest);
    }
    let tmp = dest.with_extension(format!("tmp-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    targz::extract_to(tgz_path, &tmp)?;
    if fs::rename(&tmp, &dest).is_err() {
        // Someone got there first (a race) — use what's ready, remove our temp.
        let _ = fs::remove_dir_all(&tmp);
        if !dest.exists() {
            return Err(format!("Failed to lay out {name}@{version} into the store"));
        }
    }
    Ok(dest)
}

/// Lay out a package from the store into dest with hardlinks (fallback to copy on cross-device).
pub fn link_package(src: &Path, dest: &Path) -> Result<(), String> {
    let _ = fs::remove_dir_all(dest);
    link_tree(src, dest)
}

fn link_tree(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("Can't create {}: {e}", dest.display()))?;
    let entries = fs::read_dir(src).map_err(|e| format!("Can't read {}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            link_tree(&from, &to)?;
        } else {
            link_file(&from, &to)?;
        }
    }
    Ok(())
}

fn link_file(from: &Path, to: &Path) -> Result<(), String> {
    let _ = fs::remove_file(to);
    if fs::hard_link(from, to).is_err() {
        // EXDEV (a different volume) and such — just copy.
        fs::copy(from, to).map_err(|e| format!("Can't copy {}: {e}", from.display()))?;
    }
    Ok(())
}

pub fn clear() -> usize {
    let mut removed = 0;
    if let Ok(entries) = fs::read_dir(store_dir()) {
        for e in entries.flatten() {
            if fs::remove_dir_all(e.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}
