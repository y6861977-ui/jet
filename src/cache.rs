//! On-disk tarball cache — a port of cache.ts. ~/.jet/cache, key name@version.

use crate::config::cache_dir;
use std::fs;
use std::path::PathBuf;

fn safe_name(name: &str) -> String {
    name.replace(['/', '@'], "_")
}

pub fn tarball_path(name: &str, version: &str) -> PathBuf {
    cache_dir().join(format!("{}-{version}.tgz", safe_name(name)))
}

pub fn has(name: &str, version: &str) -> bool {
    tarball_path(name, version).exists()
}

pub fn read(name: &str, version: &str) -> Result<Vec<u8>, String> {
    fs::read(tarball_path(name, version)).map_err(|e| format!("Can't read the cache: {e}"))
}

pub fn write(name: &str, version: &str, data: &[u8]) -> Result<(), String> {
    fs::write(tarball_path(name, version), data).map_err(|e| format!("Can't write the cache: {e}"))
}

pub fn remove(name: &str, version: &str) {
    let _ = fs::remove_file(tarball_path(name, version));
}

/// (number of tarballs, total size in bytes).
pub fn stats() -> (usize, u64) {
    let mut count = 0;
    let mut bytes = 0;
    if let Ok(entries) = fs::read_dir(cache_dir()) {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "tgz") {
                count += 1;
                if let Ok(m) = e.metadata() {
                    bytes += m.len();
                }
            }
        }
    }
    (count, bytes)
}

pub fn clear() -> usize {
    let mut removed = 0;
    if let Ok(entries) = fs::read_dir(cache_dir()) {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "tgz") {
                if fs::remove_file(e.path()).is_ok() {
                    removed += 1;
                }
            }
        }
    }
    removed
}
