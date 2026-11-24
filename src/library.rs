//! Project-selected text and Graphify files. Paths stay relative to the project.
use crate::server::App;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS library_files(id INTEGER PRIMARY KEY AUTOINCREMENT,
        project TEXT NOT NULL,path TEXT NOT NULL,kind TEXT NOT NULL,digest TEXT NOT NULL,stamp TEXT NOT NULL,count INTEGER NOT NULL,
        UNIQUE(project,path)); CREATE TABLE IF NOT EXISTS library_chunks(id INTEGER PRIMARY KEY,doc INTEGER NOT NULL,
        anchor INTEGER NOT NULL,label TEXT NOT NULL,text TEXT NOT NULL,UNIQUE(doc,anchor));
        CREATE VIRTUAL TABLE IF NOT EXISTS library_search USING fts5(label,text);
        CREATE TRIGGER IF NOT EXISTS library_add AFTER INSERT ON library_chunks BEGIN
        INSERT INTO library_search(rowid,label,text) VALUES(new.id,new.label,new.text); END;
        CREATE TRIGGER IF NOT EXISTS library_drop AFTER DELETE ON library_chunks BEGIN
        DELETE FROM library_search WHERE rowid=old.id; END;")?;
    crate::semantic::initialize(db)
}
fn path_for(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let found = root.join(path).canonicalize()?;
    if !found.is_file() || !found.starts_with(&root) {
        bail!("Select a file inside this project")
    }
    Ok(found)
}
fn stamp(path: &Path) -> Result<String> {
    let m = path.metadata()?;
    Ok(format!(
        "{}:{}",
        m.len(),
        m.modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ))
}
fn documents(db: &Connection, project: &str) -> Result<Vec<Value>> {
    let mut s = db.prepare(
        "SELECT id,path,kind,digest,stamp,count FROM library_files WHERE project=? ORDER BY path",
    )?;
    let rows=s.query_map([project],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"path":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,
        "digest":r.get::<_,String>(3)?,"stamp":r.get::<_,String>(4)?,"chunks":r.get::<_,i64>(5)?})))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
