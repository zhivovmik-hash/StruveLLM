use crate::{root, server::App};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::sync::Arc;

fn string<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Required: {key}"))
}
pub fn progress(app: &App, id: &str) -> Result<Value> {
    crate::safe_id(id)?;
    let record = app.get("run", id)?;
    if matches!(
        record["status"].as_str(),
        Some("completed" | "failed" | "cancelled" | "interrupted")
    ) {
        return Ok(record);
    }
    let path = root().join(format!("jobs/{id}-progress.json"));
    let mut value = record.clone();
    if path.exists() && std::fs::metadata(&path)?.len() < 512 * 1024 {
        if let Ok(p) = serde_json::from_slice::<Value>(&std::fs::read(path)?) {
            if let Some(fields) = p.as_object() {
                for (k, v) in fields {
                    value[k] = v.clone()
                }
            }
        }
    }
    let instance = app.get("instance", id).ok();
    if !["completed", "failed", "cancelled", "interrupted"]
        .contains(&value["status"].as_str().unwrap_or(""))
    {
        if let Some(instance) = instance {
            if !["running", "starting"].contains(&instance["status"].as_str().unwrap_or("")) {
                value["status"] = json!("interrupted");
                value["error"] = json!("Worker stopped before completing the answer");
            }
        }
    }
    if ["completed", "failed", "cancelled", "interrupted"]
        .contains(&record["status"].as_str().unwrap_or(""))
    {
        value["status"] = record["status"].clone();
        if record.get("error").is_some() {
            value["error"] = record["error"].clone();
        }
    }
    Ok(value)
}

