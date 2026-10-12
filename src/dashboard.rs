//! A local HTTP dashboard — a port of dashboard*.ts. Our own minimal HTTP/1.1
//! server on std::net, aggregating the install history into the /api/dashboard contract.

use crate::history::{HistoryEvent, read_history};
use crate::json::{self, Json};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;

/// Assemble the dashboard data from the history.
pub fn aggregate(dir: &Path) -> Json {
    let events = read_history(dir);

    let total_installs = events.len();
    let mut unique: HashSet<String> = HashSet::new();
    let mut size_by_key: HashMap<String, u64> = HashMap::new();
    for e in &events {
        let key = format!("{}@{}", e.name, e.version);
        unique.insert(key.clone());
        if let Some(b) = e.bytes {
            size_by_key.entry(key).or_insert(b);
        }
    }
    let total_bytes: u64 = size_by_key.values().sum();

    // topBySize: unique packages by descending size, first 10.
    let mut top: Vec<(&HistoryEvent, u64)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for e in &events {
        let key = format!("{}@{}", e.name, e.version);
        if seen.insert(key) {
            top.push((e, e.bytes.unwrap_or(0)));
        }
    }
    top.sort_by(|a, b| b.1.cmp(&a.1));
    top.truncate(10);

    // recent: the last 10.
    let recent: Vec<&HistoryEvent> = events.iter().rev().take(10).collect();

    let ev_json = |e: &HistoryEvent| {
        let mut o = HashMap::new();
        o.insert("name".to_string(), Json::Str(e.name.clone()));
        o.insert("version".to_string(), Json::Str(e.version.clone()));
        o.insert(
            "bytes".to_string(),
            e.bytes.map(|b| Json::Num(b as f64)).unwrap_or(Json::Null),
        );
        o.insert("at".to_string(), Json::Str(e.at.clone()));
        Json::Obj(o)
    };

    let mut stats = HashMap::new();
    stats.insert(
        "totalInstalls".to_string(),
        Json::Num(total_installs as f64),
    );
    stats.insert("uniquePackages".to_string(), Json::Num(unique.len() as f64));
    stats.insert("totalBytes".to_string(), Json::Num(total_bytes as f64));
    stats.insert("failedInstalls".to_string(), Json::Num(0.0));

    let mut root = HashMap::new();
    root.insert("stats".to_string(), Json::Obj(stats));
    root.insert(
        "topBySize".to_string(),
        Json::Arr(top.iter().map(|(e, _)| ev_json(e)).collect()),
    );
    root.insert(
        "recent".to_string(),
        Json::Arr(recent.iter().map(|e| ev_json(e)).collect()),
    );
    Json::Obj(root)
}

