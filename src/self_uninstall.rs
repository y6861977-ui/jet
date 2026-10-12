//! `jet self uninstall` — removes the jet tool ITSELF from the machine.
//!
//! This is deliberately NOT the same as `jet uninstall <package>` (which removes
//! a package from a project's `node_modules`). This command tears jet out:
//!   1. the data directory `~/.jet` (cache, store, config, license, global bins);
//!   2. jet's install directory from the user PATH;
//!   3. the jet binary itself (`std::env::current_exe()`).
//!
//! It never removes anything unless the caller passed `--yes`; without it we only
//! print a preview of what WOULD be removed, so an accidental run is harmless.

use crate::config::config_dir;
use crate::ui;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Entry point for `jet self uninstall [--yes]`.
///
/// `yes = false` prints a preview and removes nothing. `yes = true` actually
/// removes the data dir, the PATH entry and the binary.
pub fn uninstall(yes: bool) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Cannot determine the jet executable path: {e}"))?;
    let exe_dir = exe.parent().map(Path::to_path_buf);
    let data_dir = config_dir(); // ~/.jet

    if !yes {
        print_preview(&data_dir, exe_dir.as_deref(), &exe);
        return Ok(());
    }

    println!("\n{}", ui::heading("Uninstalling jet"));

    // 1. Remove the install directory from the user PATH.
    match exe_dir.as_deref() {
        Some(dir) => match remove_dir_from_path(dir) {
            Ok(true) => println!(
                "{}",
                ui::step(&format!("Removed from PATH: {}", dir.display()))
            ),
            Ok(false) => println!(
                "{}",
                ui::step(&format!(
                    "Not found on PATH, nothing to change: {}",
                    dir.display()
                ))
            ),
            Err(e) => println!(
                "{}",
                ui::yellow(&format!("{} Could not update PATH: {e}", ui::WARN))
            ),
        },
        None => println!(
            "{}",
            ui::yellow(&format!(
                "{} Could not determine the install directory, skipping PATH cleanup",
                ui::WARN
            ))
        ),
    }

    // 2. Remove the data directory ~/.jet.
    if data_dir.exists() {
        match fs::remove_dir_all(&data_dir) {
            Ok(()) => println!(
                "{}",
                ui::step(&format!("Removed data directory: {}", data_dir.display()))
            ),
            Err(e) => println!(
                "{}",
                ui::yellow(&format!(
                    "{} Could not remove {}: {e}",
                    ui::WARN,
                    data_dir.display()
                ))
            ),
        }
    } else {
        println!(
            "{}",
            ui::step(&format!(
                "Data directory already gone: {}",
                data_dir.display()
            ))
        );
    }

    // 3. Delete the jet binary itself (last — on Windows this schedules a helper).
    delete_self(&exe)?;

    println!(
        "\n{} {}",
        ui::green(ui::OK),
        ui::bold_green("jet has been removed")
    );
    #[cfg(windows)]
    {
        println!(
            "{}",
            ui::hint("The jet binary is deleted by a helper as this process exits.")
        );
        println!(
            "{}",
            ui::hint("Open a new terminal for the PATH change to take effect.")
        );
    }
    #[cfg(not(windows))]
    {
        println!(
            "{}",
            ui::hint("If your shell still finds jet, open a new terminal or reload your profile.")
        );
    }
    println!();
    Ok(())
}

/// Print what WOULD be removed, without touching anything.
fn print_preview(data_dir: &Path, exe_dir: Option<&Path>, exe: &Path) {
    println!("\n{}", ui::heading("jet self uninstall"));
    println!(
        "{}",
        ui::step("This will remove jet itself from your machine:")
    );
    println!();
    let w = 14;
    println!(
        "{}",
        ui::field("data directory", w, &data_dir.display().to_string())
    );
    match exe_dir {
        Some(dir) => println!("{}", ui::field("PATH entry", w, &dir.display().to_string())),
        None => println!("{}", ui::field("PATH entry", w, "(unknown)")),
    }
    println!("{}", ui::field("jet binary", w, &exe.display().to_string()));
    println!(
        "\n{}",
        ui::yellow(&format!("{} Nothing has been removed yet.", ui::WARN))
    );
    println!("{}", ui::hint("To actually uninstall, re-run:"));
    println!("{}{}", ui::PAD, ui::cyan("jet self uninstall --yes"));
    println!();
}

/// Normalize a path for comparison: strip trailing separators, and on Windows
/// fold separators and case so `C:/Foo\` == `c:\foo`.
fn normalize(p: &Path) -> String {
    let s = p
        .to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .replace('/', "\\");
    if cfg!(windows) { s.to_lowercase() } else { s }
}

/// Remove `dir` from the user's persistent PATH. Returns `Ok(true)` if a change
/// was made, `Ok(false)` if `dir` was not present.
///
/// Windows: the installer adds jet to the HKCU `Environment` `Path`. We read the
/// *user* Path (not the merged process `PATH`), drop entries equal to `dir`, and
/// write it back — the same `[Environment]::…EnvironmentVariable(…, 'User')`
/// mechanism the installer/`jet install -g` hint uses.
#[cfg(windows)]
fn remove_dir_from_path(dir: &Path) -> Result<bool, String> {
    let current = read_user_path_windows()?;
    let target = normalize(dir);
    let entries: Vec<&str> = current.split(';').filter(|p| !p.trim().is_empty()).collect();
    let kept: Vec<&str> = entries
        .iter()
        .copied()
        .filter(|p| normalize(Path::new(p.trim())) != target)
        .collect();
    if kept.len() == entries.len() {
        return Ok(false); // nothing matched
    }
    write_user_path_windows(&kept.join(";"))?;
    Ok(true)
}

