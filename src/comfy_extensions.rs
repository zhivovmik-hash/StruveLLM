//! ComfyUI packages are installed in the chosen Comfy workspace, independently of sd-cli.
use crate::{root,server::App};
use anyhow::{bail,Context,Result};
use serde_json::{json,Value};
use std::{path::Path,time::Duration};
static INSTALL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn settings(app:&App)->Value {
    app.get("comfy_extensions","local").unwrap_or_else(|_| {
        let runtime=root().join("runtimes/comfy-media-cpu-0.46.0");
        let workspace=runtime.join("ComfyUI");
        let cli=runtime.join("venv/Scripts/comfy.exe");
        json!({"id":"local","workspace":if workspace.join("main.py").is_file(){workspace.to_str()}else{None},
                "cli":if cli.is_file(){cli.to_str()}else{None},"port":8188})
    })
}
fn port(conf:&Value)->Result<u16>{
    let n=conf["port"].as_u64().filter(|n|(1..=65535).contains(n)).context("Choose a local server port")?;
    Ok(n as u16)
}
fn paths(conf:&Value)->Result<(&Path,&Path)> {
    let dir=Path::new(conf["workspace"].as_str().unwrap_or(""));
    let cli=Path::new(conf["cli"].as_str().unwrap_or(""));
    if !dir.is_absolute() || !dir.join("main.py").is_file() || !dir.join("folder_paths.py").is_file() || !dir.join("custom_nodes").is_dir(){bail!("Select the ComfyUI directory containing main.py and custom_nodes")}
    if !cli.is_absolute() || !cli.is_file() || !cli.file_name().is_some_and(|n|n.eq_ignore_ascii_case("comfy.exe")) {bail!("Select comfy.exe from the Comfy environment")}
    Ok((dir,cli))
}
pub fn save(app:&App,mut conf:Value)->Result<Value>{
    port(&conf)?;
    if conf["cli"].as_str().is_some_and(|s|!s.is_empty()) || conf["workspace"].as_str().is_some_and(|s|!s.is_empty()){ paths(&conf)?; }
    conf["id"]=json!("local");app.save("comfy_extensions",conf)
}
pub async fn start_cpu(app:std::sync::Arc<App>)->Result<Value>{
    let conf=settings(&app);let port=port(&conf)?;let (workspace,cli)=paths(&conf)?;
    if tokio::net::TcpStream::connect(("127.0.0.1",port)).await.is_ok(){bail!("A local service is already listening on this port. Use Test connection.")}
    let python=cli.with_file_name("python.exe");
    if !python.is_file(){bail!("Missing Python in the selected environment")}
    let profile="comfy-node-check-cpu";
    let mut record=json!({"id":profile,"name":"ComfyUI · CPU nodes","kind":"inference","gpu_ids":[],"ram_gib":8,"vram_gib":0,"cpu_threads":1,"exclusive":false});
    if let Ok(saved)=app.get("profile",profile){record=saved}
    else{app.save("profile",record.clone())?;}
    if !record["gpu_ids"].as_array().is_some_and(Vec::is_empty){bail!("Choose a CPU-only profile for the node check server")}
    let spec=json!({"id":format!("comfy-cpu-{}",uuid::Uuid::new_v4().simple()),"name":"ComfyUI · CPU nodes",
        "command":python,"cwd":workspace,"profile_id":profile,"args":[workspace.join("main.py"),"--cpu","--listen","127.0.0.1","--port",port.to_string(),"--disable-auto-launch"],
        "env":{"OMP_NUM_THREADS":"1","MKL_NUM_THREADS":"1","OPENBLAS_NUM_THREADS":"1","UV_THREADPOOL_SIZE":"1"}});
    crate::server::start_instance(app,spec).await
}
fn registry(app:&App,id:&str)->Result<(String,String,String)>{
    let bundled=crate::media_extensions::builtin_catalog().iter().find(|c|c["id"]==id).and_then(|c|c.get("registry"));
    let class=bundled.and_then(|r|r["node"].as_str()).unwrap_or("").to_string();
    if let Some(row)=crate::market::remote(app,id){
        if row["installable"]!=true {bail!("The registry has no active installable release for this package")}
        return Ok((row["slug"].as_str().unwrap().into(),row["version"].as_str().unwrap().into(),class));
    }
    let reg=bundled.context("Package is absent from the local registry snapshot. Refresh the catalog.")?;
    Ok((reg["id"].as_str().context("Missing registry ID")?.into(),reg["version"].as_str().context("Missing version")?.into(),class))
}
pub fn installed_nodes(app:&App,conf:&Value)->Result<Vec<Value>>{
    let Some(workspace)=conf["workspace"].as_str() else{return Ok(vec![])};
    let dir=Path::new(workspace).join("custom_nodes");
    if !dir.is_dir(){return Ok(vec![])}
    let receipts=app.store.lock().unwrap().list("comfy_node")?;
    let mut rows=Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let folder=entry?.path();
        if !folder.join("__init__.py").is_file(){continue}
        let Some(folder_name)=folder.file_name().and_then(|s|s.to_str()) else{continue};
        let info=std::fs::read_to_string(folder.join("pyproject.toml")).ok().and_then(|s|toml::from_str::<toml::Value>(&s).ok());
        let project=info.as_ref().and_then(|i|i.get("project"));
        let field=|k:&str|project.and_then(|p|p.get(k)).and_then(|s|s.as_str());
        let slug=field("name").unwrap_or(folder_name);
        let receipt=receipts.iter().find(|r|r["registry_id"].as_str().is_some_and(|n|n.eq_ignore_ascii_case(folder_name))&&
            r["workspace"].as_str().is_some_and(|w|Path::new(w)==Path::new(workspace)));
        let id=receipt.and_then(|r|r["id"].as_str()).map(str::to_owned).unwrap_or_else(||crate::market::package_id(slug));
        rows.push(json!({"id":id,"workspace":workspace,"registry_id":slug,"folder":folder_name,
            "version":field("version").or_else(||receipt.and_then(|r|r["version"].as_str())),"status":"detected_on_disk"}));
    }
    Ok(rows)
}
fn client()->Result<reqwest::Client>{
    Ok(reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(5)).build()?)
}
pub async fn probe(app:&App,id:Option<&str>)->Result<Value>{
    let base=format!("http://127.0.0.1:{}",port(&settings(app))?);
    let http=client()?;
    let stats:Value=http.get(format!("{base}/system_stats")).send().await?.error_for_status()?.json().await?;
    if !stats["system"].is_object() || !stats["devices"].is_array(){bail!("This endpoint did not identify itself as ComfyUI")}
    let mut r=json!({"url":base,"devices":stats["devices"],"version":stats["system"]["comfyui_version"]});
    if let Some(id)=id{
        let (slug,_,class)=registry(app,id)?;
        let url=if class.is_empty(){format!("{base}/object_info")}else{format!("{base}/object_info/{class}")};
        let nodes:Value=http.get(url).send().await?.error_for_status()?.json().await?;
        let found=if class.is_empty(){nodes.as_object().and_then(|all|all.iter().find(|(_,v)|v["python_module"].as_str().and_then(|s|s.strip_prefix("custom_nodes.")).is_some_and(|s|s.eq_ignore_ascii_case(&slug))).map(|(name,_)|name.as_str()))}else{nodes.get(&class).map(|_|class.as_str())};
        r["loaded"]=json!(found.is_some());r["node"]=json!(found.unwrap_or(&class));
        if let Some(name)=found {
            let module=nodes[name]["python_module"].as_str().and_then(|s|s.strip_prefix("custom_nodes.")).unwrap_or(&slug);
            r["template_source"]=json!(module);
            if let Ok(reply)=http.get(format!("{base}/api/workflow_templates")).send().await {
                if let Ok(templates)=reply.json::<Value>().await {
                    r["templates"]=templates.get(module).cloned().unwrap_or_else(||json!([]));
                }
            }
        }
    }
    Ok(r)
}
pub async fn install(app:&App,p:&Value)->Result<Value>{
    let id=p["id"].as_str().context("Package ID required")?;
    let (slug,version,node)=registry(app,id)?;
    let conf=settings(app);
    let (workspace,cli)=paths(&conf)?;
    let mut plan=json!({"id":id,"registry_id":slug,"version":version,"workspace":workspace,"node":node});
    if p["dry_run"]==true{return Ok(plan)}
    if p["expected_version"].as_str().is_some_and(|v|v!=version){bail!("Package version changed. Review the installation again")}
    let bin=cli.parent().context("Missing comfy-cli environment")?;
    let python=bin.join("python.exe");
    if !python.is_file(){bail!("The selected comfy-cli must be installed in a Python environment")}
    let mut entries=vec![bin.to_path_buf()];
    entries.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    let log_dir=root().join("logs");std::fs::create_dir_all(&log_dir)?;
    let log_path=log_dir.join(format!("comfy-install-{slug}-{}.log",uuid::Uuid::new_v4().simple()));
    let log=std::fs::File::create(&log_path)?;
    let _install=INSTALL.try_lock().map_err(|_|anyhow::anyhow!("Another Comfy package installation is in progress"))?;
    let folder=workspace.join("custom_nodes").join(&slug);
    if folder.exists(){bail!("This package directory already exists. Check the loaded node or manage its version in ComfyUI: {}",folder.display())}
    let child=tokio::process::Command::new(cli).current_dir(workspace).env("PATH",std::env::join_paths(entries)?)
        .env("VIRTUAL_ENV",bin.parent().context("Missing environment root")?)
        .args(["--skip-prompt","--workspace"]).arg(workspace).args(["node","registry-install",slug.as_str(),"--version",version.as_str()])
        .stdin(std::process::Stdio::null()).stdout(log.try_clone()?).stderr(log)
        .creation_flags(0x08000000).kill_on_drop(true).spawn()?;
    let status=child.wait_with_output().await?.status;
    if !status.success() || !folder.join("__init__.py").is_file(){
        if folder.exists(){
            let hold=workspace.join(".struve-incomplete");std::fs::create_dir_all(&hold)?;
            std::fs::rename(&folder,hold.join(format!("{slug}-{}",uuid::Uuid::new_v4().simple())))?;
        }
        bail!("Comfy installation failed ({status}). {}",log_path.display())
    }
    plan["log"]=json!(log_path);
    plan["status"]=json!("installed_check_after_restart");
    let mut store=app.store.lock().unwrap();
    let previous=store.get("comfy_node",id).ok();
    store.save("comfy_node",plan,previous.and_then(|n|n["revision"].as_i64()))
}

