use crate::{envelope, hardware, root, source_dir, store::Store, VERSION};
use anyhow::{bail, Context, Result};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;

pub struct App {
    pub closing: std::sync::atomic::AtomicBool,
    exit: tokio::sync::watch::Sender<bool>,
    pub admission_lock: tokio::sync::Mutex<()>,
    pub image_leases: tokio::sync::Mutex<std::collections::BTreeMap<String,crate::image_jobs::Lease>>,
    pub exposure_lock: tokio::sync::Mutex<()>,
    pub media_queue_wake: tokio::sync::Notify,
    pub runtime_lock: tokio::sync::Mutex<()>,
    pub chat_lock: tokio::sync::Mutex<()>,
    pub store: Mutex<Store>,
    pub token: String,
    pub inventory: tokio::sync::Mutex<(Instant, Value)>,
    pub workers: tokio::sync::Mutex<std::collections::BTreeMap<String, crate::workers::Worker>>,
}
#[derive(Deserialize)]
pub struct Rpc {
    pub method: String,
    #[serde(default)]
    pub params: Value,
}
impl App {
    async fn instances(&self) -> Result<Value> {
        let mut records = self.list("instance")?;
        let workers = self.workers.lock().await;
        for record in records.as_array_mut().unwrap() {
            if let Some(worker) = workers.get(record["id"].as_str().unwrap_or("")) {
                record["usage"] = worker.usage()
            }
        }
        Ok(records)
    }
    pub(crate) async fn reap(&self) -> Result<()> {
        let mut workers = self.workers.lock().await;
        let finished: Vec<_> = workers
            .iter()
            .filter_map(|(id, w)| w.exit_code().ok().flatten().map(|code| (id.clone(), code)))
            .collect();
        for (id, code) in finished {
            let mut record = self.get("instance", &id)?;
            record["status"] = json!(if code == 0 { "completed" } else { "failed" });
            record["exit_code"] = json!(code);
            self.save("instance", record)?;
            workers.remove(&id);
        }
        Ok(())
    }
    pub async fn fresh_inventory(&self) -> Value {
        let value = hardware::inventory().await;
        *self.inventory.lock().await = (Instant::now(), value.clone());
        value
    }
    pub async fn inventory(&self) -> Value {
        let mut cached = self.inventory.lock().await;
        if cached.0.elapsed() > Duration::from_secs(5) {
            cached.1 = hardware::inventory().await;
            cached.0 = Instant::now();
        }
        cached.1.clone()
    }
    pub(crate) fn list(&self, kind: &str) -> Result<Value> {
        Ok(json!(self.store.lock().unwrap().list(kind)?))
    }
    pub(crate) fn get(&self, kind: &str, id: &str) -> Result<Value> {
        self.store.lock().unwrap().get(kind, id)
    }
    pub(crate) fn save(&self, kind: &str, value: Value) -> Result<Value> {
        let revision = value["revision"].as_i64();
        self.store.lock().unwrap().save(kind, value, revision)
    }
}
fn text<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Требуется {key}"))
}
fn credential(name: &str) -> Result<keyring::Entry> {
    crate::safe_id(name)?;
    let namespace = std::fs::read_to_string(root().join("data/credential-namespace"))
        .unwrap_or_else(|_| format!("{:x}", Sha256::digest(root().to_string_lossy().as_bytes())));
    let service = format!("StruveLLM-{}", namespace.trim());
    Ok(keyring::Entry::new(&service, name)?)
}
fn validate_connection(p: &Value) -> Result<()> {
    crate::safe_id(text(p, "id")?)?;
    text(p, "name")?;
    if !p["timeout_seconds"].is_null()
        && !p["timeout_seconds"]
            .as_u64()
            .map(|n| (1..=180).contains(&n))
            .unwrap_or(false)
    {
        bail!("Connection timeout must be 1..180 seconds")
    }
    match text(p, "transport")? {
        "stdio" | "cli" | "ide" => {
            text(p, "command")?;
            if !p["args"].is_array() || !p["args"].as_array().unwrap().iter().all(Value::is_string)
            {
                bail!("args должен быть массивом строк")
            }
        }
        "http" => {
            let u = reqwest::Url::parse(text(p, "url")?)?;
            if !["http", "https"].contains(&u.scheme())
                || !u.username().is_empty()
                || u.password().is_some()
            {
                bail!("Требуется HTTP(S) URL без встроенных учётных данных")
            }
        }
        _ => bail!("Supported transports: stdio, http, cli, ide"),
    }
    if p.get("env").is_some() || p.get("headers").is_some() {
        bail!("Секреты не сохраняются в конфигурации. Используйте имена переменных env_names / bearer_env")
    }
    for key in ["allowed_tools", "project_ids", "env_names"] {
        if !p[key].is_array() || !p[key].as_array().unwrap().iter().all(Value::is_string) {
            bail!("{key} должен быть массивом строк")
        }
    }
    if let Some(environment) = p.get("environment") {
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
            bail!("environment must contain string values")
        }
    }
    if matches!(p["transport"].as_str(), Some("cli" | "ide")) {
        if !p["tools"]
            .as_array()
            .map(|a| {
                !a.is_empty()
                    && a.iter().all(|t| {
                        t["name"].is_string()
                            && t["args"]
                                .as_array()
                                .map(|args| args.iter().all(Value::is_string))
                                .unwrap_or(false)
                            && t["inputSchema"].is_object()
                    })
            })
            .unwrap_or(false)
        {
            bail!("CLI / IDE tools require name, args and inputSchema")
        }
    }
    Ok(())
}
pub async fn bridge(
    config: Value,
    operation: &str,
    tool: Option<&str>,
    arguments: Value,
) -> Result<Value> {
    let mut resolved = serde_json::Map::new();
    let mut names: Vec<String> = config["env_names"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    if let Some(name) = config["bearer_env"].as_str().filter(|s| !s.is_empty()) {
        names.push(name.into())
    }
    for name in names {
        let value = std::env::var(&name)
            .ok()
            .or_else(|| credential(&name).ok().and_then(|c| c.get_password().ok()))
            .ok_or_else(|| {
                anyhow::anyhow!("Секрет не настроен: {name}. Добавьте его в настройках приложения.")
            })?;
        resolved.insert(name, json!(value));
    }
    let timeout = config["timeout_seconds"]
        .as_u64()
        .unwrap_or(30)
        .clamp(1, 180)
        + 8;
    let node = crate::app_dir().join("node.exe");
    let executable = if node.exists() {
        node.into_os_string()
    } else {
        "node".into()
    };
    let mut command = tokio::process::Command::new(executable);
    command
        .arg(source_dir().join("bridge/client.mjs"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .context("Не удалось запустить MCP-адаптер Node")?;
    let _tree = crate::workers::ProcessTreeGuard::attach(
        child
            .id()
            .ok_or_else(|| anyhow::anyhow!("MCP adapter exited before startup"))?,
    )
    .context("Could not assign the MCP adapter process tree")?;
    let mut input = child.stdin.take().unwrap();
    input.write_all(json!({"config":config,"secrets":resolved,"operation":operation,"tool":tool,"arguments":arguments}).to_string().as_bytes()).await?;
    input.shutdown().await?;
    drop(input);
    let output = tokio::time::timeout(Duration::from_secs(timeout), child.wait_with_output())
        .await
        .context("Таймаут MCP-адаптера")??;
    if output.stdout.len() > 64 * 1024 * 1024 {
        bail!("Слишком большой ответ MCP")
    }
    let result: Value =
        serde_json::from_slice(&output.stdout).context("MCP-адаптер вернул некорректный ответ")?;
    if result["ok"] != true {
        bail!("{}", result["error"].as_str().unwrap_or("Ошибка MCP"))
    }
    Ok(result["data"].clone())
}
pub(crate) async fn resource_plan(app:&App,profiles:&[Value],owner:Option<&str>)->Result<Value>{
    let leases:Vec<_>=app.image_leases.lock().await.values().cloned().collect();
    let inventory=app.fresh_inventory().await;
    let workers=app.workers.lock().await;
    let active=workers.values().map(|w|w.allocation()).collect::<Vec<_>>();
    let pending=crate::media_queue::outstanding(app,&leases,&workers,owner);
    Ok(hardware::plan_reserved(&inventory,&app.get("settings","system")?,profiles,&active,pending))
}
#[derive(Debug)]
pub(crate) struct TemporaryResourceShortfall(pub Value);
impl std::fmt::Display for TemporaryResourceShortfall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ресурсы недоступны: {}", self.0["reasons"])
    }
}
impl std::error::Error for TemporaryResourceShortfall {}

pub async fn start_instance(app: Arc<App>, p: Value) -> Result<Value> {
    crate::image_jobs::check_instance(&app, &p).await?;
    let id = || text(&p, "id");

    crate::workers::validate(&p)?;
    let profile = app.get("profile", text(&p, "profile_id")?)?;
    let mut spec = p.clone();
    spec.as_object_mut().unwrap().remove("dry_run");
    let leases:Vec<_>=app.image_leases.lock().await.values().cloned().collect();
    let mut workers = app.workers.lock().await;
    if app.closing.load(std::sync::atomic::Ordering::SeqCst) { bail!("Controller is shutting down"); }
    if let Ok(existing) = app.get("instance", id()?) {
        if existing["spec"] == spec {
            return Ok(existing);
        }
        bail!("ID экземпляра уже использован с другой спецификацией")
    }
    let inventory = app.fresh_inventory().await;
    let active: Vec<_> = workers.values().map(|w| w.allocation()).collect();
    let pending=crate::media_queue::outstanding(&app,&leases,&workers,p["image_owner"].as_str());
    let plan=hardware::plan_reserved(&inventory,&app.get("settings","system")?,&[profile.clone()],&active,pending);
    if p["dry_run"] == true {
        return Ok(
            json!({"dry_run":true,"plan":plan,"profile_revision":profile["revision"],"enforcement":crate::workers::enforcement(&profile,inventory["logical_cpus"].as_u64().unwrap_or(1) as usize)}),
        );
    }
    if plan["admissible"] != true {
        if plan["wait_reason"].is_object() {
            return Err(TemporaryResourceShortfall(plan).into());
        }
        bail!("Ресурсы недоступны: {}", plan["reasons"])
    }
    let mut record=app.save("instance",json!({"id":id()?,"name":p.get("name").cloned().unwrap_or_else(||p["id"].clone()),"profile_id":profile["id"],"profile_revision":profile["revision"],"profile":profile,"spec":spec,"status":"starting","created_at":chrono::Utc::now().to_rfc3339()}))?;
    match crate::workers::start(
        &p,
        profile.clone(),
        inventory["logical_cpus"].as_u64().unwrap_or(1) as usize,
    ) {
        Ok(worker) => {
            record["status"] = json!("running");
            record["pid"] = json!(worker.pid);
            record["enforcement"] = crate::workers::enforcement(
                &profile,
                inventory["logical_cpus"].as_u64().unwrap_or(1) as usize,
            );
            let saved = app.save("instance", record)?;
            workers.insert(id()?.into(), worker);
            Ok(saved)
        }
        Err(e) => {
            record["status"] = json!("failed");
            record["error"] = json!(e.to_string());
            app.save("instance", record)?;
            Err(e)
        }
    }
}
fn model_profile(app: &App, model: &Value) -> Result<Value> {
    if let Some(draft) = model.get("resource_profile").filter(|v| !v.is_null()) {
        if !draft.is_object() {
            bail!("Resource profile must be an object")
        }
        let model_id = text(model, "id")?;
        let id = format!(
            "model-resources-{}",
            &format!("{:x}", Sha256::digest(model_id.as_bytes()))[..16]
        );
        if let Ok(old) = app.get("profile", &id) {
            if old["owner_model_id"] != model_id {
                bail!("Resource profile belongs to another model")
            }
        }
        let mut profile = draft.clone();
        profile["id"] = json!(id);
        profile["owner_model_id"] = json!(model_id);
        profile["name"] = json!(format!("{} · resources", text(model, "name")?));
        profile["kind"] = json!("inference");
        hardware::validate_profile(&profile)?;
        return Ok(profile);
    }
    let profile = app.get("profile", text(model, "profile_id")?)?;
    if profile["owner_model_id"].is_string() && profile["owner_model_id"] != model["id"] {
        bail!("Choose a shared profile or this model's individual resources")
    }
    Ok(profile)
}
pub async fn dispatch(app: Arc<App>, r: Rpc) -> Result<Value> {
    app.reap().await?;
    // Serialize admission, not execution. An image lease persists between its stages.
    let admission = app.clone();
    let _admission = if matches!(
        r.method.as_str(),
        "controller.shutdown" | "chat.send"
            | "run.cancel"
            | "model.start"
            | "model.stop"
            | "instance.start"
            | "instance.stop"
            | "model.save"
            | "profile.save"
            | "settings.save"
            | "image.config.save"
            | "media.save"
            | "media.defaults.save"
            | "media.archive"
            | "chat.save"
    ) {
        Some(admission.admission_lock.lock().await)
    } else {
        None
    };
    if r.method == "controller.shutdown" {
        app.closing.store(true,std::sync::atomic::Ordering::SeqCst);
        let result=shutdown_workers(&app).await;
        if result.is_ok(){app.exit.send_replace(true);}else{app.closing.store(false,std::sync::atomic::Ordering::SeqCst);app.media_queue_wake.notify_one()}
        return result;
    }
    if app.closing.load(std::sync::atomic::Ordering::SeqCst){bail!("Controller is shutting down")}
    if matches!(
        r.method.as_str(),
        "model.save" | "profile.save" | "settings.save" | "chat.save"
    ) && !app.image_leases.lock().await.is_empty()
    {
        bail!("Wait for image workflow cleanup before changing models, resources or chat history")
    }
    let p = r.params;
    let id = || text(&p, "id");
    match r.method.as_str() {
        "ping" => Ok(json!({"version":VERSION,"pid":std::process::id(),"root":root()})),
        "doctor" => Ok(
            json!({"version":VERSION,"controller":"ready","root":root(),"models_directory":crate::models_dir(&app.get("settings","system")?),"database":root().join("data/struve.sqlite"),"auth":"local_token","models_registered":app.list("model")?.as_array().unwrap().len(),"training_adapter":"not_connected","inference_protocols":["ollama","openai-compatible"],"managed_processes":"Windows Job Objects: RAM and CPU caps","mcp_bridge":source_dir().join("bridge/client.mjs").exists(),"upstream":"Strata v0.2.15 c77572806996349b49995e4f98d41b5294d36a21"}),
        ),
        "snapshot" => {
            let inventory = app.inventory().await;
            let settings = app.get("settings", "system")?;
            let models = crate::models_dir(&settings);
            let profiles = app.list("profile")?;
            let validity:Vec<_>=profiles.as_array().unwrap().iter().map(|p|json!({"id":p["id"],"plan":hardware::plan(&inventory,&settings,&[p.clone()])})).collect();
            Ok(
                json!({"version":VERSION,"root":root(),"paths":{"app":crate::app_dir(),"models":models,"models_available":models.is_dir(),"models_volume":hardware::volume_for(inventory["storage"].as_array().unwrap(),&models),"runtime":source_dir(),"data":root().join("data"),"logs":root().join("logs"),"node":crate::app_dir().join("node.exe"),"llama_server":crate::setup::llama_server(),"engines":crate::engines::discover()},"hardware":inventory,"settings":settings,"effective_settings":hardware::effective_settings(&settings,&inventory),"profile_validity":validity,"profiles":profiles,"image_config":crate::image_jobs::config(&app),"media_models":crate::media_catalog::list(&app)?,"media_defaults":crate::media_catalog::defaults(&app),"media_extensions":app.list("media_extension")?,"projects":app.list("project")?,"chats":app.list("chat")?,"runs":crate::agent::list(&app)?,"connections":app.list("mcp")?,"plugins":app.list("plugin")?,"models":app.list("model")?,"instances":app.instances().await?,"events":app.store.lock().unwrap().recent_events(100)?}),
            )
        }
        "library.semantic_status"|"library.semantic"|"library.search"|"library.list"|"library.index"|"library.refresh"|"library.forget"|"library.source"=>{
            let method=r.method;tokio::task::spawn_blocking(move||crate::library::dispatch(&app,&method,&p)).await?
        }
        "setup.scan" => crate::setup::scan(&p).await,
        "setup.gguf" => crate::setup::gguf(&app,&p).await,
        "diagnostics.report" => crate::setup::diagnostics(&app).await,
        "resources.estimate" => crate::meter::estimate(&app,&p).await,
        "resources.history" => Ok(json!(crate::meter::list(&app.store.lock().unwrap().db,p["model_id"].as_str(),p["run_id"].as_str(),p["limit"].as_i64().unwrap_or(60))?)),
        "resources.list" => Ok(app.fresh_inventory().await),
        "resources.plan" => {
            let ids = p["profile_ids"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Требуется profile_ids"))?;
            if ids.is_empty() {
                bail!("Выберите хотя бы один профиль")
            };
            let profiles: Vec<_> = ids
                .iter()
                .map(|v| app.get("profile", v.as_str().unwrap_or("")))
                .collect::<Result<_>>()?;
            resource_plan(&app,&profiles,None).await
        }
        "profile.list" => app.list("profile"),
        "profile.get" => app.get("profile", id()?),
        "profile.save" => {
            if app
                .get("profile", p["id"].as_str().unwrap_or(""))
                .ok()
                .is_some_and(|v| v["owner_media_id"].is_string())
            {
                bail!("Change resources through the media model settings")
            }
            if p["id"] == "image-generation"
                && app
                    .get("profile", "image-generation")
                    .map(|v| v["owner_image_generator"] == true)
                    .unwrap_or(false)
            {
                bail!("Use image generation settings to change this profile")
            }
            hardware::validate_profile(&p)?;
            app.save("profile", p)
        }
        "settings.get" => app.get("settings", "system"),
        "secret.list" => app.list("secret"),
        "secret.save" => {
            let name = text(&p, "id")?;
            credential(name)?.set_password(text(&p, "value")?)?;
            let previous = app.get("secret", name).ok();
            app.save("secret",json!({"id":name,"name":name,"storage":"Windows Credential Manager","revision":previous.and_then(|v|v["revision"].as_i64())}))
        }
        "secret.delete" => {
            let name = id()?;
            credential(name)?.delete_credential()?;
            let store = app.store.lock().unwrap();
            store
                .db
                .execute("DELETE FROM objects WHERE kind='secret' AND id=?", [name])?;
            store.event("secret.delete", name, json!({}))?;
            Ok(json!({"deleted":name}))
        }
        "settings.save" => {
            if p["id"] != "system" {
                bail!("Неизвестные настройки")
            };
            for k in ["ram_reserve_gib", "cpu_reserve_threads"] {
                if p[k].as_f64().filter(|n| *n >= 0.).is_none() {
                    bail!("Некорректный резерв {k}")
                }
            }
            if !p["reserved_gpu_ids"].is_array() {
                bail!("Требуется список reserved_gpu_ids")
            }
            if let Some(mode) = p["reserve_mode"].as_str() {
                if !["auto", "manual"].contains(&mode) {
                    bail!("Invalid reserve mode")
                }
            }
            if p["cpu_reserve_threads"].as_u64().is_none() {
                bail!("CPU reserve must be an integer")
            }
            if let Some(theme) = p["theme"].as_str() {
                if !["dark", "light", "system"].contains(&theme) {
                    bail!("Unsupported theme")
                }
            }
            if let Some(language) = p["language"].as_str() {
                if !["ru", "en"].contains(&language) {
                    bail!("Unsupported interface language")
                }
            }
            if let Some(path) = p["models_directory"].as_str() {
                if path.trim().is_empty() {
                    bail!("Models directory is required")
                }
                let resolved = root().join(path);
                if !resolved.is_dir() {
                    bail!("Models directory must exist")
                }
            }
            app.save("settings", p)
        }
        "project.list" => app.list("project"),
        "project.get" => app.get("project", id()?),
        "project.save" => {
            text(&p, "name")?;
            let path = std::path::Path::new(text(&p, "path")?);
            if !path.is_absolute() || !path.is_dir() {
                bail!("Рабочая папка проекта должна существовать")
            };
            app.save("project", p)
        }
        "attachment.import" => {
            crate::attachments::import(&app, std::path::Path::new(text(&p, "path")?)).await
        }
        "attachment.list" => app.list("attachment"),
        "attachment.get" => app.get("attachment", id()?),
        "media.list" => crate::media_catalog::list(&app),
        "media.extensions.catalog" => crate::media_extensions::catalog(&app),
        "media.extensions.configure" => crate::media_extensions::configure(&app,&p),
        "media.market.list" => crate::market::browse(&app,&p),
        "media.market.refresh" => crate::market::refresh(&app,p["force"]==true).await,
        "media.comfy.save" => crate::comfy_extensions::save(&app,p),
        "media.comfy.start_cpu" => crate::comfy_extensions::start_cpu(app).await,
        "media.comfy.probe" => crate::comfy_extensions::probe(&app,p["id"].as_str()).await,
        "media.comfy.install" => crate::comfy_extensions::install(&app,&p).await,
        "media.comfy.dependencies" => crate::comfy_extensions::dependencies(&app,&p).await,
        "media.comfy.lint" => crate::comfy_extensions::lint(&app,&p).await,
        "media.extensions.install" => tokio::task::spawn_blocking(move || crate::media_extensions::install(&app,&p)).await?,
        "media.get" => app.get("media_model", id()?),
        "media.templates" => Ok(crate::media_catalog::templates(&app)),
        "media.check" => crate::media_catalog::check_status(&app, &p).await,
        "media.save" => crate::media_catalog::save(&app, p).await,
        "media.defaults.get" => Ok(crate::media_catalog::defaults(&app)),
        "media.defaults.save" => crate::media_catalog::save_defaults(&app, p),
        "media.archive" => {
            if !app.image_leases.lock().await.is_empty() {
                bail!("Wait for media workflow cleanup")
            }
            let mut m = app.get("media_model", id()?)?;
            if m["revision"] != p["revision"] {
                bail!("Revision conflict: reload model")
            }
            let d = crate::media_catalog::defaults(&app);
            if ["image", "edit", "video"]
                .iter()
                .any(|role| d[role] == m["id"])
            {
                bail!("Choose another default before archiving this model")
            }
            m["archived"] = json!(true);
            m["enabled"] = json!(false);
            app.save("media_model", m)
        }
        "media.level_exposure" => {
            let run_id = text(&p, "run_id")?.to_owned();
            let profile_id = p["profile_id"]
                .as_str()
                .unwrap_or("cpu-diagnostics")
                .to_owned();
            tokio::spawn(crate::video_exposure::level(app, run_id, profile_id)).await?
        }
        "image.config.get" => Ok(crate::image_jobs::config(&app)),
        "image.config.save" => crate::image_jobs::save(&app, p).await,
        "chat.send" => {
            if crate::image_jobs::requested(&app, &p)? {
                crate::image_jobs::submit(app, p).await
            } else {
                crate::agent::send(app, p).await
            }
        }
        "run.list" => crate::agent::list(&app),
        "run.browser.reply" => crate::agent::browser_reply(app, p).await,
        "run.get" => crate::agent::progress(&app, id()?),
        "run.cancel" => {
            if matches!(
                app.get("run", id()?)?["kind"].as_str(),
                Some("image" | "video")
            ) {
                crate::image_jobs::cancel(app.clone(), id()?).await
            } else {
                crate::agent::cancel(app.clone(), id()?).await
            }
        }
        "run.finish" => {
            if matches!(
                app.get("run", id()?)?["kind"].as_str(),
                Some("image" | "video")
            ) {
                bail!("Image completion is owned by the controller")
            }
            crate::agent::finish(app, p).await
        }
        "run.fail" => {
            if matches!(
                app.get("run", id()?)?["kind"].as_str(),
                Some("image" | "video")
            ) {
                bail!("Image completion is owned by the controller")
            }
            crate::agent::fail(app, p).await
        }
        "memory.export" | "memory.list" | "memory.search" | "memory.add" | "memory.edit" | "memory.forget" =>
            crate::project_memory::dispatch(&app.store.lock().unwrap(), &r.method, &p),
        "chat.context" => {
            let _serial=app.chat_lock.lock().await;
            let project=text(&p,"project_id")?;
            app.get("project",project)?;
            let mut chat=if let Some(id)=p["chat_id"].as_str(){app.get("chat",id)?} else {
                let english=app.get("settings","system")?["language"]=="en";
                json!({"id":format!("chat-{}",uuid::Uuid::new_v4().simple()),"name":if english {"New chat"} else {"Новый диалог"},"project_id":project,"messages":[],"untitled":true})
            };
            if chat["project_id"]!=project {bail!("Chat belongs to another project")}
            let mut model=app.get("model",text(&p,"model_id")?)?;
            chat["context_tokens"]=p.get("tokens").context("Specify tokens or null for auto")?.clone();
            crate::project_memory::apply_context(&chat,&mut model)?;
            if chat["model_id"].is_null(){chat["model_id"]=model["id"].clone()}
            app.save("chat",chat)
        }
        "chat.list" => app.list("chat"),
        "chat.get" => app.get("chat", id()?),
        "chat.save" => {
            app.get("project", text(&p, "project_id")?)?;
            text(&p, "name")?;
            if !p["messages"].is_array() {
                bail!("Требуется массив сообщений")
            };
            app.save("chat", p)
        }
        "model.list" => app.list("model"),
        "model.get" => app.get("model", id()?),
        "model.start" => crate::engines::start(app.clone(), id()?).await,
        "model.stop" => crate::engines::stop(app.clone(), id()?).await,
        "model.plan" => {
            let profile = model_profile(&app, &p)?;
            resource_plan(&app,&[profile],None).await
        }
        "instance.list" => app.instances().await,
        "instance.get" => {
            let mut record = app.get("instance", id()?)?;
            if let Some(worker) = app.workers.lock().await.get(id()?) {
                record["usage"] = worker.usage()
            }
            Ok(record)
        }
        "instance.logs" => {
            let record = app.get("instance", id()?)?;
            let lines = p["lines"].as_u64().unwrap_or(100).min(500) as usize;
            let content = crate::workers::tail(
                &root().join(format!("logs/instance-{}.log", text(&record, "id")?)),
                lines,
            )?;
            Ok(json!({"id":id()?,"text":content}))
        }
        "instance.stop" => {
            // Keep chat/model state consistent when the generic process list or CLI stops them.
            let instance_id = id()?;
            if let Some(owner) = app
                .get("instance", instance_id)
                .ok()
                .and_then(|v| v["spec"]["image_owner"].as_str().map(String::from))
            {
                let owns_instance=app.image_leases.lock().await.get(&owner).is_some_and(|lease|{
                    !(lease.planner_released&&app.get("model",&lease.model_id).is_ok_and(|m|m["instance_id"]==instance_id))
                });
                if owns_instance {
                    return crate::image_jobs::cancel(app.clone(), &owner).await;
                }
            }
            if app.get("run", instance_id).is_ok() {
                let run = crate::agent::progress(&app, instance_id)?;
                if matches!(run["status"].as_str(), Some("running" | "starting")) {
                    crate::agent::cancel(app.clone(), instance_id).await?;
                    return app.get("instance", instance_id);
                }
            }
            let models = app.list("model")?;
            if let Some(model) = models
                .as_array()
                .into_iter()
                .flatten()
                .find(|m| m["instance_id"].as_str() == Some(instance_id))
            {
                return crate::engines::stop(app.clone(), text(model, "id")?).await;
            }
            let mut workers = app.workers.lock().await;
            if let Some(worker) = workers.get(id()?) {
                worker.stop()?;
                let mut record = app.get("instance", id()?)?;
                record["status"] = json!("stopped");
                app.save("instance", record)?;
                workers.remove(id()?);
            }
            app.get("instance", id()?)
        }
        "instance.start" => {
            let mut p = p;
            if let Some(o) = p.as_object_mut() {
                o.remove("image_owner");
            }
            start_instance(app, p).await
        }
        "model.save" => {
            let _serial = app.runtime_lock.lock().await;
            let mut p = p;
            crate::model_config::validate(&p)?;
            let profile = model_profile(&app, &p)?;
            if !p["resource_mode"].is_null()
                && !matches!(p["resource_mode"].as_str(), Some("profile" | "dedicated"))
            {
                bail!("Unknown resource mode")
            }
            let has_resource_draft = p["resource_profile"].is_object();
            p["profile_id"] = profile["id"].clone();
            p["resource_mode"] = json!(if profile["owner_model_id"] == p["id"] {
                "dedicated"
            } else {
                "profile"
            });
            if let Ok(old) = app.get("model", p["id"].as_str().unwrap_or("")) {
                let old_profile = app
                    .get("profile", old["profile_id"].as_str().unwrap_or(""))
                    .ok();
                let changed_resources = has_resource_draft
                    && old_profile
                        .as_ref()
                        .map(|old| {
                            [
                                "gpu_ids",
                                "ram_gib",
                                "cpu_threads",
                                "vram_gib",
                                "vram_gib_by_gpu",
                                "exclusive",
                            ]
                            .iter()
                            .any(|k| old[*k] != profile[*k])
                        })
                        .unwrap_or(true);
                if old["instance_id"]
                    .as_str()
                    .and_then(|id| app.get("instance", id).ok())
                    .map(|r| r["status"] == "running")
                    .unwrap_or(false)
                    && (changed_resources
                        || ["profile_id", "endpoint", "protocol", "runtime"]
                            .iter()
                            .any(|k| old[*k] != p[*k]))
                {
                    bail!("Stop the model runtime before changing its resources or launch configuration")
                }
            }
            crate::engines::validate(&p)?;
            text(&p, "name")?;
            let url = reqwest::Url::parse(text(&p, "endpoint")?)?;
            if !matches!(
                url.host_str(),
                Some("127.0.0.1") | Some("localhost") | Some("[::1]")
            ) {
                bail!("В этой версии поддерживаются локальные endpoints")
            };
            if !["http", "https"].contains(&url.scheme())
                || url.password().is_some()
                || !url.username().is_empty()
            {
                bail!("Некорректный endpoint")
            };
            if !matches!(p["protocol"].as_str(), Some("ollama" | "openai")) {
                bail!("Choose Ollama or OpenAI-compatible protocol")
            };
            text(&p, "engine_model")?;
            for (key, min, max) in [
                ("context_tokens", 512, 131072),
                ("max_output_tokens", 16, 8192),
            ] {
                if !p[key]
                    .as_u64()
                    .map(|n| n >= min && n <= max)
                    .unwrap_or(false)
                {
                    bail!("Invalid {key}")
                }
            }
            if let Some(mode) = p["tool_mode"].as_str() {
                if !["native", "json", "none"].contains(&mode) {
                    bail!("Invalid tool mode")
                }
            }
            // Older wizard profiles stored the offload count directly in -ngl.
            // Normalize them when saved so later device changes reach the engine.
            if p["tokenizer"] == "llama.cpp" && p["runtime"]["kind"] == "command" {
                if let Some(args) = p["runtime"]["args"].as_array() {
                    if let Some(i) = args.windows(2).position(|pair| pair[0] == "-ngl") {
                        let old = args[i+1].as_str().and_then(|s| s.parse::<u64>().ok());
                        if old.is_some() || args[i+1] == "{{gpu_layers}}" {
                            if p["gpu_layers"].is_null() { p["gpu_layers"] = json!(old.unwrap_or(0)); }
                            if profile["gpu_ids"].as_array().is_some_and(Vec::is_empty) { p["gpu_layers"] = json!(0); }
                            p["runtime"]["args"][i+1] = json!("{{gpu_layers}}");
                        }
                    }
                }
            }
            if !p["gpu_layers"].is_null()
                && !p["gpu_layers"].as_u64().map(|n| n <= 1000).unwrap_or(false)
            {
                bail!("GPU layers must be 0..1000 or automatic")
            }
            if !p["cpu_threads"].is_null()
                && !p["cpu_threads"]
                    .as_u64()
                    .map(|n| n >= 1 && n <= profile["cpu_threads"].as_u64().unwrap_or(1))
                    .unwrap_or(false)
            {
                bail!("Inference threads must fit the selected resource profile")
            }
            if p["max_output_tokens"].as_u64() >= p["context_tokens"].as_u64() {
                bail!("Output must be smaller than the context")
            }
            if !p["temperature"]
                .as_f64()
                .map(|n| (0.0..=2.0).contains(&n))
                .unwrap_or(false)
            {
                bail!("Invalid temperature")
            }
            p.as_object_mut().unwrap().remove("resource_profile");
            if has_resource_draft {
                let saved = app
                    .store
                    .lock()
                    .unwrap()
                    .save_batch(vec![("profile", profile), ("model", p)])?;
                Ok(saved[1].clone())
            } else {
                app.save("model", p)
            }
        }
        "model.probe" => {
            let model = app.get("model", id()?)?;
            let suffix = if model["protocol"] == "ollama" {
                "/api/tags"
            } else {
                "/models"
            };
            let response = reqwest::Client::builder()
                .timeout(Duration::from_secs(6))
                .redirect(reqwest::redirect::Policy::none())
                .build()?
                .get(format!(
                    "{}{suffix}",
                    text(&model, "endpoint")?.trim_end_matches('/')
                ))
                .send()
                .await?
                .error_for_status()?
                .json::<Value>()
                .await?;
            Ok(
                json!({"reachable":true,"response":response,"managed_process":model["instance_id"].is_string()}),
            )
        }
        "mcp.list" => app.list("mcp"),
        "mcp.get" => app.get("mcp", id()?),
        "mcp.save" => {
            validate_connection(&p)?;
            let mut config = p.clone();
            if let Ok(old) = app.get("mcp", id()?) {
                if [
                    "command",
                    "args",
                    "cwd",
                    "url",
                    "transport",
                    "environment",
                    "env_names",
                    "bearer_env",
                    "tools",
                ]
                .iter()
                .any(|k| old[*k] != p[*k])
                {
                    config.as_object_mut().unwrap().remove("last_check");
                }
            }
            app.save("mcp", config)
        }
        "mcp.tool.get" => {
            let catalog = app.get("mcp_catalog", id()?)?;
            let name = text(&p, "tool")?;
            catalog["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|t| t["name"] == name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Tool schema not found; check the connection again"))
        }
        "mcp.probe" => {
            let mut config = app.get("mcp", id()?)?;
            let result = bridge(config.clone(), "probe", None, Value::Null).await;
            let result = match result {
                Ok(mut v) => {
                    let old = app.get("mcp_catalog", id()?).ok();
                    app.save("mcp_catalog",json!({"id":id()?,"revision":old.and_then(|o|o["revision"].as_i64()),"tools":v["tools"]}))?;
                    let brief:Vec<_>=v["tools"].as_array().into_iter().flatten().map(|t|json!({"name":t["name"],"description":t["description"].as_str().unwrap_or("").chars().take(400).collect::<String>()})).collect();
                    v["tools"] = json!(brief);
                    Ok(v)
                }
                Err(e) => Err(e),
            };
            config["last_check"] = match &result {
                Ok(v) => {
                    json!({"ok":true,"tools":v["tools"],"protocol":v["protocol"],"server":v["server"]})
                }
                Err(e) => json!({"ok":false,"error":e.to_string()}),
            };
            app.save("mcp", config)?;
            result
        }
        "mcp.call" => {
            let mut config = app.get("mcp", id()?)?;
            let tool = text(&p, "tool")?;
            let project = text(&p, "project_id")?;
            let project_record = app.get("project", project)?;
            if config["builtin_documents"] == true || config["builtin_internet"] == true {
                config["environment"]["STRUVE_PROJECT_ROOT"] = project_record["path"].clone();
                config["environment"]["STRUVE_APP_ROOT"] = json!(root());
            }
            if config["enabled"] != true {
                bail!("Подключение отключено")
            };
            if !config["allowed_tools"]
                .as_array()
                .unwrap()
                .contains(&json!(tool))
            {
                bail!("Инструмент не разрешён пользователем")
            };
            if !config["project_ids"]
                .as_array()
                .unwrap()
                .contains(&json!(project))
            {
                bail!("Подключение недоступно этому проекту")
            };
            if !p["arguments"].is_object() {
                bail!("arguments должен быть объектом")
            };
            // Model instructions cannot expand the dispatcher's saved task permissions.
            let task_remaining = if let Some(run_id) = p["run_id"].as_str() {
                let mut store = app.store.lock().unwrap();
                let mut run = store.get("run", run_id)?;
                if run["project_id"] != project
                    || !matches!(run["status"].as_str(), Some("running" | "starting"))
                {
                    bail!("Task is not active in this project")
                }
                let execution = &run["execution"];
                if execution["use_tools"] != true {
                    bail!("Tools are disabled for this task")
                }
                if let Some(allowed) = execution["limits"]["tool_selection"].as_array() {
                    if !allowed
                        .iter()
                        .any(|t| t["connection_id"] == id().unwrap() && t["tool"] == tool)
                    {
                        bail!("Tool is blocked by the saved task permissions")
                    }
                }
                let used = run["tool_calls_used"].as_u64().unwrap_or(0);
                if used >= execution["limits"]["max_tool_calls"].as_u64().unwrap_or(0) {
                    bail!("Task tool call limit reached")
                }
                let deadline = chrono::DateTime::parse_from_rfc3339(
                    run["deadline_at"].as_str().unwrap_or(""),
                )?;
                let millis = deadline
                    .signed_duration_since(chrono::Utc::now())
                    .num_milliseconds();
                if millis <= 0 {
                    bail!("Task time limit reached")
                }
                if config["builtin_internet"]==true && tool=="web_search" && run["execution"]["desktop_browser"]==true
                    && matches!(p["arguments"]["engine"].as_str(),Some("google"|"yandex"|"both")) {
                    config["environment"]["STRUVE_DESKTOP_SEARCH"]=json!("1");
                }
                run["tool_calls_used"] = json!(used + 1);
                let revision = run["revision"].as_i64();
                store.save("run", run, revision)?;
                Some(Duration::from_millis(millis as u64))
            } else {
                None
            };
            app.store.lock().unwrap().event(
                "mcp.call.started",
                id()?,
                json!({"tool":tool,"project_id":project,"revision":config["revision"]}),
            )?;
            let future = bridge(config, "call", Some(tool), p["arguments"].clone());
            let result = if let Some(remaining) = task_remaining {
                match tokio::time::timeout(remaining, future).await {
                    Ok(value) => value,
                    Err(_) => Err(anyhow::anyhow!(
                        "Task time limit reached during tool execution"
                    )),
                }
            } else {
                future.await
            };
            app.store.lock().unwrap().event(
                "mcp.call.finished",
                id()?,
                json!({"tool":tool,"ok":result.is_ok()}),
            )?;
            result
        }
        "plugin.list" => app.list("plugin"),
        "plugin.get" => app.get("plugin", id()?),
        "plugin.install" => {
            let manifest = &p["manifest"];
            crate::safe_id(text(manifest, "id")?)?;
            text(manifest, "name")?;
            text(manifest, "version")?;
            if manifest["format"] != "struve-plugin-v1" {
                bail!("Поддерживается формат struve-plugin-v1")
            }
            let servers = manifest["mcp_servers"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Требуется mcp_servers"))?;
            for c in servers {
                validate_connection(c)?;
            }
            let mut store = app.store.lock().unwrap();
            let existing = store.list("plugin")?;
            if existing.iter().any(|v| v["id"] == manifest["id"]) {
                bail!("Плагин уже установлен; обновления требуют отдельной операции")
            }
            let connections = store.list("mcp")?;
            let mut seen = std::collections::HashSet::new();
            for c in servers {
                if !seen.insert(c["id"].as_str().unwrap())
                    || connections.iter().any(|v| v["id"] == c["id"])
                {
                    bail!("Повторяющийся ID подключения")
                }
            }
            if p["dry_run"] == true {
                return Ok(
                    json!({"dry_run":true,"valid":true,"manifest":manifest,"connections":servers.len()}),
                );
            }
            let hash = format!("{:x}", Sha256::digest(serde_json::to_vec(manifest)?));
            store.db.execute_batch("SAVEPOINT plugin_install")?;
            let installed = (|| -> Result<Value> {
                for server in servers {
                    let mut c = server.clone();
                    c["enabled"] = json!(false);
                    c["plugin_id"] = manifest["id"].clone();
                    store.save("mcp", c, None)?;
                }
                store.save("plugin",json!({"id":manifest["id"],"name":manifest["name"],"version":manifest["version"],"instructions":manifest["instructions"],"sha256":hash,"manifest":manifest,"source":"local_manifest"}),None)
            })();
            match installed {
                Ok(v) => {
                    store.db.execute_batch("RELEASE plugin_install")?;
                    Ok(v)
                }
                Err(e) => {
                    store
                        .db
                        .execute_batch("ROLLBACK TO plugin_install; RELEASE plugin_install")?;
                    Err(e)
                }
            }
        }
        "events.list" => Ok(json!(app.store.lock().unwrap().events(
            p["after"].as_i64().unwrap_or(0),
            p["limit"].as_i64().unwrap_or(100)
        )?)),
        _ => bail!("Неизвестная операция: {}", r.method),
    }
}
async fn handler(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(r): Json<Rpc>,
) -> (StatusCode, Json<Value>) {
    if headers.get("authorization").and_then(|h| h.to_str().ok())
        != Some(&format!("Bearer {}", app.token))
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(envelope(Err(anyhow::anyhow!("Нет локальной авторизации")))),
        );
    }
    // Browser pages cannot use this API: only the native IPC client sends the token.
    if headers.contains_key("origin") {
        return (
            StatusCode::FORBIDDEN,
            Json(envelope(Err(anyhow::anyhow!("Browser origin rejected")))),
        );
    }
    let result=if matches!(r.method.as_str(),"mcp.call"|"mcp.probe"|"media.market.refresh"){
        let mut exit=app.exit.subscribe();
        tokio::select!{
            result=dispatch(app,r)=>result,
            _=exit.wait_for(|closing|*closing)=>Err(anyhow::anyhow!("Controller is shutting down")),
        }
    }else{dispatch(app,r).await};
    (StatusCode::OK, Json(envelope(result)))
}
async fn shutdown_workers(app:&Arc<App>)->Result<Value>{
    for run in app.list("run")?.as_array().unwrap(){
        if !matches!(run["status"].as_str(),Some("queued"|"starting"|"running")){continue}
        let id=text(run,"id")?;
        if matches!(run["kind"].as_str(),Some("image"|"video")){
            crate::image_jobs::cancel(app.clone(),id).await?;
        }else{
            crate::agent::cancel(app.clone(),id).await?;
        }
    }
    tokio::time::timeout(Duration::from_secs(35),async {
        while !app.image_leases.lock().await.is_empty(){tokio::time::sleep(Duration::from_millis(80)).await}
    }).await.context("Media cleanup is still running; try Exit again")?;
    let mut workers=app.workers.lock().await;
    let ids:Vec<_>=workers.keys().cloned().collect();
    for id in &ids{
        workers[id].stop()?;
        let mut row=app.get("instance",id)?;
        row["status"]=json!("stopped");
        app.save("instance",row)?;
        workers.remove(id);
    }
    Ok(json!({"stopped":ids}))
}
pub async fn serve() -> Result<()> {
    for path in ["data", "models", "logs"] {
        std::fs::create_dir_all(root().join(path))?;
    }
    use std::os::windows::fs::OpenOptionsExt;
    let _lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .share_mode(0)
        .open(root().join("data/controller.lock"))
        .context("Controller already running for this installation")?;
    let bind_port = std::env::var("STRUVE_LLM_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, bind_port))
        .await
        .context("Controller port unavailable")?;
    let token_path = root().join("data/controller.token");
    if !token_path.exists() {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&token_path)?;
        write!(
            f,
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        )?;
    }
    let namespace = root().join("data/credential-namespace");
    if !namespace.exists() {
        std::fs::write(
            namespace,
            format!("{:x}", Sha256::digest(root().to_string_lossy().as_bytes())),
        )?;
    }
    let inventory = hardware::inventory().await;
    let mut store = Store::open(&root().join("data/struve.sqlite"))?;
    if store.list("settings")?.is_empty() {
        let mut settings = hardware::suggested_settings(&inventory);
        settings["language"] = json!(crate::installer_language(&root()));
        store.save("settings", settings, None)?;
        store.save("profile", hardware::diagnostic_profile(&inventory), None)?;
        store.save(
            "project",
            json!({"id":"struve","name":"Struve","path":root(),"description":"Local models"}),
            None,
        )?;
        store.save("mcp",json!({"id":"local-diagnostics","name":"Struve diagnostics","transport":"stdio","command":crate::app_dir().join("node.exe"),"args":[source_dir().join("bridge/diagnostics.mjs")],"env_names":[],"project_ids":["struve"],"allowed_tools":["runtime_info"],"enabled":true,"builtin":true}),None)?;
    }
    // Preserve user profiles; migrate only application-owned paths and missing settings.
    let mut settings = store.get("settings", "system")?;
    let before = settings.clone();
    for (key, value) in [
        ("language", json!(crate::installer_language(&root()))),
        ("reserve_mode", json!("manual")),
        ("models_directory", json!("models")),
    ] {
        if settings.get(key).is_none() {
            settings[key] = value
        }
    }
    if settings != before {
        let rev = settings["revision"].as_i64();
        store.save("settings", settings, rev)?;
    }
    if let Ok(mut diagnostic) = store.get("mcp", "local-diagnostics") {
        if diagnostic["builtin"] == true {
            let before = diagnostic.clone();
            diagnostic["command"] = json!(crate::app_dir().join("node.exe"));
            diagnostic["args"] = json!([source_dir().join("bridge/diagnostics.mjs")]);
            if before != diagnostic {
                diagnostic.as_object_mut().unwrap().remove("last_check");
                let rev = diagnostic["revision"].as_i64();
                store.save("mcp", diagnostic, rev)?;
            }
        }
    }
    if store.get("plugin", "documents").is_err() {
        store.save("plugin",json!({"id":"documents","name":"Documents & spreadsheets","version":"0.3.5","builtin":true,"instructions":"DOCX, PDF, XLSX and source text. Enable tools for the selected project in Connections.","source":"bundled"}),None)?;
    }
    let prior=store.get("mcp","internet").ok();
    let mut internet=prior.clone().unwrap_or_else(||json!({"id":"internet","name":"Internet · Web","transport":"stdio","builtin_internet":true,
        "timeout_seconds":170,"env_names":[],"environment":{},"project_ids":["struve"],"allowed_tools":["web_search","web_read","web_download"],"enabled":true}));
    if internet["builtin_internet"]==true{
        internet["command"]=json!(crate::app_dir().join("node.exe"));
        internet["args"]=json!([source_dir().join("bridge/internet.mjs")]);
        if prior.as_ref()!=Some(&internet){let rev=internet["revision"].as_i64();store.save("mcp",internet,rev)?;}
    }
    let previous = store.get("mcp", "documents").ok();
    let mut documents=previous.clone().unwrap_or_else(||json!({"id":"documents","name":"Documents · DOCX / PDF / XLSX","transport":"stdio","env_names":[],"environment":{},"project_ids":["struve"],"allowed_tools":["document_read"],"enabled":false,"builtin_documents":true,"plugin_id":"documents"}));
    if documents["builtin_documents"] == true {
        let before = documents.clone();
        documents["command"] = json!(crate::app_dir().join("node.exe"));
        documents["args"] = json!([source_dir().join("bridge/documents.mjs")]);
        if previous.is_none() || before != documents {
            let rev = documents["revision"].as_i64();
            store.save("mcp", documents, rev)?;
        }
    }
    if let Ok(previous) = store.get("installation", "current") {
        if previous["root"] != json!(root()) {
            if let Ok(mut project) = store.get("project", "struve") {
                if project["path"] == previous["root"] {
                    project["path"] = json!(root());
                    let rev = project["revision"].as_i64();
                    store.save("project", project, rev)?;
                }
            }
        }
    }
    let previous = store.get("installation", "current").ok();
    if previous
        .as_ref()
        .map(|p| p["root"] != json!(root()))
        .unwrap_or(true)
    {
        store.save(
            "installation",
            json!({"id":"current","root":root()}),
            previous.and_then(|p| p["revision"].as_i64()),
        )?;
    }
    std::fs::write(
        root().join("data/controller.endpoint.json"),
        json!({"port":listener.local_addr()?.port(),"pid":std::process::id(),"root":root()})
            .to_string(),
    )?;
    for mut record in store.list("instance")? {
        if ["running", "starting"].contains(&record["status"].as_str().unwrap_or("")) {
            let rev = record["revision"].as_i64();
            record["status"] = json!("interrupted");
            record["recovery_note"]=json!("Диспетчер перезапущен. Процессы прошлой сессии ограничивались Job Object; автоматический повтор отключён.");
            store.save("instance", record, rev)?;
        }
    }
    for mut record in store.list("run")? {
        if matches!(record["kind"].as_str(), Some("image" | "video"))
            && matches!(record["status"].as_str(), Some("running" | "starting"))
        {
            let rev = record["revision"].as_i64();
            record["status"] = json!("interrupted");
            record["phase"] = json!("done");
            record["error"]=json!("Controller restarted during image workflow. Owned processes were terminated; retry explicitly. LLM restoration was not replayed.");
            store.save("run", record, rev)?;
        }
    }
    let app = Arc::new(App {
        closing: std::sync::atomic::AtomicBool::new(false),
        exit: tokio::sync::watch::channel(false).0,
        admission_lock: tokio::sync::Mutex::new(()),
        image_leases: Default::default(),
        exposure_lock: tokio::sync::Mutex::new(()),
        media_queue_wake: tokio::sync::Notify::new(),
        runtime_lock: tokio::sync::Mutex::new(()),
        chat_lock: tokio::sync::Mutex::new(()),
        store: Mutex::new(store),
        token: crate::token()?,
        inventory: tokio::sync::Mutex::new((Instant::now(), inventory)),
        workers: tokio::sync::Mutex::new(std::collections::BTreeMap::new()),
    });
    crate::agent::recover_text_runs(&app)?;
    crate::media_catalog::migrate(&app)?;
    tokio::spawn(crate::media_queue::schedule(app.clone()));
    tokio::spawn(crate::library::refresh_idle(app.clone()));
    let router = Router::new()
        .route("/rpc", post(handler))
        .layer(axum::extract::DefaultBodyLimit::max(4 * 1024 * 1024))
        .with_state(app.clone());
    let mut exit=app.exit.subscribe();
    axum::serve(listener, router).with_graceful_shutdown(async move{let _=exit.wait_for(|closing|*closing).await;}).await?;
    Ok(())
}