/// Read the persistent *user* Path from the registry via PowerShell.
#[cfg(windows)]
fn read_user_path_windows() -> Result<String, String> {
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Environment]::GetEnvironmentVariable('Path','User')",
        ])
        .output()
        .map_err(|e| format!("could not run PowerShell: {e}"))?;
    if !out.status.success() {
        return Err("PowerShell could not read the user PATH".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_string())
}

/// Write the persistent *user* Path back via PowerShell. The new value is passed
/// through a child env var (not interpolated into the command string) so a PATH
/// containing shell metacharacters cannot inject a command.
#[cfg(windows)]
fn write_user_path_windows(new_path: &str) -> Result<(), String> {
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Environment]::SetEnvironmentVariable('Path', $env:JET_NEW_PATH, 'User')",
        ])
        .env("JET_NEW_PATH", new_path)
        .status()
        .map_err(|e| format!("could not run PowerShell: {e}"))?;
    if !status.success() {
        return Err("PowerShell could not update the user PATH".into());
    }
    Ok(())
}

/// Remove `dir` from the user's PATH on Unix: strip PATH-export lines that add
/// exactly this directory from the common shell profiles. Best-effort and
/// conservative — only lines that both touch PATH and mention this exact dir are
/// removed, so unrelated profile content is left intact.
#[cfg(not(windows))]
fn remove_dir_from_path(dir: &Path) -> Result<bool, String> {
    let home = crate::config::home_dir();
    let candidates = [
        ".profile",
        ".bash_profile",
        ".bashrc",
        ".zshrc",
        ".zprofile",
    ];
    let dir_str = dir.to_string_lossy().to_string();
    let mut changed_any = false;

    for name in candidates {
        let file = home.join(name);
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        let mut removed = false;
        let mut kept: Vec<&str> = Vec::new();
        for line in content.lines() {
            let l = line.trim_start();
            let touches_path = l.starts_with("export PATH") || l.contains("PATH=");
            if touches_path && line.contains(&dir_str) {
                removed = true;
            } else {
                kept.push(line);
            }
        }
        if removed {
            let mut new_content = kept.join("\n");
            if content.ends_with('\n') {
                new_content.push('\n');
            }
            fs::write(&file, new_content)
                .map_err(|e| format!("could not update {}: {e}", file.display()))?;
            changed_any = true;
        }
    }
    Ok(changed_any)
}

/// Delete the running jet binary. On Unix a process can unlink its own
/// executable, so we delete it directly.
#[cfg(not(windows))]
fn delete_self(exe: &Path) -> Result<(), String> {
    fs::remove_file(exe)
        .map_err(|e| format!("Could not delete the jet binary {}: {e}", exe.display()))?;
    println!(
        "{}",
        ui::step(&format!("Removed jet binary: {}", exe.display()))
    );
    Ok(())
}

/// Delete the running jet binary on Windows. A running `.exe` is locked and
/// cannot delete itself, so we spawn a detached `cmd` that waits for this process
/// to exit (a short `ping` sleep) and then deletes the file. We then let the
/// process exit normally.
#[cfg(windows)]
fn delete_self(exe: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    // Run the helper without flashing a console window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // current_exe() is our own trusted path (Windows paths cannot contain `"`),
    // so embedding it in the command string carries no injection risk.
    let exe_str = exe.to_string_lossy().to_string();
    let script = format!("ping 127.0.0.1 -n 2 >nul & del /f /q \"{exe_str}\"");
    Command::new("cmd")
        .args(["/C", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Could not schedule deletion of the jet binary: {e}"))?;
    println!(
        "{}",
        ui::step(&format!(
            "Scheduled deletion of jet binary on exit: {}",
            exe.display()
        ))
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_and_folds() {
        if cfg!(windows) {
            assert_eq!(normalize(Path::new("C:/Foo/Bar\\")), "c:\\foo\\bar");
            assert_eq!(
                normalize(Path::new("c:\\foo\\bar")),
                normalize(Path::new("C:/Foo/Bar/"))
            );
        } else {
            assert_eq!(normalize(Path::new("/home/u/.local/bin/")), "/home/u/.local/bin");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_path_cleanup_removes_only_matching_export_lines() {
        let dir = std::env::temp_dir().join(format!("jet-self-unix-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let profile = dir.join(".profile");
        let bin = "/opt/jet/bin";
        fs::write(
            &profile,
            format!(
                "# my profile\nexport PATH=\"{bin}:$PATH\"\nexport EDITOR=vim\nalias ll='ls -l'\n"
            ),
        )
        .unwrap();

        // Point home at our temp dir and run the cleanup.
        let prev_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &dir);
        }
        let changed = remove_dir_from_path(Path::new(bin)).unwrap();
        if let Some(h) = prev_home {
            unsafe {
                std::env::set_var("HOME", h);
            }
        }

        let after = fs::read_to_string(&profile).unwrap();
        let _ = fs::remove_dir_all(&dir);

        assert!(changed, "the PATH export line should have been removed");
        assert!(!after.contains(bin), "jet's PATH entry is gone");
        assert!(after.contains("export EDITOR=vim"), "unrelated lines kept");
        assert!(after.contains("alias ll='ls -l'"), "unrelated lines kept");
        assert!(after.contains("# my profile"), "comments kept");
    }
}
