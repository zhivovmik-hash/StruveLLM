//! Opt-in latent semantic indexing of selected project sources. A sparse TF-IDF matrix
//! stays on CPU. Truncated SVD maps queries and fragments into the same topic space.
use crate::server::App;
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

const MAX_CHUNKS: usize = 1200;
const MAX_TERMS: usize = 6000;
type SparseRow = Vec<(usize, f64)>;

pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS semantic_indexes(
        project TEXT PRIMARY KEY, signature TEXT NOT NULL, encoded BLOB NOT NULL, fragments INTEGER NOT NULL, dimensions INTEGER NOT NULL);")?;
    Ok(())
}

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric()).filter_map(|s| {
        let n = s.chars().count();
        if !(2..=64).contains(&n) {
            return None;
        }
        let word = s.to_lowercase();
        if matches!(
            word.as_str(),
            "есть"
                | "для"
                | "или"
                | "его"
                | "она"
                | "это"
                | "как"
                | "при"
                | "под"
                | "что"
                | "том"
                | "and"
                | "the"
                | "with"
                | "not"
                | "from"
                | "for"
                | "this"
                | "на"
                | "по"
                | "он"
                | "от"
                | "до"
                | "то"
                | "in"
                | "is"
                | "of"
                | "to"
                | "мы"
                | "вы"
        ) {
            None
        } else {
            Some(word)
        }
    })
}
fn norm(vector: &[f64]) -> f64 {
    vector.iter().map(|x| x * x).sum::<f64>().sqrt()
}
fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
fn weighted_terms(matrix: &[SparseRow], vector: &[f64], terms: usize) -> Vec<f64> {
    let mut result = vec![0.; terms];
    for (row, &weight) in matrix.iter().zip(vector) {
        for &(i, x) in row {
            result[i] += x * weight;
        }
    }
    result
}
#[derive(Serialize, Deserialize)]
struct Index {
    terms: BTreeMap<String, (f64, Vec<f64>)>,
    documents: Vec<(i64, Vec<f64>)>,
}
fn build(input: &[(i64, String)]) -> Result<Index> {
    if !(3..=MAX_CHUNKS).contains(&input.len()) {
        bail!("Semantic search needs 3–{MAX_CHUNKS} indexed fragments")
    }
    let counts: Vec<_> = input
        .iter()
        .map(|(_, text)| {
            let mut m = BTreeMap::new();
            for word in words(text) {
                *m.entry(word).or_insert(0u32) += 1;
            }
            m
        })
        .collect();
    let mut frequencies = BTreeMap::new();
    for row in &counts {
        for word in row.keys() {
            *frequencies.entry(word.clone()).or_insert(0usize) += 1;
        }
    }
    let mut vocab: Vec<_> = frequencies
        .into_iter()
        .filter(|(_, df)| *df < input.len())
        .collect();
    vocab.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    vocab.truncate(MAX_TERMS);
    let entries: HashMap<_, _> = vocab
        .iter()
        .enumerate()
        .map(|(i, (word, _))| (word, i))
        .collect();
    let idf: Vec<_> = vocab
        .iter()
        .map(|(_, df)| ((input.len() + 1) as f64 / (*df + 1) as f64).ln() + 1.)
        .collect();
    let matrix: Vec<SparseRow> = counts
        .iter()
        .map(|row| {
            let mut v: SparseRow = row
                .iter()
                .filter_map(|(w, tf)| {
                    entries
                        .get(w)
                        .map(|&i| (i, (1. + (*tf as f64).ln()) * idf[i]))
                })
                .collect();
            let len = v.iter().map(|(_, x)| x * x).sum::<f64>().sqrt().max(1e-12);
            for (_, x) in &mut v {
                *x /= len;
            }
            v
        })
        .collect();
    let dimensions = ((input.len() as f64).sqrt() as usize)
        .clamp(2, 8)
        .min(vocab.len())
        .min(input.len() - 1);
    let mut basis: Vec<Vec<f64>> = Vec::new();
    let mut terms: Vec<Vec<f64>> = vec![Vec::new(); vocab.len()];
    let mut documents = vec![Vec::new(); input.len()];
    for axis in 0..dimensions {
        let mut seed = 271828u64 + axis as u64;
        let mut v: Vec<_> = (0..input.len())
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((seed >> 32) as f64 / u32::MAX as f64) - 0.5
            })
            .collect();
        let mut valid = true;
        for _ in 0..22 {
            let at = weighted_terms(&matrix, &v, vocab.len());
            let mut next: Vec<_> = matrix
                .iter()
                .map(|row| row.iter().map(|(i, x)| x * at[*i]).sum::<f64>())
                .collect();
            for old in &basis {
                let a = dot(&next, old);
                for (x, y) in next.iter_mut().zip(old) {
                    *x -= a * y;
                }
            }
            let len = norm(&next);
            if len < 1e-8 {
                valid = false;
                break;
            }
            for x in &mut next {
                *x /= len;
            }
            let change = 1. - dot(&v, &next).abs();
            v = next;
            if change.abs() < 1e-6 {
                break;
            }
        }
        if !valid {
            break;
        }
        let at = weighted_terms(&matrix, &v, vocab.len());
        let sigma = norm(&at);
        if sigma < 1e-8 {
            break;
        }
        for (row, x) in terms.iter_mut().zip(at) {
            row.push(x / sigma);
        }
        for (row, &x) in documents.iter_mut().zip(&v) {
            row.push(x * sigma);
        }
        basis.push(v);
    }
    if basis.is_empty() {
        bail!("The selected fragments have no distinct terms")
    }
    Ok(Index {
        terms: vocab
            .into_iter()
            .zip(idf)
            .zip(terms)
            .map(|(((w, _), f), v)| (w, (f, v)))
            .collect(),
        documents: input.iter().map(|(id, _)| *id).zip(documents).collect(),
    })
}
impl Index {
    fn rank(&self, query: &str) -> Vec<(i64, f64)> {
        let dims = self.documents.first().map(|(_, v)| v.len()).unwrap_or(0);
        let mut q = vec![0.; dims];
        for w in words(query) {
            if let Some((idf, v)) = self.terms.get(&w) {
                for (x, y) in q.iter_mut().zip(v) {
                    *x += idf * y;
                }
            }
        }
        let len = norm(&q);
        if len < 1e-8 {
            return vec![];
        }
        let mut scores: Vec<_> = self
            .documents
            .iter()
            .filter_map(|(id, v)| {
                let d = norm(v);
                let s = dot(&q, v) / (len * d);
                (d > 1e-8 && s > 0.40).then_some((*id, s))
            })
            .collect();
        scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scores
    }
}
fn signature(db: &Connection, project: &str) -> Result<String> {
    Ok(db.query_row("SELECT COALESCE(group_concat(item,';'),'') FROM (SELECT id||':'||kind||':'||digest AS item FROM library_files WHERE project=? ORDER BY id)",[project],|r|r.get(0))?)
}
pub fn configure(app: &App, project: &str, enable: bool) -> Result<Value> {
    if !enable {
        app.store
            .lock()
            .unwrap()
            .db
            .execute("DELETE FROM semantic_indexes WHERE project=?", [project])?;
        return Ok(json!({"enabled":false}));
    }
    let (sig, rows) = {
        let store = app.store.lock().unwrap();
        let db = &store.db;
        let mut stmt=db.prepare("SELECT c.id,c.label||' '||substr(c.text,1,3200) FROM library_chunks c JOIN library_files f ON f.id=c.doc WHERE f.project=? ORDER BY c.id LIMIT ?")?;
        let rows = stmt
            .query_map(params![project, (MAX_CHUNKS + 1) as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        (signature(db, project)?, rows)
    };
    let index = build(&rows)?;
    let dimensions = index.documents[0].1.len();
    let encoded = serde_json::to_vec(&index)?;
    let store = app.store.lock().unwrap();
    if signature(&store.db, project)? != sig {
        bail!("Sources were changed. Rebuild semantic search.")
    }
    store.db.execute("INSERT INTO semantic_indexes VALUES(?,?,?,?,?) ON CONFLICT(project) DO UPDATE SET
        signature=excluded.signature,encoded=excluded.encoded,fragments=excluded.fragments,dimensions=excluded.dimensions",
        params![project,sig,encoded,rows.len() as i64,dimensions as i64])?;
    Ok(json!({"enabled":true,"fragments":rows.len(),"dimensions":dimensions,"current":true}))
}
pub fn status(db: &Connection, project: &str) -> Result<Value> {
    let row = db
        .query_row(
            "SELECT signature,fragments,dimensions FROM semantic_indexes WHERE project=?",
            [project],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((sig, n, k)) = row else {
        return Ok(json!({"enabled":false}));
    };
    Ok(json!({"enabled":true,"fragments":n,"dimensions":k,"current":sig==signature(db,project)?}))
}
pub fn ranked(db: &Connection, project: &str, query: &str) -> Result<Vec<(i64, f64)>> {
    if status(db, project)?["current"] != true {
        return Ok(vec![]);
    }
    let bytes: Vec<u8> = db.query_row(
        "SELECT encoded FROM semantic_indexes WHERE project=?",
        [project],
        |r| r.get(0),
    )?;
    let index: Index = serde_json::from_slice(&bytes)?;
    Ok(index.rank(query))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn topics_retrieve_related_fragments_without_query_overlap() {
        let texts = [
            "telescope sky astronomy",
            "telescope eyepiece observer",
            "eyepiece lens observer",
            "garden soil tree",
            "garden compost soil",
            "compost tree leaf",
        ];
        let data: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(i, s)| (i as i64, s.to_string()))
            .collect();
        let ranks = build(&data).unwrap().rank("astronomy");
        assert!(ranks.iter().any(|(id, _)| *id == 2));
        assert!(!ranks.iter().any(|(id, _)| *id >= 3));
    }
}
