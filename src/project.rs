//! Working with the project's package.json — a port of project.ts.

use crate::json::{self, Json};
use std::collections::HashMap;
use std::path::Path;

fn manifest_path(root: &Path) -> std::path::PathBuf {
    root.join("package.json")
}

/// Read a text file, detecting the encoding by BOM: UTF-8 (including with a BOM),
/// UTF-16 LE and UTF-16 BE. Editors and the Windows console often write UTF-16 or
/// UTF-8-BOM (`echo ... > file.json` in PowerShell 5.1 is UTF-16 LE), and a bare
/// `read_to_string` panics on such files (not valid UTF-8). None — no file or it
/// doesn't decode.
pub fn read_text_tolerant(path: &Path) -> Option<String> {
    decode_text(&std::fs::read(path).ok()?)
}

fn decode_text(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(rest.to_vec()).ok(); // UTF-8 BOM
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, true); // UTF-16 LE
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, false); // UTF-16 BE
    }
    String::from_utf8(bytes.to_vec()).ok() // plain UTF-8
}

fn decode_utf16(bytes: &[u8], le: bool) -> Option<String> {
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16(&units).ok()
}

/// Read package.json as Json (or None if missing/corrupt).
pub fn read_manifest(root: &Path) -> Option<Json> {
    let text = read_text_tolerant(&manifest_path(root))?;
    json::parse(&text).ok()
}

/// Collect dependencies (prod + dev) into a list of (name, range).
pub fn collect_dependencies(manifest: &Json) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for field in ["dependencies", "devDependencies"] {
        if let Some(obj) = manifest.get(field).and_then(Json::as_object) {
            for (name, range) in obj {
                if let Some(r) = range.as_str() {
                    out.push((name.clone(), r.to_string()));
                }
            }
        }
    }
    out
}

/// Add/update a dependency in package.json (creates the file if it's missing).
pub fn add_dependency(root: &Path, name: &str, range: &str) -> Result<(), String> {
    let mut map = match read_manifest(root) {
        Some(Json::Obj(m)) => m,
        _ => HashMap::new(),
    };
    map.entry("name".to_string())
        .or_insert_with(|| Json::Str("my-project".to_string()));
    map.entry("version".to_string())
        .or_insert_with(|| Json::Str("1.0.0".to_string()));

    let mut deps = match map.get("dependencies") {
        Some(Json::Obj(d)) => d.clone(),
        _ => HashMap::new(),
    };
    deps.insert(name.to_string(), Json::Str(range.to_string()));
    map.insert("dependencies".to_string(), Json::Obj(deps));

    let text = json::stringify_pretty(&Json::Obj(map)) + "\n";
    std::fs::write(manifest_path(root), text).map_err(|e| format!("Can't write package.json: {e}"))
}

/// Remove a dependency from package.json (dependencies and devDependencies).
/// Returns true if the entry was in at least one of the sections.
pub fn remove_dependency(root: &Path, name: &str) -> Result<bool, String> {
    let Some(Json::Obj(mut map)) = read_manifest(root) else {
        return Ok(false); // no package.json — nothing to remove
    };
    let mut found = false;
    for field in ["dependencies", "devDependencies"] {
        if let Some(Json::Obj(mut deps)) = map.get(field).cloned() {
            if deps.remove(name).is_some() {
                found = true;
                map.insert(field.to_string(), Json::Obj(deps));
            }
        }
    }
    if found {
        let text = json::stringify_pretty(&Json::Obj(map)) + "\n";
        std::fs::write(manifest_path(root), text)
            .map_err(|e| format!("Can't write package.json: {e}"))?;
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_detects_encodings() {
        let s = "{\"name\":\"t\"}";
        // Plain UTF-8.
        assert_eq!(decode_text(s.as_bytes()).as_deref(), Some(s));
        // UTF-8 with BOM.
        let mut u8bom = vec![0xEF, 0xBB, 0xBF];
        u8bom.extend_from_slice(s.as_bytes());
        assert_eq!(decode_text(&u8bom).as_deref(), Some(s));
        // UTF-16 LE (as PowerShell 5.1 `echo > file` writes it).
        let mut le = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_text(&le).as_deref(), Some(s));
        // UTF-16 BE.
        let mut be = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            be.extend_from_slice(&u.to_be_bytes());
        }
        assert_eq!(decode_text(&be).as_deref(), Some(s));
    }

    #[test]
    fn read_manifest_reads_utf16_package_json() {
        let dir = std::env::temp_dir().join(format!("jet-utf16-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Write package.json in UTF-16 LE with a BOM — like interactive PowerShell.
        let json_str = "{\"name\":\"t\",\"dependencies\":{\"lodash\":\"4.17.20\"}}";
        let mut bytes = vec![0xFF, 0xFE];
        for u in json_str.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(dir.join("package.json"), bytes).unwrap();
        let m = read_manifest(&dir);
        let deps = m.as_ref().map(collect_dependencies);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            deps,
            Some(vec![("lodash".to_string(), "4.17.20".to_string())])
        );
    }

    #[test]
    fn remove_from_both_sections() {
        let dir = std::env::temp_dir().join(format!("jet-proj-rm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            manifest_path(&dir),
            r#"{"name":"p","dependencies":{"a":"^1.0.0","b":"^2.0.0"},"devDependencies":{"c":"^3.0.0"}}"#,
        )
        .unwrap();

        assert!(
            remove_dependency(&dir, "a").unwrap(),
            "a was in dependencies"
        );
        assert!(
            remove_dependency(&dir, "c").unwrap(),
            "c was in devDependencies"
        );
        assert!(
            !remove_dependency(&dir, "no-such").unwrap(),
            "a missing one isn't there"
        );

        let m = read_manifest(&dir).unwrap();
        let deps = m.get("dependencies").and_then(Json::as_object).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!deps.contains_key("a"), "a removed");
        assert!(deps.contains_key("b"), "b left untouched");
    }
}
