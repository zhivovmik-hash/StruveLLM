use crate::{root, server::App};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::Path;
pub async fn import(app: &App, path: &Path) -> Result<Value> {
    if !path.is_absolute() || !path.is_file() {
        bail!("Choose an existing file")
    }
    let bytes = std::fs::metadata(path)?.len();
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = if ["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) {
        "image"
    } else if ["mp4", "mkv", "mov", "webm", "avi", "m4v"].contains(&ext.as_str()) {
        "video"
    } else if ["docx", "pdf", "xlsx"].contains(&ext.as_str()) {
        "document"
    } else {
        "text"
    };
    let limit = match kind {
        "video" => 200 * 1024 * 1024,
        "image" => 20 * 1024 * 1024,
        "document" => 30 * 1024 * 1024,
        _ => 4 * 1024 * 1024,
    };
    if bytes > limit {
        bail!(
            "File exceeds the {kind} attachment limit ({} MiB)",
            limit / 1024 / 1024
        )
    }
    if kind == "text" {
        let bytes = std::fs::read(path)?;
        if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            bail!("This attachment requires UTF-8 text, a supported document, image or video")
        }
    }
    let id = format!("file-{}", uuid::Uuid::new_v4().simple());
    let directory = root().join("attachments").join(&id);
    std::fs::create_dir_all(&directory)?;
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let destination = directory.join(&name);
    std::fs::copy(path, &destination)?;
    let mut assets = json!({"assets":[],"asset_bytes":0,"warnings":[]});
    if ["md", "markdown"].contains(&ext.as_str()) {
        let mut command = tokio::process::Command::new(crate::app_dir().join("node.exe"));
        command
            .arg(crate::source_dir().join("bridge/markdown-assets.mjs"))
            .arg(path)
            .arg(&directory)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(20), command.output()).await;
        match result {
            Ok(Ok(output)) if output.status.success() => {
                assets = serde_json::from_slice(&output.stdout)?;
            }
            _ => {
                assets["warnings"] = json!([{"image":"","reason":"Could not preserve Markdown images; the document text is intact"}]);
            }
        }
    }
    app.save("attachment",json!({"id":id,"name":name,"path":destination,"kind":kind,"format":ext,"bytes":bytes,"assets":assets["assets"],"asset_bytes":assets["asset_bytes"],"warnings":assets["warnings"],"imported_at":chrono::Utc::now().to_rfc3339()}))
}
