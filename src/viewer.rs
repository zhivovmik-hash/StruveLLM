use serde_json::{json, Value};
use std::io::Read;
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
pub fn local(view: &tauri::Webview) -> Result<(), String> {
    if view.label() != "main" {
        Err("Application commands are unavailable to browser tabs".into())
    } else {
        Ok(())
    }
}
#[tauri::command]
pub async fn preview_image(
    webview: tauri::Webview,
    base_path: String,
    relative_path: String,
) -> Result<String, String> {
    local(&webview)?;
    let base = std::path::PathBuf::from(base_path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let folder = base.parent().ok_or("No document folder")?;
    let relative = std::path::PathBuf::from(relative_path);
    if relative.is_absolute() {
        return Err("Only relative document images are supported".into());
    }
    let path = folder
        .join(relative)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !path.starts_with(folder) {
        return Err("Image must be inside the document folder".into());
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !["png", "jpg", "jpeg", "gif", "webp", "bmp"].contains(&ext.as_str()) {
        return Err("Unsupported image format".into());
    }
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 20 * 1024 * 1024 {
        return Err("Image exceeds 20 MiB".into());
    }
    use base64::Engine;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mime = if ext == "jpg" { "jpeg" } else { &ext };
    Ok(format!(
        "data:image/{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}
#[tauri::command]
pub async fn preview_file(app: tauri::AppHandle, webview: tauri::Webview) -> Result<Value, String> {
    local(&webview)?;
    let selected =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_file())
            .await
            .map_err(|e| e.to_string())?;
    let Some(selected) = selected else {
        return Ok(Value::Null);
    };
    preview_path_inner(selected.into_path().map_err(|e| e.to_string())?).await
}
#[tauri::command]
pub async fn preview_path(webview: tauri::Webview, path: String) -> Result<Value, String> {
    local(&webview)?;
    preview_path_inner(std::path::PathBuf::from(path)).await
}
async fn preview_path_inner(path: std::path::PathBuf) -> Result<Value, String> {
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ["xlsx", "docx"].contains(&extension.as_str()) {
        let mut command = tokio::process::Command::new(struve_llm::app_dir().join("node.exe"));
        command
            .arg(struve_llm::source_dir().join("bridge/preview.mjs"))
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let out = tokio::time::timeout(std::time::Duration::from_secs(25), command.output())
            .await
            .map_err(|_| "Document preview timed out")?
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(1500)
                .collect());
        }
        if out.stdout.len() > 8 * 1024 * 1024 {
            return Err("Spreadsheet preview exceeds 8 MiB".into());
        }
        let doc: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
        return Ok(
            json!({"name":path.file_name().unwrap_or_default().to_string_lossy(),"path":path,"kind":if extension=="xlsx"{"spreadsheet"}else{"rich-document"},"format":extension,"sheets":doc["sheets"],"sheets_truncated":doc["sheets_truncated"],"html":doc["html"],"warnings":doc["warnings"]}),
        );
    }
    tauri::async_runtime::spawn_blocking(move || read_basic_preview(&path))
        .await
        .map_err(|e| e.to_string())?
}
fn read_basic_preview(path: &std::path::Path) -> Result<Value, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if ["mp4", "webm"].contains(&ext.as_str()) {
        if size > 100 * 1024 * 1024 {
            return Err("Video preview limit: 100 MiB".into());
        }
        use base64::Engine;
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        return Ok(
            json!({"name":name,"path":path,"kind":"video","url":format!("data:video/{ext};base64,{}",base64::engine::general_purpose::STANDARD.encode(bytes))}),
        );
    }
    if size > 30 * 1024 * 1024 {
        return Err("Preview limit: 30 MiB".into());
    }
    if ext == "pdf" {
        return Ok(
            json!({"name":name,"path":path,"kind":"pdf","url":tauri::Url::from_file_path(path).map_err(|_|"Invalid file path")?.to_string()}),
        );
    }
    if ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"].contains(&ext.as_str()) {
        use base64::Engine;
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let mime = match ext.as_str() {
            "jpg" | "jpeg" => "jpeg",
            "ico" => "x-icon",
            other => other,
        };
        return Ok(
            json!({"name":name,"path":path,"kind":"image","url":format!("data:image/{mime};base64,{}",base64::engine::general_purpose::STANDARD.encode(bytes))}),
        );
    }
    let content = if ext == "docx" {
        let mut archive =
            zip::ZipArchive::new(std::fs::File::open(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let entry = archive
            .by_name("word/document.xml")
            .map_err(|e| e.to_string())?;
        let mut xml = String::new();
        entry
            .take(4 * 1024 * 1024 + 1)
            .read_to_string(&mut xml)
            .map_err(|e| e.to_string())?;
        if xml.len() > 4 * 1024 * 1024 {
            return Err("Document text exceeds 4 MiB".into());
        }
        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut out = String::new();
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Text(t)) => {
                    out.push_str(&t.unescape().map_err(|e| e.to_string())?)
                }
                Ok(quick_xml::events::Event::End(t)) if t.name().as_ref() == b"w:p" => {
                    out.push('\n')
                }
                Ok(quick_xml::events::Event::Eof) => break,
                Err(e) => return Err(e.to_string()),
                _ => {}
            }
        }
        out
    } else {
        if size > 4 * 1024 * 1024 {
            return Err("Text preview limit: 4 MiB".into());
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        if bytes.contains(&0) {
            return Err("Unsupported binary document format".into());
        }
        String::from_utf8(bytes).map_err(|_| "Text preview requires UTF-8".to_string())?
    };
    Ok(
        json!({"name":name,"path":path,"kind":if ["md","txt","docx","log","csv"].contains(&ext.as_str()){"document"}else{"code"},"content":content,"format":ext}),
    )
}
// Resolve an opaque Google result with the same WebView2 profile. Stop at the
// outgoing navigation: the destination is read later through the project's web_read permission.
async fn result_destination(app: tauri::AppHandle, referral: String) -> Option<String> {
    let link = tauri::Url::parse(&referral).ok()?;
    if link.scheme() != "https" || link.host_str() != Some("www.google.com")
        || !matches!(link.path(), "/goto"|"/url") || referral.len() > 4096 { return None }
    let window = app.get_window("main")?;
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    let builder = tauri::webview::WebviewBuilder::new(
        format!("search-link-{}", uuid::Uuid::new_v4().simple()),
        tauri::WebviewUrl::External("about:blank".parse().ok()?),
    ).data_directory(struve_llm::root().join("data/webview"))
    .on_new_window(|_,_|tauri::webview::NewWindowResponse::Deny)
    .on_navigation(move |url| {
        if url.as_str()=="about:blank" {return true}
        if !matches!(url.scheme(),"https"|"http") || !url.username().is_empty() || url.password().is_some() {return false}
        if url.host_str().is_some_and(|h|h=="google.com" || h.ends_with(".google.com")) {
            return url.scheme()=="https" && matches!(url.path(),"/url"|"/goto")
        }
        if url.host_str().is_some() && url.as_str().len()<=4096 {let _=send.try_send(url.to_string());}
        false
    });
    let child=window.add_child(builder,tauri::LogicalPosition::new(-4000.,-4000.),tauri::LogicalSize::new(240.,180.)).ok()?;
    let _=child.hide();
    let result=if child.navigate(link).is_ok(){
        tokio::time::timeout(std::time::Duration::from_secs(8),receive.recv()).await.ok().flatten()
    }else{None};
    let _=child.close();
    result
}

