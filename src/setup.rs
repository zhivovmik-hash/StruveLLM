use crate::{hardware, server::App};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

pub fn local_endpoint(raw: &str) -> Result<reqwest::Url> {
    let u = reqwest::Url::parse(raw.trim())?;
    if !matches!(u.scheme(), "http" | "https")
        || !matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        bail!("Choose a loopback HTTP endpoint without credentials or a query string")
    }
    Ok(u)
}
pub async fn scan(p: &Value) -> Result<Value> {
    let ollama = match p["protocol"].as_str() {
        Some("ollama") => true,
        Some("openai") => false,
        _ => bail!("Unknown inference protocol"),
    };
    let u = local_endpoint(p["endpoint"].as_str().context("Missing endpoint")?)?;
    let base = u.as_str().trim_end_matches('/');
    let url = format!("{base}/{}", if ollama { "api/tags" } else { "models" });
    let mut r = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    let mut data = Vec::new();
    while let Some(chunk) = r.chunk().await? {
        if data.len() + chunk.len() > 2 * 1024 * 1024 {
            bail!("Local model catalog is too large")
        }
        data.extend_from_slice(&chunk);
    }
    let data: Value = serde_json::from_slice(&data)?;
    let models:Vec<_>=data[if ollama{"models"}else{"data"}].as_array().context("The endpoint returned no model catalog")?
        .iter().take(1000).filter_map(|m|{
            let name=m[if ollama{"name"}else{"id"}].as_str()?.trim();
            if name.is_empty()||name.len()>512{return None}
            Some(json!({"name":name,"bytes":m["size"],"quantization":m["details"]["quantization_level"],"tokenizer":if m["owned_by"].as_str().is_some_and(|v|matches!(v,"llamacpp"|"llama.cpp")){Some("llama.cpp")}else{None}}))
        }).collect();
    Ok(json!({"endpoint":base,"protocol":p["protocol"],"models":models}))
}

pub fn llama_server() -> Option<PathBuf> {
    let mut paths = vec![
        crate::app_dir().join("llama-server.exe"),
        crate::root().join("runtimes/llama.cpp/llama-server.exe"),
    ];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path).map(|d| d.join("llama-server.exe")));
    }
    paths.into_iter().find(|p| p.is_absolute() && p.is_file())
}
fn gguf_placement(p: &Value, size: f64, free: Option<f64>) -> Result<(u64, f64)> {
    let manual = !p["gpu_layers"].is_null() || !p["vram_gib"].is_null();
    let Some(free) = free else {
        anyhow::ensure!(!manual, "Choose a GPU for manual offload");
        return Ok((0, 0.0));
    };
    let (layers, budget) = if manual {
        let layers = p["gpu_layers"].as_u64().filter(|v| (1..=1000).contains(v))
            .context("Choose 1 to 1000 GPU layers")?;
        let budget = p["vram_gib"].as_f64().filter(|v| v.is_finite() && *v > 0.0)
            .context("Set a positive VRAM budget in GiB")?;
        (layers, budget)
    } else {
        (999, (size + 1.).ceil())
    };
    anyhow::ensure!(budget <= free, "VRAM budget exceeds available memory. Choose fewer layers and a smaller budget, or CPU.");
    Ok((layers, budget))
}

