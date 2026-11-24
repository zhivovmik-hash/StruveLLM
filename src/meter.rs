use crate::server::App;
use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS resource_observations(
        run TEXT NOT NULL,stage TEXT NOT NULL,model TEXT NOT NULL,updated TEXT NOT NULL,
        payload TEXT NOT NULL,PRIMARY KEY(run,stage));
        CREATE INDEX IF NOT EXISTS observations_model ON resource_observations(model,updated DESC);")?;
    Ok(())
}
pub fn list(
    db: &Connection,
    model: Option<&str>,
    run: Option<&str>,
    limit: i64,
) -> Result<Vec<Value>> {
    let mut stmt = db.prepare(
        "SELECT o.payload,json_extract(r.payload,'$.status'),json_extract(r.payload,'$.kind') FROM resource_observations o
        LEFT JOIN objects r ON r.kind='run' AND r.id=o.run WHERE (?1 IS NULL OR o.model=?1)
        AND (?2 IS NULL OR o.run=?2) ORDER BY o.updated DESC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![model, run, limit.clamp(1, 500)], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?))
    })?;
    rows.map(|row| {
        let (s, status, kind) = row?;
        let mut v: Value = serde_json::from_str(&s)?;
        v["run_status"] = json!(status);
        if kind.as_deref() == Some("image") && v["stage"] == "render" {
            if let Some(settings) = v["settings"].as_object_mut() {
                settings.remove("video_frames");
            }
        }
        Ok(v)
    })
    .collect()
}
fn maximum(record: &mut Value, key: &str, n: Option<u64>) {
    if let Some(n) = n {
        record[key] = json!(record[key].as_u64().unwrap_or(0).max(n));
    }
}
fn file_identity(value: &Value) -> Value {
    let Some(path) = value.as_str() else {
        return Value::Null;
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return Value::Null;
    };
    json!({"name":std::path::Path::new(path).file_name().and_then(|s|s.to_str()),"bytes":meta.len(),
        "modified":meta.modified().ok().and_then(|t|t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d|d.as_secs())})
}
fn ollama_digest(model: &Value) -> Option<String> {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(model["runtime"]["models_directory"].as_str()?);
    let name = model["engine_model"].as_str()?;
    let (repository, tag) = name.rsplit_once(':').unwrap_or((name, "latest"));
    let parts = repository.split('/').collect::<Vec<_>>();
    let (host, scope, repo) = match parts.as_slice() {
        [repo] => ("registry.ollama.ai", "library", *repo),
        [scope, repo] => ("registry.ollama.ai", *scope, *repo),
        [host, scope, repo] => (*host, *scope, *repo),
        _ => return None,
    };
    if [host, scope, repo, tag].iter().any(|s| {
        s.is_empty()
            || *s == "."
            || *s == ".."
            || !s
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
    }) {
        return None;
    }
    let file = root
        .join("manifests")
        .join(host)
        .join(scope)
        .join(repo)
        .join(tag);
    if file.metadata().ok()?.len() > 1024 * 1024 {
        return None;
    }
    Some(format!("{:x}", Sha256::digest(std::fs::read(file).ok()?)))
}
const PLACEMENT_FLAGS: &[&str] = &[
    "flash_attention",
    "text_encoder_cpu",
    "vae_cpu",
    "diffusion_cpu",
    "vae_tiling",
    "h3_attention_chunk_tokens",
    "h3_mixed_precision",
    "h3_host_cache",
    "h3_first_last_frame",
    "clip_on_cpu",
    "batch_size",
    "ubatch_size",
];
fn placement_settings(model: &Value, media_kind: Option<&str>) -> Value {
    let mut settings = json!({});
    for key in [
        "engine_model",
        "protocol",
        "architecture",
        "backend",
        "context_tokens",
        "gpu_layers",
        "width",
        "height",
        "steps",
        "h3_linear_chunk_tokens",
        "offload_to_cpu",
        "vae_tiling",
        "vram_gib",
        "applied_extensions",
        "flash_attn",
    ] {
        if let Some(value) = model.get(key) {
            settings[key] = value.clone()
        }
    }
    if media_kind == Some("video") {
        if let Some(frames) = model.get("video_frames") {
            settings["video_frames"] = frames.clone();
        }
    }
    for &key in PLACEMENT_FLAGS {
        if let Some(value) = model.get(key) {
            settings[key] = value.clone();
        }
    }
    if model["runtime"]["kind"] == "ollama" {
        settings["digest"] = json!(ollama_digest(model));
    }
    let gguf = model["runtime"]["args"]
        .as_array()
        .and_then(|args| args.windows(2).find(|a| a[0] == "-m"))
        .map(|a| &a[1]);
    settings["weights"] = file_identity(gguf.unwrap_or(&model["model_path"]));
    settings["engine"] = file_identity(if media_kind.is_some() {
        &model["command"]
    } else {
        &model["runtime"]["command"]
    });
    settings
}

