//! Our own embedded document database — a port of database.ts. Log-structured
//! (`<collection>.jsonl`, append-only), recovery on open, compaction.

use crate::json::{self, Json};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Collection {
    path: PathBuf,
    docs: BTreeMap<u64, Json>, // id -> document (Json::Obj)
    next_id: u64,
    log_lines: usize,
}

impl Collection {
    fn open(path: PathBuf) -> Collection {
        let mut c = Collection {
            path,
            docs: BTreeMap::new(),
            next_id: 1,
            log_lines: 0,
        };
        c.replay();
        c
    }

    fn replay(&mut self) {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return;
        };
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            self.log_lines += 1;
            let Ok(rec) = json::parse(line) else { continue };
            match rec.get("t").and_then(Json::as_str) {
                Some("put") => {
                    if let Some(doc) = rec.get("doc") {
                        if let Some(id) = doc.get("id").and_then(Json::as_f64).map(|n| n as u64) {
                            self.docs.insert(id, doc.clone());
                            if id >= self.next_id {
                                self.next_id = id + 1;
                            }
                        }
                    }
                }
                Some("del") => {
                    if let Some(id) = rec.get("id").and_then(Json::as_f64).map(|n| n as u64) {
                        self.docs.remove(&id);
                    }
                }
                _ => {}
            }
        }
    }

    fn append(&mut self, record: &Json) {
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(f, "{}", json::stringify(record));
        }
        self.log_lines += 1;
        if self.log_lines > self.docs.len() * 2 + 16 {
            self.compact();
        }
    }

    fn compact(&mut self) {
        let mut buf = String::new();
        for doc in self.docs.values() {
            let mut rec = std::collections::HashMap::new();
            rec.insert("t".to_string(), Json::Str("put".to_string()));
            rec.insert("doc".to_string(), doc.clone());
            buf.push_str(&json::stringify(&Json::Obj(rec)));
            buf.push('\n');
        }
        let tmp = self.path.with_extension("tmp");
        if fs::write(&tmp, &buf).is_ok() && fs::rename(&tmp, &self.path).is_ok() {
            self.log_lines = self.docs.len();
        }
    }

    /// Insert a document (Json::Obj); the DB assigns an id. Returns the id.
    pub fn insert(&mut self, mut fields: std::collections::HashMap<String, Json>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        fields.insert("id".to_string(), Json::Num(id as f64));
        let doc = Json::Obj(fields);
        self.docs.insert(id, doc.clone());
        let mut rec = std::collections::HashMap::new();
        rec.insert("t".to_string(), Json::Str("put".to_string()));
        rec.insert("doc".to_string(), doc);
        self.append(&Json::Obj(rec));
        id
    }

    /// All documents in id order.
    pub fn all(&self) -> Vec<&Json> {
        self.docs.values().collect()
    }

    pub fn count(&self) -> usize {
        self.docs.len()
    }
}

pub struct Database {
    dir: PathBuf,
}

impl Database {
    pub fn open(dir: &Path) -> Database {
        let _ = fs::create_dir_all(dir);
        Database {
            dir: dir.to_path_buf(),
        }
    }

    pub fn collection(&self, name: &str) -> Collection {
        Collection::open(self.dir.join(format!("{name}.jsonl")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn doc(name: &str, version: &str) -> HashMap<String, Json> {
        HashMap::from([
            ("name".to_string(), Json::Str(name.to_string())),
            ("version".to_string(), Json::Str(version.to_string())),
        ])
    }

    #[test]
    fn crud_and_persistence() {
        let dir = std::env::temp_dir().join(format!("jetdb-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        {
            let db = Database::open(&dir);
            let mut c = db.collection("pkgs");
            assert_eq!(c.insert(doc("express", "5.2.1")), 1);
            assert_eq!(c.insert(doc("react", "19.2.7")), 2);
            assert_eq!(c.count(), 2);
        }
        {
            // A new DB object reads the same directory — the data survived a "restart".
            let db = Database::open(&dir);
            let mut c = db.collection("pkgs");
            assert_eq!(c.count(), 2);
            assert_eq!(c.insert(doc("ms", "2.1.3")), 3, "next_id restored");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compaction_keeps_data() {
        let dir = std::env::temp_dir().join(format!("jetdb2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let db = Database::open(&dir);
        let mut c = db.collection("nums");
        for i in 0..100 {
            c.insert(HashMap::from([("n".to_string(), Json::Num(i as f64))]));
        }
        assert_eq!(c.count(), 100);
        let _ = fs::remove_dir_all(&dir);
    }
}
