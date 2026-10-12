//! The audit layer: arguments and output. Network and cache belong to audit.rs.
use crate::audit::{AuditReport, Finding, Severity};
use crate::json::{self, Json};
use crate::ui;

#[derive(Default, Debug, PartialEq)]
pub struct Options {
    pub json: bool,
    pub severity: Option<Severity>,
    pub fix: bool,
    pub exit_code: bool,
    pub no_cache: bool,
    pub clear_cache: bool,
    pub help: bool,
}

pub const HELP: &str = "Usage: jet audit [flags]
  --json              report to stdout, JSON errors to stderr
  --severity=<level>  show low/medium/high/critical and above
  --fix               show update commands, don't run them
  --exit-code         code 1 on any findings, regardless of the filter
  --no-cache          request fresh OSV data, bypassing the cache
  --clear-cache       clear the entire cache before auditing (for debugging)
  --help, -h          this help

Source: jet-lock.json or package-lock.json; the OSV.dev database.
Cache: ~/.jet/audit-cache/, TTL 24 hours.
The update command is not in the Rust port yet; the --fix hints are meant for the TS version of jet.";

pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut out = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => out.json = true,
            "--fix" => out.fix = true,
            "--exit-code" => out.exit_code = true,
            "--no-cache" => out.no_cache = true,
            "--clear-cache" => out.clear_cache = true,
            "--help" | "-h" => out.help = true,
            "--severity" => {
                out.severity = Some(parse_severity(
                    args.next().map(String::as_str).unwrap_or(""),
                )?)
            }
            value if value.starts_with("--severity=") => {
                out.severity = Some(parse_severity(&value[11..])?)
            }
            _ => {
                return Err(format!(
                    "Unknown audit argument: {}. Help: jet audit --help",
                    ui::sanitize(arg)
                ));
            }
        }
    }
    Ok(out)
}

fn parse_severity(value: &str) -> Result<Severity, String> {
    match value.to_ascii_lowercase().as_str() {
        "low" => Ok(Severity::Low),
        "medium" => Ok(Severity::Medium),
        "high" => Ok(Severity::High),
        "critical" => Ok(Severity::Critical),
        _ => Err("Specify --severity=low|medium|high|critical".into()),
    }
}

fn visible<'a>(report: &'a AuditReport, options: &Options) -> Vec<&'a Finding> {
    report
        .findings
        .iter()
        .filter(|f| options.severity.is_none_or(|s| f.severity >= s))
        .collect()
}

// The filter only changes the display, not the check result for CI.
pub fn exit_code(report: &AuditReport, options: &Options) -> i32 {
    i32::from(options.exit_code && !report.findings.is_empty())
}

fn summary(findings: &[&Finding]) -> String {
    [
        Severity::Critical,
        Severity::High,
        Severity::Medium,
        Severity::Low,
        Severity::Unknown,
    ]
    .into_iter()
    .filter_map(|s| {
        let n = findings.iter().filter(|f| f.severity == s).count();
        (n > 0).then(|| format!("{n} {}", s.label()))
    })
    .collect::<Vec<_>>()
    .join(" · ")
}

fn fixes(findings: &[&Finding]) -> Vec<String> {
    // One package can have several CVEs. Don't repeat commands and don't put an
    // untrusted name from a foreign lock file into a shell hint.
    let mut versions = std::collections::BTreeMap::<&str, crate::semver::Version>::new();
    for f in findings {
        if !crate::registry::is_valid_package_name(&f.name) {
            continue;
        }
        if let Some(version) = f.fixed_version.as_deref().and_then(safe_fix_version) {
            // Several CVEs of one package: suggest the highest fix version.
            versions
                .entry(f.name.as_str())
                .and_modify(|v| {
                    if version > *v {
                        *v = version.clone();
                    }
                })
                .or_insert(version);
        }
    }
    let mut commands: Vec<String> = versions
        .iter()
        .map(|(name, version)| format!("jet update {name}@{}", version.raw()))
        .collect();
    if !versions.is_empty() {
        commands.push(format!(
            "jet update {}",
            versions.keys().copied().collect::<Vec<_>>().join(" ")
        ));
    }
    commands
}

fn safe_fix_version(value: &str) -> Option<crate::semver::Version> {
    // The version goes into a shell command: the semver parser allows arbitrary
    // prerelease text, so we separately reject shell metacharacters.
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
    {
        return None;
    }
    crate::semver::try_parse(value)
}

