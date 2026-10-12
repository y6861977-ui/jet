//! jet audit — scan installed packages for known vulnerabilities via OSV.dev
//! (Google's free public database, covering npm/PyPI/crates and more).
//!
//! Layers: this is the CORE (collecting from the lock, the OSV network client,
//! parsing the untrusted response, computing CVSS, severity mapping). Printing the
//! report, the flags (--json/--severity/--fix/--exit-code) and the command wiring
//! are in the layer on top (`main.rs`, Codex's area).
//!
//! All on `std`: HTTP via the system `curl` (like `registry.rs`), JSON via our own
//! `json.rs`, the CVSS base score computed by hand from the CVSS 3.1 formula. OSV
//! response fields are untrusted, so text is neutralized by `ui::sanitize` before
//! handing it off.

use crate::json::{self, Json};
use crate::lockfile::{read_lock, split_key};
use crate::{config, ui};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Severity level. The order of variants = the Ord order: Critical is the highest.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    Unknown,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn label(&self) -> &'static str {
        match self {
            Severity::Critical => "CRITICAL",
            Severity::High => "HIGH",
            Severity::Medium => "MEDIUM",
            Severity::Low => "LOW",
            Severity::Unknown => "UNKNOWN",
        }
    }

    /// Numeric CVSS base score -> level (thresholds per the owner's spec).
    pub fn from_score(score: f64) -> Severity {
        if score >= 9.0 {
            Severity::Critical
        } else if score >= 7.0 {
            Severity::High
        } else if score >= 4.0 {
            Severity::Medium
        } else if score > 0.0 {
            Severity::Low
        } else {
            Severity::Unknown
        }
    }

    /// A text label from database_specific.severity (GHSA style).
    fn from_text(t: &str) -> Option<Severity> {
        match t.to_ascii_uppercase().as_str() {
            "CRITICAL" => Some(Severity::Critical),
            "HIGH" => Some(Severity::High),
            "MODERATE" | "MEDIUM" => Some(Severity::Medium),
            "LOW" => Some(Severity::Low),
            _ => None,
        }
    }

    /// Parse our label (`label()`) back — for reading from the cache.
    fn from_label(s: &str) -> Severity {
        match s {
            "CRITICAL" => Severity::Critical,
            "HIGH" => Severity::High,
            "MEDIUM" => Severity::Medium,
            "LOW" => Severity::Low,
            _ => Severity::Unknown,
        }
    }
}

/// One found vulnerability for a specific installed package.
#[derive(Clone, Debug)]
pub struct Finding {
    pub name: String,
    pub version: String,
    pub id: String,
    pub summary: String,
    pub severity: Severity,
    pub cvss: Option<f64>,
    pub fixed_version: Option<String>,
}

#[derive(Debug, Default)]
pub struct AuditReport {
    pub checked: usize,
    /// Sorted by descending severity.
    pub findings: Vec<Finding>,
    /// How many packages were taken from the cache (not re-queried).
    pub from_cache: usize,
    /// How many packages were queried from OSV this time.
    pub fresh: usize,
    /// OSV was unavailable — some data is from a stale cache (degradation).
    pub osv_unavailable: bool,
    /// The maximum age (sec) of the stale cache shown during degradation.
    pub stale_cache_age: Option<u64>,
    /// Packages that couldn't be checked at all (no OSV and no cache).
    pub unchecked: Vec<String>,
    /// Non-critical warnings (cache write/read failures, etc.).
    pub warnings: Vec<String>,
}

#[derive(Default)]
pub struct AuditOptions {
    /// Ignore the cache when reading (a fresh response is still saved).
    pub no_cache: bool,
    /// Clear the entire cache before auditing (for debugging).
    pub clear_cache: bool,
}

const OSV_BATCH: &str = "https://api.osv.dev/v1/querybatch";
const OSV_VULN: &str = "https://api.osv.dev/v1/vulns";
const BATCH_LIMIT: usize = 1000;

// ---------------------------------------------------------------------------
// Collecting installed packages
// ---------------------------------------------------------------------------

/// A list of `(name, version)` for all installed packages (including transitive).
/// Source — jet-lock.json; fallback to package-lock.json if there's no jet-lock.
pub fn collect_installed(root: &Path) -> Vec<(String, String)> {
    if let Some(lock) = read_lock(root) {
        let mut out: Vec<(String, String)> =
            lock.packages.keys().filter_map(|k| split_key(k)).collect();
        out.sort();
        out.dedup();
        return out;
    }
    collect_from_npm_lock(root)
}