const HTML: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>jet — dashboard</title>
<style>body{margin:0;background:#0e1016;color:#e7e9ee;font:14px system-ui,sans-serif}
.wrap{max-width:900px;margin:0 auto;padding:28px 20px}h1{font-size:20px}
.tiles{display:grid;grid-template-columns:repeat(4,1fr);gap:12px;margin:20px 0}
.tile{background:#161923;border:1px solid #262b37;border-radius:12px;padding:14px}
.tile .n{font-size:24px;font-weight:700}.tile .l{color:#98a1b2;font-size:11px;text-transform:uppercase}
.card{background:#161923;border:1px solid #262b37;border-radius:12px;padding:16px;margin-bottom:16px}
.row{display:grid;grid-template-columns:150px 1fr auto;align-items:center;gap:10px;padding:3px 0}
.track{height:12px;background:#1e222c;border-radius:4px;overflow:hidden}.fill{height:100%;background:#7c78f0;border-radius:4px}
.v{color:#98a1b2;font-size:11px}table{width:100%;border-collapse:collapse}td,th{text-align:left;padding:6px 8px;border-bottom:1px solid #262b37;font-size:13px}
th{color:#98a1b2;font-size:11px;text-transform:uppercase}.num{text-align:right}</style></head>
<body><div class="wrap"><h1>jet · dashboard</h1><p class="v">Install analytics (native version)</p>
<div class="tiles" id="tiles"></div>
<div class="card"><h2>Top by size</h2><div id="bars"></div></div>
<div class="card"><h2>Recent installs</h2><table><thead><tr><th>Package</th><th>Version</th><th class="num">Size</th></tr></thead><tbody id="recent"></tbody></table></div>
</div><script>
const fb=b=>b==null?'—':b<1024?b+' B':b<1048576?(b/1024).toFixed(1)+' KB':(b/1048576).toFixed(2)+' MB';
fetch('/api/dashboard').then(r=>r.json()).then(d=>{
const s=d.stats;document.getElementById('tiles').innerHTML=[[s.totalInstalls,'installs'],[s.uniquePackages,'packages'],[fb(s.totalBytes),'size'],[s.failedInstalls,'failures']].map(t=>`<div class="tile"><div class="n">${t[0]}</div><div class="l">${t[1]}</div></div>`).join('');
const max=Math.max(1,...d.topBySize.map(x=>x.bytes||0));
document.getElementById('bars').innerHTML=d.topBySize.map(x=>`<div class="row"><div>${x.name} <span class="v">${x.version}</span></div><div class="track"><div class="fill" style="width:${(x.bytes||0)/max*100}%"></div></div><div class="v">${fb(x.bytes)}</div></div>`).join('')||'<p class="v">empty</p>';
document.getElementById('recent').innerHTML=d.recent.map(x=>`<tr><td>${x.name}</td><td class="v">${x.version}</td><td class="num">${fb(x.bytes)}</td></tr>`).join('')||'<tr><td colspan=3 class="v">Nothing installed yet</td></tr>';
});</script></body></html>"#;

fn respond(stream: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn handle(mut stream: TcpStream, dir: &Path) {
    let mut buf = [0u8; 2048];
    let n = stream.read(&mut buf).unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/");
    if path.starts_with("/api/dashboard") {
        let body = json::stringify(&aggregate(dir));
        respond(
            &mut stream,
            "200 OK",
            "application/json; charset=utf-8",
            body.as_bytes(),
        );
    } else if path == "/" {
        respond(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            HTML.as_bytes(),
        );
    } else {
        respond(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found",
        );
    }
}

/// Start the server, auto-picking a free port starting from 8765.
pub fn serve(dir: &Path) -> Result<(), String> {
    let listener = (8765..8800)
        .find_map(|p| TcpListener::bind(("127.0.0.1", p)).ok())
        .ok_or("Couldn't find a free port")?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    println!("\n{}", crate::ui::heading("Dashboard started"));
    println!(
        "{}",
        crate::ui::field(
            "address",
            5,
            &crate::ui::cyan(&format!("http://127.0.0.1:{port}/"))
        )
    );
    println!(
        "{}",
        crate::ui::field(
            "API",
            5,
            &crate::ui::dim(&format!("http://127.0.0.1:{port}/api/dashboard"))
        )
    );
    println!("\n{}\n", crate::ui::hint("Stop: Ctrl+C"));
    for stream in listener.incoming() {
        match stream {
            Ok(s) => handle(s, dir),
            Err(_) => continue,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::ResolvedNode;

    #[test]
    fn aggregate_from_history() {
        let dir = std::env::temp_dir().join(format!("jetdash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let node = |n: &str, v: &str, s: u64| ResolvedNode {
            name: n.into(),
            version: v.into(),
            requested: format!("^{v}"),
            unpacked_size: Some(s),
            ..Default::default()
        };
        crate::history::record_installs(
            &[node("express", "5.2.1", 500), node("react", "19.0.0", 300)],
            &dir,
        );
        let agg = aggregate(&dir);
        assert_eq!(
            agg.get("stats")
                .unwrap()
                .get("totalInstalls")
                .unwrap()
                .as_f64(),
            Some(2.0)
        );
        assert_eq!(
            agg.get("stats")
                .unwrap()
                .get("totalBytes")
                .unwrap()
                .as_f64(),
            Some(800.0)
        );
        let top = agg.get("topBySize").unwrap();
        if let Json::Arr(items) = top {
            assert_eq!(items[0].get("name").unwrap().as_str(), Some("express")); // the largest
        } else {
            panic!("topBySize is not an array");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
