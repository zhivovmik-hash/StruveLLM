use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;

pub struct Store {
    pub db: Connection,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS objects(kind TEXT NOT NULL,id TEXT NOT NULL,revision INTEGER NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(kind,id)); CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT,at TEXT DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')),action TEXT NOT NULL,object_id TEXT NOT NULL,detail TEXT NOT NULL);")?;
        db.execute_batch("CREATE INDEX IF NOT EXISTS run_admission ON objects(json_extract(payload,'$.status'),json_extract(payload,'$.model_id'),json_extract(payload,'$.created_at')) WHERE kind='run';")?;
        crate::project_memory::initialize(&db)?;
        crate::meter::initialize(&db)?;
        crate::library::initialize(&db)?;
        Ok(Self { db })
    }
    pub fn list(&self, kind: &str) -> Result<Vec<Value>> {
        let mut q = self
            .db
            .prepare("SELECT payload,revision FROM objects WHERE kind=? ORDER BY id")?;
        let rows = q.query_map([kind], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        rows.map(|r| {
            let (text, rev) = r?;
            let mut v: Value = serde_json::from_str(&text)?;
            v["revision"] = json!(rev);
            Ok(v)
        })
        .collect()
    }
    pub fn get(&self, kind: &str, id: &str) -> Result<Value> {
        let (payload, revision): (String, i64) = self
            .db
            .query_row(
                "SELECT payload,revision FROM objects WHERE kind=? AND id=?",
                params![kind, id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("Объект не найден: {kind}/{id}"))?;
        let mut value: Value = serde_json::from_str(&payload)?;
        value["revision"] = json!(revision);
        Ok(value)
    }
    pub fn busy(&self)->Result<bool>{
        Ok(self.db.query_row("SELECT EXISTS(SELECT 1 FROM objects WHERE kind='run' AND json_extract(payload,'$.status') IN ('starting','running','queued'))",[],|r|r.get(0))?)
    }
    pub fn live_runs(&self,model:Option<&str>)->Result<Vec<Value>>{
        let mut stmt=self.db.prepare("SELECT id,json_extract(payload,'$.model_id'),json_extract(payload,'$.chat_id')
            FROM objects WHERE kind='run' AND json_extract(payload,'$.status') IN ('starting','running') AND (?1 IS NULL OR json_extract(payload,'$.model_id')=?1)")?;
        let rows=stmt.query_map([model],|r|Ok(json!({"id":r.get::<_,String>(0)?,"model_id":r.get::<_,Option<String>>(1)?,"chat_id":r.get::<_,Option<String>>(2)?})))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn queued_media(&self) -> Result<Vec<String>> {
        let mut stmt=self.db.prepare(
            "SELECT id FROM objects WHERE kind='run' AND json_extract(payload,'$.status')='queued'
             AND json_extract(payload,'$.kind') IN ('image','video','text')
             ORDER BY json_extract(payload,'$.created_at'),rowid")?;
        let rows=stmt.query_map([],|row|row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn save(&mut self, kind: &str, mut value: Value, expected: Option<i64>) -> Result<Value> {
        let id = value["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Требуется id"))?
            .to_string();
        crate::safe_id(&id)?;
        let tx = self.db.savepoint()?;
        let old: Option<i64> = tx
            .query_row(
                "SELECT revision FROM objects WHERE kind=? AND id=?",
                params![kind, id],
                |r| r.get(0),
            )
            .optional()?;
        if old.is_some() && expected.is_none() {
            bail!("Для обновления требуется revision")
        }
        if let Some(rev) = expected {
            if old.unwrap_or(0) != rev {
                bail!("Конфликт ревизии: перечитайте объект")
            }
        }
        let revision = old.unwrap_or(0) + 1;
        value["revision"] = json!(revision);
        tx.execute("INSERT INTO objects(kind,id,revision,payload) VALUES(?,?,?,?) ON CONFLICT(kind,id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload",params![kind,id,revision,value.to_string()])?;
        tx.execute(
            "INSERT INTO events(action,object_id,detail) VALUES(?,?,?)",
            params![
                format!("{kind}.save"),
                id,
                json!({"revision":revision}).to_string()
            ],
        )?;
        tx.commit()?;
        Ok(value)
    }
    pub fn events(&self, after: i64, limit: i64) -> Result<Vec<Value>> {
        let mut q = self.db.prepare(
            "SELECT seq,at,action,object_id,detail FROM events WHERE seq>? ORDER BY seq LIMIT ?",
        )?;
        let rows=q.query_map(params![after,limit.clamp(1,500)],|r|Ok(json!({"seq":r.get::<_,i64>(0)?,"at":r.get::<_,String>(1)?,"action":r.get::<_,String>(2)?,"object_id":r.get::<_,String>(3)?,"detail":serde_json::from_str::<Value>(&r.get::<_,String>(4)?).unwrap_or(Value::Null)})))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn recent_events(&self, limit: i64) -> Result<Vec<Value>> {
        let mut query = self.db.prepare(
            "SELECT seq,at,action,object_id,detail FROM events ORDER BY seq DESC LIMIT ?",
        )?;
        let rows = query.query_map([limit.clamp(1,500)], |r| Ok(json!({"seq":r.get::<_,i64>(0)?,"at":r.get::<_,String>(1)?,"action":r.get::<_,String>(2)?,"object_id":r.get::<_,String>(3)?,"detail":serde_json::from_str::<Value>(&r.get::<_,String>(4)?).unwrap_or(Value::Null)})))?;
        let mut values = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        values.reverse();
        Ok(values)
    }
    pub fn save_batch(&mut self, values: Vec<(&str, Value)>) -> Result<Vec<Value>> {
        self.db.execute_batch("SAVEPOINT object_batch")?;
        let result = values
            .into_iter()
            .map(|(kind, value)| {
                let revision = value["revision"].as_i64();
                self.save(kind, value, revision)
            })
            .collect::<Result<Vec<_>>>();
        match result {
            Ok(values) => {
                self.db.execute_batch("RELEASE object_batch")?;
                Ok(values)
            }
            Err(error) => {
                self.db
                    .execute_batch("ROLLBACK TO object_batch; RELEASE object_batch")?;
                Err(error)
            }
        }
    }
    pub fn event(&self, action: &str, id: &str, detail: Value) -> Result<()> {
        self.db.execute(
            "INSERT INTO events(action,object_id,detail) VALUES(?,?,?)",
            params![action, id, detail.to_string()],
        )?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recent_journal_moves_forward_without_losing_cursor_history() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        for number in 1..=130 {
            s.event("fixture", &number.to_string(), json!({"number":number}))
                .unwrap();
        }
        let recent = s.recent_events(100).unwrap();
        assert_eq!(recent.len(), 100);
        assert_eq!(recent.first().unwrap()["seq"], 31);
        assert_eq!(recent.last().unwrap()["seq"], 130);
        let original = s.events(0, 100).unwrap();
        assert_eq!(original.first().unwrap()["seq"], 1);
        assert_eq!(original.last().unwrap()["seq"], 100);
        assert_eq!(s.events(100, 100).unwrap().len(), 30);
    }
    #[test]
    fn revisions_are_atomic_and_old_payload_survives_conflict() {
        let mut s = Store::open(Path::new(":memory:")).unwrap();
        s.save("profile", json!({"id":"x","ram_gib":8}), None)
            .unwrap();
        assert!(s
            .save("profile", json!({"id":"x","ram_gib":99}), Some(0))
            .is_err());
        assert_eq!(s.get("profile", "x").unwrap()["ram_gib"], 8);
        assert_eq!(s.events(0, 100).unwrap().len(), 1);
    }
    #[test]
    fn chat_and_run_commit_or_rollback_together() {
        let mut s = Store::open(Path::new(":memory:")).unwrap();
        let chat = s.save("chat", json!({"id":"chat"}), None).unwrap();
        s.save("run", json!({"id":"task","status":"running"}), None)
            .unwrap();
        let mut updated = chat.clone();
        updated["answer"] = json!("complete");
        assert!(s
            .save_batch(vec![
                ("chat", updated),
                (
                    "run",
                    json!({"id":"task","revision":0,"status":"completed"})
                )
            ])
            .is_err());
        assert_eq!(s.get("chat", "chat").unwrap(), chat);
        assert_eq!(s.events(0, 100).unwrap().len(), 2);
    }
}