/// Parse package-lock.json (v2/v3 "packages", v1 "dependencies").
fn collect_from_npm_lock(root: &Path) -> Vec<(String, String)> {
    let Some(text) = crate::project::read_text_tolerant(&root.join("package-lock.json")) else {
        return Vec::new();
    };
    let Ok(doc) = json::parse(&text) else {
        return Vec::new();
    };
    let mut out = Vec::new();

    // v2/v3: { "packages": { "node_modules/lodash": { "version": "4.17.20" }, ... } }
    if let Some(pkgs) = doc.get("packages").and_then(Json::as_object) {
        for (path, entry) in pkgs {
            let Some(name) = path
                .rsplit("node_modules/")
                .next()
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            if let Some(v) = entry.get("version").and_then(Json::as_str) {
                out.push((name.to_string(), v.to_string()));
            }
        }
    }
    // v1: { "dependencies": { "lodash": { "version": "4.17.20", "dependencies": {...} } } }
    if out.is_empty() {
        if let Some(deps) = doc.get("dependencies").and_then(Json::as_object) {
            collect_v1_deps(deps, &mut out);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn collect_v1_deps(
    deps: &std::collections::HashMap<String, Json>,
    out: &mut Vec<(String, String)>,
) {
    for (name, entry) in deps {
        if let Some(v) = entry.get("version").and_then(Json::as_str) {
            out.push((name.clone(), v.to_string()));
        }
        if let Some(nested) = entry.get("dependencies").and_then(Json::as_object) {
            collect_v1_deps(nested, out);
        }
    }
}

// ---------------------------------------------------------------------------
// On-disk OSV cache — PER PACKAGE (~/.jet/audit-cache/npm/<name>@<version>.json)
// ---------------------------------------------------------------------------
//
// The key is (ecosystem, name, version), so lookup goes by file name (O(1), with
// no scanning of the whole directory). The value is the already-parsed findings of
// the package plus the query time. An empty findings list is cached too ("checked,
// no vulnerabilities"). We don't take SQLite — that's a crate, and the project is
// std-only; a JSON file per package solves the same tasks (fast lookup, atomicity
// via temp+rename, simple time-based invalidation).

const AUDIT_TTL_SECS: u64 = 24 * 60 * 60;
const CACHE_MAX_BYTES: u64 = 50 * 1024 * 1024; // threshold
const CACHE_TRIM_BYTES: u64 = 40 * 1024 * 1024; // what we trim down to

fn cache_root() -> PathBuf {
    config::config_dir().join("audit-cache")
}

/// The cache directory for an ecosystem (created on demand).
fn cache_dir(ecosystem: &str) -> PathBuf {
    let dir = cache_root().join(ecosystem);
    let _ = fs::create_dir_all(&dir);
    dir
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A cache file name safe for the FS: letters/digits/`-`/`.`/`_`, anything else -> `_`.
fn fname_safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn cache_file(dir: &Path, name: &str, version: &str) -> PathBuf {
    dir.join(format!("{}@{}.json", fname_safe(name), fname_safe(version)))
}

/// Only the vulnerability-specific fields (the package name/version are known from
/// the file name, we don't duplicate them).
fn findings_to_json(findings: &[Finding], now: u64) -> String {
    let arr = findings
        .iter()
        .map(|f| {
            Json::Obj(
                [
                    ("id".to_string(), Json::Str(f.id.clone())),
                    ("summary".to_string(), Json::Str(f.summary.clone())),
                    (
                        "severity".to_string(),
                        Json::Str(f.severity.label().to_string()),
                    ),
                    (
                        "cvss".to_string(),
                        f.cvss.map(Json::Num).unwrap_or(Json::Null),
                    ),
                    (
                        "fixed".to_string(),
                        f.fixed_version.clone().map(Json::Str).unwrap_or(Json::Null),
                    ),
                ]
                .into_iter()
                .collect(),
            )
        })
        .collect();
    let root = [
        ("queried_at".to_string(), Json::Num(now as f64)),
        ("findings".to_string(), Json::Arr(arr)),
    ]
    .into_iter()
    .collect();
    json::stringify_pretty(&Json::Obj(root))
}

/// A raw cache read WITHOUT a TTL check: `(findings, query time)`.
/// None — no file or corrupt (we don't panic).
fn read_cache_raw(dir: &Path, name: &str, version: &str) -> Option<(Vec<Finding>, u64)> {
    let text = fs::read_to_string(cache_file(dir, name, version)).ok()?;
    let doc = json::parse(&text).ok()?;
    let ts = doc.get("queried_at").and_then(Json::as_f64)? as u64;
    let arr = doc.get("findings").and_then(Json::as_array)?;
    let findings = arr
        .iter()
        .map(|v| Finding {
            name: name.to_string(),
            version: version.to_string(),
            id: v.get("id").and_then(Json::as_str).unwrap_or("").to_string(),
            summary: v
                .get("summary")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string(),
            severity: Severity::from_label(v.get("severity").and_then(Json::as_str).unwrap_or("")),
            cvss: v.get("cvss").and_then(Json::as_f64),
            fixed_version: v.get("fixed").and_then(Json::as_str).map(str::to_string),
        })
        .collect();
    Some((findings, ts))
}

/// A fresh read (within the TTL). None — missing, stale, corrupt, or `no_cache`.
fn cache_read(
    dir: &Path,
    name: &str,
    version: &str,
    no_cache: bool,
    now: u64,
) -> Option<Vec<Finding>> {
    if no_cache {
        return None;
    }
    let (findings, ts) = read_cache_raw(dir, name, version)?;
    if now.saturating_sub(ts) > AUDIT_TTL_SECS {
        return None; // stale
    }
    Some(findings)
}

/// A stale read for the fallback (any age): `(findings, age in seconds)`.
fn cache_read_stale(
    dir: &Path,
    name: &str,
    version: &str,
    now: u64,
) -> Option<(Vec<Finding>, u64)> {
    read_cache_raw(dir, name, version).map(|(f, ts)| (f, now.saturating_sub(ts)))
}

/// An atomic cache write for a package (temp + rename) — safe with parallel runs.
/// An Err isn't fatal for the audit: the caller turns it into a warning.
fn cache_write(
    dir: &Path,
    name: &str,
    version: &str,
    findings: &[Finding],
    now: u64,
) -> Result<(), String> {
    let body = findings_to_json(findings, now);
    let final_path = cache_file(dir, name, version);
    let tmp = final_path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, &body).map_err(|e| format!("{e}"))?;
    fs::rename(&tmp, &final_path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("{e}")
    })
}

/// Tear down the whole cache (for `--clear-cache`).
fn cache_clear() -> Result<(), String> {
    let root = cache_root();
    if root.exists() {
        fs::remove_dir_all(&root).map_err(|e| format!("{e}"))?;
    }
    Ok(())
}

/// If the cache outgrew the threshold — delete the oldest files until it's ≤ CACHE_TRIM.
fn cache_enforce_limit() -> Result<(), String> {
    let root = cache_root();
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total = 0u64;
    for eco in fs::read_dir(&root).into_iter().flatten().flatten() {
        for entry in fs::read_dir(eco.path()).into_iter().flatten().flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total += meta.len();
                    let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
                    files.push((entry.path(), meta.len(), mtime));
                }
            }
        }
    }
    if total <= CACHE_MAX_BYTES {
        return Ok(());
    }
    files.sort_by_key(|(_, _, mtime)| *mtime); // oldest first
    for (path, size, _) in files {
        if total <= CACHE_TRIM_BYTES {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// OSV network client: timeouts + retries with backoff + proxy (via curl)
// ---------------------------------------------------------------------------

const CONNECT_TIMEOUT: u64 = 10; // seconds to establish the connection
const MAX_RETRIES: u32 = 3;
const BACKOFF: [u64; 3] = [1, 3, 9]; // delays between attempts, sec
const RETRY_AFTER_DEFAULT: u64 = 60; // if 429 without Retry-After

/// The outcome of one HTTP attempt.
enum Attempt {
    Ok(String),
    /// Retryable (network/timeout/5xx/429). Some(sec) — delay from Retry-After.
    Retry(Option<u64>),
    /// Not fatal for a retry (4xx except 429, no curl) — retrying is pointless.
    Fatal(String),
}

/// A retry loop with exponential backoff. `fetch` and `sleep` are injected — so the
/// logic is tested without a real network and without real pauses.
fn with_retry(
    mut fetch: impl FnMut() -> Attempt,
    mut sleep: impl FnMut(u64),
) -> Result<String, String> {
    for attempt in 0..MAX_RETRIES {
        match fetch() {
            Attempt::Ok(body) => return Ok(body),
            Attempt::Fatal(msg) => return Err(msg),
            Attempt::Retry(delay) => {
                if attempt + 1 >= MAX_RETRIES {
                    break; // attempts exhausted
                }
                sleep(delay.unwrap_or(BACKOFF[attempt as usize]));
            }
        }
    }
    Err("OSV unavailable (attempts exhausted)".to_string())
}

/// Parse the Retry-After header (seconds) from curl's header dump.
fn parse_retry_after(headers: &str) -> Option<u64> {
    for line in headers.lines() {
        if let Some(rest) = line.to_ascii_lowercase().strip_prefix("retry-after:") {
            if let Ok(secs) = rest.trim().parse::<u64>() {
                return Some(secs);
            }
        }
    }
    None
}

/// Classify an HTTP code into an attempt outcome.
fn classify(code: u16, body: String, headers: &str) -> Attempt {
    match code {
        200..=299 => Attempt::Ok(body),
        429 => Attempt::Retry(Some(
            parse_retry_after(headers).unwrap_or(RETRY_AFTER_DEFAULT),
        )),
        500..=599 => Attempt::Retry(None),
        400..=499 => Attempt::Fatal(format!("OSV rejected the request (HTTP {code})")),
        _ => Attempt::Retry(None),
    }
}

/// One request attempt via curl. `-sS` without `-f`: on 4xx/5xx curl exits with 0,
/// and we read the code (%{http_code}) and headers (-D) ourselves for classification.
fn curl_once(url: &str, post_body: Option<&str>) -> Attempt {
    let pid = std::process::id();
    let body_path = std::env::temp_dir().join(format!("jet-osv-body-{pid}"));
    let hdr_path = std::env::temp_dir().join(format!("jet-osv-hdr-{pid}"));
    let req_path = std::env::temp_dir().join(format!("jet-osv-req-{pid}.json"));

    let read_timeout = config::audit_timeout().to_string();
    let connect = CONNECT_TIMEOUT.to_string();
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sS",
        "--connect-timeout",
        &connect,
        "--max-time",
        &read_timeout,
        "--retry",
        "0",
    ]);
    cmd.arg("-o").arg(&body_path);
    cmd.arg("-D").arg(&hdr_path);
    cmd.args(["-w", "%{http_code}"]);
    if let Some(p) = config::proxy() {
        cmd.args(["-x", &p]);
    }
    if let Some(body) = post_body {
        if std::fs::write(&req_path, body).is_err() {
            return Attempt::Fatal("Failed to write the request body".to_string());
        }
        cmd.args(["-X", "POST", "-H", "Content-Type: application/json"]);
        cmd.arg("--data-binary")
            .arg(format!("@{}", req_path.display()));
    }
    cmd.arg(url);

    let out = cmd.output();
    let body = std::fs::read_to_string(&body_path).unwrap_or_default();
    let headers = std::fs::read_to_string(&hdr_path).unwrap_or_default();
    for p in [&body_path, &hdr_path, &req_path] {
        let _ = std::fs::remove_file(p);
    }

    match out {
        Err(_) => Attempt::Fatal("Failed to run curl (not on PATH?)".to_string()),
        Ok(o) if !o.status.success() => Attempt::Retry(None), // network/DNS/timeout — retryable
        Ok(o) => {
            let code: u16 = String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse()
                .unwrap_or(0);
            classify(code, body, &headers)
        }
    }
}

