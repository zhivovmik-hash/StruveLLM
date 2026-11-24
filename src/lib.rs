pub mod agent;
pub mod attachments;
pub mod engines;
mod h3_progress;
pub mod hardware;
mod gpu_memory;
mod meter;
mod setup;
mod library;
mod semantic;
pub mod image_edit;
pub mod image_jobs;
pub mod media_catalog;
pub mod media_extensions;
mod comfy_extensions;
mod market;
mod media_queue;
pub mod model_config;
pub mod project_memory;
pub mod server;
pub mod store;
pub mod video_edit;
mod video_exposure;
pub mod workers;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub fn installation_root(executable: &std::path::Path) -> PathBuf {
    let directory = executable.parent().unwrap_or(executable);
    if directory
        .file_name()
        .map(|s| s.to_string_lossy().eq_ignore_ascii_case("app"))
        .unwrap_or(false)
    {
        directory.parent().unwrap_or(directory).to_path_buf()
    } else {
        directory.to_path_buf()
    }
}
pub fn root() -> PathBuf {
    let base = installation_root(&std::env::current_exe().expect("Executable location"));
    std::env::var_os("STRUVE_LLM_ROOT")
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { base.join(p) })
        .unwrap_or(base)
}
pub fn port() -> u16 {
    std::env::var("STRUVE_LLM_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            std::fs::read(root().join("data/controller.endpoint.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|v| v["port"].as_u64())
                .map(|v| v as u16)
        })
        .unwrap_or(0)
}
pub fn app_dir() -> PathBuf {
    root().join("app")
}
pub fn installer_language(base: &std::path::Path) -> &'static str {
    match std::fs::read_to_string(base.join("app/installer-language.txt")) {
        Ok(value) if value.trim() == "en" => "en",
        _ => "ru",
    }
}

/// Choose the language before the WebView or tray shows any text.
pub fn startup_language(base: &std::path::Path) -> &'static str {
    let saved = rusqlite::Connection::open_with_flags(
        base.join("data/struve.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .and_then(|db| db.query_row(
        "SELECT payload FROM objects WHERE kind='settings' AND id='system'",
        [], |row| row.get::<_, String>(0),
    ))
    .ok()
    .and_then(|payload| serde_json::from_str::<Value>(&payload).ok());
    match saved.as_ref().and_then(|s| s["language"].as_str()) {
        Some("en") => "en",
        Some("ru") => "ru",
        _ => installer_language(base),
    }
}
pub fn source_dir() -> PathBuf {
    std::env::var_os("STRUVE_LLM_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let runtime = app_dir().join("runtime");
            if runtime.is_dir() {
                runtime
            } else {
                root().join("source/Strata/apps/struve")
            }
        })
}
pub fn models_dir(settings: &Value) -> PathBuf {
    root().join(
        settings["models_directory"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("models"),
    )
}
pub fn token() -> Result<String> {
    Ok(
        std::fs::read_to_string(root().join("data/controller.token"))?
            .trim()
            .to_string(),
    )
}
pub fn envelope(result: Result<Value>) -> Value {
    match result {
        Ok(v) => json!({"schema_version":1,"ok":true,"data":v}),
        Err(e) => {
            json!({"schema_version":1,"ok":false,"error":{"code":"REQUEST_FAILED","message":e.to_string(),"retryable":false}})
        }
    }
}
pub async fn request(method: &str, params: Value) -> Result<Value> {
    if port() == 0 {
        bail!("Диспетчер ещё не запущен для этой установки")
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(match method {
            "media.level_exposure" | "media.market.refresh" => 330,
            "mcp.call" | "mcp.probe" => 200,
            _ => 45,
        }))
        .build()?;
    let response = client
        .post(format!("http://127.0.0.1:{}/rpc", port()))
        .bearer_auth(token()?)
        .json(&json!({"method":method,"params":params}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(response)
}
fn same_installation(value: &Value) -> bool {
    let Some(other) = value.as_str() else {
        return false;
    };
    let normalize = |path: &std::path::Path| {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        canonical
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .replace('/', r"\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    normalize(std::path::Path::new(other)) == normalize(&root())
}
pub async fn ensure_controller() -> Result<()> {
    if let Ok(v) = request("ping", json!({})).await {
        if v["ok"] == true && same_installation(&v["data"]["root"]) {
            return Ok(());
        }
    }
    let executable = std::env::current_exe()?.with_file_name("struve-llm.exe");
    std::fs::create_dir_all(root().join("logs"))?;
    // No inherited handles: a coordinator may capture CLI stdout in a pipe.
    // Inheriting any pipe writer would prevent its EOF after the CLI exits.
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*};
        let application: Vec<u16> = executable
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let command = format!(
            "{} serve",
            crate::workers::quote(&executable.to_string_lossy())
        );
        let mut startup = STARTUPINFOW::default();
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut spawned = false;
        for flags in [
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
        ] {
            let mut line: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
            let mut info = PROCESS_INFORMATION::default();
            unsafe {
                if CreateProcessW(
                    application.as_ptr(),
                    line.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    flags,
                    std::ptr::null(),
                    std::ptr::null(),
                    &startup,
                    &mut info,
                ) != 0
                {
                    CloseHandle(info.hThread);
                    CloseHandle(info.hProcess);
                    spawned = true;
                    break;
                }
            }
        }
        if !spawned {
            return Err(std::io::Error::last_os_error()).context("Cannot start controller");
        }
    }
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        if let Ok(v) = request("ping", json!({})).await {
            if v["ok"] == true && same_installation(&v["data"]["root"]) {
                return Ok(());
            }
        }
    }
    bail!("Диспетчер не отвечает. Проверьте logs/controller.log")
}
pub fn safe_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 100
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("Некорректный идентификатор")
    }
    Ok(())
}
#[cfg(test)]
mod portability_tests {
    use super::*;
    #[test]
    fn installer_language_seeds_a_new_profile_and_saved_language_wins() {
        let base = std::env::temp_dir().join(format!("struve-locale-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(base.join("app")).unwrap();
        assert_eq!(startup_language(&base), "ru");
        assert!(!base.join("data").exists());
        let marker = base.join("app/installer-language.txt");
        std::fs::write(&marker, "en").unwrap();
        assert_eq!(startup_language(&base), "en");
        let path = base.join("data/struve.sqlite");
        {
            let mut store = store::Store::open(&path).unwrap();
            store.save("settings", json!({"id":"system","language":"ru"}), None).unwrap();
        }
        assert_eq!(startup_language(&base), "ru");
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn root_follows_executable_not_cwd_or_drive() {
        assert_eq!(
            installation_root(std::path::Path::new(r"F:\Apps\Struve\app\struve-llm.exe")),
            PathBuf::from(r"F:\Apps\Struve")
        );
        assert_eq!(
            installation_root(std::path::Path::new(r"C:\Portable\struve-llm.exe")),
            PathBuf::from(r"C:\Portable")
        );
    }
}
