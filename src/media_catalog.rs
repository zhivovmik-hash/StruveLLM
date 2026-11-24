//! Versioned local media models. Each saved model owns a separate resource profile.
use crate::{image_jobs, server::App};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

pub fn defaults(app: &App) -> Value {
    app.get("media_defaults", "default")
        .unwrap_or(json!({"id":"default","image":"","edit":"","video":""}))
}
pub fn list(app: &App) -> Result<Value> {
    Ok(json!(app
        .list("media_model")?
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["archived"] != true)
        .cloned()
        .collect::<Vec<_>>()))
}
pub fn profile(c: &Value) -> Value {
    json!({"id":format!("media-{}",c["id"].as_str().unwrap_or("invalid")),"name":format!("{} · resources",c["name"].as_str().unwrap_or("Media")),"owner_media_id":c["id"],"kind":"inference","gpu_ids":[c["gpu_id"]],"ram_gib":c["ram_gib"],"vram_gib":c["vram_gib"],"cpu_threads":c["cpu_threads"],"exclusive":true})
}
pub fn templates(app: &App) -> Value {
    let base = image_jobs::legacy_config(app);
    json!([
        ("sd15", "Stable Diffusion 1.5", "image", 20, 7.0),
        ("sdxl", "Stable Diffusion XL", "image", 25, 7.0),
        ("sd35", "Stable Diffusion 3.5", "image", 25, 4.5),
        ("flux1", "FLUX.1", "image", 20, 1.0),
        ("flux2-klein", "FLUX.2 Klein 4B", "image", 4, 1.0),
        ("minimax-h3", "MiniMax H3", "video", 20, 1.0)
    ]
    .iter()
    .map(|(arch, name, kind, steps, cfg)| {
        let mut c = base.clone();
        c.as_object_mut().unwrap().remove("revision");
        c["id"] = json!(format!("media-{}", uuid::Uuid::new_v4().simple()));
        c["name"] = json!(name);
        c["architecture"] = json!(arch);
        c["kind"] = json!(kind);
        c["enabled"] = json!(false);
        c["model_path"] = json!("");
        c["steps"] = json!(steps);
        c["cfg_scale"] = json!(cfg);
        c["offload_to_cpu"] = json!(true);
        c["text_encoder_cpu"] = json!(true);
        c["vae_cpu"] = json!(false);
        c["diffusion_cpu"] = json!(false);
        c["flash_attention"] = json!(false);
        c["sampling_method"] = json!(if *arch == "sd15" { "euler_a" } else { "euler" });
        c["sampling_schedule"] = json!("standard");
        c["video_frames"] = json!(39);
        c["fps"] = json!(24);
        c["ram_gib"] = json!(if *kind == "video" { 64 } else { 24 });
        c["timeout_seconds"] = json!(if *kind == "video" { 3600 } else { 1200 });
        for key in [
            "llm_path",
            "vae_path",
            "clip_l_path",
            "clip_g_path",
            "t5_path",
            "audio_vae_path",
            "connector_path",
        ] {
            c[key] = json!("");
        }
        c
    })
    .collect::<Vec<_>>())
}
pub fn migrate(app: &App) -> Result<()> {
    if !app.list("media_model")?.as_array().unwrap().is_empty() {
        return Ok(());
    }
    let mut c = image_jobs::legacy_config(app);
    // A fresh installation has no legacy model to migrate.
    if c["model_path"].as_str().unwrap_or("").is_empty() {
        return Ok(());
    }
    c.as_object_mut().unwrap().remove("revision");
    c["id"] = json!("sd15-original");
    c["name"] = json!("Stable Diffusion 1.5");
    c["kind"] = json!("image");
    c["architecture"] = json!("sd15");
    c["offload_to_cpu"] = json!(false);
    c["text_encoder_cpu"] = json!(true);
    c["vae_cpu"] = json!(false);
    c["flash_attention"] = json!(false);
    c["sampling_method"] = json!("euler_a");
    let mut p = profile(&c);
    p["revision"] = json!(0);
    c["profile_id"] = p["id"].clone();
    app.store.lock().unwrap().save_batch(vec![
        ("profile", p),
        ("media_model", c),
        (
            "media_defaults",
            json!({"id":"default","image":"sd15-original","edit":"sd15-original","video":""}),
        ),
    ])?;
    Ok(())
}
pub fn validate_schema(c: &Value) -> Result<()> {
    crate::safe_id(c["id"].as_str().context("Model ID required")?)?;
    if !c["name"]
        .as_str()
        .map(|s| !s.trim().is_empty() && s.len() < 200)
        .unwrap_or(false)
    {
        bail!("Model name required (max 200 bytes)")
    }
    let arch = c["architecture"].as_str().unwrap_or("");
    if !["sd15", "sdxl", "sd35", "flux1", "flux2-klein", "minimax-h3"].contains(&arch) {
        bail!("Unsupported media architecture")
    }
    if c["kind"]
        != if arch == "minimax-h3" {
            "video"
        } else {
            "image"
        }
    {
        bail!("Model kind does not match its architecture")
    }
    image_jobs::validate_parameters(c)?;
    crate::hardware::validate_profile(&profile(c))?;
    if c["kind"] == "video" {
        if !c["video_frames"]
            .as_u64()
            .map(valid_h3_frames)
            .unwrap_or(false)
        {
            bail!("MiniMax H3: choose 5 + 17*N frames, from 5 to 362 (5, 22, 39, 56, ...)")
        }
        if c["fps"] != 24 {
            bail!("MiniMax H3 uses 24 fps")
        }
    }
    for key in [
        "offload_to_cpu",
        "text_encoder_cpu",
        "vae_cpu",
        "diffusion_cpu",
        "flash_attention",
    ] {
        if !c[key].is_null() && !c[key].is_boolean() {
            bail!("{key} must be boolean")
        }
    }
    if !c["sampling_method"].is_null()
        && !matches!(
            c["sampling_method"].as_str(),
            Some("euler" | "euler_a" | "dpm++2m")
        )
    {
        bail!("Unsupported sampler")
    }
    if !c["h3_linear_chunk_tokens"].is_null()
        && (arch != "minimax-h3"
            || !matches!(c["h3_linear_chunk_tokens"].as_u64(), Some(0..=65536)))
    {
        bail!("H3 compute chunk must be a whole number from 0 to 65536 and requires a MiniMax H3 model")
    }
    if !c["h3_reference_exposure"].is_null()
        && (!c["h3_reference_exposure"].is_boolean()
            || (c["h3_reference_exposure"] == true && arch != "minimax-h3"))
    {
        bail!("Reference exposure is a MiniMax H3 video setting and must be true or false")
    }
    validate_sampling_schedule(c)?;
    Ok(())
}
fn validate_sampling_schedule(c: &Value) -> Result<()> {
    let schedule = match c.get("sampling_schedule") {
        None | Some(Value::Null) => "standard",
        Some(Value::String(value)) => value,
        _ => bail!("Sampling schedule must be a string"),
    };
    let steps = match schedule {
        "standard" => return Ok(()),
        "hyperflow-h3-flash-4" => 4,
        "hyperflow-h3-flash-6" => 6,
        "hyperflow-h3-flash-8" => 8,
        _ => bail!("Unsupported sampling schedule"),
    };
    if c["architecture"] != "minimax-h3"
        || c["steps"].as_u64() != Some(steps)
        || c["sampling_method"] != "euler"
        || c["cfg_scale"].as_f64() != Some(1.0)
    {
        bail!("HyperFlow H3 Flash requires matching Flash weights, Euler, CFG 1 and the schedule's step count")
    }
    Ok(())
}
pub fn components(c: &Value) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![(
        "model_path",
        if matches!(
            c["architecture"].as_str(),
            Some("flux1" | "flux2-klein" | "minimax-h3")
        ) {
            "--diffusion-model"
        } else {
            "-m"
        },
    )];
    for (k, flag) in [
        ("vae_path", "--vae"),
        ("llm_path", "--llm"),
        ("clip_l_path", "--clip_l"),
        ("clip_g_path", "--clip_g"),
        ("t5_path", "--t5xxl"),
        ("audio_vae_path", "--audio-vae"),
        ("connector_path", "--embeddings-connectors"),
    ] {
        if c[k].as_str().is_some_and(|s| !s.is_empty()) {
            v.push((k, flag))
        }
    }
    v
}
fn valid_h3_frames(n: u64) -> bool {
    (5..=362).contains(&n) && n % 17 == 5
}
pub fn files(c: &Value) -> Result<()> {
    let arch = c["architecture"].as_str().unwrap_or("sd15");
    let required = match arch {
        "flux2-klein" | "minimax-h3" => vec!["llm_path", "vae_path"],
        "flux1" => vec!["clip_l_path", "t5_path", "vae_path"],
        // Official SD 3.5 checkpoints include their VAE; an external VAE can override it.
        "sd35" => vec!["clip_l_path", "clip_g_path", "t5_path"],
        _ => vec![],
    };
    for key in required {
        if c[key].as_str().unwrap_or("").is_empty() {
            bail!("{key} is required for {arch}")
        }
    }
    for (key, _) in components(c) {
        let path = Path::new(c[key].as_str().unwrap_or(""));
        if !path.is_absolute() || !path.is_file() {
            bail!("{key}: select an existing absolute weight file")
        }
    }
    if c["h3_reference_exposure"] == true && !supports_reference_exposure(c) {
        bail!("Select a Struve H3 runtime with reference exposure support or use scene lighting from the prompt")
    }
    Ok(())
}
pub async fn check(app: &App, c: &Value) -> Result<Value> {
    validate_schema(c)?;
    files(c)?;
    image_jobs::validate(app, c).await?;
    Ok(
        json!({"ready":true,"profile":profile(c),"note":"Files and resources checked; this is not a model quality or CUDA execution test"}),
    )
}
pub async fn check_status(app: &App, c: &Value) -> Result<Value> {
    let mut result = check(app, c).await?;
    let mut assignments = Vec::new();
    for (id, worker) in app.workers.lock().await.iter() {
        if worker.profile["gpu_ids"]
            .as_array()
            .is_some_and(|cards| cards.contains(&c["gpu_id"]))
            && worker.exit_code()?.is_none()
        {
            assignments.push(json!({"instance_id":id,"pid":worker.pid}));
        }
    }
    result["selected_gpu_assignments"] = json!(assignments);
    Ok(result)
}
pub async fn save(app: &App, mut c: Value) -> Result<Value> {
    if !app.image_leases.lock().await.is_empty() {
        bail!("Wait for the media workflow to finish")
    }
    validate_schema(&c)?;
    crate::media_extensions::resolve(app,&c,&c["extension_ids"])?;
    if c["enabled"] == true {
        check(app, &c).await?;
    }
    let mut p = profile(&c);
    if let Ok(old) = app.get("profile", p["id"].as_str().unwrap()) {
        if old["owner_media_id"] != c["id"] {
            bail!("Resource profile belongs to another owner")
        }
        // Model revision is the concurrency token for the inseparable model/profile pair.
        p["revision"] = old["revision"].clone();
    }
    c["profile_id"] = p["id"].clone();
    app.store
        .lock()
        .unwrap()
        .save_batch(vec![("profile", p), ("media_model", c.clone())])?;
    app.get("media_model", c["id"].as_str().unwrap())
}
pub fn save_defaults(app: &App, mut d: Value) -> Result<Value> {
    d["id"] = json!("default");
    for role in ["image", "edit", "video"] {
        let id = d[role]
            .as_str()
            .context("Default model IDs must be strings")?;
        if id.is_empty() {
            continue;
        }
        let m = app.get("media_model", id)?;
        if m["enabled"] != true
            || m["archived"] == true
            || m["kind"] != if role == "video" { "video" } else { "image" }
        {
            bail!("Default {role} must be an enabled compatible model")
        }
    }
    app.save("media_defaults", d)
}
pub fn select(app: &App, p: &Value, role: &str) -> Result<Value> {
    let d = defaults(app);
    let id = p["media_model_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(d[role].as_str().unwrap_or(""));
    if id.is_empty() {
        if role != "video" && app.list("media_model")?.as_array().unwrap().is_empty() {
            return Ok(image_jobs::legacy_config(app));
        }
        bail!("Choose a default {role} model in Models")
    }
    let c = app.get("media_model", id)?;
    if c["enabled"] != true || c["archived"] == true {
        bail!("Selected media model is disabled")
    }
    if c["kind"] != if role == "video" { "video" } else { "image" } {
        bail!("Selected model does not support this task type")
    }
    Ok(c)
}
pub fn video_request(p: &Value) -> bool {
    if p["mode"] == "video" {
        return true;
    }
    if p["mode"] != "auto" {
        return false;
    }
    let s = crate::image_edit::normalized_request(p["content"].as_str().unwrap_or(""));
    [
        "создай видео",
        "сгенерируй видео",
        "сделай видео",
        "оживи ",
        "анимируй ",
        "create a video",
        "generate a video",
        "animate ",
        "/video ",
    ]
    .iter()
    .any(|x| s.starts_with(x))
}
pub fn supports_reference_exposure(config: &Value) -> bool {
    runtime_feature(config,"h3_reference_exposure")
}
pub fn runtime_feature(config:&Value,feature:&str)->bool {
    config["architecture"] == "minimax-h3" && Path::new(config["command"].as_str().unwrap_or(""))
        .parent()
        .and_then(|parent| std::fs::read(parent.join("struve-runtime.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|m| m["features"][feature] == true)
}

pub fn reference_exposure_used(setting: &Value, scene: &Value, eligible: bool) -> bool {
    eligible && setting.as_bool().unwrap_or(scene["lighting"] == "constant" && scene["camera"] == "fixed")
}

pub fn video_end_args(exposure_used: bool, end: &Path) -> Vec<Value> {
    let mut args = vec![json!("--end-img"), json!(end)];
    if exposure_used {
        args.extend([json!("--extra-sample-args"), json!("h3_reference_exposure=1")]);
    }
    args
}

pub fn render_args(
    c: &Value,
    prompt: &str,
    output: &Path,
    width: u32,
    height: u32,
    seed: i64,
    source: Option<&Path>,
    mask: Option<&Path>,
) -> Value {
    let mut args = vec![];
    for (key, flag) in components(c) {
        args.extend([json!(flag), c[key].clone()]);
    }
    args.extend(
        json!([
            "-p",
            prompt,
            "-n",
            c["negative_prompt"],
            "-o",
            output,
            "-W",
            width.to_string(),
            "-H",
            height.to_string(),
            "--steps",
            c["steps"].to_string(),
            "--cfg-scale",
            c["cfg_scale"].to_string(),
            "--seed",
            seed.to_string(),
            "-t",
            c["cpu_threads"].to_string(),
            "--sampling-method",
            c["sampling_method"].as_str().unwrap_or("euler_a"),
            "--vae-tiling"
        ])
        .as_array()
        .unwrap()
        .clone(),
    );
    if c["text_encoder_cpu"] != false {
        args.push(json!("--clip-on-cpu"));
    }
    if c["offload_to_cpu"] == true {
        args.push(json!("--offload-to-cpu"));
    }
    if let Some(budget) = c["vram_gib"].as_f64().filter(|n| *n > 0.0) {
        args.extend([json!("--max-vram"), json!(budget.to_string())]);
    }
    if let Some(schedule) = c["sampling_schedule"].as_str().filter(|s| *s != "standard") {
        args.extend([json!("--sigmas"), json!(schedule)]);
    }
    if c["vae_cpu"] == true {
        args.push(json!("--vae-on-cpu"));
    }
    if c["diffusion_cpu"] == true {
        args.extend([json!("--backend"), json!("diffusion=cpu")]);
    }
    args.extend([
        json!("--flash-attn"),
        json!(if c["flash_attention"] == true {
            "on"
        } else {
            "off"
        }),
    ]);
    if c["kind"] == "video" {
        if let Some(tokens) = c["h3_linear_chunk_tokens"].as_u64() {
            args.extend([
                json!("--model-args"),
                json!(format!("minimax_h3_linear_chunk_tokens={tokens}")),
            ]);
        }
        args.extend(
            json!([
                "-M",
                "vid_gen",
                "--video-frames",
                c["video_frames"].to_string(),
                "--fps",
                c["fps"].to_string(),
                "--rng",
                "cpu"
            ])
            .as_array()
            .unwrap()
            .clone(),
        );
    }
    if let Some(source) = source {
        args.extend([
            json!(if c["architecture"] == "flux2-klein" {
                "-r"
            } else {
                "--init-img"
            }),
            json!(source),
        ]);
        if c["kind"] != "video" && c["architecture"] != "flux2-klein" {
            if let Some(mask) = mask {
                args.extend([json!("--mask"), json!(mask)]);
            }
            args.extend([json!("--strength"), json!(c["edit_strength"].to_string())]);
        }
    }
    json!(args)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lighting_defaults_to_scene_and_respects_framing_and_model_override() {
        let end = Path::new("last.png");
        let plain = vec![json!("--end-img"), json!(end)];
        let steady = json!({"lighting":"constant","camera":"fixed"});
        for scene in [json!({"lighting":"changing","camera":"fixed"}),json!({"lighting":"constant","camera":"moving"}),Value::Null] {
            assert!(!reference_exposure_used(&Value::Null,&scene,true));
        }
        assert!(reference_exposure_used(&Value::Null,&steady,true));
        assert!(!reference_exposure_used(&Value::Null,&steady,false));
        assert!(!reference_exposure_used(&json!(false),&steady,true));
        assert!(reference_exposure_used(&json!(true),&Value::Null,true));
        assert!(!reference_exposure_used(&json!(true),&steady,false));
        assert_eq!(video_end_args(false, end), plain);
        assert_eq!(video_end_args(true, end)[2..], [json!("--extra-sample-args"),json!("h3_reference_exposure=1")]);
    }
    #[test]
    fn flash_schedule_cannot_silently_disagree_with_visible_settings() {
        let mut c = json!({"architecture":"minimax-h3","steps":6,"sampling_method":"euler","cfg_scale":1,"sampling_schedule":"hyperflow-h3-flash-6","vram_gib":6.5});
        assert!(validate_sampling_schedule(&c).is_ok());
        let args = render_args(&c, "clip", Path::new("out.avi"), 864, 480, 1, None, None);
        let args = args.as_array().unwrap();
        assert!(args
            .windows(2)
            .any(|p| p == [json!("--sigmas"), json!("hyperflow-h3-flash-6")]));
        assert!(args
            .windows(2)
            .any(|p| p == [json!("--max-vram"), json!("6.5")]));
        c["steps"] = json!(20);
        assert!(validate_sampling_schedule(&c).is_err());
        c["steps"] = json!(6);
        c["architecture"] = json!("flux1");
        assert!(validate_sampling_schedule(&c).is_err());
        c["architecture"] = json!("minimax-h3");
        c["cfg_scale"] = json!(7);
        assert!(validate_sampling_schedule(&c).is_err());
        c["sampling_schedule"] = json!("standard");
        assert!(validate_sampling_schedule(&c).is_ok());
        c["sampling_schedule"] = json!("unknown");
        assert!(validate_sampling_schedule(&c).is_err());
        assert!(validate_sampling_schedule(&json!({})).is_ok());
    }
    #[test]
    fn h3_frame_counts_match_the_native_vae_alignment() {
        for n in [5, 22, 39, 56, 243, 260, 362] {
            assert!(valid_h3_frames(n));
        }
        for n in [0, 8, 24, 240, 261, 379] {
            assert!(!valid_h3_frames(n));
        }
    }
    #[test]
    fn h3_accepts_native_480p_with_a_bounded_longer_deadline() {
        let mut c = json!({"architecture":"minimax-h3","width":864,"height":480,"video_frames":362,"timeout_seconds":10800,"steps":20,"cpu_threads":6,"cfg_scale":1,"seed":42,"negative_prompt":"","restore_llm":true,"enabled":false});
        assert!(image_jobs::validate_parameters(&c).is_ok());
        c["height"] = json!(481);
        assert!(image_jobs::validate_parameters(&c).is_err());
        c["height"] = json!(480);
        c["timeout_seconds"] = json!(14401);
        assert!(image_jobs::validate_parameters(&c).is_err());
        c["architecture"] = json!("sd15");
        c["timeout_seconds"] = json!(1200);
        assert!(image_jobs::validate_parameters(&c).is_err());
    }
    #[test]
    fn routing_does_not_turn_video_questions_into_generation() {
        assert!(video_request(
            &json!({"mode":"auto","content":"Создай видео про звёзды"})
        ));
        assert!(!video_request(
            &json!({"mode":"auto","content":"Как создать видео?"})
        ));
        assert!(!video_request(
            &json!({"mode":"text","content":"Создай видео"})
        ));
    }
    #[test]
    fn reference_edit_and_video_use_distinct_cli_contracts() {
        let c = json!({"architecture":"flux2-klein","kind":"image","model_path":"a.gguf","llm_path":"b.gguf","vae_path":"v.safetensors","width":512,"height":512,"steps":4,"cfg_scale":1,"cpu_threads":4,"negative_prompt":"","offload_to_cpu":true});
        let a = render_args(
            &c,
            "edit",
            Path::new("out.png"),
            512,
            512,
            1,
            Some(Path::new("in.png")),
            Some(Path::new("mask.png")),
        );
        let a = a.as_array().unwrap();
        assert!(a.contains(&json!("-r")));
        assert!(!a.contains(&json!("--mask")));
        assert!(a.contains(&json!("--diffusion-model")));
        let mut c = c;
        c["architecture"] = json!("sd35");
        assert_eq!(components(&c)[0], ("model_path", "-m"));
        c["architecture"] = json!("minimax-h3");
        c["kind"] = json!("video");
        c["video_frames"] = json!(8);
        c["fps"] = json!(24);
        let a = render_args(
            &c,
            "clip",
            Path::new("out.avi"),
            512,
            512,
            1,
            Some(Path::new("in.png")),
            None,
        );
        assert!(a.as_array().unwrap().contains(&json!("vid_gen")));
        assert!(a.as_array().unwrap().contains(&json!("--init-img")));
        assert!(!a.as_array().unwrap().contains(&json!("diffusion=cpu")));
        c["diffusion_cpu"] = json!(true);
        let cpu_args = render_args(&c, "clip", Path::new("out.avi"), 864, 480, 1, None, None);
        assert!(cpu_args
            .as_array()
            .unwrap()
            .windows(2)
            .any(|pair| { pair == [json!("--backend"), json!("diffusion=cpu")] }));
        assert!(cpu_args
            .as_array()
            .unwrap()
            .contains(&json!("--offload-to-cpu")));
    }
}