/// The controller owns the worker jobs. None can survive this installation's previous controller.
pub fn recover_text_runs(app: &App) -> Result<()> {
    let active = app.store.lock().unwrap().live_runs(None)?;
    for row in active {
        let id = string(&row, "id")?;
        let mut run = app.get("run", id)?;
        if matches!(run["kind"].as_str(), Some("image" | "video")) { continue; }
        let last = progress(app, id)?;
        for field in ["content", "steps", "tool_calls_used"] {
            if let Some(value) = last.get(field) { run[field] = value.clone(); }
        }
        run["status"] = json!("interrupted");
        run["phase"] = json!("done");
        run["completed_at"] = json!(chrono::Utc::now().to_rfc3339());
        run["error"] = json!("Controller restarted before the answer was committed.");
        let mut batch = Vec::new();
        if let Some(mut chat) = run["chat_id"].as_str().and_then(|id| app.get("chat",id).ok()) {
            if let Some(messages) = chat["messages"].as_array_mut() {
                if !messages.iter().any(|m| m["role"] == "assistant" && m["run_id"] == id) {
                    messages.push(json!({"role":"assistant","kind":"interrupted","run_id":id,
                        "model_id":run["model_id"],"at":run["completed_at"],"content":run["content"],
                        "steps":run["steps"]}));
                    batch.push(("chat",chat));
                }
            }
        }
        batch.push(("run",run));
        app.store.lock().unwrap().save_batch(batch)?;
    }
    Ok(())
}
// The desktop sends only headlines and links from the tab belonging to this pending request.
pub async fn browser_reply(app: Arc<App>, p: Value) -> Result<Value> {
    let _guard = app.chat_lock.lock().await;
    let id = string(&p, "id")?;
    let current = progress(&app, id)?;
    let pending = &current["browser_search"];
    if current["status"] != "running" || current["execution"]["desktop_browser"] != true
        || current["phase"] != "browser_search" || pending["key"].as_str().is_none()
        || pending["key"] != p["key"] || current["chat_id"] != p["chat_id"] {
        bail!("Browser search is no longer pending for this chat")
    }
    let connection = app.get("mcp", "internet")?;
    if connection["enabled"] != true || !connection["project_ids"].as_array().is_some_and(|v|v.contains(&current["project_id"]))
        || !connection["allowed_tools"].as_array().is_some_and(|v|v.contains(&json!("web_search"))) {
        bail!("Web search is disabled for this project")
    }
    let status = string(&p, "status")?;
    let rows = p["results"].as_array().ok_or_else(||anyhow::anyhow!("Missing search results"))?;
    if !matches!(status, "ok"|"empty"|"skipped"|"selected") || rows.len()>10
        || matches!(status,"ok"|"selected") == rows.is_empty() || (status=="selected" && rows.len()!=1) {
        bail!("Invalid browser result")
    }
    let mut results = Vec::new();
    for row in rows {
        let raw = string(row,"url")?;
        let url = reqwest::Url::parse(raw)?;
        let title = string(row,"title")?;
        if !matches!(url.scheme(),"http"|"https") || url.host_str().is_none() || !url.username().is_empty()
            || url.password().is_some() || raw.len()>4096 || title.chars().count()>300 {
            bail!("Invalid browser result link")
        }
        results.push(json!({"url":url.as_str(),"title":title}));
    }
    let mut record = app.get("run",id)?;
    if record["browser_reply"]["key"] != p["key"] {
        record["browser_reply"]=json!({"key":p["key"],"status":status,"results":results});
        app.save("run",record)?;
    }
    Ok(json!({"accepted":true}))
}
pub fn list(app: &App) -> Result<Value> {
    let records = app.list("run")?;
    let mut values = records
        .as_array()
        .unwrap()
        .iter()
        .map(|r| progress(app, r["id"].as_str().unwrap()))
        .collect::<Result<Vec<_>>>()?;
    values.sort_by_key(|r| {
        (
            matches!(
                r["status"].as_str(),
                Some("running" | "starting" | "queued")
            ),
            r["created_at"]
                .as_str()
                .or(r["started_at"].as_str())
                .unwrap_or("")
                .to_string(),
        )
    });
    values.reverse();
    values.truncate(100);
    Ok(json!(values))
}
pub async fn send(app: Arc<App>, p: Value) -> Result<Value> {
    let content = string(&p, "content")?;
    if content.len() > 64000 {
        bail!("Message exceeds 64 KiB")
    }
    let project = app.get("project", string(&p, "project_id")?)?;
    let mut model = app.get("model", string(&p, "model_id")?)?;
    // Reject policy overrides before launching a potentially expensive runtime.
    let policy = crate::model_config::policy(&model, &p)?;
    let registered=model.clone();
    let profile = app.get("profile", string(&model, "profile_id")?)?;
    let threads = profile["cpu_threads"].as_u64().unwrap_or(1);
    model["cpu_threads"] = json!(model["cpu_threads"]
        .as_u64()
        .unwrap_or(threads)
        .min(threads));
    let _serial = app.chat_lock.lock().await;
    string(&model, "engine_model")?;
    if policy["use_tools"] == true && model["tool_mode"] == "none" {
        bail!("Tool calling is disabled for this model")
    }
    let mut chat = if let Some(id) = p["chat_id"].as_str() {
        app.get("chat", id)?
    } else {
        json!({"id":format!("chat-{}",uuid::Uuid::new_v4().simple()),"name":content.chars().take(52).collect::<String>(),"project_id":project["id"],"messages":[]})
    };
    if chat["project_id"] != project["id"] {
        bail!("Chat belongs to another project")
    }
    crate::project_memory::apply_context(&chat,&mut model)?;
    if chat["untitled"]==true {
        chat["name"]=json!(content.chars().take(52).collect::<String>());
        chat["untitled"]=json!(false);
    }
    if let Some(id) = chat["last_run_id"].as_str() {
        let previous = progress(&app, id)?;
        if ["running", "starting", "queued"].contains(&previous["status"].as_str().unwrap_or("")) {
            bail!("A task is already running in this chat")
        }
    }
    let mut attachments = vec![];
    if let Some(ids) = p["attachment_ids"].as_array() {
        if ids.len() > 8 {
            bail!("Maximum 8 attachments")
        }
        for id in ids {
            attachments.push(app.get("attachment", id.as_str().unwrap_or(""))?);
        }
    }
    if model["vision"] != true
        && attachments
            .iter()
            .any(|a| matches!(a["kind"].as_str(), Some("image" | "video")))
    {
        bail!("Choose a vision model for image or video attachments")
    }
    if attachments
        .iter()
        .map(|a| match a["kind"].as_str() {
            Some("video") => 4,
            Some("image") => 1,
            _ => 0,
        })
        .sum::<u32>()
        > 8
    {
        bail!("Maximum 8 images or sampled video frames per message (4 frames per video)")
    }
    if let Some(selection) = p.get("tool_selection").filter(|v| !v.is_null()) {
        if !selection
            .as_array()
            .map(|a| {
                a.len() <= 24
                    && a.iter().all(|t| {
                        t["connection_id"]
                            .as_str()
                            .map(|s| !s.is_empty())
                            .unwrap_or(false)
                            && t["tool"]
                                .as_str()
                                .map(|s| !s.is_empty() && s.len() <= 256)
                                .unwrap_or(false)
                    })
            })
            .unwrap_or(false)
        {
            bail!("Select at most 24 project tools")
        }
    }
    let id = format!("task-{}", uuid::Uuid::new_v4().simple());
    let user = json!({"role":"user","content":content,"at":chrono::Utc::now().to_rfc3339(),"run_id":id,"attachments":attachments});
    let mut messages = chat["messages"].as_array().unwrap().clone();
    messages.push(user.clone());
    let budget = (model["context_tokens"]
        .as_u64()
        .unwrap_or(2048)
        .saturating_sub(model["max_output_tokens"].as_u64().unwrap_or(512))
        .max(1)
        * 2) as usize;
    // Bound the bridge snapshot independently of the context estimate when a local tokenizer is available.
    let budget=if model["tokenizer"]=="llama.cpp"{1024*1024}else{budget};
    if content.chars().count() > budget {
        bail!("Message is too long for the selected context size")
    }
    let memory=crate::project_memory::recall_project(&app.store.lock().unwrap().db,&project,content)?;
    let mut history: Vec<Value> = vec![];
    let mut size = 0;
    for message in messages.iter().rev().filter(|m| {
        matches!(m["role"].as_str(), Some("user" | "assistant")) && !matches!(m["kind"].as_str(),Some("cancelled"|"interrupted"))
    }) {
        let text = message["content"].as_str().unwrap_or("");
        let n = text.chars().count();
        if size + n > budget {
            break;
        }
        size += n;
        history.push(
            json!({"role":message["role"],"content":text,"attachments":message["attachments"]}),
        );
    }
    history.reverse();
    let omitted = messages.iter().filter(|m| !matches!(m["kind"].as_str(),Some("cancelled"|"interrupted"))).count() - history.len();
    std::fs::create_dir_all(root().join("jobs"))?;
    let input = root().join(format!("jobs/{id}-input.json"));
    let max_calls = policy["max_tool_calls"].as_u64().unwrap();
    let created = chrono::Utc::now();
    let deadline = (created
        + chrono::Duration::seconds(policy["timeout_seconds"].as_i64().unwrap()))
    .to_rfc3339();
    std::fs::write(
        &input,
        serde_json::to_vec(
            &json!({"id":id,"chat_id":chat["id"],"project_id":project["id"],"model":model,"messages":history,"omitted_messages":omitted,"project_memory":memory,"attachments":attachments,"use_tools":policy["use_tools"]==true,"tool_selection":policy["tool_selection"],"desktop_browser":p["desktop_browser"]==true,"max_tool_calls":max_calls,"deadline_at":deadline,"timeout_seconds":policy["timeout_seconds"],"language":app.get("settings","system")?["language"]}),
        )?,
    )?;
    let profile_id = "agent-runtime";
    if app.get("profile", profile_id).is_err() {
        app.save("profile",json!({"id":profile_id,"name":"Agent coordinator","kind":"inference","gpu_ids":[],"ram_gib":0.5,"vram_gib":0,"cpu_threads":1,"exclusive":false}))?;
    }
    let devices=profile["gpu_ids"].as_array().cloned().unwrap_or_default();
    let mut reason=None;
    if model["runtime"].is_object(){
        let ids=app.store.lock().unwrap().queued_media()?;
        let earlier=ids.into_iter()
            .map(|id|app.get("run",&id)).collect::<Result<Vec<_>>>()?;
        reason=crate::media_queue::earlier_conflict(&earlier,&model,&devices);
        if reason.is_none(){reason=crate::media_queue::wait_reason(&app,&model,&devices,&profile,true).await?;}
    }
    let status=if reason.is_some(){"queued"}else{"starting"};
    let run=json!({"id":id,"chat_id":chat["id"],"project_id":project["id"],"model_id":model["id"],"kind":"text","status":status,"wait_reason":reason,
        "phase":if reason.is_some(){"waiting_for_resources"}else{"starting"},"created_at":created.to_rfc3339(),"deadline_at":if reason.is_some(){Value::Null}else{json!(deadline)},
        "execution":{"desktop_browser":p["desktop_browser"]==true,"model":model,"profile":profile,"registered_model_revision":registered["revision"],
        "registered_model_settings":crate::media_queue::stable_model(&registered),"lease_gpu_ids":devices,"limits":policy,"use_tools":policy["use_tools"]==true},
        "tool_calls_used":0,"content":"","omitted_messages":omitted,"steps":[]});
    chat["messages"]=json!(messages);chat["last_run_id"]=json!(id);chat["model_id"]=model["id"].clone();
    app.store.lock().unwrap().save_batch(vec![("chat",chat.clone()),("run",run)])?;
    drop(_serial);
    if reason.is_some(){app.media_queue_wake.notify_one();}else{launch_text(app.clone(),&id).await?;}
    Ok(json!({"id":id,"chat_id":chat["id"],"status":if reason.is_some(){"queued"}else{"running"},"wait_reason":reason,"omitted_messages":omitted}))
}

