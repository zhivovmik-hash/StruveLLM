use anyhow::{bail, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::Path, process::Stdio};
use sysinfo::{Disks, System};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG_SOFTWARE,
};
const GIB: f64 = 1073741824.;
fn dxgi_inventory() -> Result<Vec<Value>> {
    let mut devices = vec![];
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(index) {
            index += 1;
            let mut desc = DXGI_ADAPTER_DESC1::default();
            adapter.GetDesc1(&mut desc)?;
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let name = String::from_utf16_lossy(
                &desc.Description[..desc
                    .Description
                    .iter()
                    .position(|&x| x == 0)
                    .unwrap_or(desc.Description.len())],
            );
            devices.push(json!({"id":format!("dxgi-{:08x}-{:08x}",desc.AdapterLuid.HighPart as u32,desc.AdapterLuid.LowPart),"uuid":null,"name":name,"vendor_id":desc.VendorId,"present":true,"vram_gib":desc.DedicatedVideoMemory as f64/GIB,"shared_memory_gib":desc.SharedSystemMemory as f64/GIB,"used_gib":null,"temperature_c":null,"utilization_pct":null,"source":"Windows DXGI","identity_kind":"session_luid","compute_backend":null}));
        }
    }
    Ok(devices)
}
fn gpu_name(s: &str) -> String {
    s.to_lowercase()
        .replace("nvidia ", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn merge_gpus(mut nvidia: Vec<Value>, display: Vec<Value>) -> Vec<Value> {
    let mut counts=std::collections::HashMap::new();
    for gpu in &nvidia{*counts.entry(gpu_name(gpu["name"].as_str().unwrap_or(""))).or_insert(0usize)+=1;}
    let mut matched = HashSet::new();
    for adapter in display {
        let found = nvidia.iter().enumerate().position(|(index, g)| {
            !matched.contains(&index)
                && gpu_name(g["name"].as_str().unwrap_or(""))
                    == gpu_name(adapter["name"].as_str().unwrap_or(""))
        });
        if let Some(index) = found {
            matched.insert(index);
            if counts.get(&gpu_name(adapter["name"].as_str().unwrap_or("")))==Some(&1){
                nvidia[index]["display_adapter_id"] = adapter["id"].clone();
            }
            nvidia[index]["vendor_id"] = adapter["vendor_id"].clone();
        } else {
            nvidia.push(adapter);
        }
    }
    nvidia
}
pub fn volume_for<'a>(storage: &'a [Value], path: &Path) -> Option<&'a Value> {
    let wanted = path.to_string_lossy().replace('/', "\\").to_lowercase();
    storage
        .iter()
        .filter(|d| {
            d["mount"]
                .as_str()
                .map(|p| wanted.starts_with(&p.replace('/', "\\").to_lowercase()))
                .unwrap_or(false)
        })
        .max_by_key(|d| d["mount"].as_str().unwrap_or("").len())
}
pub async fn inventory() -> Value {
    let mut system = System::new();
    system.refresh_memory();
    system.refresh_cpu_all();
    let mut command = tokio::process::Command::new("nvidia-smi");
    command
        .args([
            "--query-gpu=uuid,name,memory.total,memory.used,temperature.gpu,utilization.gpu,driver_version,driver_model.current",
            "--format=csv,noheader,nounits",
        ])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut nvidia = vec![];
    let mut warnings = vec![];
    if let Ok(Ok(output)) =
        tokio::time::timeout(std::time::Duration::from_secs(4), command.output()).await
    {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let p: Vec<_> = line.split(',').map(str::trim).collect();
                if p.len() >= 6 && p[0].starts_with("GPU-") {
                    let number = |s: &str| s.parse::<f64>().ok();
                    nvidia.push(json!({"id":p[0],"uuid":p[0],"name":p[1],"present":true,"vram_gib":number(p[2]).map(|n|n/1024.),"used_gib":number(p[3]).map(|n|n/1024.),"temperature_c":number(p[4]),"utilization_pct":number(p[5]),"driver":p.get(6),"driver_model":p.get(7),"source":"NVIDIA driver / nvidia-smi","identity_kind":"gpu_uuid","compute_backend":"cuda"}));
                }
            }
        }
    }
    let display = match dxgi_inventory() {
        Ok(v) => v,
        Err(_) => {
            warnings.push("Windows DXGI не вернул список видеокарт");
            vec![]
        }
    };
    let devices = merge_gpus(nvidia, display);
    if devices.iter().any(|g| g["used_gib"].is_null()) {
        warnings.push("Для части GPU доступен только инвентарь Windows; недоступные показатели не заменяются нулями")
    }
    let disks = Disks::new_with_refreshed_list();
    let storage:Vec<_>=disks.list().iter().map(|d|json!({"mount":d.mount_point(),"name":d.name().to_string_lossy(),"kind":d.kind().to_string(),"file_system":d.file_system().to_string_lossy(),"removable":d.is_removable(),"available_gib":d.available_space() as f64/GIB,"total_gib":d.total_space() as f64/GIB})).collect();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut result = json!({"gpus":devices,"ram_total_gib":system.total_memory() as f64/GIB,"ram_available_gib":system.available_memory() as f64/GIB,"physical_cores":system.physical_core_count(),"logical_cpus":system.cpus().len(),"cpu_name":system.cpus().first().map(|c|c.brand()),"storage":storage,"os":{"name":System::name(),"version":System::os_version(),"architecture":System::cpu_arch()},"collected_at_ms":timestamp,"warnings":warnings});
    result["configuration_id"] = json!(configuration_id(&result));
    result
}
pub fn configuration_id(i: &Value) -> String {
    let devices: Vec<_> = i["gpus"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| json!([g["id"], g["name"], g["vram_gib"]]))
        .collect();
    let storage: Vec<_> = i["storage"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| json!([d["mount"], d["total_gib"]]))
        .collect();
    format!(
        "{:x}",
        Sha256::digest(
            json!([
                i["cpu_name"],
                i["physical_cores"],
                i["logical_cpus"],
                i["ram_total_gib"],
                devices,
                storage
            ])
            .to_string()
            .as_bytes()
        )
    )
}
pub fn suggested_settings(i: &Value) -> Value {
    let total = i["ram_total_gib"].as_f64().unwrap_or(0.);
    let threads = i["logical_cpus"].as_u64().unwrap_or(1);
    json!({"id":"system","reserve_mode":"auto","ram_reserve_gib":(total*0.2).min(32.).max(0.25).min(total*0.5),"cpu_reserve_threads":if threads>1{(threads/6).max(1)}else{0},"reserved_gpu_ids":[],"models_directory":"models"})
}
pub fn effective_settings(s: &Value, i: &Value) -> Value {
    let mut result = s.clone();
    if s["reserve_mode"] == "auto" {
        let auto = suggested_settings(i);
        result["ram_reserve_gib"] = auto["ram_reserve_gib"].clone();
        result["cpu_reserve_threads"] = auto["cpu_reserve_threads"].clone()
    }
    result
}
pub fn diagnostic_profile(i: &Value) -> Value {
    json!({"id":"cpu-diagnostics","name":"Диагностика CPU","gpu_ids":[],"ram_gib":(i["ram_total_gib"].as_f64().unwrap_or(1.)*0.1).clamp(0.125,1.),"vram_gib":0,"cpu_threads":1,"kind":"inference","exclusive":true})
}
pub fn validate_profile(p: &Value) -> Result<()> {
    crate::safe_id(p["id"].as_str().unwrap_or(""))?;
    if p["name"].as_str().unwrap_or("").trim().is_empty() {
        bail!("Введите имя профиля")
    }
    for key in ["ram_gib", "vram_gib", "cpu_threads"] {
        let v = p[key]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("Требуется число {key}"))?;
        if v < 0. || !v.is_finite() {
            bail!("Некорректный бюджет {key}")
        }
    }
    if p["cpu_threads"].as_u64().unwrap_or(0) == 0 {
        bail!("Количество потоков должно быть целым и положительным")
    }
    if p["ram_gib"].as_f64().unwrap_or(0.) < 0.125 {
        bail!("RAM budget must be at least 0.125 GiB")
    }
    let ids = p["gpu_ids"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Требуется список GPU"))?;
    if ids
        .iter()
        .any(|id| id.as_str().filter(|s| !s.is_empty()).is_none())
    {
        bail!("Некорректный идентификатор GPU")
    }
    if let Some(budgets) = p.get("vram_gib_by_gpu") {
        let budgets = budgets
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Per-GPU budgets must be an object"))?;
        for (gpu, value) in budgets {
            if !ids.contains(&json!(gpu))
                || !value
                    .as_f64()
                    .map(|n| n.is_finite() && n >= 0.)
                    .unwrap_or(false)
            {
                bail!("Invalid per-GPU VRAM budget")
            }
        }
    }
    if ids.is_empty() && p["vram_gib"].as_f64().unwrap_or(0.) > 0. {
        bail!("У CPU-профиля VRAM должна быть равна нулю")
    };
    Ok(())
}
pub fn plan(inventory: &Value, settings: &Value, profiles: &[Value]) -> Value {
    plan_with_active(inventory, settings, profiles, &[])
}
pub fn plan_with_active(
    inventory: &Value,
    settings: &Value,
    profiles: &[Value],
    active: &[(Value, f64)],
) -> Value {
    plan_reserved(inventory, settings, profiles, active, (0., 0))
}
pub fn vram_wait(id: &Value, needed: f64, total: f64, used: f64) -> Value {
    json!({"code":"device_vram","gpu_ids":[id],"needed_gib":needed,
        "available_gib":((total-used).max(0.)*10.).floor()/10.,"capacity_gib":total,
        "used_gib":(used*100.).round()/100.})
}
pub fn plan_reserved(inventory:&Value, settings:&Value, profiles:&[Value], active:&[(Value,f64)], pending:(f64,u64))->Value {
    let new_count = profiles.len();
    let requested_ram:f64 = profiles.iter().filter_map(|p| p["ram_gib"].as_f64()).sum();
    let credit: f64 = active.iter().map(|(_, used)| used.max(0.)).sum();
    let profiles: Vec<Value> = profiles
        .iter()
        .cloned()
        .chain(active.iter().map(|(p, _)| p.clone()))
        .collect();
    let settings = effective_settings(settings, inventory);
    let mut reasons = vec![];
    let mut temporary = Vec::new();
    let mut devices = HashSet::new();
    let mut ram = pending.0;
    let mut cpu = pending.1 as f64;
    let gpu_list = inventory["gpus"].as_array().cloned().unwrap_or_default();
    let reserved = settings["reserved_gpu_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (index, p) in profiles.iter().enumerate() {
        if let Err(e) = validate_profile(p) {
            reasons.push(e.to_string());
            continue;
        }
        ram += p["ram_gib"].as_f64().unwrap_or(0.);
        cpu += p["cpu_threads"].as_f64().unwrap_or(0.);
        for id in p["gpu_ids"].as_array().unwrap() {
            let name = id.as_str().unwrap_or("");
            if reserved.contains(id) {
                reasons.push(format!("{name}: карта зарезервирована для системы"))
            }
            if !devices.insert(name.to_string()) {
                reasons.push(format!("{name}: конфликт эксклюзивных назначений"))
            }
            match gpu_list
                .iter()
                .find(|g| g["id"] == *id && g["present"] == true)
            {
                None => reasons.push(format!(
                    "{name}: устройство отсутствует. Выберите GPU из текущей конфигурации"
                )),
                Some(g) => {
                    if g["compute_backend"] != "cuda" {
                        reasons.push(format!(
                            "{name}: вычислительный адаптер для этой GPU ещё не подключён"
                        ));
                        continue;
                    }
                    let needed = p["vram_gib_by_gpu"][name]
                        .as_f64()
                        .unwrap_or_else(|| p["vram_gib"].as_f64().unwrap_or(0.));
                    if index >= new_count {
                        if needed > g["vram_gib"].as_f64().unwrap_or(0.) {
                            reasons.push(format!("{name}: бюджет превышает объём VRAM"));
                        }
                        continue;
                    }
                    match (g["vram_gib"].as_f64(), g["used_gib"].as_f64()) {
                        (Some(total), Some(used)) if needed <= total - used => {}
                        (Some(total), Some(used)) => {
                            if needed <= total {
                                temporary.push(vram_wait(id, needed, total, used));
                            }
                            reasons.push(format!("{name}: недостаточно свободной VRAM"))
                        }
                        (total, _) => {
                            if total.is_none_or(|n| needed<=n) {
                                temporary.push(json!({"code":"device_telemetry","gpu_ids":[id]}));
                            }
                            reasons.push(format!("{name}: свободная VRAM неизвестна"))
                        }
                    }
                }
            }
        }
    }
    let reserve = settings["ram_reserve_gib"].as_f64().unwrap_or(0.);
    if ram + reserve > inventory["ram_total_gib"].as_f64().unwrap_or(0.) {
        reasons.push(
            "Суммарный бюджет RAM с резервом системы превышает текущую физическую память".into(),
        )
    }
    if (ram - credit).max(0.) + reserve > inventory["ram_available_gib"].as_f64().unwrap_or(0.) {
        let os_free = inventory["ram_available_gib"].as_f64().unwrap_or(0.);
        let held_ram = ram - requested_ram;
        temporary.push(json!({"code":"system_ram","needed_gib":requested_ram,
            "available_gib":((os_free+credit-held_ram-reserve).max(0.)*10.).floor()/10.,
            "os_available_gib":(os_free*10.).floor()/10.,"system_reserve_gib":reserve,
            "managed_reservations_gib":held_ram,"resident_credit_gib":(credit*10.).floor()/10.}));
        reasons.push("Сейчас недостаточно доступной RAM с учётом системного резерва".into())
    }
    if cpu + settings["cpu_reserve_threads"].as_f64().unwrap_or(0.)
        > inventory["logical_cpus"].as_f64().unwrap_or(0.)
    {
        reasons.push("Превышен бюджет текущих CPU-потоков".into())
    }
    let only_gpu_pressure = !temporary.is_empty() && temporary.len()==reasons.len()
        && temporary.iter().all(|r|matches!(r["code"].as_str(),Some("device_vram"|"device_telemetry")));
    let wait = if temporary.len()==reasons.len() {temporary.into_iter().next()} else {None};
    json!({"admissible":reasons.is_empty(),"reasons":reasons,"wait_reason":wait,"only_gpu_pressure":only_gpu_pressure,"ram_gib":ram,"additional_ram_gib":(ram-credit).max(0.),"reserved_between_stages_ram_gib":pending.0,"reserved_between_stages_threads":pending.1,"owned_resident_credit_gib":credit,"cpu_threads":cpu,"ram_reserve_gib":reserve,"configuration_id":inventory["configuration_id"],"collected_at_ms":inventory["collected_at_ms"],"profile_ids":profiles.iter().map(|p|p["id"].clone()).collect::<Vec<_>>(),"scope":"Предварительная проверка текущей конфигурации. Запуск повторно проверит ресурсы и зафиксирует профиль.","hard_isolation":false})
}
#[cfg(test)]
mod tests {
    use super::*;
    fn machine(ram: f64, cpu: u64, gpu: &str) -> Value {
        json!({"cpu_name":"fixture","ram_total_gib":ram,"ram_available_gib":ram*0.8,"logical_cpus":cpu,"gpus":if gpu.is_empty(){vec![]}else{vec![json!({"id":gpu,"name":"fixture GPU","present":true,"vram_gib":8,"used_gib":1,"compute_backend":"cuda"})]},"storage":[]})
    }
    #[test]
    fn temporary_pressure_does_not_hide_an_impossible_profile() {
        let i=machine(64.,12,"g");
        let s=suggested_settings(&i);
        let mut p=json!({"id":"render","name":"Render","gpu_ids":["g"],"ram_gib":4,"vram_gib":7.5,"cpu_threads":1});
        let available=|profile:&Value| plan(&i,&s,std::slice::from_ref(profile));
        assert_eq!(available(&p)["wait_reason"]["code"],"device_vram");
        p["ram_gib"]=json!(42);
        assert!(available(&p)["wait_reason"].is_object());
        p["vram_gib"]=json!(6);
        assert_eq!(available(&p)["wait_reason"]["code"],"system_ram");
        p["vram_gib"]=json!(9);
        assert!(available(&p)["wait_reason"].is_null());
        p["vram_gib"]=json!(7.5);
        p["ram_gib"]=json!(65);
        assert!(available(&p)["wait_reason"].is_null());
    }
    #[test]
    fn new_machine_has_only_detected_devices_and_fitting_defaults() {
        for i in [
            machine(4., 2, ""),
            machine(64., 24, "gpu-new"),
            machine(512., 96, "gpu-large"),
        ] {
            let s = suggested_settings(&i);
            let p = diagnostic_profile(&i);
            assert_eq!(plan(&i, &s, &[p])["admissible"], true);
            assert!(s["reserved_gpu_ids"].as_array().unwrap().is_empty())
        }
    }
    #[test]
    fn changed_ram_cpu_gpu_invalidates_old_profile() {
        let p = json!({"id":"a","name":"A","gpu_ids":["gpu-old"],"ram_gib":12,"vram_gib":4,"cpu_threads":6});
        let big = machine(64., 12, "gpu-old");
        assert_eq!(
            plan(&big, &suggested_settings(&big), &[p.clone()])["admissible"],
            true
        );
        let small = machine(8., 2, "gpu-new");
        let r = plan(&small, &suggested_settings(&small), &[p]);
        assert_eq!(r["admissible"], false);
        assert!(r["reasons"].as_array().unwrap().len() >= 3);
        assert_ne!(configuration_id(&big), configuration_id(&small))
    }
    #[test]
    fn duplicate_gpu_and_system_reserve_are_rejected() {
        let i = machine(64., 12, "g");
        let p =
            json!({"id":"a","name":"A","gpu_ids":["g"],"ram_gib":4,"vram_gib":2,"cpu_threads":1});
        let mut s = suggested_settings(&i);
        assert_eq!(plan(&i, &s, &[p.clone(), p.clone()])["admissible"], false);
        s["reserved_gpu_ids"] = json!(["g"]);
        assert_eq!(plan(&i, &s, &[p])["admissible"], false)
    }
    #[test]
    fn planned_ram_fits_the_launcher_minimum() {
        for ram in [0., 0.05, 0.125] {
            let p = json!({"id":"small","name":"Small","gpu_ids":[],"ram_gib":ram,"vram_gib":0,"cpu_threads":1});
            assert_eq!(validate_profile(&p).is_ok(), ram >= 0.125);
        }
    }
    #[test]
    fn adapter_inventory_is_not_a_fixed_brand_list() {
        let gpu = json!({"id":"nv","name":"NVIDIA GeForce Example"});
        let merged = merge_gpus(
            vec![gpu],
            vec![
                json!({"id":"dxgi1","name":"NVIDIA GeForce Example"}),
                json!({"id":"intel","name":"Intel Example"}),
                json!({"id":"amd","name":"AMD Example"}),
            ],
        );
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0]["id"], "nv");
        assert_eq!(merged[1]["id"], "intel")
    }
    #[test]
    fn storage_tracks_the_selected_mount_not_a_fixed_drive() {
        let d = vec![
            json!({"mount":"C:\\"}),
            json!({"mount":"F:\\"}),
            json!({"mount":"F:\\Models\\"}),
        ];
        assert_eq!(
            volume_for(&d, Path::new(r"F:\Models\weights")).unwrap()["mount"],
            "F:\\Models\\"
        );
        assert!(volume_for(&d, Path::new(r"Z:\gone")).is_none())
    }
    #[test]
    fn active_gpu_allocation_does_not_block_a_cpu_agent() {
        let mut i = machine(64., 12, "g");
        i["gpus"][0]["used_gib"] = json!(5.);
        i["ram_available_gib"] = json!(20.);
        let running = json!({"id":"model","name":"Model","gpu_ids":["g"],"ram_gib":30,"vram_gib":6,"cpu_threads":6});
        let agent = json!({"id":"agent","name":"Agent","gpu_ids":[],"ram_gib":1,"vram_gib":0,"cpu_threads":1});
        let s = suggested_settings(&i);
        assert_eq!(plan_with_active(&i,&s,&[agent.clone()],&[(running.clone(),20.)])["admissible"], false);
        i["ram_available_gib"]=json!(32.);
        let allowed = plan_with_active(&i, &s, &[agent.clone()], &[(running.clone(), 20.)]);
        assert_eq!(plan_reserved(&i,&s,&[agent],&[(running.clone(),20.)],(20.,0))["admissible"],false);
        assert_eq!(allowed["admissible"], true);
        assert_eq!(allowed["additional_ram_gib"], 11.);
        assert_eq!(
            plan_with_active(&i, &s, &[running.clone()], &[(running, 20.)])["admissible"],
            false
        );
    }
    #[test]
    fn heterogeneous_gpu_budgets_use_each_selected_device() {
        let mut i = machine(128., 16, "small");
        i["gpus"].as_array_mut().unwrap().push(json!({"id":"large","present":true,"vram_gib":24,"used_gib":1,"compute_backend":"cuda"}));
        let mut p = json!({"id":"mixed","name":"Mixed","gpu_ids":["small","large"],"ram_gib":20,"vram_gib":6,"vram_gib_by_gpu":{"small":6,"large":20},"cpu_threads":4});
        let s = suggested_settings(&i);
        assert_eq!(plan(&i, &s, &[p.clone()])["admissible"], true);
        p["vram_gib_by_gpu"]["small"] = json!(12);
        assert_eq!(plan(&i, &s, &[p])["admissible"], false);
    }
}
