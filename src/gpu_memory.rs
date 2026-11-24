//! Dedicated video memory charged by WDDM to the processes in a Windows Job.
use std::{collections::BTreeMap, ptr};
use serde_json::{json, Value};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use windows_sys::Win32::System::Performance::*;

struct Query(PDH_HQUERY);

/// TCC adapters can report per-process memory directly through the NVIDIA driver.
/// WDDM "N/A" cells remain unavailable. Never attribute total GPU usage to one job.
async fn nvidia_rows() -> Option<Vec<(u64, String, u64)>> {
    let mut cmd = tokio::process::Command::new("nvidia-smi");
    cmd.args([
        "--query-compute-apps=pid,gpu_uuid,used_gpu_memory",
        "--format=csv,noheader,nounits",
    ])
    .stdin(std::process::Stdio::null())
    .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let output = tokio::time::timeout(std::time::Duration::from_secs(2), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut values = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split(',').map(str::trim);
        let Some(pid) = fields.next().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        let Some(uuid) = fields.next().filter(|s| s.starts_with("GPU-")) else {
            continue;
        };
        let Some(mib) = fields.next().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        values.push((pid, uuid.to_owned(), mib.saturating_mul(1048576)));
    }
    Some(values)
}
pub async fn nvidia_processes(pids: &[u64]) -> Option<BTreeMap<String, u64>> {
    let mut values = BTreeMap::new();
    for (pid, uuid, bytes) in nvidia_rows().await? {
        if pids.contains(&pid) { *values.entry(uuid).or_default() += bytes; }
    }
    (!values.is_empty()).then_some(values)
}

pub fn display_id(adapter: &str) -> Option<String> {
    let mut parts = adapter.split('_');
    let high = u32::from_str_radix(parts.next()?.strip_prefix("0x")?, 16).ok()?;
    let low = u32::from_str_radix(parts.next()?.strip_prefix("0x")?, 16).ok()?;
    Some(format!("dxgi-{high:08x}-{low:08x}"))
}

fn largest_processes(charges: BTreeMap<u64,u64>) -> Vec<Value> {
    let mut rows:Vec<_> = charges.into_iter().filter(|(_,bytes)| *bytes >= 1024*1024).collect();
    rows.sort_unstable_by_key(|&(pid,bytes)| (std::cmp::Reverse(bytes),pid));
    rows.truncate(3);
    let ids:Vec<_> = rows.iter().map(|&(pid,_)| Pid::from_u32(pid as u32)).collect();
    let mut os = System::new();
    if !ids.is_empty() {
        os.refresh_processes_specifics(ProcessesToUpdate::Some(&ids), true, ProcessRefreshKind::nothing());
    }
    rows.into_iter().zip(ids).map(|((pid,bytes),id)|
        json!({"pid":pid,"name":os.process(id).map(|p|p.name().to_string_lossy()),"dedicated_mib":bytes/1048576})
    ).collect()
}

/// Only for a blocked GPU. Driver-unattributed usage stays in the board total.
pub async fn consumers(gpu: &Value) -> Vec<Value> {
    if gpu["driver_model"] == "TCC" {
        let mut charges = BTreeMap::new();
        for (pid,uuid,bytes) in nvidia_rows().await.unwrap_or_default() {
            if gpu["id"] == uuid { *charges.entry(pid).or_default() += bytes; }
        }
        return tokio::task::spawn_blocking(move||largest_processes(charges)).await.unwrap_or_default()
    }
    let Some(luid) = gpu["display_adapter_id"].as_str().map(str::to_owned) else { return vec![] };
    tokio::task::spawn_blocking(move|| {
        let mut charges = BTreeMap::new();
        visit_dedicated(|pid,adapter,bytes| {
            if display_id(adapter).as_deref() == Some(&luid) { *charges.entry(pid).or_default() += bytes; }
        });
        largest_processes(charges)
    }).await.unwrap_or_default()
}
impl Drop for Query {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.0);
        }
    }
}

pub fn dedicated(pids: &[u64]) -> Option<BTreeMap<String, u64>> {
    if pids.is_empty() {
        return None;
    }
    let mut values = BTreeMap::new();
    visit_dedicated(|pid, adapter, bytes| {
        if pids.contains(&pid) { *values.entry(adapter.to_owned()).or_default() += bytes; }
    })?;
    (!values.is_empty()).then_some(values)
}

fn visit_dedicated(mut observe: impl FnMut(u64, &str, u64)) -> Option<()> {
    let path: Vec<u16> = r"\GPU Process Memory(*)\Dedicated Usage"
        .encode_utf16()
        .chain([0])
        .collect();
    unsafe {
        let mut handle = ptr::null_mut();
        if PdhOpenQueryW(ptr::null(), 0, &mut handle) != 0 {
            return None;
        }
        let query = Query(handle);
        let mut counter = ptr::null_mut();
        if PdhAddEnglishCounterW(query.0, path.as_ptr(), 0, &mut counter) != 0
            || PdhCollectQueryData(query.0) != 0
        {
            return None;
        }
        let mut bytes = 0;
        let mut count = 0;
        if PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_LARGE,
            &mut bytes,
            &mut count,
            ptr::null_mut(),
        ) != PDH_MORE_DATA
            || bytes > 16 * 1024 * 1024
        {
            return None;
        }
        let mut storage = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        let items = storage.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        if PdhGetFormattedCounterArrayW(counter, PDH_FMT_LARGE, &mut bytes, &mut count, items) != 0
        {
            return None;
        }
        for item in std::slice::from_raw_parts(items, count as usize) {
            if item.FmtValue.CStatus > 1 || item.szName.is_null() {
                continue;
            }
            let mut len = 0;
            while *item.szName.add(len) != 0 {
                len += 1
            }
            let name = String::from_utf16_lossy(std::slice::from_raw_parts(item.szName, len));
            let Some((pid, adapter)) = name
                .strip_prefix("pid_")
                .and_then(|v| v.split_once("_luid_"))
            else {
                continue;
            };
            let Ok(pid) = pid.parse::<u64>() else {continue};
            let value = item.FmtValue.Anonymous.largeValue;
            if value >= 0 {
                observe(pid,adapter,value as u64);
            }
        }
        Some(())
    }
}