pub async fn gguf(app: &App, p: &Value) -> Result<Value> {
    let path = Path::new(p["file"].as_str().context("Choose a GGUF file")?);
    if !path.is_absolute()
        || !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gguf"))
    {
        bail!("Choose a local .gguf file")
    }
    let mut f = std::fs::File::open(path)?;
    let mut magic = [0; 4];
    f.read_exact(&mut magic)?;
    if magic != *b"GGUF" {
        bail!("The file does not have a GGUF header")
    }
    let command = p["command"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(llama_server)
        .context("Select the installed llama-server.exe")?;
    if !command.is_absolute()
        || !command.is_file()
        || !command
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("llama-server.exe"))
    {
        bail!("Choose llama-server.exe from a local llama.cpp installation")
    }
    let hw = app.fresh_inventory().await;
    let system = hardware::effective_settings(&app.get("settings", "system")?, &hw);
    let bytes = f.metadata()?.len();
    let size = bytes as f64 / 1073741824.;
    // Keep a conservative host budget for loading and metadata, even with full GPU offload.
    let ram = (size * 1.3 + 2.).ceil().max(2.);
    let gpu = p["gpu_id"].as_str().filter(|s| !s.is_empty());
    let threads = hw["logical_cpus"]
        .as_u64()
        .unwrap_or(1)
        .saturating_sub(system["cpu_reserve_threads"].as_u64().unwrap_or(1) + 1)
        .clamp(1, 8);
    let free = if let Some(id) = gpu {
        let g = hw["gpus"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|g| g["id"] == id && g["compute_backend"] == "cuda")
            .context("Assigned GPU is unavailable")?;
        if system["reserved_gpu_ids"]
            .as_array()
            .is_some_and(|a| a.contains(&json!(id)))
        {
            bail!("GPU reserved for the system")
        }
        Some(g["vram_gib"].as_f64().unwrap_or(0.) - g["used_gib"].as_f64().unwrap_or(f64::INFINITY))
    } else {
        None
    };
    let (layers, vram) = gguf_placement(p, size, free)?;
    let id = format!("gguf-{}", uuid::Uuid::new_v4().simple());
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()?
        .port();
    let name = path
        .file_stem()
        .and_then(|p| p.to_str())
        .context("Invalid filename")?;
    let profile = json!({"id":id,"name":name,"kind":"inference","ram_gib":ram,"vram_gib":vram,
            "gpu_ids":gpu.into_iter().collect::<Vec<_>>(),"cpu_threads":threads,"exclusive":true});
    let plan = hardware::plan(&hw, &system, &[profile.clone()]);
    if plan["admissible"] != true
        || ram
            > hw["ram_available_gib"].as_f64().unwrap_or(0.)
                - system["ram_reserve_gib"].as_f64().unwrap_or(0.)
    {
        bail!(
            "Insufficient resources for this GGUF after system reservations: {}",
            plan["reasons"]
        )
    }
    Ok(
        json!({"id":id,"name":name,"engine_model":id,"endpoint":format!("http://127.0.0.1:{port}/v1"),"protocol":"openai",
        "context_tokens":4096,"max_output_tokens":512,"tokenizer":"llama.cpp","temperature":0.3,"vision":false,"tool_mode":"none",
        "resource_mode":"dedicated","resource_profile":profile,
        "gpu_layers":layers,"runtime":{"kind":"command","command":command,"args":["-m",path,"--alias",id,"--host","127.0.0.1","--port","{{port}}","-c","{{context}}","-ngl","{{gpu_layers}}"]},
        "weights_bytes":bytes}),
    )
}

