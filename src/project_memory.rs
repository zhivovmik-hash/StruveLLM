use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, Row};
use serde_json::{json, Value};

pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("
        CREATE TABLE IF NOT EXISTS project_notes (
            id INTEGER PRIMARY KEY AUTOINCREMENT, project TEXT NOT NULL, chat TEXT NOT NULL,
            run TEXT NOT NULL, role TEXT NOT NULL, note TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 1,
            UNIQUE(project,chat,run,role));
        CREATE VIRTUAL TABLE IF NOT EXISTS note_search USING fts5(note);
        CREATE TRIGGER IF NOT EXISTS note_insert AFTER INSERT ON project_notes BEGIN
            INSERT INTO note_search(rowid,note) VALUES(new.id,new.note); END;
        CREATE TRIGGER IF NOT EXISTS note_update AFTER UPDATE OF note ON project_notes BEGIN
            UPDATE note_search SET note=new.note WHERE rowid=new.id; END;
        CREATE TRIGGER IF NOT EXISTS note_delete AFTER DELETE ON project_notes BEGIN
            DELETE FROM note_search WHERE rowid=old.id; END;")?;
    Ok(())
}

fn field<'a>(p: &'a Value, name: &str) -> Result<&'a str> {
    p[name].as_str().filter(|s| !s.trim().is_empty()).with_context(|| format!("Missing {name}"))
}

fn note_text(text: &str) -> Result<&str> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 2000 { bail!("A memory note must contain 1–2000 characters") }
    Ok(text)
}

fn row(r: &Row<'_>) -> rusqlite::Result<Value> {
    let chat: String = r.get(2)?;
    let run: String = r.get(3)?;
    let role: String = r.get(4)?;
    Ok(json!({"id":r.get::<_,i64>(0)?,"project_id":r.get::<_,String>(1)?,"chat_id":chat,
        "run_id":run,"role":role,"text":r.get::<_,String>(5)?,"revision":r.get::<_,i64>(6)?,
        "source":format!("#/source/{chat}/{run}/{role}")}))
}

// Operators supplied by the user are words, never FTS expressions. Short Cyrillic prefixes
// cover common case endings. English identifiers with underscores remain searchable by part.
pub(crate) fn terms(query: &str) -> String {
    let mut terms = Vec::new();
    for word in query.split(|c: char| !c.is_alphanumeric()).map(str::to_lowercase) {
        let len = word.chars().count();
        if len < 2 || matches!(word.as_str(),"какой"|"какая"|"какое"|"какие"|"что"|"как"|"для"|"где"|"это"|"мы"|"по"|"на"|"из"|"the"|"what"|"was"|"for"|"our"|"and"|"with"|"please") {continue}
        let prefix: String = word.chars().take(if len >= 6 && word.chars().all(|c| ('а'..='я').contains(&c)) {len-2} else {len.min(80)}).collect();
        let term = format!("\"{prefix}\"*");
        if !terms.contains(&term) { terms.push(term); }
        let aliases:&[&str]=if word.starts_with("контекст")||word=="context"{&["context","контекст"]}
            else if word.starts_with("температур")||word=="temperature"{&["temperature","температур"]}
            else if word.starts_with("видеопамят")||word=="vram"{&["vram","видеопамят"]}
            else if word.starts_with("мозаик")||word=="mosaic"{&["mosaic","мозаик"]}else{&[]};
        for alias in aliases{let term=format!("\"{alias}\"*");if !terms.contains(&term){terms.push(term)}}
        if terms.len() == 32 { break }
    }
    terms.join(" OR ")
}

