//! Checks against the real binary without an external network.
use std::process::Command;

fn jet(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_jet"))
        .args(args)
        .output()
        .expect("run jet")
}

#[test]
fn audit_help_is_registered_and_needs_no_network() {
    let help = jet(&["help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("audit"));
    let output = jet(&["audit", "--help"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    for flag in ["--json", "--severity", "--fix", "--exit-code", "--no-cache"] {
        assert!(text.contains(flag));
    }
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn invalid_flags_fail_with_json_only() {
    let output = jet(&["audit", "--severity=invalid", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.starts_with('{') && text.trim_end().ends_with('}'));
    assert!(text.contains("\"ok\": false"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn missing_packages_are_an_error_not_a_clean_audit() {
    let dir = std::env::temp_dir().join(format!("jet-audit-empty-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jet"))
        .args(["audit", "--json", "--no-cache"])
        .current_dir(&dir)
        .output();
    std::fs::remove_dir(&dir).unwrap();
    let output = output.unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("\"ok\": false"));
}

#[test]
fn no_arguments_open_help() {
    let output = jet(&[]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage"));
}
