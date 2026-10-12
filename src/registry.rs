//! Registry client — a port of registry.ts.
//! HTTPS via the system curl (present on Windows 10+/macOS/Linux); the response
//! is parsed by our own JSON parser. We run no registry of our own, only read others'.

use crate::json::{self, Json};
use crate::types::{Dist, Packument, VersionManifest};
use std::collections::HashMap;
use std::process::Command;

const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";

/// Does the error message look like the registry being unreachable (rather than
/// "no such package")? Errors like "package not found" / "bad integrity" aren't
/// fixed by a mirror.
///
/// Lives here because it classifies the messages produced by `fetch_raw` and the
/// tarball downloader. Two callers use it: the installer (deciding whether to
/// switch mirror) and the resolver (deciding whether to complain about a failed
/// optional dependency).
pub fn looks_unreachable(msg: &str) -> bool {
    let m = msg.to_lowercase();
    if m.contains("not found") || m.contains("integrity") {
        return false; // a 404 and a bad tarball will be the same on another mirror
    }
    m.contains("unreachable")
        || m.contains("curl")
        || m.contains("timed out")
        || m.contains("resolve host")
        || m.contains("download the tarball")
}

/// Validate the package name before the request: reject path traversal and junk.
pub fn is_valid_package_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 214 {
        return false;
    }
    if name.contains("..") {
        return false;
    }
    if name.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') {
        return false;
    }
    if name.chars().any(char::is_whitespace) {
        return false; // npm forbids spaces in package names
    }
    if let Some(scoped) = name.strip_prefix('@') {
        // @scope/name — exactly one slash, no spaces.
        let parts: Vec<&str> = scoped.split('/').collect();
        return parts.len() == 2
            && parts
                .iter()
                .all(|p| !p.is_empty() && !p.contains(char::is_whitespace));
    }
    !name.contains('/') && !name.contains('\\')
}

/// Encode a name for the URL. For scoped names encode the scope and the tail, slash -> %2F.
fn encode_package_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix('@') {
        if let Some((scope, pkg)) = rest.split_once('/') {
            return format!(
                "{}%2F{}",
                encode_component(&format!("@{scope}")),
                encode_component(pkg)
            );
        }
    }
    encode_component(name)
}