fn comparable(mut value: Value) -> Value {
    for key in ["quantization"] {
        if let Some(obj) = value.as_object_mut() {
            obj.remove(key);
        }
    }
    for key in ["engine", "weights"] {
        // Older controllers serialized Windows filenames as UTF-16 arrays.
        if let Some(obj) = value[key].as_object_mut() {
            obj.remove("name");
        }
    }
    if value["applied_extensions"]
        .as_array()
        .is_some_and(Vec::is_empty)
    {
        value.as_object_mut().unwrap().remove("applied_extensions");
    }
    value
}
fn budget_with_headroom(peak: Option<u64>, ceiling: Option<f64>) -> (Option<f64>, Option<f64>) {
    let target = peak.map(|bytes| ((bytes as f64 / 1073741824. * 1.25 + 0.5) * 4.).ceil() / 4.);
    (target, target.filter(|&n| ceiling.is_some_and(|limit| n <= limit)))
}

#[test]
fn frame_counts_only_describe_video_renders_in_current_and_older_history() {
    let defaults = json!({"video_frames":24,"flash_attention":false});
    assert!(placement_settings(&defaults,Some("image")).get("video_frames").is_none());
    let video = placement_settings(&defaults,Some("video"));
    assert_eq!(video["video_frames"],24);
    assert_eq!(video["flash_attention"],false);
    let db = Connection::open_in_memory().unwrap();
    initialize(&db).unwrap();
    db.execute_batch("CREATE TABLE objects(kind TEXT,id TEXT,payload TEXT)").unwrap();
    for (id,kind) in [("still","image"),("clip","video")] {
        db.execute("INSERT INTO objects VALUES('run',?,?)",params![id,json!({"kind":kind,"status":"completed"}).to_string()]).unwrap();
        db.execute("INSERT INTO resource_observations VALUES(?,'render','demo','now',?)",
            params![id,json!({"stage":"render","settings":defaults}).to_string()]).unwrap();
        let row=&list(&db,None,Some(id),1).unwrap()[0];
        assert_eq!(row["settings"].get("video_frames").is_some(),kind=="video");
    }
}

#[test]
fn headroom_recommendations_respect_physical_capacity() {
    assert_eq!(budget_with_headroom(Some(6456168448), Some(8.)), (Some(8.25), None));
    assert_eq!(budget_with_headroom(Some(6456168448), Some(24.)), (Some(8.25), Some(8.25)));
    assert_eq!(budget_with_headroom(Some(6456168448), Some(8.25)), (Some(8.25), Some(8.25)));
    assert_eq!(budget_with_headroom(None, Some(8.)), (None, None));
    assert_eq!(budget_with_headroom(Some(6456168448), None), (Some(8.25), None));
}