pub(crate) async fn launch_text(app:Arc<App>,id:&str)->Result<()>{
    let result=async{
        let mut run=app.get("run",id)?;
        let registered=app.get("model",string(&run,"model_id")?)?;
        let profile=app.get("profile",string(&registered,"profile_id")?)?;
        if crate::media_queue::stable_model(&registered)!=run["execution"]["registered_model_settings"] || profile["revision"]!=run["execution"]["profile"]["revision"]{
            bail!("Model or resource settings changed while waiting. Submit a new request.")
        }
        let input=root().join(format!("jobs/{id}-input.json"));
        let mut task:Value=serde_json::from_slice(&std::fs::read(&input)?)?;
        let project=app.get("project",string(&task,"project_id")?)?;
        let chat=app.get("chat",string(&task,"chat_id")?)?;
        if registered["runtime"].is_object(){
            let worker=crate::engines::start_owned(app.clone(),string(&run,"model_id")?,None).await?;
            task["model"]["instance_id"]=worker["id"].clone();
        }
        let now=chrono::Utc::now();
        task["deadline_at"]=json!((now+chrono::Duration::seconds(task["timeout_seconds"].as_i64().unwrap_or(1200))).to_rfc3339());
        if run["status"]=="queued"{
            let query=task["messages"].as_array().and_then(|a|a.last()).and_then(|m|m["content"].as_str()).unwrap_or("");
            task["project_memory"]=json!(crate::project_memory::recall_project(&app.store.lock().unwrap().db,&project,query)?);
        }
        run["last_wait_reason"]=run["wait_reason"].take();
        run["started_at"]=json!(now.to_rfc3339());run["deadline_at"]=task["deadline_at"].clone();
        run["status"]=json!("starting");run["phase"]=json!("starting");run["execution"]["model"]=task["model"].clone();
        std::fs::write(&input,serde_json::to_vec(&task)?)?;
        let run=app.save("run",run)?;
        crate::server::start_instance(app.clone(),json!({"id":id,"name":format!("Chat: {}",chat["name"].as_str().unwrap_or("")),
            "profile_id":"agent-runtime","command":crate::app_dir().join("node.exe"),"args":[crate::source_dir().join("bridge/agent.mjs"),input],"cwd":project["path"]})).await?;
        let owner=registered["runtime"].is_object().then(||task["model"]["instance_id"].as_str()).flatten().map(str::to_owned);
        crate::meter::start(app.clone(),&run,"chat",owner,task["model"].clone(),profile);
        Ok::<(),anyhow::Error>(())
    }.await;
    if let Err(e)=&result{
        let mut run=app.get("run",id)?;run["status"]=json!("failed");run["phase"]=json!("done");run["error"]=json!(e.to_string());run["completed_at"]=json!(chrono::Utc::now().to_rfc3339());
        app.save("run",run)?;
    }
    result
}
pub async fn cancel(app: Arc<App>, id: &str) -> Result<Value> {
    let _serial = app.chat_lock.lock().await;
    let mut run = progress(&app, id)?;
    if ["completed", "failed", "cancelled", "interrupted"]
        .contains(&run["status"].as_str().unwrap_or(""))
    {
        return Ok(run);
    }
    let mut workers = app.workers.lock().await;
    if let Some(worker) = workers.remove(id) {
        worker.stop()?;
    }
    if let Ok(mut instance)=app.get("instance",id){instance["status"]=json!("stopped");app.save("instance",instance)?;}
    app.media_queue_wake.notify_one();
    run["status"] = json!("cancelled");
    run["phase"] = json!("done");
    run["completed_at"] = json!(chrono::Utc::now().to_rfc3339());
    let mut chat = app.get("chat", string(&run, "chat_id")?)?;
    if !chat["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "assistant" && m["run_id"] == id)
    {
        chat["messages"].as_array_mut().unwrap().push(json!({
            "role":"assistant", "kind":"cancelled", "content":run["content"],
            "run_id":id, "model_id":run["model_id"], "at":run["completed_at"], "steps":run["steps"]
        }));
    }
    std::fs::write(
        root().join(format!("jobs/{id}-progress.json")),
        serde_json::to_vec(&run)?,
    )?;
    run["revision"] = app.get("run", id)?["revision"].clone();
    Ok(app
        .store
        .lock()
        .unwrap()
        .save_batch(vec![("chat", chat), ("run", run)])?
        .pop()
        .unwrap())
}
pub async fn finish(app: Arc<App>, p: Value) -> Result<Value> {
    let _serial = app.chat_lock.lock().await;
    let id = string(&p, "id")?;
    let mut run = progress(&app, id)?;
    if run["status"] == "completed" {
        return Ok(run);
    }
    if ["failed", "cancelled", "interrupted"].contains(&run["status"].as_str().unwrap_or("")) {
        bail!("Task is no longer active")
    }
    let content = string(&p, "content")?;
    if content.len() > 256 * 1024 {
        bail!("Answer too large")
    }
    let mut chat = app.get("chat", string(&run, "chat_id")?)?;
    if !chat["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "assistant" && m["run_id"] == id)
    {
        chat["messages"].as_array_mut().unwrap().push(json!({"role":"assistant","content":content,"run_id":id,"model_id":run["model_id"],"at":chrono::Utc::now().to_rfc3339(),"usage":p["usage"],"steps":p["steps"],"finish_reason":p["finish_reason"],"memory_sources":p["memory_sources"]}));
    }
    run["status"] = json!("completed");
    run["phase"] = json!("done");
    run["content"] = json!(content);
    run["usage"] = p["usage"].clone();
    run["context_usage"]=p["context_usage"].clone();
    run["memory_sources"]=p["memory_sources"].clone();
    if p["omitted_messages"].is_u64(){run["omitted_messages"]=p["omitted_messages"].clone();}
    run["steps"] = p["steps"].clone();
    run["finish_reason"] = p["finish_reason"].clone();
    run["completed_at"] = json!(chrono::Utc::now().to_rfc3339());
    Ok(app
        .store
        .lock()
        .unwrap()
        .save_batch(vec![("chat", chat), ("run", run)])?
        .pop()
        .unwrap())
}
pub async fn fail(app: Arc<App>, p: Value) -> Result<Value> {
    let _serial = app.chat_lock.lock().await;
    let id = string(&p, "id")?;
    let record = app.get("run", id)?;
    if matches!(
        record["status"].as_str(),
        Some("completed" | "cancelled" | "interrupted" | "failed")
    ) {
        return Ok(record);
    }
    let error = string(&p, "error")?;
    if error.len() > 16000 || p["content"].as_str().map(str::len).unwrap_or(0) > 256 * 1024 {
        bail!("Task failure payload is too large")
    }
    let mut run = progress(&app, id)?;
    run["revision"] = record["revision"].clone();
    run["status"] = json!("failed");
    run["phase"] = json!("done");
    run["error"] = json!(error);
    run["content"] = p["content"].clone();
    run["steps"] = p["steps"].clone();
    run["completed_at"] = json!(chrono::Utc::now().to_rfc3339());
    app.save("run", run)
}