pub fn render(report: &AuditReport, options: &Options) -> String {
    let findings = visible(report, options);
    let commands = if options.fix {
        fixes(&findings)
    } else {
        Vec::new()
    };
    if options.json {
        let cache_stats = object([
            ("from_cache", Json::Num(report.from_cache as f64)),
            ("fresh", Json::Num(report.fresh as f64)),
            (
                "stale_cache_age_seconds",
                report
                    .stale_cache_age
                    .map(|age| Json::Num(age as f64))
                    .unwrap_or(Json::Null),
            ),
        ]);
        let data = object([
            ("checked", Json::Num(report.checked as f64)),
            ("totalFindings", Json::Num(report.findings.len() as f64)),
            (
                "hiddenFindings",
                Json::Num((report.findings.len() - findings.len()) as f64),
            ),
            ("cache_stats", cache_stats),
            ("osv_unavailable", Json::Bool(report.osv_unavailable)),
            (
                "unchecked",
                Json::Arr(
                    report
                        .unchecked
                        .iter()
                        .map(|n| Json::Str(ui::sanitize(n)))
                        .collect(),
                ),
            ),
            (
                "findings",
                Json::Arr(findings.iter().map(|f| finding_json(f)).collect()),
            ),
            (
                "fixes",
                Json::Arr(commands.into_iter().map(Json::Str).collect()),
            ),
        ]);
        let warnings = Json::Arr(
            report
                .warnings
                .iter()
                .map(|w| Json::Str(ui::sanitize(w)))
                .collect(),
        );
        return json::stringify_pretty(&object([
            ("ok", Json::Bool(true)),
            ("data", data),
            ("warnings", warnings),
        ]));
    }
    let mut lines = vec![
        ui::heading("Security audit — OSV.dev"),
        format!(
            "Packages checked: {} (from cache: {}, fresh: {})",
            report.checked, report.from_cache, report.fresh
        ),
    ];
    // Degradation: OSV didn't answer, we show stale cache — warn honestly.
    if report.osv_unavailable {
        let hours = report.stale_cache_age.map(|s| s / 3600).unwrap_or(0);
        lines.push(ui::yellow(&format!(
            "{} OSV unavailable, showing data from cache (age: {hours} h). Fresh vulnerabilities may be missed.",
            ui::WARN
        )));
    }
    if !report.unchecked.is_empty() {
        lines.push(ui::yellow(&format!(
            "{} Not checked (no OSV and no cache): {}",
            ui::WARN,
            report
                .unchecked
                .iter()
                .map(|n| ui::sanitize(n))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    for f in &findings {
        let title = format!(
            "{}  {}@{}",
            f.severity.label(),
            ui::sanitize(&f.name),
            ui::sanitize(&f.version)
        );
        let title = match f.severity {
            Severity::Critical | Severity::High => ui::red(&title),
            Severity::Medium => ui::yellow(&title),
            _ => ui::cyan(&title),
        };
        lines.push(format!("\n{}{}", ui::PAD, title));
        lines.push(format!(
            "{}{}{}",
            ui::PAD,
            ui::sanitize(&f.id),
            f.cvss.map(|v| format!("  CVSS {v:.1}")).unwrap_or_default()
        ));
        if !f.summary.is_empty() {
            lines.push(format!("{}{}", ui::PAD, ui::sanitize(&f.summary)));
        }
        if let Some(version) = &f.fixed_version {
            lines.push(format!(
                "{}Fixed in: {}",
                ui::PAD,
                ui::sanitize(version)
            ));
        } else if options.fix {
            lines.push(format!("{}no fix available", ui::PAD));
        }
    }
    if report.findings.is_empty() && report.unchecked.is_empty() {
        lines.push(ui::green("✓ No vulnerabilities found"));
    } else if report.findings.is_empty() {
        lines.push("No vulnerabilities found in the available data; the report is incomplete.".into());
    } else if findings.is_empty() {
        lines.push("No vulnerabilities at the selected level; findings below the threshold are hidden.".into());
    } else {
        lines.push(format!("\nSummary: {}", summary(&findings)));
    }
    let hidden = report.findings.len() - findings.len();
    if hidden > 0 {
        lines.push(format!("Hidden by filter: {hidden}"));
    }
    if options.fix {
        if commands.is_empty() {
            lines.push("No update suggestions available for the shown findings.".into());
        } else {
            lines.push(
                "\nSuggested commands (not executed; update exists only in the TS version of jet for now):"
                    .into(),
            );
            lines.extend(commands);
        }
    }
    // Non-critical cache warnings go at the end, not in the way of the main report.
    for w in &report.warnings {
        lines.push(format!("{} {}", ui::WARN, ui::sanitize(w)));
    }
    lines.join("\n")
}

fn object<const N: usize>(entries: [(&str, Json); N]) -> Json {
    Json::Obj(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn finding_json(f: &Finding) -> Json {
    object([
        ("name", Json::Str(f.name.clone())),
        ("version", Json::Str(f.version.clone())),
        ("id", Json::Str(f.id.clone())),
        ("summary", Json::Str(f.summary.clone())),
        ("severity", Json::Str(f.severity.label().to_string())),
        ("cvss", f.cvss.map(Json::Num).unwrap_or(Json::Null)),
        (
            "fixedVersion",
            f.fixed_version.clone().map(Json::Str).unwrap_or(Json::Null),
        ),
    ])
}

pub fn error_json(message: &str) -> String {
    json::stringify_pretty(&object([
        ("ok", Json::Bool(false)),
        ("data", Json::Null),
        ("error", Json::Str(ui::sanitize(message))),
        ("warnings", Json::Arr(vec![])),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }
    fn fixture() -> AuditReport {
        AuditReport {
            checked: 3,
            findings: [
                Severity::High,
                Severity::Medium,
                Severity::Low,
                Severity::Unknown,
            ]
            .into_iter()
            .map(|severity| Finding {
                name: "lodash".into(),
                version: "4.17.20".into(),
                id: format!("TEST-{}", severity.label()),
                summary: "A quote \" and a line\nbreak".into(),
                severity,
                cvss: None,
                fixed_version: Some("4.17.21".into()),
            })
            .collect(),
            ..Default::default()
        }
    }
    #[test]
    fn parses_flags_and_defaults() {
        assert_eq!(parse(&[]).unwrap(), Options::default());
        assert_eq!(
            parse(&argv(&[
                "--json",
                "--fix",
                "--exit-code",
                "--no-cache",
                "--clear-cache",
                "--severity=HIGH"
            ]))
            .unwrap(),
            Options {
                json: true,
                fix: true,
                exit_code: true,
                no_cache: true,
                clear_cache: true,
                severity: Some(Severity::High),
                help: false
            }
        );
        assert_eq!(
            parse(&argv(&["--severity", "medium"])).unwrap().severity,
            Some(Severity::Medium)
        );
        assert!(parse(&argv(&["-h"])).unwrap().help);
    }
    #[test]
    fn rejects_invalid_arguments() {
        for args in [
            vec!["--severity"],
            vec!["--severity="],
            vec!["--severity=unknown"],
            vec!["--severity=urgent"],
            vec!["--fix=true"],
            vec!["lodash"],
            vec!["--wat"],
        ] {
            assert!(parse(&argv(&args)).is_err(), "{args:?}");
        }
    }
    #[test]
    fn filter_is_inclusive_and_does_not_change_ci_result() {
        let report = fixture();
        let mut opts = Options {
            severity: Some(Severity::High),
            exit_code: true,
            ..Options::default()
        };
        assert_eq!(visible(&report, &opts).len(), 1);
        opts.severity = Some(Severity::Critical);
        assert!(visible(&report, &opts).is_empty());
        assert_eq!(exit_code(&report, &opts), 1);
        assert!(!render(&report, &opts).contains("✓ No vulnerabilities found"));
        assert_eq!(exit_code(&report, &Options::default()), 0);
        assert_eq!(
            exit_code(
                &AuditReport {
                    checked: 1,
                    findings: vec![],
                    ..Default::default()
                },
                &opts
            ),
            0
        );
    }
    #[test]
    fn renders_summary_and_clean_result() {
        assert_eq!(
            summary(&fixture().findings.iter().collect::<Vec<_>>()),
            "1 HIGH · 1 MEDIUM · 1 LOW · 1 UNKNOWN"
        );
        assert!(
            render(
                &AuditReport {
                    checked: 1,
                    findings: vec![],
                    ..Default::default()
                },
                &Options::default()
            )
            .contains("✓ No vulnerabilities found")
        );
    }
    #[test]
    fn json_is_one_envelope_with_filtered_findings() {
        let opts = Options {
            json: true,
            fix: true,
            severity: Some(Severity::Medium),
            ..Options::default()
        };
        let mut report = fixture();
        report.from_cache = 2;
        report.fresh = 1;
        report.osv_unavailable = true;
        report.stale_cache_age = Some(9_000);
        report.unchecked = vec!["axios@0.21.0".into()];
        report.warnings = vec!["stale cache used".into()];
        let rendered = render(&report, &opts);
        let parsed = json::parse(&rendered).unwrap();
        assert_eq!(parsed.get("ok").and_then(Json::as_bool), Some(true));
        let data = parsed.get("data").unwrap();
        assert_eq!(data.get("checked").and_then(Json::as_f64), Some(3.0));
        assert_eq!(data.get("hiddenFindings").and_then(Json::as_f64), Some(2.0));
        assert_eq!(
            data.get("findings").and_then(Json::as_array).unwrap().len(),
            2
        );
        let stats = data.get("cache_stats").and_then(Json::as_object).unwrap();
        assert_eq!(stats.get("from_cache").and_then(Json::as_f64), Some(2.0));
        assert_eq!(stats.get("fresh").and_then(Json::as_f64), Some(1.0));
        assert_eq!(
            stats.get("stale_cache_age_seconds").and_then(Json::as_f64),
            Some(9_000.0)
        );
        assert_eq!(
            data.get("osv_unavailable").and_then(Json::as_bool),
            Some(true)
        );
        assert_eq!(
            data.get("unchecked").and_then(Json::as_array),
            Some(&[Json::Str("axios@0.21.0".into())][..])
        );
        assert_eq!(
            parsed.get("warnings").and_then(Json::as_array),
            Some(&[Json::Str("stale cache used".into())][..])
        );
        assert_eq!(
            data.get("fixes").and_then(Json::as_array).unwrap(),
            &[
                Json::Str("jet update lodash@4.17.21".into()),
                Json::Str("jet update lodash".into())
            ]
        );
        assert_eq!(
            json::parse(&error_json("OSV unavailable"))
                .unwrap()
                .get("ok")
                .and_then(Json::as_bool),
            Some(false)
        );
    }
    #[test]
    fn renders_osv_degradation_with_stale_cache_age() {
        let mut report = fixture();
        report.osv_unavailable = true;
        report.stale_cache_age = Some(27 * 3600 + 42);
        let text = render(&report, &Options::default());
        assert!(text.contains("OSV unavailable, showing data from cache (age: 27 h)"));
    }
    #[test]
    fn renders_unchecked_packages_without_claiming_a_clean_audit() {
        let report = AuditReport {
            checked: 2,
            osv_unavailable: true,
            unchecked: vec!["axios@0.21.0".into(), "minimist@0.0.8".into()],
            ..Default::default()
        };
        let text = render(&report, &Options::default());
        assert!(text.contains("Not checked"));
        assert!(text.contains("axios@0.21.0, minimist@0.0.8"));
        assert!(text.contains("report is incomplete"));
        assert!(!text.contains("✓ No vulnerabilities found"));
    }
    #[test]
    fn fixes_are_opt_in_and_do_not_include_unsafe_names() {
        let mut report = fixture();
        assert!(!render(&report, &Options::default()).contains("jet update"));
        report.findings[0].name = "foo; calc".into();
        assert_eq!(
            fixes(&report.findings.iter().collect::<Vec<_>>()),
            vec!["jet update lodash@4.17.21", "jet update lodash"]
        );
    }
    #[test]
    fn fixes_select_highest_version_and_exclude_unfixable_packages() {
        let mut report = fixture();
        report.findings[0].fixed_version = Some("4.9.0".into());
        report.findings[1].fixed_version = Some("4.17.21".into());
        report.findings[2].name = "axios".into();
        report.findings[2].fixed_version = Some("1.12.0".into());
        report.findings[3].name = "unfixable".into();
        report.findings[3].fixed_version = None;
        let opts = Options {
            fix: true,
            ..Options::default()
        };
        assert_eq!(
            fixes(&visible(&report, &opts)),
            vec![
                "jet update axios@1.12.0",
                "jet update lodash@4.17.21",
                "jet update axios lodash",
            ]
        );
        let text = render(&report, &opts);
        assert!(text.contains("no fix available"));
        assert!(!text.contains("jet update unfixable"));
        report
            .findings
            .iter_mut()
            .for_each(|f| f.fixed_version = None);
        assert!(fixes(&visible(&report, &opts)).is_empty());
    }
    #[test]
    fn fix_versions_cannot_inject_shell_commands() {
        for value in [
            "4.17.21-foo;calc",
            "4.17.21-$(calc)",
            "4.17.21 & calc",
            "",
            "garbage",
        ] {
            assert!(safe_fix_version(value).is_none(), "{value}");
        }
        assert_eq!(
            safe_fix_version("4.17.21-beta.1").unwrap().raw(),
            "4.17.21-beta.1"
        );
    }
}