type Chunk = (i64, String, String);
fn text_chunks(text: &str, name: &str) -> Vec<Chunk> {
    let mut rows = Vec::new();
    let mut block = String::new();
    let mut start = 1;
    let mut heading = name.to_owned();
    for (i, line) in text.lines().enumerate() {
        if !block.is_empty()
            && (block.len() + line.len() > 2400 || i + 1 - start >= 18 || line.starts_with("# "))
        {
            rows.push((start as i64, heading.clone(), std::mem::take(&mut block)));
        }
        if block.is_empty() {
            start = i + 1;
            if line.starts_with('#') {
                heading = line
                    .trim_start_matches('#')
                    .trim()
                    .chars()
                    .take(160)
                    .collect();
            }
        }
        block.push_str(line);
        block.push('\n');
    }
    if !block.trim().is_empty() {
        rows.push((start as i64, heading, block));
    }
    rows
}
fn short(v: &Value) -> String {
    v.as_str().unwrap_or("").chars().take(240).collect()
}
fn graph_chunks(bytes: &[u8]) -> Result<Vec<Chunk>> {
    let graph: Value = serde_json::from_slice(bytes)?;
    let nodes = graph["nodes"]
        .as_array()
        .context("Expected Graphify nodes")?;
    let edges = graph
        .get("links")
        .or_else(|| graph.get("edges"))
        .and_then(Value::as_array)
        .context("Expected Graphify links or edges")?;
    if nodes.len() > 50000 || edges.len() > 250000 {
        bail!("Graph exceeds the local import limit (50,000 nodes / 250,000 edges)")
    }
    let mut names = HashMap::new();
    for n in nodes {
        let id = n["id"].as_str().context("Graph node ID must be text")?;
        if names
            .insert(id, n["label"].as_str().unwrap_or(id))
            .is_some()
        {
            bail!("Duplicate graph node ID")
        }
    }
    let mut adjacent: HashMap<&str, Vec<String>> = HashMap::new();
    for e in edges {
        let (Some(a), Some(b)) = (e["source"].as_str(), e["target"].as_str()) else {
            continue;
        };
        if !names.contains_key(a) || !names.contains_key(b) {
            continue;
        }
        let line = format!(
            "{} — {} → {} [{}]",
            names[a],
            short(&e["relation"]),
            names[b],
            short(&e["confidence"])
        );
        for id in [a, b] {
            let out = adjacent.entry(id).or_default();
            if out.len() < 8 {
                out.push(line.chars().take(320).collect());
            }
        }
    }
    Ok(nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let id = n["id"].as_str().unwrap();
            let label = names[id].chars().take(240).collect::<String>();
            let text = format!(
                "{} ({})\n{} {}\n{}",
                label,
                id,
                short(&n["source_file"]),
                short(&n["source_location"]),
                adjacent.get(id).map(|v| v.join("\n")).unwrap_or_default()
            );
            ((i + 1) as i64, label, text)
        })
        .collect())
}
fn index(
    app: &App,
    project: &str,
    root: &Path,
    path: &Path,
    kind: &str,
    previous: Option<&Value>,
) -> Result<Value> {
    let full = path_for(root, path)?;
    let relative = full
        .strip_prefix(root.canonicalize()?)?
        .to_string_lossy()
        .replace('\\', "/");
    let extension = full
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let max = match kind {
        "graph" if extension == "json" => 32 * 1024 * 1024,
        "text"
            if [
                "md", "txt", "rst", "rs", "py", "js", "jsx", "ts", "tsx", "cpp", "h", "hpp", "c",
                "cs", "toml", "yaml", "yml", "json", "csv",
            ]
            .contains(&extension.as_str()) =>
        {
            4 * 1024 * 1024
        }
        _ => bail!("Choose a text source or a Graphify JSON file"),
    };
    let file_stamp = stamp(&full)?;
    if full.metadata()?.len() > max {
        bail!("File exceeds the indexing limit")
    }
    let bytes = std::fs::read(&full)?;
    if bytes.len() as u64 > max || stamp(&full)? != file_stamp {
        bail!("Source changed during indexing; refresh again")
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if previous.is_some_and(|f| f["digest"] == digest && f["kind"] == kind) {
        let db = app.store.lock().unwrap();
        let updated = db.db.execute(
            "UPDATE library_files SET stamp=? WHERE project=? AND path=? AND digest=?",
            params![file_stamp, project, relative, digest],
        )?;
        if updated == 0 {
            bail!("Source was removed or updated concurrently")
        }
        return Ok(json!({"unchanged":true,"digest":digest,"path":relative}));
    }
    let rows = if kind == "graph" {
        graph_chunks(&bytes)?
    } else {
        text_chunks(
            std::str::from_utf8(&bytes)?.trim_start_matches('\u{feff}'),
            &relative,
        )
    };
    if rows.len() > 50000 {
        bail!("Too many source fragments")
    }
    let mut store = app.store.lock().unwrap();
    let tx = store.db.transaction()?;
    let current: Option<(i64, String)> = tx
        .query_row(
            "SELECT id,digest FROM library_files WHERE project=? AND path=?",
            params![project, relative],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if current.as_ref().map(|r| r.1.as_str()) != previous.and_then(|p| p["digest"].as_str()) {
        bail!("Source changed concurrently; refresh again")
    }
    tx.execute("INSERT INTO library_files(project,path,kind,digest,stamp,count) VALUES(?,?,?,?,?,?)
        ON CONFLICT(project,path) DO UPDATE SET kind=excluded.kind,digest=excluded.digest,stamp=excluded.stamp,count=excluded.count",
        params![project,relative,kind,digest,file_stamp,rows.len()])?;
    let id = current
        .map(|r| r.0)
        .unwrap_or_else(|| tx.last_insert_rowid());
    tx.execute("DELETE FROM library_chunks WHERE doc=?", [id])?;
    {
        let mut put =
            tx.prepare("INSERT INTO library_chunks(doc,anchor,label,text) VALUES(?,?,?,?)")?;
        for (anchor, label, text) in &rows {
            put.execute(params![id, anchor, label, text])?;
        }
    }
    tx.commit()?;
    Ok(json!({"id":id,"kind":kind,"path":relative,"chunks":rows.len(),"digest":digest}))
}
pub fn dispatch(app: &App, method: &str, p: &Value) -> Result<Value> {
    let project = p["project_id"].as_str().context("Missing project")?;
    let project_row = app.get("project", project)?;
    let root = Path::new(
        project_row["path"]
            .as_str()
            .context("Missing project directory")?,
    );
    let files = documents(&app.store.lock().unwrap().db, project)?;
    match method {
        "library.semantic_status" => {
            crate::semantic::status(&app.store.lock().unwrap().db, project)
        }
        "library.semantic" => crate::semantic::configure(
            app,
            project,
            p["enabled"]
                .as_bool()
                .context("Select enabled or disabled")?,
        ),
        "library.list" => Ok(json!(files)),
        "library.search" => Ok(json!(search(
            &app.store.lock().unwrap().db,
            project,
            root,
            p["query"].as_str().context("Enter a search query")?,
            20
        )?)),
        "library.index" => {
            let file = Path::new(p["file"].as_str().context("Choose a project file")?);
            let full = path_for(root, file)?;
            let rel = full
                .strip_prefix(root.canonicalize()?)?
                .to_string_lossy()
                .replace('\\', "/");
            let previous = files.iter().find(|f| f["path"] == rel);
            if previous.is_none() && files.len() >= 256 {
                bail!("Project has 256 selected sources")
            }
            index(
                app,
                project,
                root,
                file,
                p["kind"].as_str().unwrap_or("text"),
                previous,
            )
        }
        "library.refresh" => {
            let mut results = Vec::new();
            for f in files {
                let path = Path::new(f["path"].as_str().unwrap());
                match path_for(root, path).and_then(|full| stamp(&full)) {
                    Ok(s) if f["stamp"] == s => continue,
                    Ok(_) => results.push(
                        index(
                            app,
                            project,
                            root,
                            path,
                            f["kind"].as_str().unwrap(),
                            Some(&f),
                        )
                        .unwrap_or_else(|e| json!({"path":path,"error":e.to_string()})),
                    ),
                    Err(_) => results.push(json!({"path":path,"unavailable":true})),
                }
            }
            Ok(json!(results))
        }
        "library.forget" => {
            let id = p["id"].as_i64().context("Missing file ID")?;
            let mut store = app.store.lock().unwrap();
            let tx = store.db.transaction()?;
            if tx.execute(
                "DELETE FROM library_files WHERE project=? AND id=?",
                params![project, id],
            )? != 1
            {
                bail!("Source is not in this project")
            }
            tx.execute("DELETE FROM library_chunks WHERE doc=?", [id])?;
            tx.commit()?;
            Ok(json!({"removed":id}))
        }
        "library.source" => {
            let id = p["file_id"].as_i64().context("Missing file ID")?;
            let a = p["anchor"].as_i64().context("Missing source position")?;
            let f = files
                .iter()
                .find(|f| f["id"] == id && f["digest"] == p["digest"])
                .context("This source was updated or removed")?;
            let path = path_for(root, Path::new(f["path"].as_str().unwrap()))?;
            if stamp(&path)? != f["stamp"] {
                bail!("Source changed on disk. Refresh the project index.")
            }
            let s = app.store.lock().unwrap();
            let (label, text): (String, String) = s.db.query_row(
                "SELECT label,text FROM library_chunks WHERE doc=? AND anchor=?",
                params![id, a],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok(
                json!({"path":path,"relative":f["path"],"kind":f["kind"],"anchor":a,"label":label,"text":text}),
            )
        }
        _ => bail!("Unknown library operation"),
    }
}
pub fn search(
    db: &Connection,
    project: &str,
    root: &Path,
    question: &str,
    limit: i64,
) -> Result<Vec<Value>> {
    let terms = crate::project_memory::terms(question);
    if terms.is_empty() {
        return Ok(vec![]);
    }
    let fields = "f.id,f.path,f.stamp,f.digest,c.anchor,c.label,substr(c.text,1,3200)";
    let lookup=format!("SELECT {fields} FROM library_search s JOIN library_chunks c ON c.id=s.rowid
        JOIN library_files f ON f.id=c.doc WHERE f.project=? AND library_search MATCH ? ORDER BY rank LIMIT ?");
    let mut stmt = db.prepare(&lookup)?;
    let source = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Value> {
        let id = r.get::<_, i64>(0)?;
        let digest = r.get::<_, String>(3)?;
        let anchor = r.get::<_, i64>(4)?;
        Ok(
            json!({"id":id,"type":"library","relative":r.get::<_,String>(1)?,"stamp":r.get::<_,String>(2)?,"revision":digest,
            "label":r.get::<_,String>(5)?,"text":r.get::<_,String>(6)?,"source":format!("#/library/{id}/{digest}/{anchor}")}),
        )
    };
    let is_current = |row: &Value| {
        path_for(root, Path::new(row["relative"].as_str().unwrap()))
            .and_then(|p| stamp(&p))
            .is_ok_and(|s| row["stamp"] == s)
    };
    let mut out = Vec::new();
    for row in stmt.query_map(params![project, terms, limit * 4], source)? {
        let row = row?;
        if is_current(&row) {
            out.push(row);
        }
        if out.len() >= limit as usize {
            return Ok(out);
        }
    }
    let related = crate::semantic::ranked(db, project, question)?;
    if !related.is_empty() {
        let mut get=db.prepare(&format!("SELECT {fields} FROM library_chunks c JOIN library_files f ON f.id=c.doc WHERE f.project=? AND c.id=?"))?;
        for (id, score) in related {
            if let Some(mut row) = get.query_row(params![project, id], source).optional()? {
                if is_current(&row) && !out.iter().any(|r| r["source"] == row["source"]) {
                    row["match"] = json!("semantic");
                    row["score"] = json!(score);
                    out.push(row);
                }
            }
            if out.len() >= limit as usize {
                break;
            }
        }
    }
    Ok(out)
}
pub async fn refresh_idle(app: Arc<App>) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        if app.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if app.store.lock().unwrap().busy().unwrap_or(true) {
            continue;
        }
        let copy = app.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let projects = {
                let store = copy.store.lock().unwrap();
                let mut s = store
                    .db
                    .prepare("SELECT DISTINCT project FROM library_files")?;
                let rows = s.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for p in projects {
                let _ = dispatch(&copy, "library.refresh", &json!({"project_id":p}));
            }
            Ok::<(), anyhow::Error>(())
        })
        .await;
    }
}
