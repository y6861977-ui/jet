//! Unpacking .tgz — a port of targz.ts.
//!
//! The main path is the system `tar` (present on macOS, Linux, and stock Windows
//! 10+, like curl). `tar` handles gzip itself and processes paths safely (traversal).
//!
//! The fallback is 7-Zip: on trimmed Windows builds `tar.exe` from System32 may be
//! missing, and then without a fallback no install works at all. 7-Zip unpacks a
//! .tgz in two steps (gzip, then tar), so we go through a temp directory and strip
//! the top `package/` ourselves (the equivalent of `--strip-components=1`).
//!
//! Our own inflate + tar implementation in pure std would remove both external
//! utilities — that's still planned (see README, the "What's left" section).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Unpack a .tgz file into the dest directory with strip_components=1 (npm puts things under package/).
pub fn extract_to(tgz_path: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("Can't create {}: {e}", dest.display()))?;
    match extract_with_tar(tgz_path, dest) {
        Ok(()) => Ok(()),
        // tar ran and honestly said "no" — that's an archive error, a mirror
        // fallback won't fix it, so we return it as is.
        Err(TarError::Failed(msg)) => Err(msg),
        Err(TarError::NotFound) => match seven_zip_path() {
            Some(seven) => extract_with_7z(&seven, tgz_path, dest),
            None => Err(
                "The system has neither tar nor 7-Zip — nothing to unpack the package with.\n  \
                         Windows 10/11 usually ships tar.exe in System32; if it's missing, \
                         install 7-Zip (7-zip.org)."
                    .to_string(),
            ),
        },
    }
}

enum TarError {
    /// There's no `tar` executable in the system.
    NotFound,
    /// `tar` ran but returned an error.
    Failed(String),
}

fn extract_with_tar(tgz_path: &Path, dest: &Path) -> Result<(), TarError> {
    let out = Command::new("tar")
        .arg("-xzf")
        .arg(tgz_path)
        .arg("-C")
        .arg(dest)
        .arg("--strip-components=1")
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => TarError::NotFound,
            _ => TarError::Failed(format!("Failed to run tar: {e}")),
        })?;
    if !out.status.success() {
        return Err(TarError::Failed(format!(
            "tar exited with an error: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Path to 7-Zip: PATH first, then the standard Windows install directories.
fn seven_zip_path() -> Option<PathBuf> {
    for name in ["7z", "7za"] {
        if Command::new(name).arg("--help").output().is_ok() {
            return Some(PathBuf::from(name));
        }
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(base) = std::env::var(var) {
            let candidate = Path::new(&base).join("7-Zip").join("7z.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn extract_with_7z(seven: &Path, tgz_path: &Path, dest: &Path) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!(
        "jet-untgz-{}-{}",
        std::process::id(),
        // A "pid + archive name" pair is enough; in parallel the same package
        // unpacks into the same temp directory, but it's under our control and
        // cleaned up below.
        tgz_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("pkg")
            .replace(['/', '\\', ':'], "_")
    ));
    let _ = fs::remove_dir_all(&tmp);
    let result = unpack_two_steps(seven, tgz_path, &tmp, dest);
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn unpack_two_steps(seven: &Path, tgz_path: &Path, tmp: &Path, dest: &Path) -> Result<(), String> {
    let gz_out = tmp.join("gz");
    let tar_out = tmp.join("tar");
    fs::create_dir_all(&gz_out).map_err(|e| format!("Can't create {}: {e}", gz_out.display()))?;

    // Step 1: strip gzip. Inside a .tgz there's exactly one file — the .tar.
    run_7z(seven, &["x", "-y"], tgz_path, &gz_out)?;
    let tar_file = single_entry(&gz_out)
        .ok_or_else(|| "7-Zip: expected exactly one .tar inside the .tgz".to_string())?;

    // Step 2: unpack the tar. 7-Zip versions 16+ drop entries with `..` and
    // absolute paths, but the directory is temporary anyway: we only copy out
    // what lies inside it.
    run_7z(seven, &["x", "-y"], &tar_file, &tar_out)?;

    // strip_components=1: npm packs everything under package/.
    let root = match single_entry(&tar_out) {
        Some(dir) if dir.is_dir() => dir,
        // A nonstandard archive with no common root — take the contents as is.
        _ => tar_out.clone(),
    };
    move_children(&root, dest)
}

fn run_7z(seven: &Path, args: &[&str], archive: &Path, out_dir: &Path) -> Result<(), String> {
    let output = Command::new(seven)
        .args(args)
        .arg(format!("-o{}", out_dir.display()))
        .arg(archive)
        .output()
        .map_err(|e| format!("Failed to run 7-Zip: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        let msg = if err.trim().is_empty() { out } else { err };
        return Err(format!("7-Zip exited with an error: {}", msg.trim()));
    }
    Ok(())
}

/// The single entry of a directory, if there's exactly one.
fn single_entry(dir: &Path) -> Option<PathBuf> {
    let mut entries = fs::read_dir(dir).ok()?.flatten();
    let first = entries.next()?.path();
    match entries.next() {
        None => Some(first),
        Some(_) => None,
    }
}

/// Move the contents of src into dest (rename, with a fallback to copying).
fn move_children(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("Can't create {}: {e}", dest.display()))?;
    let entries = fs::read_dir(src).map_err(|e| format!("Can't read {}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if fs::rename(&from, &to).is_ok() {
            continue;
        }
        // A different volume (temp and ~/.jet may live on different drives) — copy.
        if from.is_dir() {
            move_children(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("Can't copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_entry_counts_exactly_one() {
        let dir = std::env::temp_dir().join(format!("jettargz-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        assert!(
            single_entry(&dir).is_none(),
            "an empty directory isn't one entry"
        );

        fs::write(dir.join("a.tar"), b"x").unwrap();
        assert_eq!(single_entry(&dir).unwrap().file_name().unwrap(), "a.tar");

        fs::write(dir.join("b.tar"), b"x").unwrap();
        assert!(single_entry(&dir).is_none(), "two entries aren't one");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn move_children_strips_the_root() {
        let base = std::env::temp_dir().join(format!("jetmove-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("package");
        fs::create_dir_all(root.join("lib")).unwrap();
        fs::write(root.join("package.json"), b"{}").unwrap();
        fs::write(root.join("lib").join("index.js"), b"1").unwrap();

        let dest = base.join("dest");
        move_children(&root, &dest).unwrap();

        assert!(dest.join("package.json").is_file());
        assert!(dest.join("lib").join("index.js").is_file());
        assert!(
            !dest.join("package").exists(),
            "the top package/ should be stripped"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn seven_zip_lookup_does_not_panic() {
        // On a machine without 7-Zip we expect None, with 7-Zip — an existing path.
        if let Some(p) = seven_zip_path() {
            assert!(!p.as_os_str().is_empty());
        }
    }
}