/// GET with retries and real pauses.
fn osv_get(url: &str) -> Result<String, String> {
    with_retry(|| curl_once(url, None), real_sleep)
}

/// POST batch with retries and real pauses.
fn osv_post(url: &str, body: &str) -> Result<String, String> {
    with_retry(|| curl_once(url, Some(body)), real_sleep)
}

fn real_sleep(secs: u64) {
    std::thread::sleep(std::time::Duration::from_secs(secs));
}

/// The querybatch request body for a slice of packages.
fn build_batch_body(pkgs: &[(String, String)]) -> String {
    let queries = pkgs
        .iter()
        .map(|(name, version)| {
            let mut pkg = std::collections::HashMap::new();
            pkg.insert("name".to_string(), Json::Str(name.clone()));
            pkg.insert("ecosystem".to_string(), Json::Str("npm".to_string()));
            let mut q = std::collections::HashMap::new();
            q.insert("package".to_string(), Json::Obj(pkg));
            q.insert("version".to_string(), Json::Str(version.clone()));
            Json::Obj(q)
        })
        .collect();
    let mut root = std::collections::HashMap::new();
    root.insert("queries".to_string(), Json::Arr(queries));
    json::stringify_pretty(&Json::Obj(root))
}

/// Parse the querybatch response: a list of vulnerability ids per query (by index).
/// A pure function — tested without the network.
pub fn parse_batch(body: &str, n_queries: usize) -> Result<Vec<Vec<String>>, String> {
    let doc = json::parse(body)?;
    let mut per_query = vec![Vec::new(); n_queries];
    if let Some(results) = doc.get("results").and_then(Json::as_array) {
        for (i, res) in results.iter().enumerate() {
            if i >= n_queries {
                break;
            }
            if let Some(vulns) = res.get("vulns").and_then(Json::as_array) {
                for v in vulns {
                    if let Some(id) = v.get("id").and_then(Json::as_str) {
                        per_query[i].push(id.to_string());
                    }
                }
            }
        }
    }
    Ok(per_query)
}