pub fn query(db: &Connection, project: &str, search: Option<&str>, limit: i64, offset:i64) -> Result<Vec<Value>> {
    let words = search.map(terms);
    if words.as_deref() == Some("") {return Ok(vec![])}
    let (join,filter,order) = if words.is_some() {
        ("JOIN note_search ON note_search.rowid=n.id"," AND note_search MATCH ?4","rank,n.id DESC")
    } else {("","","n.id DESC")};
    // Legacy chat.save permits replacing a chat. Never retrieve a note whose source disappeared
    // or whose chat now belongs to a different project.
    let sql = format!("SELECT n.id,n.project,n.chat,n.run,n.role,n.note,n.revision FROM project_notes n {join}
        JOIN objects c ON c.kind='chat' AND c.id=n.chat AND json_extract(c.payload,'$.project_id')=n.project
        WHERE n.project=?1 {filter} AND EXISTS (SELECT 1 FROM json_each(c.payload,'$.messages') m
        WHERE json_extract(m.value,'$.run_id')=n.run AND json_extract(m.value,'$.role')=n.role)
        ORDER BY {order} LIMIT ?2 OFFSET ?3");
    let mut stmt = db.prepare(&sql)?;
    let rows = if let Some(words)=words {stmt.query_map(params![project,limit,offset,words],row)?}
        else {stmt.query_map(params![project,limit,offset],row)?};
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn recall(db:&Connection,project:&str,question:&str) -> Result<Vec<Value>> {
    let found=query(db,project,Some(question),3,0)?;
    if found.is_empty() {
        let question=question.to_lowercase();
        if ["что мы решили","что мы сохранили","что ты помнишь","какое решение мы сохранили","what did we decide",
            "what did we save","what do you remember"].iter().any(|phrase|question.contains(phrase)) {
            return query(db,project,None,3,0);
        }
    }
    Ok(found)
}

pub fn recall_project(db:&Connection,project:&Value,question:&str)->Result<Vec<Value>>{
    let id=field(project,"id")?;
    let mut notes=recall(db,id,question)?;
    let docs=crate::library::search(db,id,std::path::Path::new(field(project,"path")?),question,3)?;
    if !docs.is_empty(){notes.truncate(2);notes.extend(docs.into_iter().take(3-notes.len()));}
    Ok(notes)
}
pub fn dispatch(store: &crate::store::Store, method: &str, p: &Value) -> Result<Value> {
    let project=field(p,"project_id")?;
    store.get("project",project)?;
    let db=&store.db;
    match method {
        "memory.export" => {
            let mut md=String::from("# Project memory\n\n");
            let mut offset=0;
            loop{
                let rows=query(db,project,None,100,offset)?;
                if rows.is_empty(){break}
                for n in &rows{
                    md.push_str(&format!("## {} · r{}\n\n{}\n\n[Source]({})\n\n",n["id"],n["revision"],n["text"].as_str().unwrap_or(""),n["source"].as_str().unwrap_or("")));
                }
                offset+=rows.len() as i64;
                if md.len()>32*1024*1024{bail!("Project memory exceeds 32 MiB")}
            }
            Ok(json!({"project_id":project,"entries":offset,"markdown":md}))
        }
        "memory.list" | "memory.search" => {
            let search=if method=="memory.search" {Some(field(p,"query")?)} else {None};
            Ok(json!(query(db,project,search,p["limit"].as_i64().unwrap_or(50).clamp(1,100),p["offset"].as_i64().unwrap_or(0).max(0))?))
        }
        "memory.add" => {
            let chat_id=field(p,"chat_id")?;
            let run=field(p,"run_id")?;
            crate::safe_id(run)?;
            let role=field(p,"role")?;
            let chat=store.get("chat",chat_id)?;
            if chat["project_id"]!=project || !matches!(role,"user"|"assistant") {bail!("Invalid memory source for this project")}
            let message=chat["messages"].as_array().context("Invalid chat")?.iter()
                .find(|m| m["run_id"]==run && m["role"]==role).context("Source message not found")?;
            let text=note_text(p["text"].as_str().unwrap_or(message["content"].as_str().unwrap_or("")))?;
            db.execute("INSERT INTO project_notes(project,chat,run,role,note) VALUES(?,?,?,?,?)
                ON CONFLICT(project,chat,run,role) DO NOTHING",params![project,chat_id,run,role,text])?;
            let saved=db.query_row("SELECT * FROM project_notes WHERE project=? AND chat=? AND run=? AND role=?",params![project,chat_id,run,role],row)?;
            if saved["text"]!=text {bail!("This message already has a note. Edit the existing revision.")}
            Ok(saved)
        }
        "memory.edit" | "memory.forget" => {
            let id=p["id"].as_i64().context("Missing note ID")?;
            let rev=p["revision"].as_i64().context("Missing note revision")?;
            let changed=if method=="memory.edit" {
                db.execute("UPDATE project_notes SET note=?,revision=revision+1 WHERE project=? AND id=? AND revision=?",params![note_text(field(p,"text")?)?,project,id,rev])?
            } else {db.execute("DELETE FROM project_notes WHERE project=? AND id=? AND revision=?",params![project,id,rev])?};
            if changed!=1 {bail!("Note missing or changed. Reload project memory.")}
            if method=="memory.forget" {return Ok(json!({"forgotten":id}))}
            Ok(db.query_row("SELECT * FROM project_notes WHERE project=? AND id=?",params![project,id],row)?)
        }
        _ => bail!("Unknown memory operation")
    }
}

pub fn apply_context(chat: &Value, model: &mut Value) -> Result<()> {
    if let Some(value)=chat.get("context_tokens").filter(|v| !v.is_null()) {
        let n=value.as_u64().context("Invalid chat context")?;
        if !(512..=131072).contains(&n) || n<=model["max_output_tokens"].as_u64().unwrap_or(512) {
            bail!("Chat context must be 512–131072 tokens and larger than the model's output reserve")
        }
        if model["runtime"]["args"].as_array().is_some_and(|a|a.iter().any(|arg|arg=="{{context}}")) && n>model["context_tokens"].as_u64().unwrap_or(2048) {
            bail!("This llama.cpp server has a smaller context. Increase its model setting and restart the runtime first.")
        }
        model["context_tokens"]=json!(n);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    #[test]
    fn notes_are_versioned_scoped_and_indexed_with_their_source() {
        let mut s=Store::open(std::path::Path::new(":memory:")).unwrap();
        for project in ["a","b"]{s.save("project",json!({"id":project}),None).unwrap();}
        let mut chat=s.save("chat",json!({"id":"chat","project_id":"a","messages":[
            {"role":"user","run_id":"run","content":"Маркировка мозаики Кварц"}]}),None).unwrap();
        let add=json!({"project_id":"a","chat_id":"chat","run_id":"run","role":"user"});
        let first=dispatch(&s,"memory.add",&add).unwrap();
        assert_eq!(first,dispatch(&s,"memory.add",&add).unwrap());
        assert_eq!(first["source"],"#/source/chat/run/user");
        assert_eq!(recall(&s.db,"a","маркировку мозаики?").unwrap(),[first.clone()]);
        assert!(recall(&s.db,"b","мозаики").unwrap().is_empty());
        let mut edit=json!({"project_id":"a","id":first["id"],"revision":1,"text":"Эталон Сапфир"});
        dispatch(&s,"memory.edit",&edit).unwrap();
        assert!(dispatch(&s,"memory.edit",&edit).is_err());
        assert!(recall(&s.db,"a","Кварц").unwrap().is_empty());
        assert_eq!(recall(&s.db,"a","Сапфир").unwrap().len(),1);
        chat["messages"]=json!([]);s.save("chat",chat,Some(1)).unwrap();
        assert!(recall(&s.db,"a","Сапфир").unwrap().is_empty());
        edit["revision"]=json!(2);edit["project_id"]=json!("b");
        assert!(dispatch(&s,"memory.forget",&edit).is_err());
        edit["project_id"]=json!("a");dispatch(&s,"memory.forget",&edit).unwrap();
        assert_eq!(s.db.query_row("SELECT count(*) FROM note_search",[],|r|r.get::<_,i32>(0)).unwrap(),0);
    }
    #[test]
    fn only_the_request_model_receives_the_override() {
        let model=json!({"context_tokens":4096,"max_output_tokens":512});
        let mut request=model.clone();
        apply_context(&json!({"context_tokens":3072}),&mut request).unwrap();
        assert_eq!(request["context_tokens"],3072);
        for invalid in [json!(512),json!("3072"),json!(131073)] {
            assert!(apply_context(&json!({"context_tokens":invalid}),&mut request).is_err());
        }
        let mut auto=model.clone();apply_context(&json!({}),&mut auto).unwrap();
        assert_eq!(auto,model);
    }
}
