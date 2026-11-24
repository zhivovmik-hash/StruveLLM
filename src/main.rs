use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::{io::Read, path::PathBuf, process::Stdio};
use struve_llm::{ensure_controller, envelope, request};

#[derive(Parser)]
#[command(
    version,
    about = "Struve LLM: локальные проекты, ресурсы, плагины и MCP"
)]
struct Cli {
    #[arg(long, global = true, help = "Стабильный JSON в stdout")]
    json: bool,
    #[arg(long, global = true, help = "Не запускать диспетчер автоматически")]
    no_start: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Shareable diagnostics without local paths, prompts or credentials
    Diagnostics,
    /// Discover models on a loopback server or prepare a local GGUF model profile
    Setup { #[command(subcommand)] command:Setup },
    /// Shared project notes with links to source messages
    Memory { #[command(subcommand)] command: Memory },
    /// Image/video model catalog, defaults and individual resources
    Media {
        #[command(subcommand)]
        command: Media,
    },
    /// Настройки локальной генерации; запуск через chat send с mode=image или auto
    Image {
        #[command(subcommand)]
        command: Image,
    },
    /// Проверить приложение и доступность локального диспетчера
    Doctor,
    /// Запустить постоянный диспетчер в текущем процессе
    Serve,
    /// Управление отдельным фоновым диспетчером
    Controller {
        #[command(subcommand)]
        command: Controller,
    },
    /// Обнаруженные GPU/RAM/CPU и проверка совместного размещения
    Resources {
        #[command(subcommand)]
        command: Resources,
    },
    /// Профили ресурсов: выбор GPU и бюджеты
    Profile {
        #[command(subcommand)]
        command: Registry,
    },
    /// Рабочие проекты
    Project {
        #[command(subcommand)]
        command: Registry,
    },
    /// Системные резервы
    Settings {
        #[command(subcommand)]
        command: Settings,
    },
    /// Секреты в Windows Credential Manager; значение принимается только из stdin
    Secret {
        #[command(subcommand)]
        command: Secret,
    },
    /// Подключения MCP и MCP-сервер для внешнего координатора
    #[command(alias = "integration")]
    Mcp {
        #[command(subcommand)]
        command: Mcp,
    },
    /// Пакеты расширений
    Plugin {
        #[command(subcommand)]
        command: Plugin,
    },
    /// Подключённые локальные серверы моделей
    Model {
        #[command(subcommand)]
        command: Model,
    },
    /// Собственные процессы с резервированием ресурсов и лимитами Windows
    Instance {
        #[command(subcommand)]
        command: Instance,
    },
    /// Файлы для диалогов
    Attachment {
        #[command(subcommand)]
        command: Attachment,
    },
    /// Диалоги и поручения локальной модели
    Chat {
        #[command(subcommand)]
        command: Chat,
    },
    /// Прогресс и остановка задач агента
    Run {
        #[command(subcommand)]
        command: Run,
    },
    /// События после курсора
    Events {
        #[arg(long, default_value_t = 0)]
        after: i64,
        #[arg(long, default_value_t = 100)]
        limit: i64,
    },
    /// Чтение API, когда специализированной команды недостаточно
    Request {
        method: String,
        #[arg(long, default_value = "{}")]
        params: String,
    },
}
#[derive(Subcommand)]
enum Comfy {
    StartCpu,
    Dependencies { id:String, #[arg(long)] plan_id:Option<String> },
    Configure { #[arg(long)] file: PathBuf },
    Probe { id: Option<String> },
    Install { id:String, #[arg(long)] dry_run:bool },
    Lint { #[arg(long)] file:PathBuf },
}
#[derive(Subcommand)]
enum Media {
    List,
    /// Browse generation extensions and locally installed adapters
    Extensions,
    Market { #[arg(long,default_value="available")] section:String, #[arg(long,default_value="")] query:String, #[arg(long,default_value="")] family:String, #[arg(long,default_value_t=0)] offset:u64, #[arg(long,default_value_t=24)] limit:u64 },
    RefreshMarket,
    ConfigureExtension { #[arg(long)] file: PathBuf },
    Comfy { #[command(subcommand)] command: Comfy },
    /// Inspect or install a generation extension from a JSON request
    InstallExtension { #[arg(long)] file: PathBuf, #[arg(long)] dry_run: bool },
    Templates,
    /// Save a separate exposure-matched copy of a completed fixed-camera video
    LevelExposure {
        run_id: String,
        #[arg(long, default_value = "cpu-diagnostics")]
        profile: String,
    },
    Get {
        id: String,
    },
    Save {
        #[arg(long)]
        file: PathBuf,
    },
    Check {
        #[arg(long)]
        file: PathBuf,
    },
    Defaults {
        #[arg(long)]
        file: Option<PathBuf>,
    },
    Archive {
        id: String,
        #[arg(long)]
        revision: i64,
    },
}
#[derive(Subcommand)]
enum Attachment {
    List,
    Get { id: String },
    Add { path: PathBuf },
}
#[derive(Subcommand)]
enum Chat {
    /// Set the context for this chat, or restore the selected model default with auto
    Context { id:String, tokens:String, #[arg(long)] project:String, #[arg(long)] model:String },
    List,
    Get {
        id: String,
    },
    Send {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        wait: bool,
    },
}
#[derive(Subcommand)]
enum Memory {
    /// CPU latent semantic index for selected project documents
    Semantic { #[arg(long)] project:String, #[arg(value_parser=["status","build","off"],default_value="status")] action:String },
    Export { #[arg(long)] project:String, #[arg(long)] output:Option<PathBuf> },
    Sources { #[arg(long)] project:String },
    FindSource { #[arg(long)] project:String,query:String },
    Index { #[arg(long)] project:String,file:PathBuf,#[arg(long)] graph:bool },
    Refresh { #[arg(long)] project:String },
    RemoveSource { #[arg(long)] project:String,id:i64 },
    Source { #[arg(long)] project:String,id:i64,digest:String,anchor:i64 },
    List { #[arg(long)] project:String, #[arg(long,default_value_t=0)] offset:u64 },
    Search { #[arg(long)] project:String, query:String, #[arg(long,default_value_t=0)] offset:u64 },
    Add { #[arg(long)] project:String, #[arg(long)] chat:String, #[arg(long)] run:String,
        #[arg(long,default_value="user")] role:String, #[arg(long)] text:Option<String> },
    Edit { id:i64, #[arg(long)] project:String, #[arg(long)] revision:i64, #[arg(long)] text:String },
    Forget { id:i64, #[arg(long)] project:String, #[arg(long)] revision:i64 },
}
#[derive(Subcommand)]
enum Run {
    List,
    Get { id: String },
    Cancel { id: String },
    Wait { id: String },
}
#[derive(Subcommand)]
enum Controller {
    Start,
    Status,
}
#[derive(Subcommand)]
enum Setup {
    Scan { #[arg(long,default_value="ollama")] protocol:String, #[arg(long,default_value="http://127.0.0.1:11434")] endpoint:String },
    Gguf { file:PathBuf, #[arg(long)] command:Option<PathBuf>, #[arg(long)] gpu:Option<String>,
        #[arg(long, requires_all=["gpu","vram_gib"])] gpu_layers:Option<u32>,
        #[arg(long, requires="gpu_layers")] vram_gib:Option<f64> }
}
#[derive(Subcommand)]
enum Resources {
    Estimate { model:String, #[arg(long)] context:Option<u64> },
    /// Measured process-tree RAM and dedicated GPU memory from prior runs
    History { #[arg(long)] model:Option<String>, #[arg(long)] run:Option<String>, #[arg(long,default_value_t=60)] limit:i64 },
    List,
    Plan {
        #[arg(long, value_delimiter = ',', required = true)]
        profiles: Vec<String>,
    },
}
#[derive(Subcommand)]
enum Registry {
    List,
    Get {
        id: String,
    },
    Resolve {
        #[arg(long)]
        name: String,
    },
    Save {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
}
#[derive(Subcommand)]
enum Settings {
    Get,
    Save {
        #[arg(long)]
        file: PathBuf,
    },
}
#[derive(Subcommand)]
enum Secret {
    List,
    Set {
        #[arg(long)]
        name: String,
    },
    Delete {
        #[arg(long)]
        name: String,
    },
}
#[derive(Subcommand)]
enum Model {
    List,
    /// Проверить ресурсы модели без сохранения и запуска
    Plan {
        #[arg(long)]
        file: PathBuf,
    },
    /// Добавить снимок инструкций SKILL.md к модели
    SkillImport {
        id: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        disabled: bool,
    },
    Get {
        id: String,
    },
    Start {
        id: String,
    },
    Stop {
        id: String,
    },
    Save {
        #[arg(long)]
        file: PathBuf,
    },
    Probe {
        id: String,
    },
}
#[derive(Subcommand)]
enum Instance {
    List,
    Get {
        id: String,
    },
    Start {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    Stop {
        id: String,
    },
    Logs {
        id: String,
        #[arg(long, default_value_t = 100)]
        lines: u64,
    },
}
#[derive(Subcommand)]
enum Image {
    Get,
    Save {
        #[arg(long)]
        file: PathBuf,
    },
}
#[derive(Subcommand)]
enum Mcp {
    List,
    Get {
        id: String,
    },
    Save {
        #[arg(long)]
        file: PathBuf,
    },
    Probe {
        id: String,
    },
    Call {
        id: String,
        #[arg(long)]
        tool: String,
        #[arg(long)]
        project: String,
        #[arg(long, default_value = "{}")]
        arguments: String,
    },
    Serve,
}
#[derive(Subcommand)]
enum Plugin {
    List,
    Get {
        id: String,
    },
    Install {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
}
fn file_json(path: PathBuf) -> Result<Value> {
    let mut s = String::new();
    if path.to_str() == Some("-") {
        std::io::stdin()
            .take(4 * 1024 * 1024)
            .read_to_string(&mut s)?;
    } else {
        if std::fs::metadata(&path)?.len() > 4 * 1024 * 1024 {
            bail!("JSON-файл слишком велик")
        };
        s = std::fs::read_to_string(path)?;
    }
    Ok(serde_json::from_str(s.trim_start_matches('\u{feff}'))?)
}
async fn wait_run(id: &str) -> Result<Value> {
    // The controller owns execution deadlines. Full H3 runs can take several hours.
    loop {
        let mut result = request("run.get", json!({"id":id})).await?;
        if result["ok"] != true {
            return Ok(result);
        }
        let status = result["data"]["status"].as_str().unwrap_or("").to_string();
        if !["running", "starting", "queued"].contains(&status.as_str()) {
            if status != "completed" {
                result["ok"] = json!(false);
                result["error"] = json!({"code":"TASK_FAILED","message":result["data"]["error"].as_str().map(str::to_string).unwrap_or_else(||format!("Task {status}")),"retryable":false});
            }
            return Ok(result);
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
async fn registry(kind: &str, cmd: Registry) -> Result<Value> {
    match cmd {
        Registry::List => request(&format!("{kind}.list"), json!({})).await,
        Registry::Get { id } => request(&format!("{kind}.get"), json!({"id":id})).await,
        Registry::Resolve { name } => {
            let result = request(&format!("{kind}.list"), json!({})).await?;
            if result["ok"] != true {
                return Ok(result);
            }
            let items = result["data"].as_array().unwrap();
            let found: Vec<_> = items
                .iter()
                .filter(|v| {
                    v["id"].as_str() == Some(&name)
                        || v["name"].as_str().map(|s| s.to_lowercase()) == Some(name.to_lowercase())
                })
                .collect();
            if found.len() != 1 {
                bail!("Ожидался один объект, найдено {}", found.len())
            }
            Ok(envelope(Ok(found[0].clone())))
        }
        Registry::Save { file, dry_run } => {
            let value = file_json(file)?;
            if dry_run {
                if kind == "profile" {
                    struve_llm::hardware::validate_profile(&value)?;
                }
                return Ok(envelope(Ok(
                    json!({"dry_run":true,"value":value,"scope":"Локальная проверка формы; конфликт ревизии проверяется при сохранении"}),
                )));
            }
            request(&format!("{kind}.save"), value).await
        }
    }
}
async fn run(cli: &Cli) -> Result<Value> {
    if matches!(cli.command, Command::Serve) {
        struve_llm::server::serve().await?;
        return Ok(envelope(Ok(json!({"stopped":true}))));
    }
    if matches!(
        cli.command,
        Command::Mcp {
            command: Mcp::Serve
        }
    ) {
        let mut child = tokio::process::Command::new(struve_llm::app_dir().join("node.exe"));
        child.env("STRUVE_LLM_ROOT", struve_llm::root());
        child
            .arg(struve_llm::source_dir().join("bridge/coordinator.mjs"))
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let status = child.status().await?;
        std::process::exit(status.code().unwrap_or(1));
    }
    if !cli.no_start
        && !matches!(
            cli.command,
            Command::Controller {
                command: Controller::Status
            }
        )
    {
        ensure_controller().await?;
    }
    // Move command separately to keep clap's parsed input readable.
    unreachable!("handled by main")
}
#[tokio::main]
async fn main() {
    let cli = match Cli::try_parse() {
        Ok(v) => v,
        Err(e) => {
            if std::env::args().any(|s| s == "--json")
                && !matches!(
                    e.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                )
            {
                println!("{}", envelope(Err(anyhow::anyhow!(e.to_string()))));
                std::process::exit(2)
            }
            e.exit()
        }
    };
    if matches!(
        cli.command,
        Command::Serve
            | Command::Mcp {
                command: Mcp::Serve
            }
    ) {
        if let Err(e) = run(&cli).await {
            let _ = std::fs::write(
                struve_llm::root().join("logs/controller.log"),
                format!("{e}\n"),
            );
            eprintln!("{e}");
            std::process::exit(1)
        }
        return;
    }
    let output=async {
        if !cli.no_start && !matches!(cli.command,Command::Controller{command:Controller::Status}){ensure_controller().await?;}
        match cli.command{
            Command::Media{command}=>match command {
                Media::List=>request("media.list",json!({})).await,
                Media::Market{section,query,family,offset,limit}=>request("media.market.list",json!({ "section":section,"query":query,"family":family,"offset":offset,"limit":limit})).await,
                Media::RefreshMarket=>request("media.market.refresh",json!({"force":true})).await,
                Media::Extensions=>request("media.extensions.catalog",json!({})).await,
                Media::ConfigureExtension{file}=>request("media.extensions.configure",file_json(file)?).await,
                Media::Comfy{command}=>match command {
                    Comfy::StartCpu=>request("media.comfy.start_cpu",json!({})).await,
                    Comfy::Dependencies{id,plan_id}=>request("media.comfy.dependencies",json!({"id":id,"plan_id":plan_id})).await,
                    Comfy::Configure{file}=>request("media.comfy.save",file_json(file)?).await,
                    Comfy::Probe{id}=>request("media.comfy.probe",json!({"id":id})).await,
                    Comfy::Install{id,dry_run}=>request("media.comfy.install",json!({"id":id,"dry_run":dry_run})).await,
                    Comfy::Lint{file}=>request("media.comfy.lint",file_json(file)?).await,
                },
                Media::InstallExtension{file,dry_run}=>{let mut p=file_json(file)?;p["dry_run"]=json!(dry_run);request("media.extensions.install",p).await},
                Media::Templates=>request("media.templates",json!({})).await,
                Media::LevelExposure{run_id,profile}=>request("media.level_exposure",json!({"run_id":run_id,"profile_id":profile})).await,
                Media::Get{id}=>request("media.get",json!({"id":id})).await,
                Media::Save{file}=>request("media.save",file_json(file)?).await,
                Media::Check{file}=>request("media.check",file_json(file)?).await,
                Media::Defaults{file}=>if let Some(file)=file{request("media.defaults.save",file_json(file)?).await}else{request("media.defaults.get",json!({})).await},
                Media::Archive{id,revision}=>request("media.archive",json!({"id":id,"revision":revision})).await,
            },
            Command::Image{command}=>match command{Image::Get=>request("image.config.get",json!({})).await,Image::Save{file}=>request("image.config.save",file_json(file)?).await},
            Command::Doctor=>request("doctor",json!({})).await,
            Command::Controller{command:Controller::Start}=>request("doctor",json!({})).await,
            Command::Controller{command:Controller::Status}=>request("ping",json!({})).await,
            Command::Diagnostics=>request("diagnostics.report",json!({})).await,
            Command::Setup{command:Setup::Scan{protocol,endpoint}}=>request("setup.scan",json!({"endpoint":endpoint,"protocol":protocol})).await,
            Command::Setup{command:Setup::Gguf{file,command,gpu,gpu_layers,vram_gib}}=>request("setup.gguf",json!({"file":file,"command":command,"gpu_id":gpu,"gpu_layers":gpu_layers,"vram_gib":vram_gib})).await,
            Command::Resources{command:Resources::Estimate{model,context}}=>request("resources.estimate",json!({"model_id":model,"context_tokens":context})).await,
            Command::Resources{command:Resources::History{model,run,limit}}=>request("resources.history",json!({"model_id":model,"run_id":run,"limit":limit})).await,
            Command::Resources{command:Resources::List}=>request("resources.list",json!({})).await,
            Command::Resources{command:Resources::Plan{profiles}}=>request("resources.plan",json!({"profile_ids":profiles})).await,
            Command::Profile{command}=>registry("profile",command).await,
            Command::Project{command}=>registry("project",command).await,
            Command::Settings{command:Settings::Get}=>request("settings.get",json!({})).await,
            Command::Settings{command:Settings::Save{file}}=>request("settings.save",file_json(file)?).await,
            Command::Secret{command:Secret::List}=>request("secret.list",json!({})).await,
            Command::Secret{command:Secret::Delete{name}}=>request("secret.delete",json!({"id":name})).await,
            Command::Secret{command:Secret::Set{name}}=>{let mut value=String::new();std::io::stdin().take(64*1024).read_to_string(&mut value)?;request("secret.save",json!({"id":name,"value":value.trim_end_matches(['\r','\n'])})).await},
            Command::Mcp{command}=>match command{Mcp::List=>request("mcp.list",json!({})).await,Mcp::Get{id}=>request("mcp.get",json!({"id":id})).await,Mcp::Save{file}=>request("mcp.save",file_json(file)?).await,Mcp::Probe{id}=>request("mcp.probe",json!({"id":id})).await,Mcp::Call{id,tool,project,arguments}=>request("mcp.call",json!({"id":id,"tool":tool,"project_id":project,"arguments":serde_json::from_str::<Value>(&arguments)?})).await,Mcp::Serve=>unreachable!()},
            Command::Plugin{command}=>match command{Plugin::List=>request("plugin.list",json!({})).await,Plugin::Get{id}=>request("plugin.get",json!({"id":id})).await,Plugin::Install{file,dry_run}=>request("plugin.install",json!({"manifest":file_json(file)?,"dry_run":dry_run})).await},
            Command::Model{command}=>match command{Model::List=>request("model.list",json!({})).await,Model::Plan{file}=>request("model.plan",file_json(file)?).await,Model::SkillImport{id,file,disabled}=>{let result=request("model.get",json!({"id":id})).await?;if result["ok"]!=true{return Ok(result)}let mut model=result["data"].clone();let mut skill=struve_llm::model_config::read_skill(&file)?;skill["enabled"]=json!(!disabled);if !model["skills"].is_array(){model["skills"]=json!([])}model["skills"].as_array_mut().unwrap().push(skill);request("model.save",model).await},Model::Get{id}=>request("model.get",json!({"id":id})).await,Model::Start{id}=>request("model.start",json!({"id":id})).await,Model::Stop{id}=>request("model.stop",json!({"id":id})).await,Model::Save{file}=>request("model.save",file_json(file)?).await,Model::Probe{id}=>request("model.probe",json!({"id":id})).await},
            Command::Instance{command}=>match command{Instance::List=>request("instance.list",json!({})).await,Instance::Get{id}=>request("instance.get",json!({"id":id})).await,Instance::Start{file,dry_run}=>{let mut spec=file_json(file)?;spec["dry_run"]=json!(dry_run);request("instance.start",spec).await},Instance::Stop{id}=>request("instance.stop",json!({"id":id})).await,Instance::Logs{id,lines}=>request("instance.logs",json!({"id":id,"lines":lines})).await},
            Command::Attachment{command}=>match command{Attachment::List=>request("attachment.list",json!({})).await,Attachment::Get{id}=>request("attachment.get",json!({"id":id})).await,Attachment::Add{path}=>request("attachment.import",json!({"path":path})).await},
            Command::Memory{command}=>match command {
                Memory::FindSource{project,query}=>request("library.search",json!({"project_id":project,"query":query})).await,
                Memory::Semantic{project,action}=>if action=="status"{request("library.semantic_status",json!({"project_id":project})).await}else{request("library.semantic",json!({"project_id":project,"enabled":action=="build"})).await},
                Memory::Sources{project}=>request("library.list",json!({"project_id":project})).await,
                Memory::Index{project,file,graph}=>request("library.index",json!({"project_id":project,"file":file,"kind":if graph{"graph"}else{"text"}})).await,
                Memory::Refresh{project}=>request("library.refresh",json!({"project_id":project})).await,
                Memory::RemoveSource{project,id}=>request("library.forget",json!({"project_id":project,"id":id})).await,
                Memory::Source{project,id,digest,anchor}=>request("library.source",json!({"project_id":project,"file_id":id,"digest":digest,"anchor":anchor})).await,
                Memory::Export{project,output}=>{
                    let value=request("memory.export",json!({"project_id":project})).await?;
                    if let Some(file)=output.filter(|_|value["ok"]==true){
                        use std::io::Write;
                        let mut out=std::fs::OpenOptions::new().create_new(true).write(true).open(&file)?;
                        out.write_all(value["data"]["markdown"].as_str().unwrap().as_bytes())?;
                        Ok(envelope(Ok(json!({"path":file,"entries":value["data"]["entries"]}))))
                    }else{Ok(value)}
                },
                Memory::List{project,offset}=>request("memory.list",json!({"project_id":project,"offset":offset})).await,
                Memory::Search{project,query,offset}=>request("memory.search",json!({"project_id":project,"query":query,"offset":offset})).await,
                Memory::Add{project,chat,run,role,text}=>request("memory.add",json!({"project_id":project,"chat_id":chat,"run_id":run,"role":role,"text":text})).await,
                Memory::Edit{id,project,revision,text}=>request("memory.edit",json!({"id":id,"project_id":project,"revision":revision,"text":text})).await,
                Memory::Forget{id,project,revision}=>request("memory.forget",json!({"id":id,"project_id":project,"revision":revision})).await,
            },
            Command::Chat{command}=>match command{
                Chat::Context{id,tokens,project,model}=>{
                    let value=if tokens.eq_ignore_ascii_case("auto"){Value::Null}else{json!(tokens.parse::<u64>()?)};
                    request("chat.context",json!({"project_id":project,"chat_id":id,"model_id":model,"tokens":value})).await
                },
                Chat::List=>request("chat.list",json!({})).await,Chat::Get{id}=>request("chat.get",json!({"id":id})).await,Chat::Send{file,wait}=>{let result=request("chat.send",file_json(file)?).await?;if wait&&result["ok"]==true{wait_run(result["data"]["id"].as_str().unwrap()).await}else{Ok(result)}}},
            Command::Run{command}=>match command{Run::List=>request("run.list",json!({})).await,Run::Get{id}=>request("run.get",json!({"id":id})).await,Run::Cancel{id}=>request("run.cancel",json!({"id":id})).await,Run::Wait{id}=>wait_run(&id).await},
            Command::Events{after,limit}=>request("events.list",json!({"after":after,"limit":limit})).await,
            Command::Request{method,params}=>{if !(method.ends_with(".list")||method.ends_with(".get")||["snapshot","doctor","ping","resources.plan","model.plan"].contains(&method.as_str())){bail!("Raw request допускает только чтение. Используйте именованную команду изменения")}request(&method,serde_json::from_str(&params)?).await},
            _=>unreachable!()
        }
    }.await;
    let value = output.unwrap_or_else(|e| envelope(Err(e)));
    let ok = value["ok"] == true;
    if cli.json {
        println!("{value}")
    } else {
        println!("{}", serde_json::to_string_pretty(&value).unwrap())
    }
    if !ok {
        std::process::exit(1)
    }
}
