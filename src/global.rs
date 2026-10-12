//! Global install — `jet install -g`. The one thing npm has been ahead of us on
//! (`npm install -g <cli>`).
//!
//! The model is pnpm-style ISOLATION, not npm's shared flat tree. Each global
//! package lives in its own mini-project `~/.jet/global/<pkg>/`, and the command
//! links (shims) go into a shared `~/.jet/bin/`, which the user adds to PATH.
//! Why isolation: npm's global flat `node_modules` is a historical source of
//! version conflicts between different CLIs. Our files live in the content store
//! anyway and are laid out with hardlinks, so a separate prefix per package costs
//! almost nothing, while the tools don't interfere with each other.
//!
//! Install scripts (postinstall) we do NOT run — same as in a regular install
//! (the main vector of supply-chain attacks, see `installer.rs`). So packages that
//! place their binary precisely in postinstall will install globally, but the
//! command may not work; we warn about that honestly.

use crate::config::config_dir;
use crate::json::Json;
use crate::project::read_manifest;
use crate::runner::safe_bin_path;
use crate::{installer, ui};
use std::fs;
use std::path::{Path, PathBuf};

/// The root of global prefixes: ~/.jet/global
pub fn global_root() -> PathBuf {
    let dir = config_dir().join("global");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// The command-links directory: ~/.jet/bin (the user keeps it on PATH).
pub fn bin_dir() -> PathBuf {
    let dir = config_dir().join("bin");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// Package name -> a safe prefix directory name. Scoped names have a slash, which
/// must not appear in a folder name: `@scope/name` -> `@scope+name`.
fn prefix_name(pkg: &str) -> String {
    pkg.replace('/', "+")
}

/// One command: what it's called and which file inside the package it points to.
pub struct BinEntry {
    pub command: String,
    pub target: PathBuf,
}

/// Parse an installed package's `bin` field — all commands, not just the first.
/// An empty list if there's no bin or it's malformed.
pub fn bin_entries(pkg_dir: &Path, pkg_name: &str) -> Vec<BinEntry> {
    let Some(manifest) = read_manifest(pkg_dir) else {
        return Vec::new();
    };
    let Some(bin) = manifest.get("bin") else {
        return Vec::new();
    };
    let unscoped = pkg_name.rsplit('/').next().unwrap_or(pkg_name);

    // Accept a command only if both the name and the path are safe for a shim.
    let accept = |out: &mut Vec<BinEntry>, command: &str, target: PathBuf| {
        if safe_command_name(command) && shim_safe(&target.to_string_lossy()) {
            out.push(BinEntry {
                command: command.to_string(),
                target,
            });
        }
    };

    let mut out = Vec::new();
    match bin {
        // "bin": "cli.js" — the command is named like the package (without scope).
        Json::Str(s) => {
            if let Some(target) = safe_bin_path(pkg_dir, s) {
                accept(&mut out, unscoped, target);
            }
        }
        // "bin": { "name": "cli.js", ... } — explicit command names.
        Json::Obj(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort(); // deterministic order
            for key in keys {
                if let Some(rel) = map.get(key).and_then(Json::as_str) {
                    if let Some(target) = safe_bin_path(pkg_dir, rel) {
                        accept(&mut out, key, target);
                    }
                }
            }
        }
        _ => {}
    }
    out
}

/// Whether a string is safe to embed inside a shim (.cmd/.ps1) and in a file name.
///
/// WHY: the bin path comes from a foreign `package.json` and goes VERBATIM into
/// the shim body inside quotes. A value like `x.js" & calc.exe & "` would close
/// the quote and run an arbitrary command when the user invokes the global command
/// (supply-chain RCE via `jet install -g`). Control characters (newline) and
/// cmd/PowerShell shell metacharacters don't occur in normal bin paths, so their
/// presence is a reason to reject the command, not to try escaping (escaping in
/// .cmd is unreliable).
fn shim_safe(s: &str) -> bool {
    !s.chars().any(|c| {
        (c as u32) < 0x20
            || c == '\u{7f}'
            || matches!(c, '"' | '&' | '|' | '<' | '>' | '^' | '%' | '!' | '$' | '`')
    })
}

/// A command name: only a safe character set for the shim's file name.
fn safe_command_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 214
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
        && name != "."
        && name != ".."
}

/// Whether the target should be run through node. A ready executable (.exe/.cmd/
/// .bat/.com) we call directly; everything else (usually .js or a file with a
/// shebang) goes through node, the way npm does it.
fn runs_directly(target: &Path) -> bool {
    matches!(
        target
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .as_deref(),
        Some("exe") | Some("cmd") | Some("bat") | Some("com")
    )
}

/// Option #1 for postinstall WITHOUT running the script.
///
/// What install scripts do for claude-code and the like: copy the real binary
/// from a platform subpackage over a small stub `bin/<name>.exe`. There's no need
/// to execute them for that — on a global install we decide ourselves WHERE the
/// command points, so we just aim the shim straight at the real binary already on
/// disk. Zero execution of foreign code, zero copying of 200 MB.
///
/// It triggers narrowly and safely:
///   - only if the bin itself is a ready executable (.exe/.cmd/...), not a JS
///     wrapper (wrappers find the binary at runtime themselves; we don't touch them);
///   - only if the installed platform subpackage has a file with the same name
///     and it's NOTICEABLY LARGER (the real binary vs the stub). If the main bin
///     is already large — the package ships the binary itself, we don't substitute.
fn real_binary_for(prefix: &Path, pkg_dir: &Path, bin_target: &Path) -> Option<PathBuf> {
    if !runs_directly(bin_target) {
        return None; // a JS wrapper — not our case
    }
    let basename = bin_target.file_name()?;
    let stub_size = fs::metadata(bin_target).map(|m| m.len()).unwrap_or(0);

    // Candidates — the platform subpackages from the main package's optionalDependencies.
    let manifest = read_manifest(pkg_dir)?;
    let opt = manifest
        .get("optionalDependencies")
        .and_then(Json::as_object)?;

    for opt_name in opt.keys() {
        // A name from a foreign manifest is joined into a path — reject `..` and junk,
        // otherwise `../../../` would lead the shim to an arbitrary file outside the package.
        if !crate::registry::is_valid_package_name(opt_name) {
            continue;
        }
        // jet lays out nested (npm@2), but the dependency may also be hoisted to the
        // top level — check both places.
        let nested = pkg_dir.join("node_modules").join(opt_name).join(basename);
        let hoisted = prefix.join("node_modules").join(opt_name).join(basename);
        for candidate in [nested, hoisted] {
            if let Ok(meta) = fs::metadata(&candidate) {
                if meta.len() > stub_size {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Create a command link in ~/.jet/bin. On Windows this is a .cmd (for cmd.exe)
/// and a .ps1 (for PowerShell) — symlinks there need special privileges, hence
/// wrappers. Returns the list of created files (to remove later on uninstall).
fn write_shims(command: &str, target: &Path) -> Result<Vec<PathBuf>, String> {
    let dir = bin_dir();
    let target_str = target.to_string_lossy().to_string();
    // A single hard gate: the name and path go into the shim body and the file name.
    // Unsafe ones are rejected outright (see shim_safe/safe_command_name), so no path
    // into write_shims can smuggle in a command injection.
    if !safe_command_name(command) {
        return Err(format!("Unsafe command name: {command:?}"));
    }
    if !shim_safe(&target_str) {
        return Err(format!(
            "Unsafe path for command {command}: {target_str}"
        ));
    }
    let mut created = Vec::new();

    let cmd_path = dir.join(format!("{command}.cmd"));
    let cmd_body = if runs_directly(target) {
        format!("@echo off\r\n\"{target_str}\" %*\r\n")
    } else {
        format!("@echo off\r\nnode \"{target_str}\" %*\r\n")
    };
    fs::write(&cmd_path, cmd_body)
        .map_err(|e| format!("Can't write {}: {e}", cmd_path.display()))?;
    created.push(cmd_path);

    let ps1_path = dir.join(format!("{command}.ps1"));
    let invoke = if runs_directly(target) {
        format!("& \"{target_str}\" @args")
    } else {
        format!("& node \"{target_str}\" @args")
    };
    // We propagate $LASTEXITCODE outward so the command's return code isn't lost.
    let ps1_body = format!("#!/usr/bin/env pwsh\r\n{invoke}\r\nexit $LASTEXITCODE\r\n");
    fs::write(&ps1_path, ps1_body)
        .map_err(|e| format!("Can't write {}: {e}", ps1_path.display()))?;
    created.push(ps1_path);

    Ok(created)
}

/// Install packages globally. Returns the names of the created commands.
pub fn install(
    specs: &[(String, String)],
    log: &mut dyn FnMut(&str),
) -> Result<Vec<String>, String> {
    let mut commands = Vec::new();
    let mut script_warnings = Vec::new();

    for (name, range) in specs {
        // The name goes into the prefix path — validate before any FS operations.
        if !crate::registry::is_valid_package_name(name) {
            return Err(format!("Invalid package name: {name:?}"));
        }
        let prefix = global_root().join(prefix_name(name));
        fs::create_dir_all(&prefix).map_err(|e| format!("Can't create the prefix: {e}"))?;

        log(&format!("Installing {name}@{range} into {}", prefix.display()));
        let report = installer::install(
            &prefix,
            &[(name.clone(), range.clone())],
            &installer::InstallOptions {
                use_lock: false,
                record_history: true,
            },
            log,
        )?;
        // Pass through the installer's warnings, EXCEPT the install-script one: for
        // a global install we handle it ourselves below (option #1) and report more
        // precisely than the generic phrase "may not work".
        for w in &report.warnings {
            if !w.contains("install script") {
                log(&ui::yellow(w));
            }
        }

        let pkg_dir = prefix.join("node_modules").join(name);
        let entries = bin_entries(&pkg_dir, name);
        if entries.is_empty() {
            return Err(format!(
                "Package {name} has no executable commands (bin) — nothing to install globally"
            ));
        }
        for e in &entries {
            // Option #1: if the bin is a stub and a real binary from a platform
            // subpackage is next to it, aim the shim straight at it (without running
            // postinstall).
            let (target, note) = match real_binary_for(&prefix, &pkg_dir, &e.target) {
                Some(real) => (
                    real,
                    Some(format!(
                        "command {} bound to the native binary (postinstall wasn't needed)",
                        e.command
                    )),
                ),
                None => (e.target.clone(), None),
            };

            if !target.exists() {
                // bin points to a file that doesn't exist, and no real binary was
                // found — the package probably needs an install script of a different
                // kind (compilation, extra download), which jet doesn't run.
                script_warnings.push(format!(
                    "command {} references a missing file — the package probably needs an install script, which jet doesn't run",
                    e.command
                ));
            }
            write_shims(&e.command, &target)?;
            if let Some(n) = note {
                log(&ui::green(&n));
            }
            commands.push(e.command.clone());
        }
    }

    for w in script_warnings {
        log(&ui::yellow(&w));
    }
    Ok(commands)
}

/// Remove a global package: tear down its prefix and its command links.
pub fn uninstall(names: &[String]) -> Result<Vec<String>, String> {
    let mut removed = Vec::new();
    for name in names {
        // The name goes into a path that is then rm -rf'd — validate before FS ops,
        // so that `..\..\` can't lead the removal outside the prefix.
        if !crate::registry::is_valid_package_name(name) {
            return Err(format!("Invalid package name: {name:?}"));
        }
        let prefix = global_root().join(prefix_name(name));
        if !prefix.exists() {
            return Err(format!("{name} is not installed globally"));
        }
        // First collect the package's commands while the prefix is still in place.
        let pkg_dir = prefix.join("node_modules").join(name);
        for e in bin_entries(&pkg_dir, name) {
            let _ = fs::remove_file(bin_dir().join(format!("{}.cmd", e.command)));
            let _ = fs::remove_file(bin_dir().join(format!("{}.ps1", e.command)));
        }
        fs::remove_dir_all(&prefix).map_err(|e| format!("Can't remove {name}: {e}"))?;
        removed.push(name.clone());
    }
    Ok(removed)
}

/// A list of globally installed packages (name + version from their manifest).
pub fn list() -> Vec<(String, String)> {
    let root = global_root();
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&root) else {
        return out;
    };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        // Reconstruct the package name from the folder name (back to scoped form).
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let pkg_name = dir_name.replace('+', "/");
        let pkg_dir = entry.path().join("node_modules").join(&pkg_name);
        let version = read_manifest(&pkg_dir)
            .and_then(|m| m.get("version").and_then(Json::as_str).map(str::to_string))
            .unwrap_or_else(|| "?".to_string());
        out.push((pkg_name, version));
    }
    out.sort();
    out
}

/// Whether ~/.jet/bin is on PATH. Compared by normalized path,
/// on Windows case-insensitively.
pub fn bin_dir_on_path() -> bool {
    let target = normalize(&bin_dir());
    let path = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    path.split(sep)
        .any(|p| !p.is_empty() && normalize(Path::new(p.trim())) == target)
}

fn normalize(p: &Path) -> String {
    let s = p
        .to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .replace('/', "\\");
    if cfg!(windows) { s.to_lowercase() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_name_handles_scoped() {
        assert_eq!(prefix_name("typescript"), "typescript");
        assert_eq!(
            prefix_name("@anthropic-ai/claude-code"),
            "@anthropic-ai+claude-code"
        );
    }

    #[test]
    fn runs_directly_only_for_executables() {
        assert!(runs_directly(Path::new("a/b/claude.exe")));
        assert!(
            runs_directly(Path::new("tool.CMD")),
            "extension case doesn't matter"
        );
        assert!(!runs_directly(Path::new("cli.js")), "js — through node");
        assert!(
            !runs_directly(Path::new("bin/cli")),
            "no extension — through node"
        );
    }

    #[test]
    fn bin_entries_from_string_bin() {
        let dir = std::env::temp_dir().join(format!("jet-global-str-{}", std::process::id()));
        let pkg = dir.join("node_modules").join("cowsay");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"cowsay","bin":"cli.js"}"#,
        )
        .unwrap();
        let entries = bin_entries(&pkg, "cowsay");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].command, "cowsay",
            "a string bin — the command is named like the package"
        );
        assert!(entries[0].target.ends_with("cli.js"));
    }

    #[test]
    fn bin_entries_from_object_and_scoped() {
        let dir = std::env::temp_dir().join(format!("jet-global-obj-{}", std::process::id()));
        let pkg = dir.join("node_modules").join("@scope").join("tool");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@scope/tool","bin":{"tool":"a.js","tool-dev":"b.js"}}"#,
        )
        .unwrap();
        let mut entries = bin_entries(&pkg, "@scope/tool");
        entries.sort_by(|a, b| a.command.cmp(&b.command));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].command, "tool");
        assert_eq!(entries[1].command, "tool-dev");
    }

    #[test]
    fn bin_entries_rejects_shell_metacharacters_in_bin_path() {
        // Supply-chain RCE: a bin value with a quote and `&` would close the quote in
        // the shim and run an arbitrary command. We don't accept such a command.
        let dir = std::env::temp_dir().join(format!("jet-global-inj-{}", std::process::id()));
        let pkg = dir.join("node_modules").join("evil");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"evil","bin":{"evil":"a.js\" & calc.exe & \"b"}}"#,
        )
        .unwrap();
        let entries = bin_entries(&pkg, "evil");
        let _ = fs::remove_dir_all(&dir);
        assert!(entries.is_empty(), "a bin with shell metacharacters is rejected");
    }

    #[test]
    fn write_shims_refuses_unsafe_target() {
        // Hard gate: even if the path arrived bypassing bin_entries, the shim isn't written.
        assert!(write_shims("ok", Path::new("C:\\pkg\\bin\\a.js\" & calc & \"")).is_err());
        assert!(
            write_shims("bad name", Path::new("C:\\pkg\\a.js")).is_err(),
            "a name with a space/junk is rejected"
        );
    }

    #[test]
    fn bin_entries_rejects_traversal_in_command_name() {
        let dir = std::env::temp_dir().join(format!("jet-global-trav-{}", std::process::id()));
        let pkg = dir.join("node_modules").join("evil");
        fs::create_dir_all(&pkg).unwrap();
        // A command name with ../ would lead the shim outside ~/.jet/bin.
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"evil","bin":{"../../hack":"x.js"}}"#,
        )
        .unwrap();
        let entries = bin_entries(&pkg, "evil");
        let _ = fs::remove_dir_all(&dir);
        assert!(entries.is_empty(), "a command name with ../ is rejected");
    }

    #[test]
    fn shim_body_picks_node_for_js_and_direct_for_exe() {
        // Check via contents: JS is called through node, exe — directly.
        let js_files = write_shims("tsc-test-shim", Path::new("C:\\pkg\\bin\\tsc.js")).unwrap();
        let exe_files = write_shims("claude-test-shim", Path::new("C:\\pkg\\claude.exe")).unwrap();

        let js_cmd = fs::read_to_string(&js_files[0]).unwrap();
        let exe_cmd = fs::read_to_string(&exe_files[0]).unwrap();
        assert!(js_cmd.contains("node \""), "for .js the shim calls node");
        assert!(
            !exe_cmd.contains("node \""),
            "for .exe the shim calls the file directly"
        );
        assert!(exe_cmd.contains("claude.exe"));

        // Clean up after ourselves, so as not to leave commands in ~/.jet/bin.
        for f in js_files.into_iter().chain(exe_files) {
            let _ = fs::remove_file(f);
        }
    }

    #[test]
    fn real_binary_replaces_stub_from_platform_subpackage() {
        // Reproduce claude-code's layout: a small stub bin/claude.exe and the real
        // large binary in a platform subpackage next to it.
        let prefix = std::env::temp_dir().join(format!("jet-realbin-{}", std::process::id()));
        let pkg_dir = prefix.join("node_modules").join("tool");
        let bin_dir = pkg_dir.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        // A manifest with a platform optional dependency.
        fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"tool","bin":{"tool":"bin/tool.exe"},"optionalDependencies":{"tool-win32-x64":"1.0.0"}}"#,
        )
        .unwrap();
        // The stub — 3 bytes.
        fs::write(bin_dir.join("tool.exe"), b"XXX").unwrap();
        // The real binary in a nested subpackage — larger than the stub.
        let plat = pkg_dir.join("node_modules").join("tool-win32-x64");
        fs::create_dir_all(&plat).unwrap();
        fs::write(plat.join("tool.exe"), vec![0u8; 10_000]).unwrap();

        let stub = bin_dir.join("tool.exe");
        let real = real_binary_for(&prefix, &pkg_dir, &stub);
        let _ = fs::remove_dir_all(&prefix);

        let real = real.expect("the real binary should be found");
        assert!(real.ends_with(Path::new("tool-win32-x64").join("tool.exe")));
    }

    #[test]
    fn real_binary_leaves_js_wrapper_and_own_binary_alone() {
        let prefix = std::env::temp_dir().join(format!("jet-realbin2-{}", std::process::id()));
        let pkg_dir = prefix.join("node_modules").join("tool");
        fs::create_dir_all(pkg_dir.join("bin")).unwrap();
        fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"tool","optionalDependencies":{"tool-win32-x64":"1.0.0"}}"#,
        )
        .unwrap();
        // We don't touch the JS wrapper — it finds the binary at runtime itself.
        let js = pkg_dir.join("bin").join("cli.js");
        fs::write(&js, b"console.log(1)").unwrap();
        assert!(
            real_binary_for(&prefix, &pkg_dir, &js).is_none(),
            ".js — we don't substitute"
        );

        // The package ships its OWN large binary, there's no platform subpackage —
        // nothing to substitute with and nothing to substitute.
        let own = pkg_dir.join("bin").join("tool.exe");
        fs::write(&own, vec![0u8; 5_000]).unwrap();
        assert!(
            real_binary_for(&prefix, &pkg_dir, &own).is_none(),
            "no platform file — leave as is"
        );

        let _ = fs::remove_dir_all(&prefix);
    }
}
