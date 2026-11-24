use crate::{root, server::App, source_dir};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{io, path::Path, sync::Arc, time::Duration};
use tokio::time::{sleep, timeout};

fn file_hash(path: &str) -> Result<String> {
    let mut source = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    io::copy(&mut source, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

async fn wait_for_worker(app: &App, id: &str) -> Result<()> {
    loop {
        app.reap().await?;
        let worker = app.get("instance", id)?;
        match worker["status"].as_str() {
            Some("completed") => return Ok(()),
            Some("starting" | "running") => sleep(Duration::from_millis(300)).await,
            Some("stopped") => bail!("Exposure correction was stopped"),
            _ => bail!("Exposure correction failed. See Resources for the {id} process log."),
        }
    }
}

pub(crate) async fn level(app: Arc<App>, run_id: String, profile_id: String) -> Result<Value> {
    crate::safe_id(&run_id)?;
    let _grading = app
        .exposure_lock
        .try_lock()
        .context("An exposure correction is already running")?;
    let run = app.get("run", &run_id)?;
    if run["kind"] != "video" || run["status"] != "completed" {
        bail!("Choose a completed video in the chat")
    }
    let artifact = run["artifacts"]
        .as_array()
        .and_then(|files| files.iter().find(|file| file["kind"] == "video"))
        .context("This run has no generated video")?;
    let source = app.get(
        "attachment",
        artifact["id"].as_str().context("Missing video ID")?,
    )?;
    let source_path = source["path"].as_str().context("Missing video file")?;
    let saved = &run["exposure_copy"];
    if let Some(path) = saved["attachment"]["path"]
        .as_str()
        .filter(|p| Path::new(p).is_file())
    {
        if saved["metrics"]["source_sha256"] == file_hash(source_path)?
            && saved["metrics"]["output_sha256"] == file_hash(path)?
        {
            return Ok(saved.clone());
        }
        bail!("The source or saved exposure copy has changed on disk")
    }
    let executable = root().join("runtimes/image-edit-python/python.exe");
    let script = source_dir().join("bridge/video-exposure.py");
    let ffmpeg = source_dir().join("node_modules/ffmpeg-static/ffmpeg.exe");
    if !executable.is_file() || !ffmpeg.is_file() || !script.is_file() {
        bail!("The local media editor requires bundled Python, FFmpeg and video-exposure.py")
    }
    let id = format!("exposure-{}", uuid::Uuid::new_v4().simple());
    let directory = root().join("outputs").join(&id);
    {
        let _admission = app.admission_lock.lock().await;
        let profile = app.get("profile", &profile_id)?;
        if !profile["gpu_ids"]
            .as_array()
            .is_some_and(|gpus| gpus.is_empty())
        {
            bail!("Select a CPU-only resource profile for exposure correction")
        }
        std::fs::create_dir_all(&directory)?;
        let job = directory.join("job.json");
        let params = json!({"source":source_path,"directory":directory,"ffmpeg":ffmpeg,"cpu_threads":profile["cpu_threads"]});
        std::fs::write(&job, serde_json::to_vec_pretty(&params)?)?;
        crate::server::start_instance(
            app.clone(),
            json!({
                "id":id,"name":"Video · exposure","profile_id":profile_id,"command":executable,
                "args":[script,job],"cwd":directory
            }),
        )
        .await?;
    }
    match timeout(Duration::from_secs(300), wait_for_worker(&app, &id)).await {
        Ok(result) => {
            if let Err(error) = result {
                if let Ok(message) = std::fs::read_to_string(directory.join("exposure-error.txt")) {
                    bail!("{message}")
                }
                return Err(error);
            }
        }
        Err(_) => {
            let mut workers = app.workers.lock().await;
            if let Some(worker) = workers.remove(&id) {
                worker.stop()?;
                let mut record = app.get("instance", &id)?;
                record["status"] = json!("stopped");
                app.save("instance", record)?;
            }
            bail!("Exposure correction exceeded five minutes")
        }
    }
    let metrics: Value =
        serde_json::from_slice(&std::fs::read(directory.join("exposure-metrics.json"))?)?;
    let output = directory.join("exposure-matched.mp4");
    let attachment = crate::attachments::import(&app, &output).await?;
    let copy_path = attachment["path"]
        .as_str()
        .context("Missing imported video path")?;
    if metrics["source_sha256"] != file_hash(source_path)?
        || metrics["output_sha256"] != file_hash(copy_path)?
    {
        bail!("Exposure copy failed verification")
    }
    let copy = json!({"attachment":attachment,"source_attachment_id":source["id"],"metrics":metrics,
                      "instance_id":id,"profile_id":profile_id,"created_at":chrono::Utc::now().to_rfc3339()});
    let _serial = app.chat_lock.lock().await;
    let mut run = app.get("run", &run_id)?;
    let mut chat = app.get("chat", run["chat_id"].as_str().context("Missing chat ID")?)?;
    let message = chat["messages"]
        .as_array_mut()
        .and_then(|messages| {
            messages
                .iter_mut()
                .find(|m| m["role"] == "assistant" && m["run_id"] == run_id)
        })
        .context("The video message is missing from its chat")?;
    message["generation"]["exposure_copy"] = copy.clone();
    run["exposure_copy"] = copy.clone();
    app.store
        .lock()
        .unwrap()
        .save_batch(vec![("run", run), ("chat", chat)])?;
    Ok(copy)
}
