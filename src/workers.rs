//! Owned Windows processes. Limits are assigned while the main thread is suspended.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom},
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{JobObjects::*, Threading::*},
};

/// A short-lived integration owns its descendants even when the adapter times out.
/// Attach before sending configuration: the bridge cannot spawn a server until stdin arrives.
pub struct ProcessTreeGuard(usize);
impl ProcessTreeGuard {
    pub fn attach(pid: u32) -> Result<Self> {
        unsafe {
            let job = CreateJobObjectW(ptr::null(), ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let guard = Self(job as usize);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let assigned = AssignProcessToJobObject(job, process);
            let error = std::io::Error::last_os_error();
            CloseHandle(process);
            if assigned == 0 {
                return Err(error.into());
            }
            Ok(guard)
        }
    }
}
impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0 as HANDLE);
        }
    }
}

pub struct Worker {
    job: usize,
    process: usize,
    pub pid: u32,
    pub profile: Value,
}
impl Drop for Worker {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.process as HANDLE);
            CloseHandle(self.job as HANDLE);
        }
    }
}
impl Worker {
    pub fn usage(&self) -> Value {
        // Query all processes in this Job, including runtime descendants.
        #[repr(C)]
        struct Ids {
            assigned: u32,
            count: u32,
            pids: [usize; 512],
        }
        #[repr(C)]
        #[derive(Default)]
        struct Memory {
            committed: u64,
            peak: u64,
        }
        let mut ids = Ids {
            assigned: 0,
            count: 0,
            pids: [0; 512],
        };
        let mut memory = Memory::default();
        let mut resident = 0u64;
        let mut measured_processes=0usize;
        let mut private_resident = 0u64;
        let mut measured_private=false;
        let mut pids = vec![];
        unsafe {
            let memory_ok = QueryInformationJobObject(
                self.job as HANDLE,
                28,
                &mut memory as *mut _ as *mut _,
                std::mem::size_of_val(&memory) as u32,
                ptr::null_mut(),
            ) != 0;
            let ids_ok = QueryInformationJobObject(
                self.job as HANDLE,
                JobObjectBasicProcessIdList,
                &mut ids as *mut _ as *mut _,
                std::mem::size_of_val(&ids) as u32,
                ptr::null_mut(),
            ) != 0;
            if ids_ok {
                for pid in ids.pids.iter().take(ids.count.min(512) as usize) {
                    pids.push(*pid);
                    let process =
                        OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, *pid as u32);
                    if process.is_null() {
                        continue;
                    }
                    use windows_sys::Win32::System::ProcessStatus::*;
                    let mut info=PROCESS_MEMORY_COUNTERS_EX2::default();
                    info.cb=std::mem::size_of_val(&info) as u32;
                    if K32GetProcessMemoryInfo(process,(&mut info as *mut PROCESS_MEMORY_COUNTERS_EX2).cast(),info.cb)!=0 {
                        measured_processes+=1;
                        resident+=info.WorkingSetSize as u64;
                        // Shared or paged-out pages cannot be credited against physical RAM.
                        private_resident+=info.PrivateWorkingSetSize.min(info.WorkingSetSize) as u64;
                        measured_private=true;
                    }else{
                        let mut older=PROCESS_MEMORY_COUNTERS_EX::default();
                        older.cb=std::mem::size_of_val(&older) as u32;
                        if K32GetProcessMemoryInfo(process,(&mut older as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),older.cb)!=0{
                            measured_processes+=1;
                            resident+=older.WorkingSetSize as u64;
                        }
                    }
                    CloseHandle(process);
                }
            }
            json!({"pid":self.pid,"process_ids":pids,"measured_processes":measured_processes,"resident_bytes":if measured_processes>0{Some(resident)}else{None},"admission_credit_bytes":if measured_private{Some(private_resident)}else{None},"committed_bytes":if memory_ok{Some(memory.committed)}else{None},"peak_committed_bytes":if memory_ok{Some(memory.peak)}else{None}})
        }
    }
    pub fn allocation(&self) -> (Value, f64) {
        let used =
            self.usage()["admission_credit_bytes"].as_u64().unwrap_or(0) as f64 / 1073741824.;
        (
            self.profile.clone(),
            used.min(self.profile["ram_gib"].as_f64().unwrap_or(0.)),
        )
    }
    pub fn exit_code(&self) -> Result<Option<u32>> {
        unsafe {
            if WaitForSingleObject(self.process as HANDLE, 0) == WAIT_TIMEOUT {
                return Ok(None);
            }
            let mut code = 0;
            if GetExitCodeProcess(self.process as HANDLE, &mut code) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(Some(code))
        }
    }
    pub fn stop(&self) -> Result<()> {
        unsafe {
            if TerminateJobObject(self.job as HANDLE, 1) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if WaitForSingleObject(self.process as HANDLE, 3000) == WAIT_TIMEOUT {
                bail!("Процесс ещё останавливается")
            }
            // The launcher can exit before its CUDA/server children release resources.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
                if QueryInformationJobObject(
                    self.job as HANDLE,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut _,
                    std::mem::size_of_val(&info) as u32,
                    ptr::null_mut(),
                ) == 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                if info.ActiveProcesses == 0 {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    bail!("Дочерние процессы ещё освобождают ресурсы")
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        Ok(())
    }
}
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}
// Windows CommandLineToArgvW/CRT quoting; never invokes a command shell.
pub fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in s.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '"' {
            out.push_str(&"\\".repeat(slashes * 2 + 1));
            out.push('"')
        } else {
            out.push_str(&"\\".repeat(slashes));
            out.push(c)
        }
        slashes = 0;
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}
pub fn validate(spec: &Value) -> Result<()> {
    crate::safe_id(spec["id"].as_str().unwrap_or(""))?;
    let path = Path::new(spec["command"].as_str().unwrap_or(""));
    if !path.is_absolute()
        || !path.is_file()
        || path
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.to_ascii_lowercase())
            != Some("exe".into())
    {
        bail!("Укажите существующий .exe полным путём")
    }
    if !spec["args"]
        .as_array()
        .map(|a| {
            a.iter()
                .all(|v| v.as_str().map(|s| !s.contains('\0')).unwrap_or(false))
        })
        .unwrap_or(false)
    {
        bail!("args должен содержать строки без NUL")
    }
    if let Some(cwd) = spec["cwd"].as_str() {
        if !Path::new(cwd).is_absolute() || !Path::new(cwd).is_dir() {
            bail!("Рабочая папка не существует")
        }
    }
    Ok(())
}
pub fn start(spec: &Value, profile: Value, logical_cpus: usize) -> Result<Worker> {
    validate(spec)?;
    let memory = profile["ram_gib"].as_f64().unwrap_or(0.);
    if memory < 0.125 {
        bail!("Для запуска требуется минимум 0.125 ГиБ RAM")
    }
    let command = spec["command"].as_str().unwrap();
    let id = spec["id"].as_str().unwrap();
    let mut args = vec![quote(command)];
    for arg in spec["args"].as_array().unwrap() {
        args.push(quote(arg.as_str().unwrap()));
    }
    let mut cmdline = wide(OsStr::new(&args.join(" ")));
    if cmdline.len() > 32767 {
        bail!("Командная строка слишком длинна")
    }
    let application = wide(OsStr::new(command));
    let cwd = wide(OsStr::new(spec["cwd"].as_str().unwrap_or(
        command.rsplit_once(['\\', '/']).map(|x| x.0).unwrap_or("."),
    )));
    let mut env: BTreeMap<String, String> = std::env::vars()
        .filter(|(k, _)| {
            !k.eq_ignore_ascii_case("CUDA_VISIBLE_DEVICES")
                && !["OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS"]
                    .iter()
                    .any(|s| k.eq_ignore_ascii_case(s))
        })
        .collect();
    let devices = profile["gpu_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(",");
    env.insert("CUDA_VISIBLE_DEVICES".into(), devices);
    let threads = profile["cpu_threads"].as_u64().unwrap_or(1);
    for key in ["OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS"] {
        env.insert(key.into(), threads.to_string());
    }
    let mut environment: Vec<u16> = vec![];
    for (k, v) in env {
        environment.extend(format!("{k}={v}").encode_utf16());
        environment.push(0);
    }
    environment.push(0);
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(crate::root().join(format!("logs/instance-{id}.log")))?;
    let input = OpenOptions::new().read(true).open("NUL")?;
    unsafe {
        let job = CreateJobObjectW(ptr::null(), ptr::null());
        if job.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut worker = Worker {
            job: job as usize,
            process: 0,
            pid: 0,
            profile,
        };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        limits.JobMemoryLimit = (memory * 1073741824.) as usize;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of_val(&limits) as u32,
        ) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let cpu_rate =
            ((threads as f64 / logical_cpus.max(1) as f64) * 10000.).clamp(1., 10000.) as u32;
        let cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
            ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 { CpuRate: cpu_rate },
        };
        if SetInformationJobObject(
            job,
            JobObjectCpuRateControlInformation,
            &cpu as *const _ as *const _,
            std::mem::size_of_val(&cpu) as u32,
        ) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let out_handle = log.as_raw_handle() as HANDLE;
        let in_handle = input.as_raw_handle() as HANDLE;
        if SetHandleInformation(out_handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0
            || SetHandleInformation(in_handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut startup = STARTUPINFOW::default();
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        startup.dwFlags = STARTF_USESTDHANDLES;
        startup.hStdInput = in_handle;
        startup.hStdOutput = out_handle;
        startup.hStdError = out_handle;
        let mut info = PROCESS_INFORMATION::default();
        let created = CreateProcessW(
            application.as_ptr(),
            cmdline.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr() as *const _,
            cwd.as_ptr(),
            &startup,
            &mut info,
        );
        let create_error = std::io::Error::last_os_error();
        SetHandleInformation(out_handle, HANDLE_FLAG_INHERIT, 0);
        SetHandleInformation(in_handle, HANDLE_FLAG_INHERIT, 0);
        if created == 0 {
            return Err(create_error).context("Запуск процесса Windows");
        }
        worker.process = info.hProcess as usize;
        worker.pid = info.dwProcessId;
        if AssignProcessToJobObject(job, info.hProcess) == 0 {
            let error = std::io::Error::last_os_error();
            TerminateProcess(info.hProcess, 1);
            CloseHandle(info.hThread);
            return Err(error).context("Назначение ограничений Job Object");
        }
        if ResumeThread(info.hThread) == u32::MAX {
            let error = std::io::Error::last_os_error();
            CloseHandle(info.hThread);
            return Err(error).context("Продолжение процесса после настройки лимитов");
        }
        CloseHandle(info.hThread);
        Ok(worker)
    }
}
pub fn enforcement(profile: &Value, logical_cpus: usize) -> Value {
    json!({"ram":"Windows Job Object total committed memory limit","ram_bytes":(profile["ram_gib"].as_f64().unwrap_or(0.)*1073741824.) as u64,"cpu":"Windows Job Object hard CPU rate cap","cpu_rate_percent":profile["cpu_threads"].as_f64().unwrap_or(1.)/logical_cpus.max(1) as f64*100.,"gpu":"CUDA_VISIBLE_DEVICES UUID selection; not hardware isolation","vram":"Admission budget only; no hard per-process VRAM quota","controller_exit":"owned process tree terminated by Job Object"})
}
pub fn tail(path: &Path, lines: usize) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let offset = len.saturating_sub(1024 * 1024);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<_> = text.lines().collect();
    let start = all
        .len()
        .saturating_sub(lines.min(500))
        .max(if offset > 0 { 1 } else { 0 })
        .min(all.len());
    Ok(all[start..].join("\n"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_quotes_preserve_trailing_slashes_and_quotes() {
        assert_eq!(quote("x y"), "\"x y\"");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote("a\\"), "\"a\\\\\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
    }
}
