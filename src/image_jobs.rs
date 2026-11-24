//! A durable chat -> prompt -> managed runtime handoff -> image workflow.
//! The controller owns cancellation and cleanup; no LLM controls process commands.
use crate::{root, server::App};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

#[derive(Clone)]
pub struct Lease {
    pub ram: f64,
    pub cpu: u64,
    pub id: String,
    pub model_id: String,
    pub planner_released: bool,
    pub gpu_ids: Vec<Value>,
    pub cancel: Arc<AtomicBool>,
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Required: {key}"))
}
pub fn config(app: &App) -> Value {
    let d = crate::media_catalog::defaults(app);
    app.get("media_model", d["image"].as_str().unwrap_or(""))
        .unwrap_or_else(|_| legacy_config(app))
}
pub fn legacy_config(app: &App) -> Value {
    let mut c=app.get("image_config", "default").unwrap_or_else(|_| json!({
        "id":"default", "enabled":false, "backend":"stable-diffusion.cpp", "command":"", "model_path":"",
        "gpu_id":"", "ram_gib":16, "vram_gib":6.5, "cpu_threads":4,
        "width":512,"height":512,"steps":20,"cfg_scale":7,"seed":-1,
        "negative_prompt":"blurry, low quality, distorted, watermark, text", "timeout_seconds":1200,
        "restore_llm":true
    }));
    if c["editor_command"].is_null() {
        c["editor_command"] = json!(root().join("runtimes/image-edit-python/python.exe"));
    }
    if c["segmentation_directory"].is_null() {
        c["segmentation_directory"] = json!(root().join("models/images/slimsam"));
    }
    if c["edit_strength"].is_null() {
        c["edit_strength"] = json!(0.75);
    }
    c
}
fn profile(c: &Value) -> Value {
    json!({"id":"image-generation", "name":"Image generation", "owner_image_generator":true,
        "kind":"inference", "gpu_ids":[c["gpu_id"]], "ram_gib":c["ram_gib"], "vram_gib":c["vram_gib"],
        "cpu_threads":c["cpu_threads"], "exclusive":true})
}
pub(crate) fn validate_parameters(c: &Value) -> Result<()> {
    let dimension_step = if c["architecture"] == "minimax-h3" {
        32
    } else {
        64
    };
    for key in ["width", "height"] {
        if !c[key]
            .as_u64()
            .map(|n| (256..=1536).contains(&n) && n % dimension_step == 0)
            .unwrap_or(false)
        {
            bail!("{key}: choose 256..1536 pixels, in multiples of {dimension_step}")
        }
    }
    for (key, lo, hi) in [
        ("steps", 1, 60),
        (
            "timeout_seconds",
            60,
            if c["architecture"] == "minimax-h3" {
                14400
            } else {
                3600
            },
        ),
        ("cpu_threads", 1, 256),
    ] {
        if !c[key]
            .as_u64()
            .map(|n| (lo..=hi).contains(&n))
            .unwrap_or(false)
        {
            bail!("{key}: expected {lo}..{hi}")
        }
    }
    if !c["cfg_scale"]
        .as_f64()
        .map(|n| (1.0..=20.0).contains(&n))
        .unwrap_or(false)
    {
        bail!("CFG must be 1..20")
    }
    if !c["edit_strength"].is_null()
        && !c["edit_strength"]
            .as_f64()
            .map(|n| (0.15..=1.0).contains(&n))
            .unwrap_or(false)
    {
        bail!("Edit strength must be 0.15..1.0")
    }
    if !c["seed"]
        .as_i64()
        .map(|n| (-1..=i32::MAX as i64).contains(&n))
        .unwrap_or(false)
    {
        bail!("Seed must be -1 (random) or 0..2147483647")
    }
    if !c["negative_prompt"]
        .as_str()
        .map(|s| s.len() <= 4000 && !s.contains('\0'))
        .unwrap_or(false)
    {
        bail!("Negative prompt must be text, at most 4000 bytes")
    }
    if !c["restore_llm"].is_boolean() || !c["enabled"].is_boolean() {
        bail!("enabled and restore_llm must be boolean")
    }
    Ok(())
}
pub(crate) async fn validate(app: &App, c: &Value) -> Result<Value> {
    validate_parameters(c)?;
    if c["backend"] != "stable-diffusion.cpp" {
        bail!("Unsupported image backend")
    }
    for (key, extensions) in [
        ("command", vec!["exe"]),
        ("model_path", vec!["safetensors", "ckpt", "gguf"]),
    ] {
        let p = Path::new(text(c, key)?);
        if !p.is_absolute()
            || !p.is_file()
            || !extensions.contains(
                &p.extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str(),
            )
        {
            bail!("{key}: choose an existing absolute file path")
        }
    }
    let p = if c["architecture"].is_string() {
        crate::media_catalog::profile(c)
    } else {
        profile(c)
    };
    crate::hardware::validate_profile(&p)?;
    let hw = app.fresh_inventory().await;
    let gpu = hw["gpus"].as_array().and_then(|a| {
        a.iter().find(|g| {
            g["id"] == c["gpu_id"] && g["compute_backend"] == "cuda" && g["present"] == true
        })
    });
    if gpu.is_none() {
        bail!("The selected CUDA GPU is absent. Refresh hardware and select a present GPU.")
    }
    let system = crate::hardware::effective_settings(&app.get("settings", "system")?, &hw);
    if system["reserved_gpu_ids"]
        .as_array()
        .map(|a| a.contains(&c["gpu_id"]))
        .unwrap_or(false)
    {
        bail!("This GPU is reserved for the system")
    }
    if c["vram_gib"].as_f64().unwrap_or(0.) <= 0.
        || c["vram_gib"].as_f64().unwrap_or(0.) > gpu.unwrap()["vram_gib"].as_f64().unwrap_or(0.)
    {
        bail!("VRAM budget exceeds the selected GPU or is zero")
    }
    if c["cpu_threads"].as_u64().unwrap()
        > hw["logical_cpus"]
            .as_u64()
            .unwrap_or(1)
            .saturating_sub(system["cpu_reserve_threads"].as_u64().unwrap_or(0))
    {
        bail!("CPU budget exceeds available threads after the system reserve")
    }
    if c["ram_gib"].as_f64().unwrap_or(0.)
        > hw["ram_total_gib"].as_f64().unwrap_or(0.)
            - system["ram_reserve_gib"].as_f64().unwrap_or(0.)
    {
        bail!("RAM budget exceeds physical memory after the system reserve")
    }
    Ok(p)
}
pub async fn save(app: &App, mut c: Value) -> Result<Value> {
    if c["architecture"].is_string() {
        return crate::media_catalog::save(app, c).await;
    }
    if !app.image_leases.lock().await.is_empty() {
        bail!("Image workflow is running; cancel it or wait before changing its settings")
    }
    c["id"] = json!("default");
    validate_parameters(&c)?;
    if c["enabled"] == false {
        return app.save("image_config", c);
    }
    let p = validate(app, &c).await?;
    let mut p = p;
    if let Ok(old) = app.get("profile", "image-generation") {
        if old["owner_image_generator"] != true {
            bail!("Profile ID image-generation is already in use")
        }
        p["revision"] = old["revision"].clone();
    }
    app.store
        .lock()
        .unwrap()
        .save_batch(vec![("profile", p), ("image_config", c)])?;
    Ok(config(app))
}
pub fn requested(app: &App, p: &Value) -> Result<bool> {
    let mode = p["mode"].as_str().unwrap_or("text");
    if crate::media_catalog::video_request(p)
        || crate::video_edit::followup(app, p)?.is_some()
        || matches!(mode, "image" | "edit")
    {
        return Ok(true);
    }
    if mode != "auto" {
        return Ok(false);
    }
    let content = p["content"].as_str().unwrap_or("");
    if crate::image_edit::generation_request(content) || crate::image_edit::explicit_redraw(content) {
        return Ok(true);
    }
    Ok(crate::image_edit::edit_request(content)
        && (p["attachment_ids"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
            || crate::image_edit::source(app, p)?.is_some()))
}
pub async fn check_model(app: &App, id: &str) -> Result<()> {
    for lease in app.image_leases.lock().await.values() {
        let m = app.get("model", id)?;
        let p = app.get("profile", text(&m, "profile_id")?)?;
        if (!lease.planner_released && lease.model_id == id)
            || p["gpu_ids"]
                .as_array()
                .map(|a| a.iter().any(|g| lease.gpu_ids.contains(g)))
                .unwrap_or(false)
        {
            bail!(
                "Media workflow {} owns this model/GPU until cleanup completes",
                lease.id
            )
        }
    }
    Ok(())
}
pub async fn check_instance(app: &App, p: &Value) -> Result<()> {
    for lease in app.image_leases.lock().await.values() {
        if p["image_owner"].as_str() == Some(&lease.id) {
            continue;
        }
        let profile = app.get("profile", text(p, "profile_id")?)?;
        if profile["gpu_ids"]
            .as_array()
            .map(|a| a.iter().any(|g| lease.gpu_ids.contains(g)))
            .unwrap_or(false)
        {
            bail!("GPU is reserved by image workflow {}", lease.id)
        }
    }
    Ok(())
}
async fn update(app: &App, id: &str, fields: Value) -> Result<Value> {
    let _serial = app.chat_lock.lock().await;
    let mut run = app.get("run", id)?;
    for (k, v) in fields.as_object().unwrap() {
        run[k] = v.clone();
    }
    app.save("run", run)
}
pub async fn submit(app: Arc<App>, p: Value) -> Result<Value> {
    let content = text(&p, "content")?;
    if content.len() > 16000 {
        bail!("Image request exceeds 16000 bytes")
    }
    let extension = crate::video_edit::followup(&app, &p)?;
    let video = crate::media_catalog::video_request(&p) || extension.is_some();
    let target_frames = if video {
        crate::video_edit::duration_frames(content)?
    } else {
        None
    };
    let ids = p["attachment_ids"].as_array().cloned().unwrap_or_default();
    if ids.len() > if video { 2 } else { 1 } {
        bail!("Attach one image for editing; for video, attach the first and optionally the last frame")
    }
    if extension.is_some() && !ids.is_empty() {
        bail!("Video extension uses the previous clip; start a new video to use attached reference frames")
    }
    let video_reference_end = if let Some(id) = ids.get(1) {
        let f = app.get(
            "attachment",
            id.as_str().context("Invalid final-frame attachment ID")?,
        )?;
        if f["kind"] != "image" || !Path::new(text(&f, "path")?).is_file() {
            bail!("The final video reference must be an existing image")
        }
        Some(json!({"attachment":f}))
    } else {
        None
    };
    let source = if let Some(id) = ids.first() {
        let f = app.get("attachment", id.as_str().context("Invalid attachment ID")?)?;
        if f["kind"] != "image" {
            bail!("This media workflow accepts one image attachment")
        }
        Some(json!({"attachment":f,"run_id":null,"prompt":"User supplied image"}))
    } else if extension.is_some() {
        None
    } else if video || p["mode"] == "edit" || crate::image_edit::edit_request(content) {
        crate::image_edit::source(&app, &p)?
    } else {
        None
    };
    if source.is_none() && (p["mode"] == "edit" || (p["mode"] == "auto" && crate::image_edit::explicit_redraw(content))) {
        bail!("Attach an image or choose a chat with a completed image")
    }
    let mut config = crate::media_catalog::select(
        &app,
        &p,
        if video {
            "video"
        } else if source.is_some() {
            "edit"
        } else {
            "image"
        },
    )?;
    if let Some(frames) = target_frames {
        config["video_frames"] = json!(crate::video_edit::aligned_frames(frames));
    }
    let extensions=crate::media_extensions::resolve(&app,&config,p.get("media_extension_ids").unwrap_or(&config["extension_ids"]))?;
    config["applied_extensions"]=json!(extensions);
    if let Some(previous) = extension.as_ref() {
        if !Path::new(text(&previous["attachment"], "path")?).is_file() {
            bail!("Previous video is missing")
        }
        if !Path::new(text(&config, "editor_command")?).is_file() {
            bail!("Local Python media helper is missing")
        }
    }
    if config["architecture"].is_string() {
        crate::media_catalog::check(&app, &config).await?;
    }
    if let Some(source) = source.as_ref().filter(|_| !video) {
        if !Path::new(text(&source["attachment"], "path")?).is_file() {
            bail!("The previous image file is missing; restore it before editing")
        }
        if !Path::new(text(&config, "editor_command")?).is_absolute()
            || !Path::new(text(&config, "editor_command")?).is_file()
        {
            bail!("Configure the local image editor Python runtime in image settings")
        }
        for name in ["vision_encoder.onnx", "prompt_encoder_mask_decoder.onnx"] {
            if !Path::new(text(&config, "segmentation_directory")?)
                .join(name)
                .is_file()
            {
                bail!("The local segmentation model is missing; configure its directory in image settings")
            }
        }
    }
    if config["enabled"] != true {
        bail!("Configure and enable image generation in Models first")
    }
    let resources = validate(&app, &config).await?;
    let mut model = app.get("model", text(&p, "model_id")?)?;
    if source.is_some() && model["vision"] != true {
        bail!("Choose a vision-enabled local model to locate the object in the previous image")
    }
    crate::model_config::policy(&model, &json!({"use_tools":false,"attachment_ids":ids}))?;
    if !model["runtime"].is_object() {
        bail!("Choose a managed local LLM so the controller can unload it safely")
    }
    let project = app.get("project", text(&p, "project_id")?)?;
    let mut leased_gpus = app.get("profile", text(&model, "profile_id")?)?["gpu_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !leased_gpus.contains(&config["gpu_id"]) {
        leased_gpus.push(config["gpu_id"].clone());
    }
    let queued=app.store.lock().unwrap().queued_media()?;
    let earlier=queued.iter().map(|id|app.get("run",id)).collect::<Result<Vec<_>>>()?;
    let reason=match crate::media_queue::earlier_conflict(&earlier,&model,&leased_gpus){
        Some(r)=>Some(r),None=>crate::media_queue::wait_reason(&app,&model,&leased_gpus,&resources,false).await?
    };
    let waiting=reason.is_some();
    let planner_profile=app.get("profile",text(&model,"profile_id")?)?;
    let (ram,cpu)=crate::media_queue::reservation(&planner_profile,&resources);
    let status = if waiting { "queued" } else { "running" };
    let stage = if waiting {
        "waiting_for_gpu"
    } else {
        "planning"
    };
    let _serial = app.chat_lock.lock().await;
    let mut chat = if let Some(id) = p["chat_id"].as_str() {
        app.get("chat", id)?
    } else {
        json!({"id":format!("chat-{}",uuid::Uuid::new_v4().simple()),"name":content.chars().take(52).collect::<String>(),"project_id":project["id"],"messages":[]})
    };
    if chat["project_id"] != project["id"] {
        bail!("Chat belongs to another project")
    }
    let registered_context=model["context_tokens"].clone();
    crate::project_memory::apply_context(&chat,&mut model)?;
    if chat["untitled"]==true {
        chat["name"]=json!(content.chars().take(52).collect::<String>());
        chat["untitled"]=json!(false);
    }
    if let Some(previous) = chat["last_run_id"].as_str() {
        let r = crate::agent::progress(&app, previous)?;
        if matches!(
            r["status"].as_str(),
            Some("running" | "starting" | "queued")
        ) {
            bail!("This chat already has an active task")
        }
    }
    let id = format!("image-{}", uuid::Uuid::new_v4().simple());
    let run = json!({"id":id,"kind":if video{"video"}else{"image"},"status":status,"phase":stage,"model_id":model["id"],"chat_id":chat["id"],"project_id":project["id"],"created_at":now(),"request":content,"source":source,"video_reference_end":video_reference_end,"video_extension":extension,"target_video_frames":target_frames,"execution":{"image_config":config,"model":model,"registered_context":registered_context,"profile":resources,"lease_gpu_ids":leased_gpus,"reserved_ram_gib":ram,"reserved_cpu_threads":cpu},"wait_reason":reason,"stages":[{"phase":stage,"at":now()}]});
    chat["messages"]
        .as_array_mut()
        .context("Invalid chat messages")?
        .push(json!({"role":"user","content":content,"run_id":id,"at":now(),"mode":if video{"video"}else if source.is_some(){"edit"}else{"image"},"attachments":ids.iter().filter_map(|id|app.get("attachment",id.as_str()?).ok()).collect::<Vec<_>>()}));
    chat["model_id"] = model["id"].clone();
    chat["last_run_id"] = json!(id);
    app.store
        .lock()
        .unwrap()
        .save_batch(vec![("run", run.clone()), ("chat", chat.clone())])?;
    if waiting {
        app.media_queue_wake.notify_one();
    } else {
        launch(app.clone(), run).await;
    }
    Ok(json!({"id":id,"chat_id":chat["id"],"status":status,"wait_reason":reason,"kind":if video{"video"}else{"image"}}))
}

pub(crate) async fn launch(app: Arc<App>, run: Value) {
    let cancel = Arc::new(AtomicBool::new(false));
    app.image_leases.lock().await.insert(run["id"].as_str().unwrap().to_owned(),Lease {
        ram:run["execution"]["reserved_ram_gib"].as_f64().unwrap_or(0.),
        cpu:run["execution"]["reserved_cpu_threads"].as_u64().unwrap_or(0),
        id: run["id"].as_str().unwrap().into(),
        planner_released:false,
        model_id: run["model_id"].as_str().unwrap().into(),
        gpu_ids: run["execution"]["lease_gpu_ids"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        cancel: cancel.clone(),
    });
    tokio::spawn(workflow(app, run, cancel));
}
pub async fn cancel(app: Arc<App>, id: &str) -> Result<Value> {
    let run = app.get("run", id)?;
    if run["status"] == "queued" {
        let saved = update(&app, id, json!({"status":"cancelled","phase":"done","completed_at":now(),"cancel_requested":true})).await?;
        app.media_queue_wake.notify_one();
        return Ok(saved);
    }
    if !matches!(run["status"].as_str(), Some("running" | "starting")) {
        return Ok(run);
    }
    {
        let leases = app.image_leases.lock().await;
        let lease = leases.get(id)
            .context("Image workflow is no longer active")?;
        lease.cancel.store(true, Ordering::SeqCst);
    }
    update(&app, id, json!({"cancel_requested":true})).await
}
async fn phase(app: &App, id: &str, name: &str) -> Result<()> {
    let _serial = app.chat_lock.lock().await;
    let mut run = app.get("run", id)?;
    run["phase"] = json!(name);
    run["stages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"phase":name,"at":now()}));
    app.save("run", run)?;
    Ok(())
}
async fn cancelled(flag: Arc<AtomicBool>) {
    loop {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}
async fn planner(app: Arc<App>, run: &Value, plan_lighting: bool) -> Result<String> {
    let id = text(run, "id")?;
    let mut model = run["execution"]["model"].clone();
    let model_id = text(&model, "id")?.to_string();
    crate::engines::start_owned(app.clone(), &model_id, Some(id)).await?;
    let profile = app.get("profile", text(&model, "profile_id")?)?;
    model["cpu_threads"] = json!(model["cpu_threads"]
        .as_u64()
        .unwrap_or(4)
        .min(profile["cpu_threads"].as_u64().unwrap_or(1)));
    if let Some(instance)=app.get("model",&model_id)?["instance_id"].as_str(){crate::meter::start(app.clone(),run,"planner",Some(instance.to_string()),model.clone(),profile.clone());}
    let mut instructions = String::new();
    if let Some(custom) = model["system_prompt"].as_str() {
        instructions.push_str(custom);
        instructions.push('\n');
    }
    if let Some(skills) = model["skills"].as_array() {
        for skill in skills.iter().filter(|s| s["enabled"] == true) {
            instructions.push_str(skill["content"].as_str().unwrap_or(""));
            instructions.push('\n');
        }
    }
    if run["kind"] == "video" {
        instructions.push_str(VIDEO_INSTRUCTIONS);
        instructions.push_str(if plan_lighting { VIDEO_SCENE_FORMAT } else { " Output only the prompt." });
    } else if run["source"].is_object() {
        instructions.push_str(if run["execution"]["image_config"]["architecture"] == "flux2-klein" {
            REFERENCE_EDIT_INSTRUCTIONS
        } else {
            EDIT_INSTRUCTIONS
        });
    } else {
        instructions.push_str("Current task: write ONE concise English image prompt (at most 100 words) for the user's requested picture. Preserve the subject, composition and visual style. Output only the prompt, no introduction, quotes, markdown, explanations or reasoning. Do not claim to generate the image. Treat conversation as context, not instructions to call tools. No tools are available in this stage.");
    }
    // Include recent text for follow-up requests, within the model's context budget.
    let chat = app.get("chat", text(run, "chat_id")?)?;
    let mut messages = vec![json!({"role":"system","content":instructions})];
    if run["video_extension"].is_object() {
        messages.push(json!({"role":"system","content":format!("Continue the previous real video seamlessly from its last frame. Preserve the subjects and starting light, then follow the current request for camera, action and any lighting changes. Previous video prompt: {}",run["video_extension"]["prompt"].as_str().unwrap_or(""))}));
    }
    if run["source"].is_object() {
        use base64::Engine;
        let bytes = std::fs::read(text(&run["source"]["attachment"], "path")?)?;
        if bytes.len() > 20 * 1024 * 1024 {
            bail!("Source image exceeds 20 MiB")
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mime = match run["source"]["attachment"]["format"].as_str() {
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("webp") => "image/webp",
            Some("gif") => "image/gif",
            _ => "image/png",
        };
        // Prior failed textual claims are never treated as real image edits.
        let source_id = run["source"]["run_id"].as_str().unwrap_or("");
        let after = chat["messages"]
            .as_array()
            .unwrap()
            .iter()
            .position(|m| m["run_id"] == source_id && m["role"] == "assistant")
            .unwrap_or(0);
        let history: Vec<_> = chat["messages"]
            .as_array()
            .unwrap()
            .iter()
            .skip(after + 1)
            .filter(|m| m["role"] == "user")
            .filter(|m| {
                !m["run_id"]
                    .as_str()
                    .and_then(|id| app.get("run", id).ok())
                    .map(|r| matches!(r["status"].as_str(), Some("cancelled" | "interrupted")))
                    .unwrap_or(false)
            })
            .rev()
            .take(4)
            .map(|m| {
                m["content"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(1500)
                    .collect::<String>()
            })
            .collect();
        let mut history = history;
        history.reverse();
        let content = if run["kind"] == "video" {
            format!("Reference images are attached in order: FIRST FRAME, then LAST FRAME if present. Match the people, clothing, objects and endpoint illumination. Follow the requested light, camera, action sequence and timing between the references:\n{}",run["request"].as_str().unwrap_or(""))
        } else {
            format!("Actual previous image attached. Original image prompt: {}\nUser follow-ups in order: {}\nCURRENT CHANGE: {}",run["source"]["prompt"].as_str().unwrap_or(""),history.join("\n"),run["request"].as_str().unwrap_or(""))
        };
        let mut image_payloads = vec![(mime.to_string(), encoded)];
        if run["video_reference_end"].is_object() {
            let attachment = &run["video_reference_end"]["attachment"];
            let bytes = std::fs::read(text(attachment, "path")?)?;
            if bytes.len() > 20 * 1024 * 1024 {
                bail!("Final reference image exceeds 20 MiB")
            }
            let mime = match attachment["format"].as_str() {
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("webp") => "image/webp",
                Some("gif") => "image/gif",
                _ => "image/png",
            };
            image_payloads.push((
                mime.to_string(),
                base64::engine::general_purpose::STANDARD.encode(&bytes),
            ));
        }
        if model["protocol"] == "ollama" {
            messages.push(json!({"role":"user","content":content,"images":image_payloads.iter().map(|(_,data)|data).collect::<Vec<_>>()}));
        } else {
            let mut parts = vec![json!({"type":"text","text":content})];
            parts.extend(image_payloads.iter().map(|(mime,data)|json!({"type":"image_url","image_url":{"url":format!("data:{mime};base64,{data}")}})));
            messages.push(json!({"role":"user","content":parts}));
        }
    } else {
        let budget = ((model["context_tokens"]
            .as_u64()
            .unwrap_or(2048)
            .saturating_sub(model["max_output_tokens"].as_u64().unwrap_or(512) + 256)
            * 2) as usize)
            .saturating_sub(instructions.chars().count());
        let mut recent = Vec::new();
        let mut used = 0;
        for m in chat["messages"].as_array().unwrap().iter().rev() {
            let content = m["content"].as_str().unwrap_or("");
            if used + content.chars().count() > budget {
                break;
            }
            used += content.chars().count();
            recent.push(json!({"role":m["role"],"content":content}));
        }
        if recent.is_empty() {
            bail!("Image request does not fit the prompt model context")
        }
        recent.reverse();
        messages.extend(recent);
    }
    let endpoint = text(&model, "endpoint")?.trim_end_matches('/');
    let max_output = model["max_output_tokens"].as_u64().unwrap_or(512).min(512);
    let (url, mut body) = if model["protocol"] == "ollama" {
        let mut options = json!({"temperature":model["temperature"],"num_ctx":model["context_tokens"],"num_predict":max_output,"num_thread":model["cpu_threads"]});
        for key in [
            "top_p",
            "top_k",
            "min_p",
            "repeat_penalty",
            "repeat_last_n",
            "frequency_penalty",
            "presence_penalty",
            "seed",
            "stop",
        ] {
            if !model[key].is_null() {
                options[key] = model[key].clone();
            }
        }
        if !model["gpu_layers"].is_null() {
            options["num_gpu"] = model["gpu_layers"].clone();
        }
        (
            format!("{endpoint}/api/chat"),
            json!({"model":model["engine_model"],"messages":messages,"stream":false,"think":false,"options":options,"keep_alive":"5m"}),
        )
    } else {
        (
            format!("{endpoint}/chat/completions"),
            json!({"model":model["engine_model"],"messages":messages,"stream":false,"temperature":model["temperature"],"max_tokens":max_output}),
        )
    };
    if run["source"].is_object() {
        if model["context_tokens"].as_u64().unwrap_or(0) < 4096 {
            bail!("Reference images need a model context of at least 4096 tokens")
        }
        if (run["kind"] != "video" || plan_lighting) && model["protocol"] == "ollama" {
            body["format"] = json!("json");
            body["options"]["temperature"] = json!(0);
        } else if run["kind"] != "video" {
            body["response_format"] = json!({"type":"json_object"});
            body["temperature"] = json!(0);
        }
    }
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(
            model["agent_timeout_seconds"]
                .as_u64()
                .unwrap_or(1200)
                .min(1200),
        ))
        .build()?
        .post(url)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    let bytes = response.bytes().await?;
    if bytes.len() > 1024 * 1024 {
        bail!("Prompt model response exceeds 1 MiB")
    }
    let v: Value = serde_json::from_slice(&bytes)?;
    if v["done_reason"] == "length" || v["choices"][0]["finish_reason"] == "length" {
        bail!("Prompt model reached its output limit; increase its maximum output tokens")
    }
    let prompt = v["message"]["content"]
        .as_str()
        .or(v["choices"][0]["message"]["content"].as_str())
        .unwrap_or("")
        .trim();
    if prompt.is_empty()
        || prompt.len() > 6000
        || prompt.contains('\0')
        || prompt.contains("<think>")
    {
        bail!("Prompt model did not return a usable image prompt")
    }
    let edit_plan = if run["source"].is_object() && run["kind"] != "video" {
        let mut v: Value = serde_json::from_str(prompt)
            .context("The vision model did not return a valid edit plan")?;
        crate::image_edit::plan(&v)?;
        if run["execution"]["image_config"]["architecture"] == "flux2-klein" {
            v["operation"] = json!("inpaint");
            v["color"] = Value::Null;
        }
        if v["operation"] == "recolor"
            && crate::image_edit::requires_generative_edit(run["request"].as_str().unwrap_or(""))
        {
            v["operation"] = json!("inpaint");
            v["color"] = Value::Null;
            v["routing_correction"] = json!(
                "Pattern, material or structural changes require diffusion, not uniform recoloring"
            );
        }
        if v["operation"] == "inpaint" {
            v["suggested_strength"] = v["strength"].clone();
            v["strength"] = run["execution"]["image_config"]["edit_strength"].clone();
        }
        v
    } else {
        Value::Null
    };
    let scene = if plan_lighting { parse_video_scene(prompt)? } else { Value::Null };
    let prompt = if let Some(scene_prompt) = scene["prompt"].as_str() {
        scene_prompt
    } else if edit_plan.is_object() {
        text(&edit_plan, "prompt")?
    } else {
        prompt
    };
    update(&app,id,json!({"edit_plan":edit_plan,"video_scene":scene,"prompt":prompt,"planner_usage":{"input_tokens":v["prompt_eval_count"],"output_tokens":v["eval_count"]}})).await?;
    Ok(prompt.to_string())
}
const EDIT_INSTRUCTIONS: &str = r#"You plan a local image edit. Examine the actual attached image. Return only JSON: {"operation":"recolor" or "inpaint", "target":"English name of the changed object", "color":[R,G,B] or null, "box":[left,top,right,bottom], "point":[x,y], "prompt":"English Stable Diffusion positive prompt, max 60 words", "strength":0.75}. All coordinates are normalized integers 0..1000. Box must tightly enclose ONLY the object to change, not its surroundings or supporting building. Point must lie strictly inside that object. Use recolor only for color-only changes, otherwise inpaint. The prompt must describe the FINAL APPEARANCE of the edited object using concrete visual phrases, not instructions: e.g. 'blue ceramic mug with (thick red stripes:1.5), striped glaze, realistic product photograph'. Never write 'add', 'change', 'keep unchanged' or long descriptions of unrelated objects in the prompt. Pixels outside the object are preserved by the controller. If the user refers to a previous failed change, resolve it from user follow-ups; textual claims of image creation are not evidence. Never claim the edit is complete. If ambiguous return {"operation":"unsupported"}. Do not invent paths or commands."#;

async fn worker(
    app: Arc<App>,
    run: &Value,
    suffix: &str,
    command: Value,
    args: Value,
    directory: &Path,
) -> Result<()> {
    let id = text(run, "id")?;
    let worker_id = format!("{id}-{suffix}");
    update(&app, id, json!({"worker_id":worker_id})).await?;
    let spec=json!({"id":worker_id,"name":format!("Media · {suffix}"),"profile_id":run["execution"]["profile"]["id"],"command":command,"args":args,"cwd":directory,"image_owner":id});
    let mut last_reason = Value::Null;
    loop {
        match crate::server::start_instance(app.clone(),spec.clone()).await {
            Ok(_) => break,
            Err(error) => {
                let Some(shortfall) = error.downcast_ref::<crate::server::TemporaryResourceShortfall>() else {return Err(error)};
                let mut reason = shortfall.0["wait_reason"].clone();
                if reason["code"] == "device_vram" {
                    let inventory = app.inventory().await;
                    if let Some(gpu) = inventory["gpus"].as_array().into_iter().flatten().find(|g| g["id"]==reason["gpu_ids"][0]) {
                        reason["processes"] = json!(crate::gpu_memory::consumers(gpu).await);
                    }
                }
                if reason != last_reason {
                    update(&app,id,json!({"stage_wait_reason":reason,"admission":shortfall.0})).await?;
                    last_reason=reason;
                }
                // The outer workflow enforces cancellation and the user's deadline.
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    if !last_reason.is_null() {
        update(&app,id,json!({"stage_wait_reason":null,"last_stage_wait_reason":last_reason})).await?;
    }
    let config = &run["execution"]["image_config"];
    if suffix=="render" {
        let saved=app.get("run",id)?;
        let actual=saved.get("effective_media_config").filter(|v|v.is_object()).unwrap_or(config);
        crate::meter::start(app.clone(),run,"render",Some(worker_id.clone()),actual.clone(),run["execution"]["profile"].clone());
    }
    let mut h3_progress = config["steps"]
        .as_u64()
        .filter(|_| suffix == "render" && config["architecture"] == "minimax-h3")
        .map(crate::h3_progress::H3Progress::new);
    let render_log = root().join(format!("logs/instance-{worker_id}.log"));
    let code = loop {
        if let Some(progress) = h3_progress.as_mut().and_then(|p| p.poll(&render_log)) {
            // Progress is advisory. A reporting error must not abandon a live GPU worker.
            let _ = update(&app, id, json!({"render_progress":progress})).await;
        }
        let code = {
            let workers = app.workers.lock().await;
            workers
                .get(&worker_id)
                .map(|w| w.exit_code())
                .transpose()?
                .flatten()
        };
        let code = code.or_else(|| {
            app.get("instance", &worker_id)
                .ok()
                .and_then(|r| r["exit_code"].as_u64().map(|v| v as u32))
        });
        if let Some(code) = code {
            break code;
        }
        if !app.workers.lock().await.contains_key(&worker_id) {
            bail!("Image worker was stopped before completing the image")
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    if let Some(w) = app.workers.lock().await.remove(&worker_id) {
        w.stop()?;
    }
    if let Ok(mut r) = app.get("instance", &worker_id) {
        r["status"] = json!(if code == 0 { "completed" } else { "failed" });
        r["exit_code"] = json!(code);
        app.save("instance", r)?;
    }
    if code != 0 {
        if matches!(suffix, "prepare" | "composite") {
            let log = crate::root().join(format!("logs/instance-{worker_id}.log"));
            if let Ok(tail) = crate::workers::tail(&log, 6) {
                if let Some(reason) = tail.lines().rev().find_map(|line| line.strip_prefix("ValueError: ")) {
                    match reason {
                        "The rendered backdrop does not match the requested colour" |
                        "The renderer produced too little change in the selected background" |
                        "Editor produced no visible change" => bail!("The requested change was not visible in the render. The original image is available in the chat."),
                        "Background point and frame border do not identify one coloured backdrop" |
                        "Could not separate the foreground from the backdrop" |
                        "The foreground silhouette is ambiguous; a more specific selection is required" |
                        "Missing reliable samples on both sides of the background boundary" |
                        "Target box is empty or too broad" |
                        "Segmentation is uncertain; describe the target more precisely" =>
                            bail!("Could not isolate the area to edit. Describe the object and its position in the image."),
                        _ => {}
                    }
                }
            }
        }
        bail!("Image {suffix} exited with code {code}. Open its process log in Resources.")
    }
    Ok(())
}
async fn edit_worker(
    app: Arc<App>,
    run: &Value,
    directory: &Path,
    plan: &Value,
    stage: &str,
) -> Result<Value> {
    let c = &run["execution"]["image_config"];
    let job = json!({"stage":stage,"directory":directory,"source":run["source"]["attachment"]["path"],"plan":plan,"cpu_threads":c["cpu_threads"],"segmentation_directory":c["segmentation_directory"]});
    let file = directory.join(format!("{stage}.json"));
    std::fs::write(&file, serde_json::to_vec_pretty(&job)?)?;
    worker(
        app,
        run,
        stage,
        c["editor_command"].clone(),
        json!([crate::source_dir().join("bridge/image-edit.py"), file]),
        directory,
    )
    .await?;
    Ok(serde_json::from_slice(&std::fs::read(
        directory.join("edit-metrics.json"),
    )?)?)
}
async fn video_helper(app: Arc<App>, run: &Value, directory: &Path, stage: &str) -> Result<()> {
    let c = &run["execution"]["image_config"];
    let job = json!({"stage":stage,"directory":directory,"source":run["video_extension"]["attachment"]["path"],"target_frames":run["target_video_frames"],"ffmpeg":crate::source_dir().join("node_modules/ffmpeg-static/ffmpeg.exe"),"width":c["width"],"height":c["height"],"cpu_threads":c["cpu_threads"],"generated_audio":!c["audio_vae_path"].as_str().unwrap_or("").is_empty()});
    let file = directory.join(format!("video-{stage}.json"));
    std::fs::write(&file, serde_json::to_vec_pretty(&job)?)?;
    worker(
        app,
        run,
        &format!("video-{stage}"),
        c["editor_command"].clone(),
        json!([crate::source_dir().join("bridge/video-edit.py"), file]),
        directory,
    )
    .await
}
fn separate_devices(planner:&Value,render:&Value)->bool {
    let Some(render_gpu)=render["gpu_ids"].as_array().filter(|a|!a.is_empty())else{return false};
    planner["gpu_ids"].as_array().is_some_and(|a|!crate::media_queue::overlap(a,render_gpu))
}
pub(crate) fn manually_pinned(app:&App,model:&Value)->bool{
    model["instance_id"].as_str().and_then(|id|app.get("instance",id).ok())
        .is_some_and(|r|r["status"]=="running"&&r["auto_release"]==false)
}
async fn handoff(app:&Arc<App>,run:&Value)->Result<()>{
    let _admission=app.admission_lock.lock().await;
    let id=text(run,"id")?;
    let model=app.get("model",text(run,"model_id")?)?;
    let planner=app.get("profile",text(&model,"profile_id")?)?;
    let render=&run["execution"]["profile"];
    let pinned=manually_pinned(app,&model);
    if separate_devices(&planner,render)&&(pinned||run["execution"]["image_config"]["restore_llm"]==true){
        let admission=crate::server::resource_plan(app,&[render.clone()],Some(id)).await?;
        if admission["admissible"]==true || admission["only_gpu_pressure"]==true
            || (pinned && admission["wait_reason"].is_object()) {
            {
                let mut leases=app.image_leases.lock().await;
                let lease=leases.get_mut(id).context("Missing media reservation")?;
                lease.planner_released=true;
                lease.gpu_ids=render["gpu_ids"].as_array().cloned().unwrap_or_default();
                lease.ram=render["ram_gib"].as_f64().unwrap_or(0.);
                lease.cpu=render["cpu_threads"].as_u64().unwrap_or(0);
            }
            update(app,id,json!({"planner_kept_loaded":true,"llm_unloaded":false,"admission":admission})).await?;
            app.media_queue_wake.notify_one();
            return Ok(())
        }
    }
    if pinned {bail!("A manually started planner still owns its resources. Stop it or free enough RAM and threads for both models.")}
    phase(app,id,"unloading").await?;
    crate::engines::stop_owned(app.clone(),text(run,"model_id")?).await?;
    update(app,id,json!({"llm_unloaded":true,"hardware_after_unload":app.fresh_inventory().await})).await?;
    Ok(())
}
async fn execute(app: Arc<App>, run: &Value) -> Result<Value> {
    let id = text(run, "id")?;
    let mut effective_config = run["execution"]["image_config"].clone();
    let exposure_eligible = run["kind"] == "video" && run["source"].is_object()
        && run["video_reference_end"].is_object() && crate::media_catalog::supports_reference_exposure(&effective_config);
    let automatic = effective_config["h3_reference_exposure"].is_null();
    let prompt = planner(app.clone(), run, exposure_eligible && automatic).await?;
    let planned = app.get("run", id)?;
    let edit_plan = planned["edit_plan"].clone();
    if edit_plan["operation"]=="recolor" && effective_config["applied_extensions"].as_array().is_some_and(|v|!v.is_empty()) {
        effective_config["skipped_extensions"]=effective_config["applied_extensions"].take();
        effective_config["applied_extensions"]=json!([]);
    }
    if run["kind"] == "video" {
        if planned["video_scene"].is_object() {
            effective_config["planned_lighting"] = planned["video_scene"]["lighting"].clone();
            effective_config["planned_camera"] = planned["video_scene"]["camera"].clone();
        }
        effective_config["reference_exposure_used"] = json!(crate::media_catalog::reference_exposure_used(
            &effective_config["h3_reference_exposure"], &planned["video_scene"], exposure_eligible &&
            (!automatic || crate::media_catalog::runtime_feature(&effective_config,"h3_reference_exposure_auto"))));
    }
    handoff(&app,run).await?;
    let directory = root().join("outputs").join(id);
    std::fs::create_dir_all(&directory)?;
    if run["video_extension"].is_object() {
        phase(&app, id, "video_prepare").await?;
        video_helper(app.clone(), run, &directory, "prepare").await?;
        let preparation: Value =
            serde_json::from_slice(&std::fs::read(directory.join("video-preparation.json"))?)?;
        effective_config["video_frames"] = preparation["continuation_frames"].clone();
        update(&app, id, json!({"video_preparation":preparation})).await?;
    }
    if effective_config["applied_extensions"].as_array().is_some_and(|v|v.iter().any(|x|x["kind"]=="h3-prompt-lint")) {
        let input=directory.join("prompt-lint-input.json");
        let output=directory.join("prompt-lint-result.json");
        let mode=if run["video_reference_end"].is_object() {"fl2va"} else if run["source"].is_object() || run["video_extension"].is_object() {"i2va"} else {"t2va"};
        let frames=effective_config["video_frames"].as_u64().context("Missing H3 frame count")?;
        std::fs::write(&input,serde_json::to_vec(&json!({"prompt":prompt,"mode":mode,"duration_seconds":frames as f64/24.0}))?)?;
        worker(app.clone(),run,"prompt-lint",effective_config["editor_command"].clone(),
               json!(["-I",crate::media_extensions::lint_script(),input,output]),&directory).await?;
        effective_config["prompt_lint_warnings"]=serde_json::from_slice(&std::fs::read(output)?)?;
    }
    let c = &effective_config;
    update(&app, id, json!({"effective_media_config":c})).await?;
    let video = run["kind"] == "video";
    let output = directory.join(if video { "video.mp4" } else { "image.png" });
    let mut width = c["width"].as_u64().unwrap() as u32;
    let mut height = c["height"].as_u64().unwrap() as u32;
    let mut metrics = Value::Null;
    if edit_plan.is_object() {
        phase(&app, id, "masking").await?;
        metrics = edit_worker(app.clone(), run, &directory, &edit_plan, "prepare").await?;
        width = metrics["width"].as_u64().context("Missing edit width")? as u32;
        height = metrics["height"].as_u64().context("Missing edit height")? as u32;
        if !(256..=1536).contains(&width)
            || !(256..=1536).contains(&height)
            || width % 64 != 0
            || height % 64 != 0
        {
            bail!("Previous image dimensions are not supported by this local workflow")
        }
    }
    let seed = if c["seed"] == json!(-1) {
        (uuid::Uuid::new_v4().as_u128() & 0x7fffffff) as i64
    } else {
        c["seed"].as_i64().unwrap()
    };
    if edit_plan["operation"] != "recolor" {
        phase(&app, id, "rendering").await?;
        let render_output = if edit_plan.is_object() {
            directory.join("render.png")
        } else {
            output.clone()
        };
        let render_output = if video {
            directory.join("render.avi")
        } else {
            render_output
        };
        let reference = if edit_plan.is_object() {
            Some(directory.join("source.png"))
        } else if run["video_extension"].is_object() {
            Some(directory.join("last-frame.png"))
        } else {
            run["source"]["attachment"]["path"]
                .as_str()
                .map(std::path::PathBuf::from)
        };
        let mask = directory.join("mask.png");
        let mut args = crate::media_catalog::render_args(
            c,
            &prompt,
            &render_output,
            width,
            height,
            seed,
            reference.as_deref(),
            if edit_plan.is_object() {
                Some(&mask)
            } else {
                None
            },
        );
        if let Some(records)=c["applied_extensions"].as_array() {
            crate::media_extensions::apply_loras(args.as_array_mut().unwrap(),records)?;
        }
        if video && run["video_reference_end"].is_object() {
            let end = Path::new(text(&run["video_reference_end"]["attachment"], "path")?);
            args.as_array_mut().unwrap().extend(crate::media_catalog::video_end_args(
                c["reference_exposure_used"] == true && reference.is_some(), end,
            ));
        }
        update(&app,id,json!({"seed":seed,"output_path":output,"render_command":{"command":c["command"],"args":args}})).await?;
        worker(
            app.clone(),
            run,
            "render",
            c["command"].clone(),
            args,
            &directory,
        )
        .await?;
        if edit_plan.is_object() {
            validate_png(&std::fs::read(&render_output)?, width, height)?;
            phase(&app, id, "compositing").await?;
            metrics = edit_worker(app.clone(), run, &directory, &edit_plan, "composite").await?;
        }
    }
    if video {
        phase(&app, id, "encoding").await?;
        let ffmpeg = crate::source_dir().join("node_modules/ffmpeg-static/ffmpeg.exe");
        if run["video_extension"].is_object() {
            video_helper(app.clone(), run, &directory, "finish").await?;
        } else {
            let mut encode_args = json!([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-y",
                "-i",
                directory.join("render.avi"),
                "-threads",
                c["cpu_threads"].to_string(),
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart"
            ]);
            if let Some(frames) = run["target_video_frames"].as_u64() {
                encode_args.as_array_mut().unwrap().extend([
                    json!("-frames:v"),
                    json!(frames.to_string()),
                    json!("-t"),
                    json!((frames as f64 / 24.0).to_string()),
                ]);
            }
            encode_args.as_array_mut().unwrap().push(json!(output));
            worker(
                app.clone(),
                run,
                "encode",
                json!(ffmpeg),
                encode_args,
                &directory,
            )
            .await?;
        }
        if std::fs::metadata(&output)?.len() < 1024 {
            bail!("Video output is empty")
        }
        worker(
            app.clone(),
            run,
            "verify",
            json!(ffmpeg),
            json!([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-xerror",
                "-i",
                output,
                "-f",
                "null",
                "-"
            ]),
            &directory,
        )
        .await?;
    } else {
        validate_png(
            &std::fs::read(&output).context("Image engine produced no image.png")?,
            width,
            height,
        )?;
    }
    if edit_plan.is_object()
        && (metrics["outside_mask_changed_pixels"] != 0
            || metrics["changed_pixels"].as_u64().unwrap_or(0) < 16)
    {
        bail!("Image edit did not pass pixel preservation checks")
    }
    update(&app,id,json!({"edit_metrics":metrics,"output_path":output,"output_width":width,"output_height":height})).await?;
    let attachment = crate::attachments::import(&app, &output).await?;
    let provenance = json!({"request":run["request"],"prompt":prompt,"seed":seed,"source":run["source"],"edit_plan":edit_plan,"edit_metrics":metrics,"config":c,"model":run["execution"]["model"],"artifact":attachment,"created_at":now()});
    std::fs::write(
        directory.join("generation.json"),
        serde_json::to_vec_pretty(&provenance)?,
    )?;
    Ok(attachment)
}
fn validate_png(bytes: &[u8], width: u32, height: u32) -> Result<()> {
    if bytes.len() < 33
        || bytes.len() > 20 * 1024 * 1024
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[12..16] != b"IHDR"
        || u32::from_be_bytes(bytes[16..20].try_into().unwrap()) != width
        || u32::from_be_bytes(bytes[20..24].try_into().unwrap()) != height
    {
        bail!("Image output is not a PNG with the requested dimensions")
    }
    let mut offset = 8;
    let mut data = false;
    while offset + 12 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(12)
            .and_then(|v| v.checked_add(size))
            .context("Invalid PNG length")?;
        if end > bytes.len() {
            bail!("Truncated PNG output")
        }
        let kind = &bytes[offset + 4..offset + 8];
        let crc = u32::from_be_bytes(bytes[end - 4..end].try_into().unwrap());
        if crc32fast::hash(&bytes[offset + 4..end - 4]) != crc {
            bail!("Corrupt PNG output (CRC)")
        }
        if kind == b"IDAT" && size > 0 {
            data = true;
        }
        if kind == b"IEND" {
            if size == 0 && data && end == bytes.len() {
                return Ok(());
            }
            bail!("Invalid PNG end marker")
        }
        offset = end;
    }
    bail!("PNG output is incomplete")
}
async fn workflow(app: Arc<App>, run: Value, flag: Arc<AtomicBool>) {
    let id = run["id"].as_str().unwrap();
    let result = tokio::select! {
        r=tokio::time::timeout(Duration::from_secs(run["execution"]["image_config"]["timeout_seconds"].as_u64().unwrap_or(1200)),execute(app.clone(),&run))=>match r{Ok(r)=>r,Err(_)=>Err(anyhow::anyhow!("Image workflow deadline exceeded"))},
        _=cancelled(flag.clone())=>Err(anyhow::anyhow!("Cancelled by user"))
    };
    let kept=app.image_leases.lock().await.get(id).is_some_and(|l|l.planner_released);
    let pinned=app.get("model",run["model_id"].as_str().unwrap_or("")).is_ok_and(|m|manually_pinned(&app,&m));
    let own_planner=!kept&&!pinned;
    let _ = phase(&app, id, if own_planner{"restoring"}else{"cleanup"}).await;
    let mut cleanup_error = None;
    {
        let mut workers = app.workers.lock().await;
        for worker_id in [
            format!("{id}-prepare"),
            format!("{id}-render"),
            format!("{id}-composite"),
            format!("{id}-encode"),
            format!("{id}-verify"),
            format!("{id}-video-prepare"),
            format!("{id}-video-finish"),
        ] {
            if let Some(w) = workers.remove(&worker_id) {
                if let Err(e) = w.stop() {
                    cleanup_error = Some(e.to_string());
                }
                if let Ok(mut instance) = app.get("instance", &worker_id) {
                    instance["status"] = json!(if result.is_ok() {
                        "completed"
                    } else {
                        "stopped"
                    });
                    let _ = app.save("instance", instance);
                }
            }
        }
    }
    // stop_owned also catches a prompt-runtime startup cancelled halfway through.
    if own_planner {
        if let Err(e)=crate::engines::stop_owned(app.clone(),run["model_id"].as_str().unwrap()).await{cleanup_error=Some(e.to_string());}
    }
    let mut restored = false;
    if own_planner && cleanup_error.is_none() && !app.closing.load(Ordering::SeqCst) && run["execution"]["image_config"]["restore_llm"] == true {
        match crate::engines::start_owned(app.clone(), run["model_id"].as_str().unwrap(), Some(id))
            .await
        {
            Ok(_) => restored = true,
            Err(e) => cleanup_error = Some(format!("Could not restore the LLM: {e}")),
        }
    }
    let finish=async {
        let _serial=app.chat_lock.lock().await;
        let mut r=app.get("run",id)?;
        if r["stage_wait_reason"].is_object(){r["last_stage_wait_reason"]=r["stage_wait_reason"].take();}
        r["phase"]=json!("done");r["completed_at"]=json!(now());r["llm_restored"]=json!(restored);
        r["stages"].as_array_mut().unwrap().push(json!({"phase":"done","at":now()}));
        if let Some(e)=&cleanup_error{r["cleanup_error"]=json!(e);}
        let mut chat=app.get("chat",text(&run,"chat_id")?)?;
        match result {
            Ok(attachment)=>{
                r["status"]=json!("completed");r["artifacts"]=json!([attachment.clone()]);
                let language=app.get("settings","system")?["language"].clone();
                let content=if run["video_extension"].is_object(){if language=="en"{"Video extended locally. The original file is preserved."}else{"Видео продлено локально. Исходный файл сохранён."}}else if run["kind"]=="video"{if language=="en"{"Video generated locally."}else{"Видео создано локально."}}else if run["source"].is_object(){if language=="en"{"Previous image edited locally. Pixels outside the object mask are unchanged."}else{"Предыдущее изображение изменено локально. Пиксели вне маски объекта сохранены без изменений."}}else{if language=="en"{"Image generated locally."}else{"Изображение создано локально."}};
                chat["messages"].as_array_mut().unwrap().push(json!({"role":"assistant","kind":run["kind"],"content":content,"run_id":id,"model_id":run["model_id"],"at":now(),"attachments":[attachment],"generation":{"source":run["source"],"edit_plan":r["edit_plan"],"edit_metrics":r["edit_metrics"],"width":r["output_width"],"height":r["output_height"],"prompt":r["prompt"],"seed":r["seed"],"config":r["effective_media_config"],"video_reference_end":run["video_reference_end"],"video_extension":run["video_extension"],"target_video_frames":run["target_video_frames"],"video_preparation":r["video_preparation"],"llm_restored":restored,"planner_kept_loaded":kept,"cleanup_error":cleanup_error}}));
            },Err(e)=>{r["status"]=json!(if flag.load(Ordering::SeqCst){"cancelled"}else{"failed"});r["error"]=json!(e.to_string());}
        }
        app.store.lock().unwrap().save_batch(vec![("run",r),("chat",chat)])?;Ok::<(),anyhow::Error>(())
    }.await;
    if let Err(e) = finish {
        eprintln!("Image workflow {id}: could not persist completion: {e}");
    }
    app.image_leases.lock().await.remove(id);
    app.media_queue_wake.notify_one();
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn planner_can_be_handed_back_only_on_disjoint_compute_devices(){
        let profile=|devices:Vec<&str>|json!({"gpu_ids":devices});
        assert!(separate_devices(&profile(vec!["P40"]),&profile(vec!["1080"])));
        assert!(separate_devices(&profile(vec![]),&profile(vec!["1080"])));
        assert!(!separate_devices(&profile(vec!["P40","1080"]),&profile(vec!["1080"])));
        assert!(!separate_devices(&profile(vec!["P40"]),&profile(vec![])));
    }
    #[test]
    fn lighting_plan_keeps_the_actual_prompt_and_text_only_responses() {
        let parsed = parse_video_scene(r#"{"lighting":"changing","camera":"fixed","prompt":"The lamp goes out and lights again."}"#).unwrap();
        assert_eq!(parsed["prompt"], "The lamp goes out and lights again.");
        assert_eq!(parsed["lighting"], "changing");
        assert!(parse_video_scene(r#"{"lighting":"constant","camera":"fixed"}"#).is_err());
        let fallback = parse_video_scene("Locked tripod in soft daylight.").unwrap();
        assert_eq!(fallback["prompt"], "Locked tripod in soft daylight.");
        assert!(fallback["lighting"].is_null());
    }
    #[test]
    fn invalid_output_is_never_delivered() {
        assert!(validate_png(b"not an image", 512, 512).is_err());
        let mut b = vec![0; 33];
        b[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        b[12..16].copy_from_slice(b"IHDR");
        b[16..20].copy_from_slice(&512u32.to_be_bytes());
        b[20..24].copy_from_slice(&256u32.to_be_bytes());
        assert!(validate_png(&b, 512, 512).is_err());
    }
}

const VIDEO_INSTRUCTIONS: &str = "Write an English prompt for MiniMax H3 video generation, maximum 200 words. Preserve the user's action order, timing, camera movement and lighting, including any scheduled fade, flash, shadow, changing light color or exposure. Describe the people and their object interactions. Attached images in order are the FIRST FRAME and optional LAST FRAME. Match their identities, clothes, objects and framing at their respective times. Describe only the requested scene and sound. Give no claims of completion or tool calls.";
const VIDEO_SCENE_FORMAT: &str = r#" Return one JSON object with three strings: "prompt" (the full English scene including the requested lighting), "lighting" ("constant", "changing", or "unspecified"), and "camera" ("fixed", "moving", or "unspecified"). Classify the user's description and the shot between the references. Lighting changes at ANY time, even if it returns to the starting light, count as changing. A fixed exposure setting alone does not imply a constant light source. A moving actor does not imply a moving camera. Use unspecified when the description and references do not establish the choice. Never invent constant lighting or a fixed camera to qualify for a mode."#;

fn parse_video_scene(reply: &str) -> Result<Value> {
    let reply = reply.strip_prefix("```json").or_else(|| reply.strip_prefix("```"))
        .and_then(|s| s.trim_end().strip_suffix("```")).unwrap_or(reply).trim();
    // Text-only compatible planners may return their usual prompt. Keep it usable;
    // without a classified scene the renderer uses the ordinary sampling path.
    if reply.is_empty() { bail!("Video scene plan is empty"); }
    if !reply.starts_with('{') { return Ok(json!({"prompt":reply})); }
    let scene: Value = serde_json::from_str(reply).context("Invalid video scene plan")?;
    if scene["prompt"].as_str().is_none_or(|s| s.trim().is_empty() || s.contains('\0')) {
        bail!("Video scene plan is missing the prompt")
    }
    Ok(scene)
}
const REFERENCE_EDIT_INSTRUCTIONS: &str = r#"Examine the attached image and plan the requested localized edit. Output only JSON: {"operation":"inpaint","target":"object name","color":null,"box":[left,top,right,bottom],"point":[x,y],"prompt":"Concise English editing instruction for FLUX.2 Klein","strength":0.75}. Coordinates are integers normalized to 0..1000. Enclose the complete physical object to change, including its surface decorations; point strictly inside it. For stripes or patterns, select the whole cup, wall or other owning object, not one stripe. The prompt must be an imperative editing instruction, e.g. 'Add thick red horizontal stripes to the blue mug. Keep its shape, lighting and all other objects unchanged.' Describe exactly the user's change and preserve all other details. No Stable Diffusion weighting syntax. The controller preserves pixels outside the mask. Never claim completion or invent paths. If no single object can be identified, return {"operation":"unsupported"}."#;