/// Percent-encode a URL component (what's needed for package names).
fn encode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// HTTP GET via curl. Returns the body as a string.
fn fetch_raw(url: &str) -> Result<String, String> {
    let out = Command::new("curl")
        .args([
            "-sSL",
            "--max-time",
            "20",
            "-H",
            "Accept: application/vnd.npm.install-v1+json, application/json",
            url,
        ])
        .output()
        .map_err(|e| format!("Failed to run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Registry unreachable: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|_| "Registry response is not UTF-8".to_string())
}

/// Fetch and parse a package's packument.
pub fn fetch_packument(name: &str) -> Result<Packument, String> {
    fetch_packument_from(name, DEFAULT_REGISTRY)
}

pub fn fetch_packument_from(name: &str, registry: &str) -> Result<Packument, String> {
    if !is_valid_package_name(name) {
        return Err(format!("Invalid package name: {name:?}"));
    }
    let url = format!(
        "{}/{}",
        registry.trim_end_matches('/'),
        encode_package_name(name)
    );
    let body = fetch_raw(&url)?;
    if body.contains("\"Not found\"") || body.trim() == "{\"error\":\"Not found\"}" {
        return Err(format!("Package not found in the registry: {name}"));
    }
    parse_packument(&body)
}

/// Parse a packument body (a pure function — tested without the network).
pub fn parse_packument(body: &str) -> Result<Packument, String> {
    let doc = json::parse(body)?;
    let name = doc
        .get("name")
        .and_then(Json::as_str)
        .ok_or("The packument has no name field")?
        .to_string();

    let mut dist_tags = HashMap::new();
    if let Some(obj) = doc.get("dist-tags").and_then(Json::as_object) {
        for (tag, ver) in obj {
            if let Some(v) = ver.as_str() {
                dist_tags.insert(tag.clone(), v.to_string());
            }
        }
    }

    let mut versions = HashMap::new();
    if let Some(obj) = doc.get("versions").and_then(Json::as_object) {
        for (ver, manifest) in obj {
            versions.insert(ver.clone(), parse_manifest(&name, ver, manifest)?);
        }
    }
    if versions.is_empty() {
        return Err(format!("Package {name} has no published versions"));
    }

    Ok(Packument {
        name,
        dist_tags,
        versions,
    })
}

/// A "name -> range" map from a manifest field (dependencies and the like).
fn parse_dep_map(manifest: &Json, field: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(obj) = manifest.get(field).and_then(Json::as_object) {
        for (dep, range) in obj {
            if let Some(r) = range.as_str() {
                out.insert(dep.clone(), r.to_string());
            }
        }
    }
    out
}

/// A list of strings from a manifest field (os/cpu). Non-string items are dropped —
/// the registry is foreign, its shape can't be trusted.
fn parse_str_list(manifest: &Json, field: &str) -> Vec<String> {
    manifest
        .get(field)
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_manifest(
    pkg_name: &str,
    version: &str,
    manifest: &Json,
) -> Result<VersionManifest, String> {
    let dependencies = parse_dep_map(manifest, "dependencies");
    let optional_dependencies = parse_dep_map(manifest, "optionalDependencies");
    let os = parse_str_list(manifest, "os");
    let cpu = parse_str_list(manifest, "cpu");
    let has_install_script = manifest
        .get("hasInstallScript")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let scripts = parse_dep_map(manifest, "scripts");
    let deprecated = manifest
        .get("deprecated")
        .and_then(Json::as_str)
        .map(str::to_string);

    let dist_json = manifest.get("dist");
    let tarball = dist_json
        .and_then(|d| d.get("tarball"))
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    let integrity = dist_json
        .and_then(|d| d.get("integrity"))
        .and_then(Json::as_str)
        .map(str::to_string);
    let unpacked_size = dist_json
        .and_then(|d| d.get("unpackedSize"))
        .and_then(Json::as_f64)
        .map(|n| n as u64);

    Ok(VersionManifest {
        name: pkg_name.to_string(),
        version: version.to_string(),
        dependencies,
        optional_dependencies,
        os,
        cpu,
        has_install_script,
        scripts,
        deprecated,
        dist: Dist {
            tarball,
            integrity,
            unpacked_size,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(is_valid_package_name("express"));
        assert!(is_valid_package_name("@scope/pkg"));
        assert!(!is_valid_package_name("../../etc/passwd"));
        assert!(!is_valid_package_name("@scope/../evil"));
        assert!(!is_valid_package_name("a/b/c"));
        assert!(!is_valid_package_name("bad name"));
        assert!(!is_valid_package_name(""));
    }

    #[test]
    fn network_failures_look_unreachable() {
        // Messages from fetch_raw / storage::download on network problems.
        assert!(looks_unreachable(
            "Registry unreachable: curl: (6) Could not resolve host: nope.example"
        ));
        assert!(looks_unreachable(
            "Registry unreachable: curl: (28) Operation timed out after 20000 ms"
        ));
        assert!(looks_unreachable(
            "Couldn't download the tarball: curl: (7) Failed to connect"
        ));
        assert!(looks_unreachable("Failed to run curl: No such file"));
    }

    #[test]
    fn missing_package_does_not_look_unreachable() {
        // A 404 and a bad tarball reproduce exactly on a mirror — don't switch.
        assert!(!looks_unreachable(
            "Package not found in the registry: no-such-package"
        ));
        assert!(!looks_unreachable(
            "Integrity check failed for left-pad@1.3.0"
        ));
        assert!(!looks_unreachable(
            "No version of left-pad satisfies \"^99.0.0\" (available: 12 versions)"
        ));
        assert!(!looks_unreachable("Invalid package name: \"../evil\""));
    }

    #[test]
    fn not_found_wins_over_curl_mention() {
        // "not found" takes priority: even if the word curl flickers nearby.
        assert!(!looks_unreachable(
            "Package not found in the registry: x (curl 200)"
        ));
    }

    #[test]
    fn name_encoding() {
        assert_eq!(encode_package_name("express"), "express");
        assert_eq!(encode_package_name("@scope/pkg"), "%40scope%2Fpkg");
    }

    #[test]
    fn parse_sample_packument() {
        let body = r#"{
            "name": "demo",
            "dist-tags": { "latest": "2.0.0" },
            "versions": {
                "1.0.0": {
                    "name": "demo", "version": "1.0.0",
                    "dependencies": { "ms": "^2.0.0" },
                    "dist": { "tarball": "https://r/demo-1.0.0.tgz", "integrity": "sha512-x", "unpackedSize": 1234 }
                },
                "2.0.0": {
                    "name": "demo", "version": "2.0.0",
                    "dist": { "tarball": "https://r/demo-2.0.0.tgz" }
                }
            }
        }"#;
        let p = parse_packument(body).unwrap();
        assert_eq!(p.name, "demo");
        assert_eq!(p.dist_tags.get("latest").map(String::as_str), Some("2.0.0"));
        assert_eq!(p.versions.len(), 2);

        let v1 = &p.versions["1.0.0"];
        assert_eq!(
            v1.dependencies.get("ms").map(String::as_str),
            Some("^2.0.0")
        );
        assert_eq!(v1.dist.tarball, "https://r/demo-1.0.0.tgz");
        assert_eq!(v1.dist.integrity.as_deref(), Some("sha512-x"));
        assert_eq!(v1.dist.unpacked_size, Some(1234));

        let v2 = &p.versions["2.0.0"];
        assert!(
            v2.dependencies.is_empty(),
            "no dependencies — an empty map"
        );
        assert_eq!(v2.dist.integrity, None, "no integrity — None");
    }

    #[test]
    fn parse_platform_fields() {
        // Shape taken from the live registry: the abbreviated packument already carries
        // optionalDependencies, os/cpu, and hasInstallScript — no extra requests needed.
        let body = r#"{
            "name": "@anthropic-ai/claude-code",
            "dist-tags": { "latest": "2.1.258" },
            "versions": {
                "2.1.258": {
                    "name": "@anthropic-ai/claude-code", "version": "2.1.258",
                    "optionalDependencies": {
                        "@anthropic-ai/claude-code-win32-x64": "2.1.258",
                        "@anthropic-ai/claude-code-darwin-arm64": "2.1.258"
                    },
                    "os": ["win32", "darwin"],
                    "cpu": ["x64"],
                    "hasInstallScript": true,
                    "dist": { "tarball": "https://r/cc.tgz" }
                }
            }
        }"#;
        let p = parse_packument(body).unwrap();
        let v = &p.versions["2.1.258"];
        assert_eq!(v.optional_dependencies.len(), 2);
        assert_eq!(
            v.optional_dependencies
                .get("@anthropic-ai/claude-code-win32-x64")
                .map(String::as_str),
            Some("2.1.258")
        );
        assert_eq!(v.os, vec!["win32", "darwin"]);
        assert_eq!(v.cpu, vec!["x64"]);
        assert!(v.has_install_script);
        assert!(
            v.dependencies.is_empty(),
            "no regular dependencies — and that's not the same as optional"
        );
    }

    #[test]
    fn platform_fields_default_to_empty() {
        let body = r#"{
            "name": "plain", "dist-tags": { "latest": "1.0.0" },
            "versions": { "1.0.0": { "name": "plain", "version": "1.0.0", "dist": { "tarball": "u" } } }
        }"#;
        let v = &parse_packument(body).unwrap().versions["1.0.0"];
        assert!(v.os.is_empty(), "no os field — means no constraints");
        assert!(v.cpu.is_empty());
        assert!(v.optional_dependencies.is_empty());
        assert!(!v.has_install_script);
    }

    #[test]
    fn malformed_os_list_does_not_crash() {
        // The registry is foreign: os may hold anything. Non-strings are silently dropped.
        let body = r#"{
            "name": "weird", "dist-tags": { "latest": "1.0.0" },
            "versions": { "1.0.0": { "name": "weird", "version": "1.0.0",
                "os": ["win32", 42, null], "cpu": "x64",
                "dist": { "tarball": "u" } } }
        }"#;
        let v = &parse_packument(body).unwrap().versions["1.0.0"];
        assert_eq!(v.os, vec!["win32"], "junk dropped, strings kept");
        assert!(
            v.cpu.is_empty(),
            "cpu is not an array — treat it as no constraints"
        );
    }

    #[test]
    fn empty_versions_is_error() {
        let body = r#"{ "name": "x", "dist-tags": {}, "versions": {} }"#;
        assert!(parse_packument(body).is_err());
    }
}
