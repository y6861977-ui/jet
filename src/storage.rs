//! Fetching tarballs (cache + integrity check + download) — a port of storage.ts.

use crate::hash;
use crate::{cache, config};
use std::path::PathBuf;
use std::process::Command;

/// Integrity check in the "sha512-<base64>" format (sha512/384/256).
pub fn verify_integrity(data: &[u8], integrity: &str) -> bool {
    let Some((algo, expected)) = integrity.split_once('-') else {
        return false;
    };
    let digest = match algo {
        "sha512" => hash::sha512(data),
        "sha384" => hash::sha384(data),
        "sha256" => hash::sha256(data),
        _ => return false,
    };
    hash::base64_encode(&digest) == expected
}

/// Download a tarball from a URL into a file via curl.
fn download(url: &str, dest: &std::path::Path) -> Result<(), String> {
    // The URL is from an untrusted packument — allow only http/https.
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("Unsafe tarball URL scheme: {url}"));
    }
    // --max-redirs caps the redirect chain: without a limit a public https URL could
    // use a 30x redirect to lead curl to an internal address (SSRF bypassing the
    // scheme check above). The contents are gated by the integrity check anyway, but
    // an extra line of defense doesn't hurt. (Full blocking of private addresses is a
    // task for the port; the TS version has it.)
    let out = Command::new("curl")
        .args(["-sSL", "--max-redirs", "5", "--max-time", "120", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|e| format!("Failed to run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Couldn't download the tarball: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

pub struct TarballResult {
    pub path: PathBuf,
    pub from_cache: bool,
}

/// Get a tarball: from the cache (if intact), otherwise download and verify integrity.
pub fn get_tarball(
    name: &str,
    version: &str,
    tarball_url: &str,
    integrity: Option<&str>,
) -> Result<TarballResult, String> {
    let path = cache::tarball_path(name, version);
    if cache::has(name, version) {
        let cached = cache::read(name, version)?;
        if integrity.is_none_or(|i| verify_integrity(&cached, i)) {
            return Ok(TarballResult {
                path,
                from_cache: true,
            });
        }
        cache::remove(name, version); // cache corrupted
    }

    download(tarball_url, &path)?;
    if let Some(i) = integrity {
        let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read the download: {e}"))?;
        if !verify_integrity(&bytes, i) {
            cache::remove(name, version);
            return Err(format!(
                "Integrity check failed for {name}@{version}"
            ));
        }
    }
    let _ = config::cache_dir(); // ensure the directory exists
    Ok(TarballResult {
        path,
        from_cache: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrity_check() {
        let data = b"hello world";
        let good = format!("sha512-{}", hash::base64_encode(&hash::sha512(data)));
        assert!(verify_integrity(data, &good));
        assert!(!verify_integrity(data, "sha512-bogus"));
        assert!(!verify_integrity(data, "md5-xxx"));
        let s256 = format!("sha256-{}", hash::base64_encode(&hash::sha256(data)));
        assert!(verify_integrity(data, &s256));
    }
}
