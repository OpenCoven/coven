use anyhow::Result;
use sysinfo::{Disks, Pid, ProcessesToUpdate, System};

#[derive(Debug)]
pub struct SystemSnapshot {
    pub cpu_usage_pct: f32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub swap_used_mb: u64,
    pub swap_total_mb: u64,
    pub uptime_secs: u64,
    pub processes: Vec<ProcessInfo>,
    pub disks: Vec<DiskInfo>,
}

#[derive(Debug)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_pct: f32,
    pub memory_mb: u64,
    /// Full argv — only included when verbose is requested
    pub argv: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct DiskInfo {
    pub mount: String,
    pub total_gb: f64,
    pub available_gb: f64,
    pub used_pct: f32,
}

pub fn snapshot(verbose: bool) -> Result<SystemSnapshot> {
    let mut sys = System::new_all();
    // Sleep the minimum interval so CPU usage is meaningful
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_all();

    let cpu_usage_pct = sys.global_cpu_usage();
    let memory_used_mb = sys.used_memory() / 1024 / 1024;
    let memory_total_mb = sys.total_memory() / 1024 / 1024;
    let swap_used_mb = sys.used_swap() / 1024 / 1024;
    let swap_total_mb = sys.total_swap() / 1024 / 1024;
    let uptime_secs = System::uptime();

    let mut processes: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, p)| ProcessInfo {
            pid: pid.as_u32(),
            name: p.name().to_string_lossy().into_owned(),
            cpu_pct: p.cpu_usage(),
            memory_mb: p.memory() / 1024 / 1024,
            argv: if verbose {
                Some(
                    p.cmd()
                        .iter()
                        .map(|arg| arg.to_string_lossy().into_owned())
                        .collect(),
                )
            } else {
                None
            },
        })
        .collect();

    // Sort by CPU descending
    processes.sort_by(|a, b| {
        b.cpu_pct
            .partial_cmp(&a.cpu_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let disk_list = Disks::new_with_refreshed_list();
    let disks: Vec<DiskInfo> = disk_list
        .iter()
        .map(|d| {
            let total = d.total_space() as f64 / 1024.0 / 1024.0 / 1024.0;
            let avail = d.available_space() as f64 / 1024.0 / 1024.0 / 1024.0;
            let used_pct = if total > 0.0 {
                ((total - avail) / total * 100.0) as f32
            } else {
                0.0
            };
            DiskInfo {
                mount: d.mount_point().to_string_lossy().to_string(),
                total_gb: total,
                available_gb: avail,
                used_pct,
            }
        })
        .collect();

    Ok(SystemSnapshot {
        cpu_usage_pct,
        memory_used_mb,
        memory_total_mb,
        swap_used_mb,
        swap_total_mb,
        uptime_secs,
        processes,
        disks,
    })
}

/// Look up a process name for pre-signal identity verification.
#[allow(dead_code)]
pub fn process_name_for_pid(pid: u32) -> Option<String> {
    let mut sys = System::new();
    let pid_key = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid_key]), true);
    sys.process(pid_key)
        .map(|p| p.name().to_string_lossy().into_owned())
}
