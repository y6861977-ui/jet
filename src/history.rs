//! Install history in the DB — a port of history.ts. One event per root package.

use crate::config::db_dir;
use crate::db::Database;
use crate::json::Json;
use crate::resolver::ResolvedNode;
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// A rough ISO-like timestamp from the system clock (UTC, no external crates).
pub fn now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Simplified: epoch seconds in an ISO-like form are enough for sorting/display.
    // A full calendar computation can be added later.
    format!("{secs}") // stored as a string; for the dashboard what matters is monotonicity
}

/// Record one event per installed root package.
pub fn record_installs(roots: &[ResolvedNode], dir: &Path) {
    if roots.is_empty() {
        return;
    }
    let db = Database::open(dir);
    let mut col = db.collection("history");
    let at = now_iso();
    for r in roots {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Json::Str(r.name.clone()));
        fields.insert("version".to_string(), Json::Str(r.version.clone()));
        fields.insert(
            "bytes".to_string(),
            r.unpacked_size
                .map(|b| Json::Num(b as f64))
                .unwrap_or(Json::Null),
        );
        fields.insert("at".to_string(), Json::Str(at.clone()));
        col.insert(fields);
    }
}

pub struct HistoryEvent {
    pub name: String,
    pub version: String,
    pub bytes: Option<u64>,
    pub at: String,
}

pub fn read_history(dir: &Path) -> Vec<HistoryEvent> {
    let db = Database::open(dir);
    let col = db.collection("history");
    col.all()
        .into_iter()
        .filter_map(|d| {
            Some(HistoryEvent {
                name: d.get("name")?.as_str()?.to_string(),
                version: d.get("version")?.as_str()?.to_string(),
                bytes: d.get("bytes").and_then(Json::as_f64).map(|n| n as u64),
                at: d.get("at").and_then(Json::as_str).unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// The default history directory (~/.jet/db).
pub fn default_dir() -> std::path::PathBuf {
    db_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, version: &str, size: Option<u64>) -> ResolvedNode {
        ResolvedNode {
            name: name.to_string(),
            version: version.to_string(),
            requested: format!("^{version}"),
            unpacked_size: size,
            ..Default::default()
        }
    }

    #[test]
    fn record_and_read() {
        let dir = std::env::temp_dir().join(format!("jethist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        record_installs(
            &[
                node("express", "5.2.1", Some(512400)),
                node("ms", "2.1.3", None),
            ],
            &dir,
        );
        let ev = read_history(&dir);
        assert_eq!(ev.len(), 2);
        let express = ev.iter().find(|e| e.name == "express").unwrap();
        assert_eq!(express.bytes, Some(512400));
        assert!(ev.iter().any(|e| e.name == "ms" && e.bytes.is_none()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
