use crate::{root, server::App};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
pub fn discover() -> Value {
    let mut paths = vec![];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        paths.push(PathBuf::from(local).join("Programs/Ollama/ollama.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            paths.push(directory.join("ollama.exe"));
        }
    }
    let mut seen = std::collections::HashSet::new();
    json!(paths
        .into_iter()
        .filter(|p| p.is_file() && seen.insert(p.to_string_lossy().to_lowercase()))
        .map(|p| json!({"kind":"ollama","name":"Ollama","command":p}))
        .collect::<Vec<_>>())
}
pub fn validate(model: &Value) -> Result<()> {
    let Some(runtime) = model.get("runtime").filter(|v| !v.is_null()) else {
        return Ok(());
    };
    if !matches!(runtime["kind"].as_str(), Some("ollama" | "command")) {
        bail!("Choose an Ollama or command runtime")
    }
    let command = PathBuf::from(runtime["command"].as_str().unwrap_or(""));
    if !command.is_absolute() || !command.is_file() {
        bail!("Runtime executable must exist")
    }
    if runtime["kind"] == "command"
        && !runtime["args"]
            .as_array()
            .map(|a| a.iter().all(Value::is_string))
            .unwrap_or(false)
    {
        bail!("Runtime args must be a string array")
    }
    if let Some(directory) = runtime["models_directory"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        if !PathBuf::from(directory).is_absolute() {
            bail!("Models directory must be absolute")
        }
    }
    if let Some(environment) = runtime.get("environment") {
        if !environment
            .as_object()
            .map(|m| {
                m.iter().all(|(k, v)| {
                    !k.is_empty()
                        && !k.contains(['=', '\0'])
                        && v.as_str().map(|s| !s.contains('\0')).unwrap_or(false)
                })
            })
            .unwrap_or(false)
        {
            bail!("Runtime environment must contain strings")
        }
    }
    Ok(())
}
pub async fn start(app: Arc<App>, id: &str) -> Result<Value> {
    crate::image_jobs::check_model(&app, id).await?;
    start_runtime(app, id, None, false).await
}
pub(crate) async fn start_owned(
    app: Arc<App>,
    id: &str,
    image_owner: Option<&str>,
) -> Result<Value> {
    start_runtime(app,id,image_owner,true).await
}
async fn start_runtime(app:Arc<App>,id:&str,image_owner:Option<&str>,automatic:bool)->Result<Value>{
    let _serial = app.runtime_lock.lock().await;
    let mut model = app.get("model", id)?;
    validate(&model)?;
    if let Some(instance) = model["instance_id"].as_str() {
        if let Ok(mut record) = app.get("instance", instance) {
            if record["status"] == "running" {
                if !automatic && record["auto_release"]==true {record["auto_release"]=json!(false);return app.save("instance",record)}
                return Ok(record);
            }
        }
    }
    if !model["runtime"].is_object() {
        bail!("This connection uses an externally started runtime")
    }
    let endpoint = reqwest::Url::parse(model["endpoint"].as_str().unwrap_or(""))?;
    let host = endpoint.host_str().unwrap_or("127.0.0.1").trim_matches(['[',']']);
    let port = endpoint.port_or_known_default().unwrap_or(11434);
    if tokio::time::timeout(
        Duration::from_millis(300),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
    {
        bail!("The model endpoint port is already in use. Choose another port or connect to that running server.")
    }
    let instance_id = format!(
        "model-{}-{}",
        id,
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let file = root()
        .join("jobs")
        .join(format!("{instance_id}-runtime.json"));
    std::fs::create_dir_all(file.parent().unwrap())?;
    let mut specification = model.clone();
    specification["root"] = json!(root());
    specification["id"] = json!(instance_id);
    std::fs::write(&file, serde_json::to_vec(&specification)?)?;
    let mut record=crate::server::start_instance(app.clone(),json!({"id":instance_id,"name":format!("Runtime: {}",model["name"].as_str().unwrap_or(id)),"profile_id":model["profile_id"],"command":crate::app_dir().join("node.exe"),"args":[crate::source_dir().join("bridge/runtime.mjs"),file],"image_owner":image_owner})).await?;
    record["auto_release"]=json!(automatic);
    record=app.save("instance",record)?;
    model["instance_id"] = json!(instance_id);
    if let Err(e) = app.save("model", model) {
        let mut workers = app.workers.lock().await;
        if let Some(worker) = workers.remove(&instance_id) {
            let _ = worker.stop();
        }
        let mut record = app.get("instance", &instance_id)?;
        record["status"] = json!("failed");
        record["error"] = json!(e.to_string());
        app.save("instance", record)?;
        return Err(e);
    }
    for _ in 0..50 {
        if tokio::time::timeout(
            Duration::from_millis(200),
            tokio::net::TcpStream::connect((host, port)),
        )
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
        {
            return Ok(record);
        }
        if app
            .workers
            .lock()
            .await
            .get(&instance_id)
            .map(|w| w.exit_code().ok().flatten().is_some())
            .unwrap_or(true)
        {
            bail!("Runtime exited during startup. Open the process log in Resources.")
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    bail!("Runtime started but its API is not ready yet. Check its log in Resources.")
}
pub async fn stop(app: Arc<App>, id: &str) -> Result<Value> {
    crate::image_jobs::check_model(&app, id).await?;
    let _serial = app.runtime_lock.lock().await;
    let model = app.get("model", id)?;
    let tasks = app.list("run")?;
    for record in tasks.as_array().unwrap() {
        if record["model_id"] == id && !matches!(record["kind"].as_str(),Some("image"|"video")) {
            let task = crate::agent::progress(&app, record["id"].as_str().unwrap())?;
            if matches!(task["status"].as_str(), Some("running" | "starting")) {
                crate::agent::cancel(app.clone(), task["id"].as_str().unwrap()).await?;
            }
        }
    }
    let instance = model["instance_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No managed runtime"))?;
    let mut workers = app.workers.lock().await;
    if let Some(worker) = workers.remove(instance) {
        worker.stop()?;
        let mut record = app.get("instance", instance)?;
        record["status"] = json!("stopped");
        app.save("instance", record)?;
    }
    app.get("instance", instance)
}

/// Handoff only: cancel logic belongs to the workflow, not its prompt model.
pub(crate) async fn stop_owned(app: Arc<App>, id: &str) -> Result<Value> {
    let _serial = app.runtime_lock.lock().await;
    let model = app.get("model", id)?;
    let Some(instance) = model["instance_id"].as_str() else {
        return Ok(Value::Null);
    };
    let mut workers = app.workers.lock().await;
    let stopped = if let Some(worker) = workers.remove(instance) {
        worker.stop()?;
        let mut record = app.get("instance", instance)?;
        record["status"] = json!("stopped");
        app.save("instance", record)?;
        true
    } else {
        false
    };
    drop(workers);
    if stopped {
        let endpoint = reqwest::Url::parse(model["endpoint"].as_str().unwrap_or(""))?;
        let host = endpoint.host_str().unwrap_or("127.0.0.1").trim_matches(['[',']']);
        let port = endpoint.port_or_known_default().unwrap_or(11434);
        let mut closed = false;
        for _ in 0..40 {
            if !tokio::time::timeout(
                Duration::from_millis(100),
                tokio::net::TcpStream::connect((host, port)),
            )
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false)
            {
                closed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if !closed {
            bail!("Stopped runtime port has not been released yet")
        }
    }
    Ok(app.get("instance", instance).unwrap_or(Value::Null))
}
