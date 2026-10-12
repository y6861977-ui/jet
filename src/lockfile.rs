//! The jet-lock.json lock file — a port of lockfile.ts. Byte-stable (keys are
//! sorted by the serializer), so parallel resolution gives the same result.
//!
//! Platform constraints (`os`/`cpu`) are written INTO THE LOCK deliberately: then a
//! file built on macOS is fully usable on Windows — the installer picks the fitting
//! one from the recorded variants itself. The opposite choice (writing only what fit
//! the current machine) would give a lock tied to a single OS.
//!
//! The `os`/`cpu`/`optional` fields are written only if there's something to say.
//! That way a regular package's entry stays exactly as it was before platform
//! selection existed: old lock files read, new ones don't bloat.

use crate::json::{self, Json};
use crate::resolver::ResolvedNode;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct LockEntry {
    pub version: String,
    pub resolved: String,
    pub integrity: Option<String>,
    pub dependencies: HashMap<String, String>,
    pub os: Vec<String>,
    pub cpu: Vec<String>,
    pub optional: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LockFile {
    pub packages: HashMap<String, LockEntry>,
}

/// Split a "name@version" key (handling scoped @scope/name@version).
pub fn split_key(key: &str) -> Option<(String, String)> {
    let at = key.rfind('@')?;
    if at == 0 {
        return None;
    }
    Some((key[..at].to_string(), key[at + 1..].to_string()))
}

/// Build the lock from the resolver's flat map.
pub fn build_lock(flat: &HashMap<String, ResolvedNode>) -> LockFile {
    let mut packages = HashMap::new();
    for (key, node) in flat {
        let mut deps = HashMap::new();
        for dep_key in &node.dependencies {
            if let Some((n, v)) = split_key(dep_key) {
                deps.insert(n, v);
            }
        }
        packages.insert(
            key.clone(),
            LockEntry {
                version: node.version.clone(),
                resolved: node.tarball.clone(),
                integrity: node.integrity.clone(),
                dependencies: deps,
                os: node.os.clone(),
                cpu: node.cpu.clone(),
                optional: node.optional,
            },
        );
    }
    LockFile { packages }
}

fn lock_path(root: &Path) -> std::path::PathBuf {
    root.join("jet-lock.json")
}

fn str_list(items: &[String]) -> Json {
    Json::Arr(items.iter().cloned().map(Json::Str).collect())
}

fn read_str_list(entry: &Json, field: &str) -> Vec<String> {
    entry
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

fn to_json(lock: &LockFile) -> Json {
    let mut packages = HashMap::new();
    for (key, e) in &lock.packages {
        let mut entry = HashMap::new();
        entry.insert("version".to_string(), Json::Str(e.version.clone()));
        entry.insert("resolved".to_string(), Json::Str(e.resolved.clone()));
        entry.insert(
            "integrity".to_string(),
            e.integrity.clone().map(Json::Str).unwrap_or(Json::Null),
        );
        let deps = e
            .dependencies
            .iter()
            .map(|(n, v)| (n.clone(), Json::Str(v.clone())))
            .collect();
        entry.insert("dependencies".to_string(), Json::Obj(deps));
        // Don't write empties: a regular package's entry stays the same.
        if !e.os.is_empty() {
            entry.insert("os".to_string(), str_list(&e.os));
        }
        if !e.cpu.is_empty() {
            entry.insert("cpu".to_string(), str_list(&e.cpu));
        }
        if e.optional {
            entry.insert("optional".to_string(), Json::Bool(true));
        }
        packages.insert(key.clone(), Json::Obj(entry));
    }
    let mut root = HashMap::new();
    root.insert("lockfileVersion".to_string(), Json::Num(1.0));
    root.insert("packages".to_string(), Json::Obj(packages));
    Json::Obj(root)
}

pub fn write_lock(root: &Path, lock: &LockFile) -> Result<(), String> {
    let text = json::stringify_pretty(&to_json(lock)) + "\n";
    std::fs::write(lock_path(root), text).map_err(|e| format!("Can't write the lock: {e}"))
}

pub fn read_lock(root: &Path) -> Option<LockFile> {
    let text = crate::project::read_text_tolerant(&lock_path(root))?;
    let doc = json::parse(&text).ok()?;
    let packages_json = doc.get("packages")?.as_object()?;
    let mut packages = HashMap::new();
    for (key, entry) in packages_json {
        let version = entry.get("version")?.as_str()?.to_string();
        let resolved = entry
            .get("resolved")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let integrity = entry
            .get("integrity")
            .and_then(Json::as_str)
            .map(str::to_string);
        let mut deps = HashMap::new();
        if let Some(dobj) = entry.get("dependencies").and_then(Json::as_object) {
            for (n, v) in dobj {
                if let Some(s) = v.as_str() {
                    deps.insert(n.clone(), s.to_string());
                }
            }
        }
        packages.insert(
            key.clone(),
            LockEntry {
                version,
                resolved,
                integrity,
                dependencies: deps,
                os: read_str_list(entry, "os"),
                cpu: read_str_list(entry, "cpu"),
                optional: entry
                    .get("optional")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
            },
        );
    }
    Some(LockFile { packages })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, version: &str, deps: Vec<&str>) -> ResolvedNode {
        ResolvedNode {
            name: name.to_string(),
            version: version.to_string(),
            requested: format!("^{version}"),
            tarball: format!("https://r/{name}-{version}.tgz"),
            integrity: Some(format!("sha512-{name}")),
            dependencies: deps.into_iter().map(String::from).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn build_and_roundtrip() {
        let mut flat = HashMap::new();
        flat.insert("a@1.0.0".to_string(), node("a", "1.0.0", vec!["b@1.0.0"]));
        flat.insert("b@1.0.0".to_string(), node("b", "1.0.0", vec![]));
        let lock = build_lock(&flat);
        assert_eq!(lock.packages.len(), 2);
        assert_eq!(
            lock.packages["a@1.0.0"]
                .dependencies
                .get("b")
                .map(String::as_str),
            Some("1.0.0")
        );

        // Serialization is deterministic (keys are sorted).
        let s1 = json::stringify_pretty(&to_json(&lock));
        let s2 = json::stringify_pretty(&to_json(&lock));
        assert_eq!(s1, s2);
        assert!(s1.contains("\"lockfileVersion\": 1"));
    }

    #[test]
    fn plain_package_entry_has_no_platform_noise() {
        let mut flat = HashMap::new();
        flat.insert("a@1.0.0".to_string(), node("a", "1.0.0", vec![]));
        let text = json::stringify_pretty(&to_json(&build_lock(&flat)));
        assert!(
            !text.contains("\"os\""),
            "a package with no constraints must have no os/cpu fields"
        );
        assert!(!text.contains("\"cpu\""));
        assert!(!text.contains("\"optional\""));
    }

    #[test]
    fn platform_constraints_survive_a_roundtrip() {
        // This is exactly why the fields are written: a lock built on another OS
        // must carry to us who each platform variant is meant for.
        let mut mac = node("@esbuild/darwin-arm64", "0.21.5", vec![]);
        mac.os = vec!["darwin".to_string()];
        mac.cpu = vec!["arm64".to_string()];
        mac.optional = true;
        let mut flat = HashMap::new();
        flat.insert("@esbuild/darwin-arm64@0.21.5".to_string(), mac);

        let lock = build_lock(&flat);
        let text = json::stringify_pretty(&to_json(&lock)) + "\n";

        let dir = std::env::temp_dir().join(format!("jet-lock-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("jet-lock.json"), &text).unwrap();
        let back = read_lock(&dir).expect("the lock reads");
        let _ = std::fs::remove_dir_all(&dir);

        let e = &back.packages["@esbuild/darwin-arm64@0.21.5"];
        assert_eq!(e.os, vec!["darwin"]);
        assert_eq!(e.cpu, vec!["arm64"]);
        assert!(e.optional);
    }

    #[test]
    fn split_key_scoped() {
        assert_eq!(
            split_key("express@5.2.1"),
            Some(("express".into(), "5.2.1".into()))
        );
        assert_eq!(
            split_key("@scope/pkg@1.0.0"),
            Some(("@scope/pkg".into(), "1.0.0".into()))
        );
    }
}
