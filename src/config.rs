//! Configuration and paths — a port of config.ts. State in files, no database.

use crate::json::{self, Json};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";

/// The license API base (the jet Pro site). TODO: replace with the real domain
/// before release. Overridden by the env `JET_LICENSE_API` or `jet set license.api`.
pub const DEFAULT_LICENSE_API: &str = "https://jet.kim";

/// The user's home directory (cross-platform, no crates).
pub fn home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h);
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        return PathBuf::from(h); // Windows
    }
    PathBuf::from(".")
}

/// The client root: ~/.jet
pub fn config_dir() -> PathBuf {
    let dir = home_dir().join(".jet");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn cache_dir() -> PathBuf {
    let dir = config_dir().join("cache");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn store_dir() -> PathBuf {
    let dir = config_dir().join("store");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn db_dir() -> PathBuf {
    let dir = config_dir().join("db");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// Read all of config.json as a map (empty if the file is missing/corrupt).
fn read_config() -> HashMap<String, Json> {
    match fs::read_to_string(config_path())
        .ok()
        .and_then(|s| json::parse(&s).ok())
    {
        Some(Json::Obj(m)) => m,
        _ => HashMap::new(),
    }
}

/// A setting's value as a string (None — no key). The shared reader for all
/// single settings, so different keys don't overwrite each other.
pub fn get_raw(key: &str) -> Option<String> {
    read_config()
        .get(key)
        .and_then(Json::as_str)
        .map(str::to_string)
}

/// Write/update ONE key, keeping the rest (a merge, not a file overwrite).
pub fn set_value(key: &str, value: &str) -> Result<(), String> {
    let mut obj = read_config();
    obj.insert(key.to_string(), Json::Str(value.to_string()));
    fs::write(
        config_path(),
        json::stringify_pretty(&Json::Obj(obj)) + "\n",
    )
    .map_err(|e| format!("Failed to write config.json: {e}"))
}

/// The current registry URL (without a trailing slash).
pub fn get_registry() -> String {
    get_raw("registry")
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Change the registry. Checks for http/https.
pub fn set_registry(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(format!("The registry must be http(s): {url}"));
    }
    set_value("registry", trimmed.trim_end_matches('/'))
}

/// The audit request timeout (sec). The `audit.timeout` setting, default 30, clamped.
pub fn audit_timeout() -> u64 {
    get_raw("audit.timeout")
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|n| n.clamp(5, 600))
        .unwrap_or(30)
}

/// The proxy for requests (the `proxy` setting). If not set — curl picks up
/// HTTP_PROXY/HTTPS_PROXY from the environment itself.
pub fn proxy() -> Option<String> {
    get_raw("proxy").filter(|s| !s.trim().is_empty())
}

/// The license API base (without a trailing slash). Priority: env `JET_LICENSE_API`
/// -> the `license.api` setting -> the default.
pub fn license_api() -> String {
    std::env::var("JET_LICENSE_API")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| get_raw("license.api"))
        .unwrap_or_else(|| DEFAULT_LICENSE_API.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// The path to the stored license: ~/.jet/license.json
pub fn license_path() -> PathBuf {
    config_dir().join("license.json")
}