pub async fn lint(app:&App,p:&Value)->Result<Value>{
    let prompt=p["prompt"].as_str().filter(|s|!s.trim().is_empty()&&s.len()<=16000).context("Enter the H3 prompt")?;
    let seconds=p["seconds"].as_f64().unwrap_or(15.0);
    if !(0.0..=15.1).contains(&seconds){bail!("Invalid H3 clip duration")}
    let mode=p["mode"].as_str().unwrap_or("fl2va");
    if !matches!(mode,"t2va"|"i2va"|"fl2va"){bail!("Unsupported H3 conditioning mode")}
    let check=probe(app,Some("h3-tools")).await?;
    if check["loaded"]!=true{bail!("Load the H3 Tools node in this ComfyUI instance first")}
    let base=check["url"].as_str().unwrap();
    let graph=json!({"1":{"class_type":"MiniMaxH3XPromptLint","inputs":{"prompt":prompt,"mode":mode,
        "duration_seconds":seconds,"ref_images":0,"ref_videos":0,"ref_audios":0,"raise_on_warning":false}},
        "2":{"class_type":"PreviewAny","inputs":{"source":["1",1]}}});
    let http=client()?;
    let res=http.post(format!("{base}/prompt")).json(&json!({"prompt":graph,"client_id":uuid::Uuid::new_v4().to_string()})).send().await?;
    let body:Value=res.json().await?;
    let id=body["prompt_id"].as_str().context(format!("ComfyUI rejected the workflow: {}",body["node_errors"]))?;
    for _ in 0..120 {
        let batch:Value=http.get(format!("{base}/history/{id}")).send().await?.error_for_status()?.json().await?;
        if let Some(done)=batch.get(id){
            if done["status"]["status_str"]!="success"{bail!("ComfyUI workflow failed: {}",done["status"]["messages"])}
            let report=done["outputs"]["2"]["text"].as_array().context("The workflow returned no text report")?;
            return Ok(json!({"prompt_id":id,"report":report}))
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    bail!("ComfyUI is still processing the queued prompt {id}")
}

pub async fn dependencies(app:&App,p:&Value)->Result<Value> {
    use std::process::Stdio;
    let id=p["id"].as_str().context("Missing package ID")?;
    let (slug,_,_)=registry(app,id)?;
    let conf=settings(app);
    let (workspace,cli)=paths(&conf)?;
    let folder=workspace.join("custom_nodes").join(&slug);
    if !folder.join("__init__.py").is_file(){bail!("Install the package first")}
    let python=cli.with_file_name("python.exe");
    let req=folder.join("requirements.txt");
    let mut args=vec![];
    if req.is_file(){args.extend(["-r".to_string(),req.to_string_lossy().into_owned()]);}
    else {
        let info:toml::Value=toml::from_str(&std::fs::read_to_string(folder.join("pyproject.toml"))?)?;
        if let Some(deps)=info.get("project").and_then(|s|s.get("dependencies")).and_then(|s|s.as_array()){
            for dep in deps {args.push(dep.as_str().context("Invalid dependency")?.to_string());}
        }
    }
    if args.is_empty(){return Ok(json!({"packages":[],"custom_setup":folder.join("install.py").is_file()}))}
    let logs=root().join("logs");std::fs::create_dir_all(&logs)?;
    let _lock=INSTALL.try_lock().map_err(|_|anyhow::anyhow!("Another package operation is in progress"))?;
    let preview=p["plan_id"].is_null();
    let ticket=if preview{uuid::Uuid::new_v4()}else{uuid::Uuid::parse_str(p["plan_id"].as_str().context("Missing dependency plan")?)?};
    let prefix=format!("comfy-deps-{slug}-{ticket}");
    let plan_path=logs.join(format!("{prefix}.json"));
    let report=logs.join(format!("{prefix}-{}.json",if preview{"preview"}else{"installed"}));
    let log_path=report.with_extension("log");
    let log=std::fs::File::create(&log_path)?;
    let mut cmd=tokio::process::Command::new(&python);
    cmd.current_dir(&folder).env("PIP_DISABLE_PIP_VERSION_CHECK","1").args(["-m","pip","install","--no-input","--report"]).arg(&report);
    if preview { cmd.arg("--dry-run"); }
    else {
        let plan:Value=serde_json::from_slice(&std::fs::read(&plan_path)?)?;
        if plan["workspace"]!=workspace.to_string_lossy().as_ref() || plan["cli"]!=cli.to_string_lossy().as_ref() {bail!("Comfy environment changed; preview dependencies again")}
        cmd.arg("--constraint").arg(logs.join(format!("{prefix}.constraints")));
    }
    let status=cmd.args(args).stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log)
        .creation_flags(0x08004000).kill_on_drop(true).status().await?;
    if !status.success(){bail!("Dependency operation failed ({status}). {}",log_path.display())}
    let resolved:Value=serde_json::from_slice(&std::fs::read(&report)?)?;
    let mut packages=Vec::new();let mut constraints=String::new();
    for item in resolved["install"].as_array().context("Incomplete pip report")? {
        let name=item["metadata"]["name"].as_str().context("Missing distribution name")?;
        let ver=item["metadata"]["version"].as_str().context("Missing distribution version")?;
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) || !ver.chars().all(|c| c.is_ascii_alphanumeric() || "-_+.!".contains(c)){bail!("Invalid Python package metadata")}
        constraints.push_str(&format!("{name}=={ver}\n"));
        packages.push(format!("{name} {ver}"));
    }
    let result=json!({"packages":packages,"plan_id":ticket.to_string(),"workspace":workspace,"cli":cli,"log":log_path,
                      "custom_setup":folder.join("install.py").is_file()});
    if preview{std::fs::write(&plan_path,serde_json::to_vec(&result)?)?;std::fs::write(logs.join(format!("{prefix}.constraints")),constraints)?;}
    Ok(result)
}