/// The details of one vulnerability, extracted from the vulns/{id} response.
#[derive(Clone, Debug, Default)]
pub struct VulnInfo {
    pub summary: String,
    pub cvss: Option<f64>,
    pub severity: Severity,
    /// (package name -> the version it's fixed in).
    pub fixed: Vec<(String, String)>,
}

impl Default for Severity {
    fn default() -> Self {
        Severity::Unknown
    }
}

/// Parse the vulns/{id} response (a pure function — tested without the network).
pub fn parse_vuln(body: &str) -> Result<VulnInfo, String> {
    let doc = json::parse(body)?;
    let mut info = VulnInfo::default();

    // Title: summary, otherwise the first words of details.
    info.summary = doc
        .get("summary")
        .and_then(Json::as_str)
        .or_else(|| doc.get("details").and_then(Json::as_str))
        .map(ui::sanitize)
        .unwrap_or_default();
    // Trim long details down to a single line.
    if let Some(nl) = info.summary.find('\n') {
        info.summary.truncate(nl);
    }

    // Severity: first the CVSS vector from severity[], otherwise the text from database_specific.
    let mut best: Option<f64> = None;
    if let Some(sev) = doc.get("severity").and_then(Json::as_array) {
        for s in sev {
            if let Some(score) = s.get("score").and_then(Json::as_str) {
                if let Some(v) = cvss31_base_score(score) {
                    best = Some(best.map_or(v, |b: f64| b.max(v)));
                }
            }
        }
    }
    if let Some(v) = best {
        info.cvss = Some(v);
        info.severity = Severity::from_score(v);
    } else if let Some(t) = doc
        .get("database_specific")
        .and_then(|d| d.get("severity"))
        .and_then(Json::as_str)
    {
        info.severity = Severity::from_text(t).unwrap_or(Severity::Unknown);
    }

    // Fixed versions: affected[].ranges[].events[].fixed for npm packages.
    if let Some(affected) = doc.get("affected").and_then(Json::as_array) {
        for a in affected {
            let name = a
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            if let Some(ranges) = a.get("ranges").and_then(Json::as_array) {
                for r in ranges {
                    if let Some(events) = r.get("events").and_then(Json::as_array) {
                        for e in events {
                            if let Some(fixed) = e.get("fixed").and_then(Json::as_str) {
                                info.fixed.push((name.clone(), fixed.to_string()));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(info)
}

// ---------------------------------------------------------------------------
// Computing the CVSS 3.1 base score (by the official formula, no external data)
// ---------------------------------------------------------------------------

/// Compute the base score from a CVSS vector like
/// `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H`. None — if it's not a 3.x vector.
pub fn cvss31_base_score(vector: &str) -> Option<f64> {
    if !(vector.starts_with("CVSS:3.0") || vector.starts_with("CVSS:3.1")) {
        return None;
    }
    let mut m: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for part in vector.split('/') {
        if let Some((k, v)) = part.split_once(':') {
            m.insert(k, v);
        }
    }
    let scope_changed = m.get("S") == Some(&"C");
    let av = match *m.get("AV")? {
        "N" => 0.85,
        "A" => 0.62,
        "L" => 0.55,
        "P" => 0.2,
        _ => return None,
    };
    let ac = match *m.get("AC")? {
        "L" => 0.77,
        "H" => 0.44,
        _ => return None,
    };
    let pr = match (*m.get("PR")?, scope_changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        ("H", false) => 0.27,
        ("H", true) => 0.5,
        _ => return None,
    };
    let ui = match *m.get("UI")? {
        "N" => 0.85,
        "R" => 0.62,
        _ => return None,
    };
    let cia = |v: &str| -> f64 {
        match v {
            "H" => 0.56,
            "L" => 0.22,
            "N" => 0.0,
            _ => -1.0,
        }
    };
    let c = cia(m.get("C")?);
    let i = cia(m.get("I")?);
    let a = cia(m.get("A")?);
    if c < 0.0 || i < 0.0 || a < 0.0 {
        return None;
    }

    let iss: f64 = 1.0 - (1.0 - c) * (1.0 - i) * (1.0 - a);
    let impact: f64 = if scope_changed {
        7.52 * (iss - 0.029) - 3.25 * (iss - 0.02).powi(15)
    } else {
        6.42 * iss
    };
    if impact <= 0.0 {
        return Some(0.0);
    }
    let exploitability: f64 = 8.22 * av * ac * pr * ui;
    let raw: f64 = if scope_changed {
        (1.08 * (impact + exploitability)).min(10.0)
    } else {
        (impact + exploitability).min(10.0)
    };
    Some(roundup(raw))
}

/// Round up to 0.1 per the CVSS 3.1 spec (integer-based, without float errors).
fn roundup(input: f64) -> f64 {
    let int_input = (input * 100000.0).round() as i64;
    if int_input % 10000 == 0 {
        int_input as f64 / 100000.0
    } else {
        ((int_input / 10000) + 1) as f64 / 10.0
    }
}

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

/// A full audit with a per-package cache.
///
/// The logic (Step 3 spec): collect packages → filter out the fresh ones from the
/// cache → query OSV only for the rest (in batches ≤1000) → write them to the cache
/// → assemble the report from (cache + fresh) + stats. Cache failures aren't fatal —
/// they go into `warnings`.
pub fn audit(root: &Path, opts: &AuditOptions) -> Result<AuditReport, String> {
    let pkgs = collect_installed(root);
    if pkgs.is_empty() {
        return Err("No installed packages found (no jet-lock.json / package-lock.json). First: jet install".to_string());
    }
    audit_packages(&pkgs, opts)
}

/// Audit an arbitrary list of `(name, version)` — the same cache/retry/fallback logic
/// as `audit`, but without reading a lockfile. Reused by jet-server (CVE scan on
/// publish). NOTE: it goes to the network (OSV) — the caller must gate air-gapped.
pub fn audit_packages(
    pkgs: &[(String, String)],
    opts: &AuditOptions,
) -> Result<AuditReport, String> {
    const ECO: &str = "npm";
    let mut warnings = Vec::new();
    if opts.clear_cache {
        if let Err(e) = cache_clear() {
            warnings.push(format!("failed to clear the cache: {e}"));
        }
    }
    let dir = cache_dir(ECO);
    let now = now_secs();

    // 1. Split into "has something fresh in the cache" and "needs querying".
    let mut findings = Vec::new();
    let mut from_cache = 0usize;
    let mut to_query: Vec<(String, String)> = Vec::new();
    for (name, version) in pkgs {
        match cache_read(&dir, name, version, opts.no_cache, now) {
            Some(cached) => {
                findings.extend(cached);
                from_cache += 1;
            }
            None => to_query.push((name.clone(), version.clone())),
        }
    }
    let fresh = to_query.len();

    // 2. Query OSV only for the non-fresh ones (in batches). If a batch doesn't
    //    answer even after retries — degrade to the stale cache, and whatever isn't
    //    covered by it — into `unchecked` (partial failure handled chunk by chunk).
    let mut vuln_memo: std::collections::HashMap<String, VulnInfo> =
        std::collections::HashMap::new();
    let mut osv_unavailable = false;
    let mut stale_cache_age: Option<u64> = None;
    let mut unchecked = Vec::new();

    for chunk in to_query.chunks(BATCH_LIMIT) {
        let body = build_batch_body(chunk);
        let resp = match osv_post(OSV_BATCH, &body) {
            Ok(r) => r,
            Err(_) => {
                // Fallback: stale cache for these packages, otherwise — unchecked.
                osv_unavailable = true;
                for (name, version) in chunk {
                    match cache_read_stale(&dir, name, version, now) {
                        Some((cached, age)) => {
                            findings.extend(cached);
                            stale_cache_age = Some(stale_cache_age.map_or(age, |a| a.max(age)));
                        }
                        None => unchecked.push(format!("{name}@{version}")),
                    }
                }
                continue;
            }
        };
        let per_query = parse_batch(&resp, chunk.len())?;

        for (idx, ids) in per_query.iter().enumerate() {
            let (name, version) = &chunk[idx];
            let mut pkg_findings = Vec::new();
            for id in ids {
                if !vuln_memo.contains_key(id) {
                    // Details unavailable -> we still record the vulnerability (the id is
                    // known), just without summary/severity, so as not to lose the finding.
                    let info = match osv_get(&format!("{OSV_VULN}/{id}")) {
                        Ok(detail) => parse_vuln(&detail).unwrap_or_default(),
                        Err(_) => VulnInfo {
                            summary: "(details unavailable)".to_string(),
                            ..VulnInfo::default()
                        },
                    };
                    vuln_memo.insert(id.clone(), info);
                }
                let info = &vuln_memo[id];
                let fixed = info
                    .fixed
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone());
                pkg_findings.push(Finding {
                    name: name.clone(),
                    version: version.clone(),
                    id: ui::sanitize(id),
                    summary: info.summary.clone(),
                    severity: info.severity,
                    cvss: info.cvss,
                    fixed_version: fixed,
                });
            }
            // 3. Write to the cache (including an empty list = "no vulnerabilities").
            if let Err(e) = cache_write(&dir, name, version, &pkg_findings, now) {
                warnings.push(format!("cache not written for {name}@{version}: {e}"));
            }
            findings.extend(pkg_findings);
        }
    }

    // 4. Cap the cache size.
    if let Err(e) = cache_enforce_limit() {
        warnings.push(format!("failed to trim the cache: {e}"));
    }

    // If nothing was checked at all (OSV went down and there's no cache) — a clear error.
    if osv_unavailable && findings.is_empty() && from_cache == 0 {
        return Err(
            "OSV unavailable. Check your internet connection or set a proxy:\n  jet set proxy <url>\n  (or the HTTP_PROXY / HTTPS_PROXY environment variables)"
                .to_string(),
        );
    }

    // By descending severity, then by name — stable.
    findings.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.name.cmp(&b.name)));
    Ok(AuditReport {
        checked: pkgs.len(),
        findings,
        from_cache,
        fresh,
        osv_unavailable,
        stale_cache_age,
        unchecked,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cvss31_known_vectors() {
        // lodash CVE-2021-23337 (prototype pollution) — the vector from NVD, base 7.2 HIGH.
        let v = cvss31_base_score("CVSS:3.1/AV:N/AC:L/PR:H/UI:N/S:U/C:H/I:H/A:H").unwrap();
        assert!((v - 7.2).abs() < 0.05, "expected ~7.2, got {v}");
        assert_eq!(Severity::from_score(v), Severity::High);

        // The classic "everything maxed" — 9.8 CRITICAL.
        let crit = cvss31_base_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H").unwrap();
        assert!((crit - 9.8).abs() < 0.05, "expected ~9.8, got {crit}");
        assert_eq!(Severity::from_score(crit), Severity::Critical);

        // Not 3.x — None.
        assert!(cvss31_base_score("CVSS:2.0/AV:N/AC:L/Au:N/C:P/I:P/A:P").is_none());
    }

    #[test]
    fn severity_ordering_and_mapping() {
        assert!(Severity::Critical > Severity::High);
        assert!(Severity::High > Severity::Medium);
        assert!(Severity::Low > Severity::Unknown);
        assert_eq!(Severity::from_score(9.0), Severity::Critical);
        assert_eq!(Severity::from_score(7.0), Severity::High);
        assert_eq!(Severity::from_score(4.0), Severity::Medium);
        assert_eq!(Severity::from_score(0.5), Severity::Low);
        assert_eq!(Severity::from_score(0.0), Severity::Unknown);
    }

    #[test]
    fn parse_batch_aligns_by_index() {
        let body = r#"{"results":[{},{"vulns":[{"id":"GHSA-aaa"},{"id":"CVE-bbb"}]},{}]}"#;
        let got = parse_batch(body, 3).unwrap();
        assert!(got[0].is_empty());
        assert_eq!(got[1], vec!["GHSA-aaa", "CVE-bbb"]);
        assert!(got[2].is_empty());
    }

    #[test]
    fn parse_vuln_extracts_fields() {
        let body = r#"{
            "id":"CVE-2021-23337",
            "summary":"Command injection in lodash",
            "severity":[{"type":"CVSS_V3","score":"CVSS:3.1/AV:N/AC:L/PR:H/UI:N/S:U/C:H/I:H/A:H"}],
            "affected":[{"package":{"name":"lodash","ecosystem":"npm"},
                "ranges":[{"type":"SEMVER","events":[{"introduced":"0"},{"fixed":"4.17.21"}]}]}]
        }"#;
        let info = parse_vuln(body).unwrap();
        assert_eq!(info.summary, "Command injection in lodash");
        assert_eq!(info.severity, Severity::High);
        assert!(info.cvss.map(|v| (v - 7.2).abs() < 0.05).unwrap_or(false));
        assert_eq!(
            info.fixed,
            vec![("lodash".to_string(), "4.17.21".to_string())]
        );
    }

    #[test]
    fn parse_vuln_falls_back_to_text_severity() {
        let body = r#"{"id":"GHSA-x","summary":"x","database_specific":{"severity":"MODERATE"}}"#;
        let info = parse_vuln(body).unwrap();
        assert_eq!(info.severity, Severity::Medium);
        assert!(info.cvss.is_none());
    }

    fn one(name: &str, sev: Severity, fixed: Option<&str>) -> Finding {
        Finding {
            name: name.into(),
            version: "1.0.0".into(),
            id: "CVE-x".into(),
            summary: "s".into(),
            severity: sev,
            cvss: Some(7.2),
            fixed_version: fixed.map(str::to_string),
        }
    }

    #[test]
    fn per_package_cache_roundtrip_and_ttl() {
        let dir = std::env::temp_dir().join(format!("jet-pkgcache-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let now = 1_000_000u64;
        let findings = vec![one("lodash", Severity::High, Some("4.17.21"))];
        cache_write(&dir, "lodash", "4.17.20", &findings, now).unwrap();

        // Fresh — a hit, fields restored.
        let got = cache_read(&dir, "lodash", "4.17.20", false, now + 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].severity, Severity::High);
        assert_eq!(got[0].fixed_version.as_deref(), Some("4.17.21"));
        assert_eq!(got[0].cvss, Some(7.2));

        // After 25 hours — stale.
        assert!(cache_read(&dir, "lodash", "4.17.20", false, now + AUDIT_TTL_SECS + 1).is_none());
        // --no-cache ignores even fresh data.
        assert!(cache_read(&dir, "lodash", "4.17.20", true, now + 10).is_none());
        // A different package — a miss.
        assert!(cache_read(&dir, "ms", "2.0.0", false, now).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_findings_are_cached_as_no_vulns() {
        // "Checked, no vulnerabilities" is cached too — otherwise clean packages
        // would be queried every time.
        let dir = std::env::temp_dir().join(format!("jet-emptycache-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        cache_write(&dir, "safe", "1.0.0", &[], 1000).unwrap();
        let got = cache_read(&dir, "safe", "1.0.0", false, 1001);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(
            got.map(|v| v.len()),
            Some(0),
            "an empty list is a valid hit"
        );
    }

    #[test]
    fn retry_succeeds_after_transient_5xx() {
        let mut n = 0;
        let mut sleeps = Vec::new();
        let r = with_retry(
            || {
                n += 1;
                if n < 3 {
                    Attempt::Retry(None)
                } else {
                    Attempt::Ok("ok".into())
                }
            },
            |s| sleeps.push(s),
        );
        assert_eq!(r.unwrap(), "ok");
        assert_eq!(
            sleeps,
            vec![1, 3],
            "exponential backoff between attempts"
        );
        assert_eq!(n, 3);
    }

    #[test]
    fn retry_stops_immediately_on_fatal_4xx() {
        let mut sleeps = Vec::new();
        let r = with_retry(|| Attempt::Fatal("HTTP 400".into()), |s| sleeps.push(s));
        assert!(r.is_err());
        assert!(sleeps.is_empty(), "4xx (except 429) is not retried");
    }

    #[test]
    fn retry_gives_up_after_max_attempts() {
        let mut n = 0;
        let mut sleeps = Vec::new();
        let r = with_retry(
            || {
                n += 1;
                Attempt::Retry(None)
            },
            |s| sleeps.push(s),
        );
        assert!(r.is_err());
        assert_eq!(n, MAX_RETRIES);
        assert_eq!(sleeps, vec![1, 3], "2 pauses between 3 attempts");
    }

    #[test]
    fn retry_honors_429_retry_after() {
        let mut n = 0;
        let mut sleeps = Vec::new();
        let _ = with_retry(
            || {
                n += 1;
                if n < 2 {
                    Attempt::Retry(Some(7))
                } else {
                    Attempt::Ok("x".into())
                }
            },
            |s| sleeps.push(s),
        );
        assert_eq!(sleeps, vec![7], "the delay from Retry-After, not backoff");
    }

    #[test]
    fn classify_and_parse_retry_after() {
        assert_eq!(
            parse_retry_after("HTTP/2 429\r\nRetry-After: 12\r\n"),
            Some(12)
        );
        assert_eq!(parse_retry_after("no such header"), None);
        assert!(matches!(classify(200, "b".into(), ""), Attempt::Ok(_)));
        assert!(matches!(
            classify(503, String::new(), ""),
            Attempt::Retry(None)
        ));
        assert!(matches!(
            classify(404, String::new(), ""),
            Attempt::Fatal(_)
        ));
        assert!(matches!(
            classify(429, String::new(), "retry-after: 30"),
            Attempt::Retry(Some(30))
        ));
        assert!(matches!(
            classify(429, String::new(), ""),
            Attempt::Retry(Some(RETRY_AFTER_DEFAULT))
        ));
    }

    #[test]
    fn stale_cache_read_ignores_ttl_and_reports_age() {
        let dir = std::env::temp_dir().join(format!("jet-stale-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let now = 1_000_000u64;
        cache_write(
            &dir,
            "lodash",
            "4.17.20",
            &[one("lodash", Severity::High, None)],
            now - 100 * 3600,
        )
        .unwrap();
        // A fresh read rejects it (stale), a stale read returns it with its age.
        assert!(cache_read(&dir, "lodash", "4.17.20", false, now).is_none());
        let (f, age) = cache_read_stale(&dir, "lodash", "4.17.20", now).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(f.len(), 1);
        assert_eq!(age, 100 * 3600);
    }

    #[test]
    fn cache_read_survives_corrupt_file() {
        // A corrupt cache file doesn't crash the read — it's treated as a miss.
        let dir = std::env::temp_dir().join(format!("jet-corrupt-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        fs::write(cache_file(&dir, "x", "1.0.0"), "{ this is not json").unwrap();
        let got = cache_read(&dir, "x", "1.0.0", false, 1000);
        let _ = fs::remove_dir_all(&dir);
        assert!(got.is_none());
    }

    #[test]
    fn sanitize_strips_ansi_from_summary() {
        // An ANSI injection in the summary must not reach the terminal.
        let body = "{\"id\":\"x\",\"summary\":\"evil\u{1b}[31mRED\u{1b}[0m\"}";
        let info = parse_vuln(body).unwrap();
        assert!(!info.summary.contains('\u{1b}'), "escape removed");
        assert!(info.summary.contains("RED"));
    }
}