#[test]
fn gguf_mixed_placement_respects_the_selected_card() {
    let manual = json!({"gpu_layers":8,"vram_gib":5.5});
    assert_eq!(gguf_placement(&manual, 17., Some(7.)).unwrap(), (8,5.5));
    assert!(gguf_placement(&manual, 17., None).is_err());
    assert!(gguf_placement(&manual, 17., Some(5.)).is_err());
    assert!(gguf_placement(&json!({"gpu_layers":2}), 17., Some(8.)).is_err());
    assert!(gguf_placement(&json!({}), 17., Some(8.)).is_err());
    assert_eq!(gguf_placement(&json!({}), 17., Some(24.)).unwrap(), (999,18.));
    assert_eq!(gguf_placement(&json!({}), 17., None).unwrap(), (0,0.));
}
fn executable_version(path: &str) -> Option<String> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    };
    let path: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
    unsafe {
        let len = GetFileVersionInfoSizeW(path.as_ptr(), ptr::null_mut());
        if len == 0 || len > 4 * 1024 * 1024 {
            return None;
        }
        let mut data = vec![0u32; (len as usize).div_ceil(4)];
        if GetFileVersionInfoW(path.as_ptr(), 0, len, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut found = ptr::null_mut();
        let mut size = 0;
        if VerQueryValueW(
            data.as_ptr().cast(),
            [92u16, 0].as_ptr(),
            &mut found,
            &mut size,
        ) == 0
            || found.is_null()
            || size < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32
        {
            return None;
        }
        let info = ptr::read_unaligned(found.cast::<VS_FIXEDFILEINFO>());
        if info.dwSignature != 0xfeef04bd {
            return None;
        }
        Some(format!(
            "{}.{}.{}.{}",
            info.dwFileVersionMS >> 16,
            info.dwFileVersionMS & 65535,
            info.dwFileVersionLS >> 16,
            info.dwFileVersionLS & 65535
        ))
    }
}
pub async fn diagnostics(app: &App) -> Result<Value> {
    let hw = app.inventory().await;
    let settings = hardware::effective_settings(&app.get("settings", "system")?, &hw);
    let gpus:Vec<_>=hw["gpus"].as_array().into_iter().flatten().map(|g|json!({"name":g["name"],"vram_gib":g["vram_gib"],
        "used_gib":g["used_gib"],"driver":g["driver"],"compute":g["compute_backend"],"reserved":settings["reserved_gpu_ids"].as_array().is_some_and(|ids|ids.contains(&g["id"]))})).collect();
    let mut binaries = std::collections::BTreeSet::new();
    for m in app
        .list("model")?
        .as_array()
        .into_iter()
        .flatten()
        .chain(app.list("media_model")?.as_array().into_iter().flatten())
    {
        let exe = &m["runtime"]["command"];
        if let Some(path) = exe
            .as_str()
            .or_else(|| m["command"].as_str())
            .filter(|v| !v.is_empty())
        {
            let engine = match Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase()
                .as_str()
            {
                "ollama.exe" => "Ollama",
                "llama-server.exe" => "llama.cpp",
                "sd.exe" | "sd-cli.exe" => "stable-diffusion.cpp",
                _ => "Other local engine",
            };
            binaries.insert((engine, executable_version(path)));
        }
    }
    let mut failures = std::collections::BTreeMap::new();
    // Send only coarse categories from recent failures. Native error strings can contain user paths.
    for row in app
        .list("run")?
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["status"] == "failed")
        .take(20)
    {
        let err = row["error"].as_str().unwrap_or("").to_lowercase();
        let reason = if err.contains("memory") || err.contains("ram") || err.contains("ресурс")
        {
            "resources"
        } else if err.contains("context") {
            "context"
        } else if err.contains("timeout") || err.contains("deadline") {
            "timeout"
        } else if err.contains("http") || err.contains("endpoint") {
            "engine_connection"
        } else {
            "other"
        };
        *failures.entry(reason).or_insert(0) += 1;
    }
    let mut reasons = std::collections::BTreeMap::new();
    let queued = app.store.lock().unwrap().queued_media()?;
    for id in queued {
        let r = app.get("run", &id)?;
        *reasons
            .entry(
                r["wait_reason"]["code"]
                    .as_str()
                    .unwrap_or("pending")
                    .to_string(),
            )
            .or_insert(0) += 1;
    }
    Ok(
        json!({"app_version":crate::VERSION,"utc":chrono::Utc::now().to_rfc3339(),"os":hw["os"],"cpu":hw["cpu_name"],
        "threads":hw["logical_cpus"],"ram_total_gib":hw["ram_total_gib"],"ram_available_gib":hw["ram_available_gib"],
        "system_reserve_ram_gib":settings["ram_reserve_gib"],"gpus":gpus,"queue_reasons":reasons,"recent_failure_categories":failures,"engines":binaries.into_iter().map(|(kind,version)|json!({"kind":kind,"version":version})).collect::<Vec<_>>(),
        "model_count":app.list("model")?.as_array().map(Vec::len),"observation_count":app.store.lock().unwrap().db.query_row("SELECT count(*) FROM resource_observations",[],|r|r.get::<_,i64>(0))?}),
    )
}