async fn inspect_search(app: &tauri::AppHandle, tab: &tauri::Webview, expected: &str, operation: &str) -> Result<Value,String> {
    let expected=tauri::Url::parse(expected).map_err(|e| e.to_string())?;
    let (host, param)=match (expected.host_str(),expected.path()){
        (Some("www.google.com"),"/search")=>("www.google.com","q"),
        (Some("yandex.ru"),"/search/")=>("yandex.ru","text"),
        _=>return Err("Invalid search engine".into())
    };
    let page=tab.url().map_err(|e|e.to_string())?;
    if !matches!(page.scheme(),"https"|"http") || !page.username().is_empty() || page.password().is_some() {
        return Ok(json!({"state":"loading"}))
    }
    let provider_host=page.host_str()==Some(host) || (host=="yandex.ru" && matches!(page.host_str(),Some("ya.ru"|"www.yandex.ru"|"yandex.com"|"www.yandex.com")));
    if provider_host && (page.path().contains("showcaptcha") || page.path().starts_with("/sorry/")) {
        return Ok(json!({"state":"verification"}))
    }
    if !provider_host {
        if operation!="select-source" { return Ok(json!({"state":"source"})) }
        return Ok(json!({"state":"selected","results":[{"title":page.host_str().unwrap_or("Page"),"url":page.as_str()}]}))
    }
    let query=|u:&tauri::Url| u.query_pairs().find(|(k,_)|k==param).map(|(_,v)|v.trim().to_string());
    if page.scheme()!="https" || page.path().trim_end_matches('/')!="/search" || query(&expected).is_none() || query(&expected)!=query(&page) {
        return Ok(json!({"state":"waiting"}))
    }
    let (send,mut recv)=tokio::sync::mpsc::channel(1);
    tab.eval_with_callback(include_str!("search-page.js"),move |json| {let _=send.try_send(json);})
        .map_err(|e|e.to_string())?;
    let raw=tokio::time::timeout(std::time::Duration::from_secs(4),recv.recv()).await
        .map_err(|_|"Browser is busy")?.ok_or("Browser tab closed")?;
    if tab.url().map_err(|e|e.to_string())? != page || raw.len()>65536 {return Ok(json!({"state":"loading"}))}
    let value:Value=serde_json::from_str(&raw).map_err(|e|e.to_string())?;
    let Some(rows)=value["results"].as_array() else{return Ok(value)};
    if !rows.iter().any(|r|r["redirect"].is_string()){return Ok(value)}
    if operation!="resolve-results"{return Ok(json!({"state":"redirects"}))}
    let mut pending=Vec::new();
    for row in rows.iter().take(6) {
        let direct=row["url"].as_str().map(str::to_string);
        let link=row["redirect"].as_str().map(str::to_string);
        let title=row["title"].as_str().unwrap_or("").to_owned();
        let app=app.clone();
        pending.push(tauri::async_runtime::spawn(async move {
            let dest=match link {Some(link)=>result_destination(app,link).await,None=>direct};
            dest.map(|url|json!({"title":title,"url":url}))
        }));
    }
    let mut results=Vec::new();
    for handle in pending {if let Ok(Some(row))=handle.await{results.push(row)}}
    if tab.url().map_err(|e|e.to_string())? != page {return Ok(json!({"state":"loading"}))}
    if results.is_empty(){return Ok(json!({"state":"choose"}))}
    Ok(json!({"state":"results","results":results}))
}
#[tauri::command]
pub async fn browser_tab(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    id: String,
    operation: String,
    url: Option<String>,
    bounds: Option<Value>,
    theme: Option<String>,
) -> Result<Value, String> {
    local(&webview)?;
    if !id.starts_with("viewer-") || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("Invalid browser tab".into());
    }
    let label = id.clone();
    if operation == "create" {
        let target = tauri::Url::parse(url.as_deref().unwrap_or("about:blank"))
            .map_err(|e| e.to_string())?;
        if !["http", "https", "about", "file"].contains(&target.scheme()) {
            return Err("Unsupported URL scheme".into());
        }
        let target_file = if target.scheme() == "file" {
            Some(target.clone())
        } else {
            None
        };
        let handle = app.clone();
        let tab_id = id.clone();
        let loaded_handle = app.clone();
        let loaded_id = id.clone();
        let builder = tauri::webview::WebviewBuilder::new(&id, tauri::WebviewUrl::External(target))
            .data_directory(struve_llm::root().join("data/webview"))
            .on_navigation(move |u| {
                let allowed = if let Some(file) = &target_file {
                    u.scheme() == "file" && u.path() == file.path()
                } else {
                    ["http", "https", "about"].contains(&u.scheme())
                };
                if allowed {
                    let _ = handle.emit_to(
                        "main",
                        "viewer-location",
                        json!({"id":tab_id,"url":u.as_str()}),
                    );
                }
                allowed
            })
            .on_page_load(move |_, _| {
                let _ = loaded_handle.emit_to("main", "viewer-loaded", json!({"id":loaded_id}));
            })
            .on_new_window({
                let app=app.clone();
                let owner=id.clone();
                move |url,_| {
                    if owner.starts_with("viewer-search-") && matches!(url.scheme(),"http"|"https")
                        && url.username().is_empty() && url.password().is_none() {
                        if let Some(tab)=app.get_webview(&owner){tauri::async_runtime::spawn(async move {let _=tab.navigate(url);});}
                    }
                    tauri::webview::NewWindowResponse::Deny
                }
            });
        let window = app.get_window("main").ok_or("Main window not found")?;
        let child = window
            .add_child(
                builder,
                tauri::LogicalPosition::new(0., 0.),
                // Chromium's PDF viewer computes its initial fit scale from this size.
                // A 1x1 hidden view can retain a near-zero page scale after restoration.
                tauri::LogicalSize::new(
                    bounds
                        .as_ref()
                        .and_then(|b| b["width"].as_f64())
                        .unwrap_or(560.)
                        .max(330.),
                    bounds
                        .as_ref()
                        .and_then(|b| b["height"].as_f64())
                        .unwrap_or(720.)
                        .max(240.),
                ),
            )
            .map_err(|e| e.to_string())?;
        child.hide().map_err(|e| e.to_string())?;
    }
    let tab = app.get_webview(&label).ok_or("Tab not found")?;
    match operation.as_str() {
        "create" => {}
        "search-results" | "resolve-results" | "select-source" if label.starts_with("viewer-search-") => {
            return inspect_search(&app,&tab,url.as_deref().unwrap_or(""),&operation).await
        }
        "close" => tab.close().map_err(|e| e.to_string())?,
        "hide" => tab.hide().map_err(|e| e.to_string())?,
        "show" => {
            if let Some(b) = bounds {
                tab.set_position(tauri::LogicalPosition::new(
                    b["x"].as_f64().unwrap_or(0.),
                    b["y"].as_f64().unwrap_or(0.),
                ))
                .map_err(|e| e.to_string())?;
                tab.set_size(tauri::LogicalSize::new(
                    b["width"].as_f64().unwrap_or(1.).max(1.),
                    b["height"].as_f64().unwrap_or(1.).max(1.),
                ))
                .map_err(|e| e.to_string())?;
            }
            let light = theme.as_deref() == Some("light");
            let style = if light {
                "html{color-scheme:light}*{scrollbar-color:#b1bab5 #eef0ee;scrollbar-width:thin}"
            } else {
                "html{color-scheme:dark}*{scrollbar-color:#454d4e #181b1c;scrollbar-width:thin}"
            };
            tab.eval(&format!("(()=>{{let s=document.getElementById('struve-scrollbars');if(!s){{s=document.createElement('style');s.id='struve-scrollbars';document.head.appendChild(s);}}s.textContent={};}})()",serde_json::to_string(style).unwrap())).map_err(|e|e.to_string())?;
            tab.show().map_err(|e| e.to_string())?;
        }
        "navigate" => {
            let target = tauri::Url::parse(url.as_deref().unwrap_or("about:blank"))
                .map_err(|e| e.to_string())?;
            if !["http", "https", "about"].contains(&target.scheme()) {
                return Err("Enter an HTTP or HTTPS address".into());
            }
            tab.navigate(target).map_err(|e| e.to_string())?
        }
        "back" => tab.eval("history.back()").map_err(|e| e.to_string())?,
        "forward" => tab.eval("history.forward()").map_err(|e| e.to_string())?,
        "reload" => tab.reload().map_err(|e| e.to_string())?,
        _ => return Err("Unknown browser action".into()),
    };
    Ok(json!({"id":id}))
}
