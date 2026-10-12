//! Running package bins (create/exec) — a port of runner.ts. A mini-npx: install
//! the package into a temp folder with our installer and run its bin via the system node.

use crate::installer::{InstallOptions, install};
use crate::json::Json;
use crate::project::read_manifest;
use crate::spec::split_name_range;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Remove the first `--` separator (npm/bun semantics), splicing the arguments.
pub fn strip_arg_separator(args: &[String]) -> Vec<String> {
    if let Some(pos) = args.iter().position(|a| a == "--") {
        let mut out = args[..pos].to_vec();
        out.extend_from_slice(&args[pos + 1..]);
        out
    } else {
        args.to_vec()
    }
}

/// A safe bin path INSIDE the package directory (bin from an untrusted package.json).
/// pub(crate): global install (`global.rs`) uses the same parsing.
pub(crate) fn safe_bin_path(pkg_dir: &Path, bin_value: &str) -> Option<PathBuf> {
    let target = pkg_dir.join(bin_value);
    let root = pkg_dir;
    // Normalize via components: there must be no escape outward.
    let mut depth: i32 = 0;
    for comp in Path::new(bin_value).components() {
        use std::path::Component::*;
        match comp {
            ParentDir => depth -= 1,
            Normal(_) | CurDir => {}
            RootDir | Prefix(_) => return None, // absolute path — stop
        }
        if depth < 0 {
            return None;
        }
    }
    let _ = root;
    Some(target)
}

fn find_bin(pkg_dir: &Path, pkg_name: &str) -> Option<PathBuf> {
    let manifest = read_manifest(pkg_dir)?;
    let bin = manifest.get("bin")?;
    let unscoped = pkg_name.rsplit('/').next().unwrap_or(pkg_name);
    match bin {
        Json::Str(s) => safe_bin_path(pkg_dir, s),
        Json::Obj(map) => {
            let key = if map.contains_key(unscoped) {
                unscoped.to_string()
            } else {
                map.keys().next()?.clone()
            };
            map.get(&key)
                .and_then(Json::as_str)
                .and_then(|s| safe_bin_path(pkg_dir, s))
        }
        _ => None,
    }
}

/// Install the package into a temp folder and run its bin via node.
pub fn run_package_bin(spec: &str, args: &[String], cwd: &Path) -> Result<i32, String> {
    let (name, range) = split_name_range(spec);
    let temp = std::env::temp_dir().join(format!("jet-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).map_err(|e| format!("temp: {e}"))?;

    let mut noop = |_: &str| {};
    let report = install(
        &temp,
        &[(name.clone(), range.clone())],
        &InstallOptions::default(),
        &mut noop,
    )?;
    let version = report
        .roots
        .iter()
        .find(|r| r.name == name)
        .map(|r| r.version.clone())
        .unwrap_or(range);

    let pkg_dir = temp.join("node_modules").join(&name);
    let bin = find_bin(&pkg_dir, &name)
        .ok_or_else(|| format!("Package {name}@{version} has no executable file (bin)"))?;

    let status = Command::new("node")
        .arg(&bin)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|e| format!("Failed to run node: {e}"))?;

    let _ = std::fs::remove_dir_all(&temp);
    Ok(status
        .code()
        .unwrap_or(if status.success() { 0 } else { 1 }))
}