pub async fn estimate(app: &App, p: &Value) -> Result<Value> {
    let id = p["model_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Select a model"))?;
    let (mut model, media) = match app.get("model", id) {
        Ok(m) => (m, false),
        Err(_) => (app.get("media_model", id)?, true),
    };
    if let Some(n) = p["context_tokens"].as_u64() {
        crate::project_memory::apply_context(&json!({"context_tokens":n}), &mut model)?;
    }
    let profile = if media {
        crate::media_catalog::profile(&model)
    } else {
        app.get("profile", model["profile_id"].as_str().unwrap_or(""))?
    };
    let hw = app.inventory().await;
    let identity = comparable(placement_settings(&model, if media {model["kind"].as_str()} else {None}));
    let history = list(&app.store.lock().unwrap().db, Some(id), None, 500)?;
    let mut applicable = Vec::new();
    for row in &history {
        let devices: Vec<_> = row["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|g| g["id"].clone())
            .collect();
        let drivers_agree = row["devices"].as_array().into_iter().flatten().all(|d| {
            hw["gpus"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|g| g["id"] == d["id"] && g["driver"] == d["driver"])
        });
        let mut recorded = row["settings"].clone();
        if media {
            if let Ok(run) = app.get("run", row["run_id"].as_str().unwrap_or("")) {
                let config = run.get("effective_media_config").filter(|v| v.is_object())
                    .unwrap_or(&run["execution"]["image_config"]);
                for &key in PLACEMENT_FLAGS {
                    if recorded.get(key).is_none() {
                        if let Some(value) = config.get(key) {
                            recorded[key] = value.clone();
                        }
                    }
                }
            }
        }
        if row["run_status"] == "completed"
            && row["configuration_id"] == hw["configuration_id"]
            && json!(devices) == profile["gpu_ids"]
            && drivers_agree
            && comparable(recorded) == identity
            && row["samples"].as_u64().unwrap_or(0) >= 2
        {
            applicable.push(row);
        }
    }
    let max = |key: &str| applicable.iter().filter_map(|r| r[key].as_u64()).max();
    // Job Object RAM limits apply to commit. A working-set measurement alone cannot set this cap.
    let commit = max("committed_peak_bytes");
    let vram = max("dedicated_peak_bytes");
    let settings = crate::hardware::effective_settings(&app.get("settings", "system")?, &hw);
    let ram_capacity = hw["ram_total_gib"].as_f64().map(|total|
        (total - settings["ram_reserve_gib"].as_f64().unwrap_or(0.)).max(0.));
    let single_gpu = profile["gpu_ids"].as_array().filter(|ids| ids.len() == 1);
    let device_capacity = single_gpu.and_then(|ids| hw["gpus"].as_array()?.iter()
        .find(|g| g["id"] == ids[0])?["vram_gib"].as_f64());
    let (ram_target, ram_suggestion) = budget_with_headroom(commit, ram_capacity);
    let (vram_target, vram_suggestion) = budget_with_headroom(vram.filter(|_|single_gpu.is_some()), device_capacity);
    Ok(
        json!({"model_id":id,"matching_runs":applicable.len(),"prior_runs":history.len(),"profile":profile,
        "configuration_id":hw["configuration_id"],"settings":identity,"last_matching_run":applicable.first().map(|r|&r["run_id"]),
        "measured_resident_bytes":max("resident_peak_bytes"),"measured_commit_bytes":commit,"measured_dedicated_bytes":vram,
        "suggested_ram_gib":ram_suggestion,"suggested_vram_gib":vram_suggestion,
        "ram_with_headroom_gib":ram_target,"vram_with_headroom_gib":vram_target,
        "usable_ram_capacity_gib":ram_capacity,"device_vram_capacity_gib":device_capacity,
        "margin_fraction":0.25,"extra_gib":0.5}),
    )
}
pub fn start(
    app: Arc<App>,
    run: &Value,
    stage: &'static str,
    worker: Option<String>,
    model: Value,
    profile: Value,
) {
    let external = worker.is_none();
    if external && model["protocol"] != "ollama" {
        return;
    }
    let run_id = run["id"].as_str().unwrap_or("").to_owned();
    let settings = placement_settings(&model, if stage == "render" {run["kind"].as_str()} else {None});
    let selected = if external {
        json!([])
    } else {
        profile["gpu_ids"].clone()
    };
    let owner = model["id"].as_str().unwrap_or("default").to_string();
    tokio::spawn(async move {
        let hardware = app.inventory().await;
        let devices: Vec<_> = hardware["gpus"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|g| selected.as_array().is_some_and(|a| a.contains(&g["id"])))
            .map(|g| json!({"id":g["id"],"name":g["name"],"driver":g["driver"]}))
            .collect();
        let mut row = json!({"run_id":run_id,"stage":stage,"model_id":owner,"name":model["name"],"settings":settings,
            "scope":if external{"external_engine"}else{"managed_process_tree"},"devices":devices,"configuration_id":hardware["configuration_id"],"profile_id":profile["id"],
            "ram_budget_gib":profile["ram_gib"],"vram_budget_gib":profile["vram_gib"],"started_at":chrono::Utc::now().to_rfc3339(),
            "samples":0,"resident_peak_bytes":null,"committed_peak_bytes":null,"dedicated_peak_bytes":null,"ollama_vram_bytes":null});
        if external {
            row["devices"] = json!([]);
            row["ram_budget_gib"] = Value::Null;
            row["vram_budget_gib"] = Value::Null;
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(800))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .ok();
        let engine_url = model["endpoint"]
            .as_str()
            .and_then(|s| reqwest::Url::parse(s).ok())
            .filter(|u| {
                u.scheme() == "http"
                    && matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
            })
            .and_then(|u| u.join("/api/ps").ok());
        let mut samples = 0u64;
        loop {
            if stage == "chat" && !app.workers.lock().await.contains_key(&run_id) {
                break;
            }
            let usage = if let Some(id) = &worker {
                let Some(usage) = app.workers.lock().await.get(id).map(|w| w.usage()) else {
                    break;
                };
                usage
            } else {
                Value::Null
            };
            // Reused planner runtimes may survive this phase. Do not count subsequent work.
            let Ok(task) = app.get("run", &run_id) else {
                break;
            };
            if !matches!(task["status"].as_str(), Some("running" | "starting"))
                || (stage == "planner" && task["phase"] != "planning")
            {
                break;
            }
            let pids: Vec<_> = usage["process_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .collect();
            let wddm_pids = pids.clone();
            let wddm =
                tokio::task::spawn_blocking(move || crate::gpu_memory::dedicated(&wddm_pids))
                    .await
                    .ok()
                    .flatten();
            let mut dedicated = 0u64;
            let mut measured = false;
            let mut counted = std::collections::BTreeSet::new();
            if let Some(gpus) = wddm {
                measured = true;
                row["gpu_counter"] = json!("WDDM");
                for (adapter, bytes) in gpus {
                    dedicated += bytes;
                    let luid = crate::gpu_memory::display_id(&adapter);
                    if let Some(device) = hardware["gpus"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|g| luid.as_deref().is_some_and(|id| g["display_adapter_id"] == id))
                    {
                        if let Some(id) = device["id"].as_str() {
                            counted.insert(id.to_owned());
                            maximum(&mut row["dedicated_by_gpu"], id, Some(bytes));
                        }
                    }
                    maximum(&mut row["dedicated_by_adapter"], &adapter, Some(bytes));
                }
            }
            if selected
                .as_array()
                .into_iter()
                .flatten()
                .any(|g| g.as_str().is_some_and(|id| !counted.contains(id)))
            {
                if let Some(driver) = crate::gpu_memory::nvidia_processes(&pids).await {
                    for (id, bytes) in driver {
                        if !counted.contains(&id)
                            && (!measured
                                || hardware["gpus"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .any(|g| g["id"] == id.as_str() && g["driver_model"] == "TCC"))
                        {
                            dedicated += bytes;
                            maximum(&mut row["dedicated_by_gpu"], &id, Some(bytes));
                            row["gpu_counter"] = json!(if counted.is_empty() {
                                "NVIDIA per-process"
                            } else {
                                "WDDM + NVIDIA per-process"
                            });
                            measured = true;
                        }
                    }
                }
            }
            if measured {
                maximum(&mut row, "dedicated_peak_bytes", Some(dedicated));
            }
            maximum(
                &mut row,
                "resident_peak_bytes",
                usage["resident_bytes"].as_u64(),
            );
            maximum(
                &mut row,
                "committed_peak_bytes",
                usage["committed_bytes"].as_u64(),
            );
            let mut observed_engine = false;
            if model["protocol"] == "ollama" {
                if let (Some(c), Some(url)) = (&client, &engine_url) {
                    if let Ok(r) = c.get(url.clone()).send().await {
                        if let Ok(v) = r.json::<Value>().await {
                            if let Some(m) =
                                v["models"].as_array().into_iter().flatten().find(|m| {
                                    m["name"] == model["engine_model"]
                                        && (m["context_length"].is_null()
                                            || m["context_length"] == model["context_tokens"])
                                })
                            {
                                observed_engine = m["size_vram"].as_u64().is_some();
                                maximum(&mut row, "ollama_vram_bytes", m["size_vram"].as_u64());
                                row["settings"]["digest"] = m["digest"].clone();
                                row["settings"]["quantization"] =
                                    m["details"]["quantization_level"].clone();
                            }
                        }
                    }
                }
            }
            if external && !observed_engine {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            samples += 1;
            row["samples"] = json!(samples);
            row["updated_at"] = json!(chrono::Utc::now().to_rfc3339());
            let saved=app.store.lock().unwrap().db.execute("INSERT INTO resource_observations(run,stage,model,updated,payload)
                VALUES(?,?,?,?,?) ON CONFLICT(run,stage) DO UPDATE SET updated=excluded.updated,payload=excluded.payload",
                params![run_id,stage,owner,row["updated_at"].as_str(),row.to_string()]);
            if let Err(e) = saved {
                eprintln!("Resource measurement: {e}");
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}
