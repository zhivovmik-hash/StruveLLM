#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use serde_json::Value;
use tauri_plugin_dialog::DialogExt;
mod viewer;
mod desktop_tray;
use tauri::Manager;
#[tauri::command]
fn window_theme(webview: tauri::Webview, theme: String) -> Result<String, String> {
    viewer::local(&webview)?;
    let preferred = match theme.as_str() {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        "system" => None,
        _ => return Err("Unknown window theme".into()),
    };
    let window = webview.window();
    // Reset the native WebView preference before resolving the system theme.
    // matchMedia alone still reflects a previous explicit light/dark choice.
    window.set_theme(preferred).map_err(|e| e.to_string())?;
    Ok(match window.theme().map_err(|e| e.to_string())? {
        tauri::Theme::Light => "light",
        _ => "dark",
    }
    .into())
}
#[tauri::command]
async fn api(webview: tauri::Webview, method: String, params: Value) -> Result<Value, String> {
    viewer::local(&webview)?;
    let state=webview.state::<desktop_tray::Desktop>();
    if state.closing(){return Err("Struve LLM is exiting".into())}
    let _guard=state.requests.read().await;
    if state.closing(){return Err("Struve LLM is exiting".into())}
    struve_llm::ensure_controller()
        .await
        .map_err(|e| e.to_string())?;
    drop(_guard);
    if state.closing(){return Err("Struve LLM is exiting".into())}
    struve_llm::request(&method, params)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn save_export(app:tauri::AppHandle,webview:tauri::Webview,name:String,content:String)->Result<Value,String>{
    viewer::local(&webview)?;
    let ext=match name.as_str(){"struve-diagnostics.json"=>"json","project-memory.md"=>"md",_=>return Err("Unknown export".into())};
    if content.len()>32*1024*1024{return Err("Export exceeds 32 MiB".into())}
    tauri::async_runtime::spawn_blocking(move||{
        let Some(file)=app.dialog().file().add_filter("Export",&[ext]).set_file_name(&name).blocking_save_file() else{return Ok(Value::Null)};
        let dest=file.into_path().map_err(|e|e.to_string())?;
        std::fs::write(&dest,content.as_bytes()).map_err(|e|e.to_string())?;
        Ok(serde_json::json!({"path":dest,"bytes":content.len()}))
    }).await.map_err(|e|e.to_string())?
}
#[tauri::command]
async fn choose_library_file(app:tauri::AppHandle,webview:tauri::Webview,graph:bool)->Result<Value,String>{
    viewer::local(&webview)?;
    tauri::async_runtime::spawn_blocking(move||{
        let files=app.dialog().file();
        let files=if graph{files.add_filter("Graphify",&["json"])}else{files.add_filter("Project source",&["md","txt","rst","rs","py","js","jsx","ts","tsx","cpp","h","hpp","c","cs","toml","yaml","yml","json","csv"])};
        let Some(file)=files.blocking_pick_file() else{return Ok(Value::Null)};
        Ok(serde_json::json!({"path":file.into_path().map_err(|e|e.to_string())?}))
    }).await.map_err(|e|e.to_string())?
}
#[tauri::command]
async fn choose_file(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    folder: bool,
) -> Result<Value, String> {
    viewer::local(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let builder = app.dialog().file();
        let result = if folder {
            builder.blocking_pick_folder()
        } else {
            builder.add_filter("JSON", &["json"]).blocking_pick_file()
        };
        match result {
            None => Ok(Value::Null),
            Some(p) => {
                let path = p.into_path().map_err(|e| e.to_string())?;
                if folder {
                    return Ok(serde_json::json!({"path":path}));
                }
                let m = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                if m.len() > 4 * 1024 * 1024 {
                    return Err("Файл слишком велик".into());
                }
                let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::json!({"path":path,"value":value}))
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn choose_skill(app: tauri::AppHandle, webview: tauri::Webview) -> Result<Value, String> {
    viewer::local(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .add_filter("Skill instructions", &["md", "txt"])
            .blocking_pick_file()
        else {
            return Ok(Value::Null);
        };
        let path = file.into_path().map_err(|e| e.to_string())?;
        struve_llm::model_config::read_skill(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn choose_executable(
    app: tauri::AppHandle,
    webview: tauri::Webview,
) -> Result<Value, String> {
    viewer::local(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .add_filter("Program", &["exe"])
            .blocking_pick_file()
        else {
            return Ok(Value::Null);
        };
        let path = file.into_path().map_err(|e| e.to_string())?;
        Ok(serde_json::json!({"path":path}))
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn choose_image_model(
    gguf_only: Option<bool>,
    app: tauri::AppHandle,
    webview: tauri::Webview,
) -> Result<Value, String> {
    viewer::local(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .add_filter(if gguf_only==Some(true){"GGUF"}else{"Image model"}, if gguf_only==Some(true){&["gguf"][..]}else{&["safetensors","gguf","ckpt"][..]})
            .blocking_pick_file()
        else {
            return Ok(Value::Null);
        };
        Ok(serde_json::json!({"path":file.into_path().map_err(|e|e.to_string())?}))
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn choose_attachments(
    app: tauri::AppHandle,
    webview: tauri::Webview,
) -> Result<Value, String> {
    viewer::local(&webview)?;
    let paths =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_files())
            .await
            .map_err(|e| e.to_string())?;
    let mut imported = vec![];
    if let Some(paths) = paths {
        if paths.len() > 8 {
            return Err("Select up to 8 attachments".into());
        }
        for file in paths {
            let path = file.into_path().map_err(|e| e.to_string())?;
            let response =
                struve_llm::request("attachment.import", serde_json::json!({"path":path}))
                    .await
                    .map_err(|e| e.to_string())?;
            if response["ok"] != true {
                return Err(response["error"]["message"]
                    .as_str()
                    .unwrap_or("Import failed")
                    .into());
            }
            imported.push(response["data"].clone());
        }
    }
    Ok(serde_json::json!(imported))
}
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app,_,_|desktop_tray::show(app)))
        .manage(desktop_tray::Desktop::default())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let root = struve_llm::root();
            let language = struve_llm::startup_language(&root);
            let directory = root.join("data/webview");
            std::fs::create_dir_all(&directory)?;
            tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?
                .data_directory(directory)
                .initialization_script(format!("window.__STRUVE_INITIAL_LANGUAGE__ = '{language}';"))
                .build()?;
            desktop_tray::install(app.handle(), language == "en")?;
            Ok(())
        })
        .on_window_event(|w,event|{if w.label()=="main" {
            if let tauri::WindowEvent::CloseRequested{api,..}=event{api.prevent_close();let _=w.hide();}
        }})
        .invoke_handler(tauri::generate_handler![
            api,
            desktop_tray::tray_update,
            desktop_tray::tray_actions,
            window_theme,
            choose_file,
            save_export,
            choose_library_file,
            choose_skill,
            choose_attachments,
            choose_executable,
            choose_image_model,
            viewer::preview_file,
            viewer::preview_path,
            viewer::preview_image,
            viewer::browser_tab
        ])
        .run(tauri::generate_context!())
        .expect("Struve desktop runtime failed");
}
