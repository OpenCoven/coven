use crate::pc::diagnostics::{DiskInfo, ProcessInfo, SystemSnapshot};
use serde_json::json;

pub enum OutputFormat {
    Compact,
    Json,
    Verbose,
}

fn health_indicator(pct: f32, warn: f32, crit: f32) -> &'static str {
    if pct >= crit {
        "[crit]"
    } else if pct >= warn {
        "[warn]"
    } else {
        "[ok]"
    }
}

pub fn print_status(snap: &SystemSnapshot, format: &OutputFormat) {
    println!("{}", format_status(snap, format));
}

fn format_status(snap: &SystemSnapshot, format: &OutputFormat) -> String {
    if matches!(format, OutputFormat::Json) {
        return format_json(snap);
    }

    let cpu_ind = health_indicator(snap.cpu_usage_pct, 70.0, 90.0);
    let mem_pct = if snap.memory_total_mb > 0 {
        snap.memory_used_mb as f32 / snap.memory_total_mb as f32 * 100.0
    } else {
        0.0
    };
    let mem_ind = health_indicator(mem_pct, 70.0, 90.0);
    format!(
        "{cpu_ind} CPU {:.1}%  {mem_ind} RAM {}/{} MB  uptime {}",
        snap.cpu_usage_pct,
        snap.memory_used_mb,
        snap.memory_total_mb,
        format_uptime(snap.uptime_secs),
    )
}

pub fn print_full(snap: &SystemSnapshot, format: &OutputFormat) {
    match format {
        OutputFormat::Json => println!("{}", format_json(snap)),
        OutputFormat::Compact | OutputFormat::Verbose => print_human(snap, format),
    }
}

fn print_human(snap: &SystemSnapshot, format: &OutputFormat) {
    println!("── System ──────────────────────────────────");
    let cpu_ind = health_indicator(snap.cpu_usage_pct, 70.0, 90.0);
    println!("  {cpu_ind} CPU      {:.1}%", snap.cpu_usage_pct);

    let mem_pct = if snap.memory_total_mb > 0 {
        snap.memory_used_mb as f32 / snap.memory_total_mb as f32 * 100.0
    } else {
        0.0
    };
    let mem_ind = health_indicator(mem_pct, 70.0, 90.0);
    println!(
        "  {mem_ind} Memory   {} / {} MB  ({:.0}%)",
        snap.memory_used_mb, snap.memory_total_mb, mem_pct,
    );

    if snap.swap_total_mb > 0 {
        let swap_pct = snap.swap_used_mb as f32 / snap.swap_total_mb as f32 * 100.0;
        let swap_ind = health_indicator(swap_pct, 50.0, 80.0);
        println!(
            "  {swap_ind} Swap     {} / {} MB  ({:.0}%)",
            snap.swap_used_mb, snap.swap_total_mb, swap_pct,
        );
    }
    println!("  ⏱  Uptime   {}", format_uptime(snap.uptime_secs));

    println!("\n── Disk ────────────────────────────────────");
    if snap.disks.is_empty() {
        println!("  No disks detected.");
    }
    for disk in &snap.disks {
        print_disk(disk);
    }

    println!("\n── Top Processes (by CPU) ──────────────────");
    if snap.processes.is_empty() {
        println!("  No processes to show.");
    }
    for proc in snap.processes.iter().take(10) {
        print_proc(proc, format);
    }
}

fn print_disk(d: &DiskInfo) {
    let ind = health_indicator(d.used_pct, 80.0, 95.0);
    println!(
        "  {ind} {}  {:.1} GB used / {:.1} GB total  ({:.0}%)",
        d.mount,
        d.total_gb - d.available_gb,
        d.total_gb,
        d.used_pct,
    );
}

fn print_proc(p: &ProcessInfo, format: &OutputFormat) {
    let base = format!(
        "  PID {:>6}  CPU {:>5.1}%  MEM {:>5} MB  {}",
        p.pid, p.cpu_pct, p.memory_mb, p.name,
    );
    if let (OutputFormat::Verbose, Some(argv)) = (format, &p.argv) {
        println!("{}  [{}]", base, argv.join(" "));
    } else {
        println!("{base}");
    }
}

pub fn print_top(snap: &SystemSnapshot, n: usize, format: &OutputFormat) {
    if matches!(format, OutputFormat::Json) {
        println!("{}", format_top_json(snap, n));
        return;
    }
    println!("── Top {} Processes (by CPU) ────────────────", n);
    if snap.processes.is_empty() {
        println!("  No processes to show.");
    }
    for proc in snap.processes.iter().take(n) {
        print_proc(proc, format);
    }
}

pub fn print_disk_usage(snap: &SystemSnapshot, format: &OutputFormat) {
    if matches!(format, OutputFormat::Json) {
        println!("{}", format_disk_json(snap));
        return;
    }
    println!("── Disk Usage ──────────────────────────────");
    if snap.disks.is_empty() {
        println!("  No disks detected.");
    }
    for disk in &snap.disks {
        print_disk(disk);
    }
}

fn format_top_json(snap: &SystemSnapshot, n: usize) -> String {
    let value = json!({
        "processes": snap.processes.iter().take(n).map(|p| {
            let mut process = json!({
                "pid": p.pid,
                "name": p.name,
                "cpu_pct": p.cpu_pct,
                "memory_mb": p.memory_mb,
            });
            // argv is only captured when --verbose requested it.
            if let Some(argv) = &p.argv {
                process["argv"] = json!(argv);
            }
            process
        }).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&value).expect("process list JSON serialization cannot fail")
}

fn format_disk_json(snap: &SystemSnapshot) -> String {
    let value = json!({
        "disks": snap.disks.iter().map(|d| {
            json!({
                "mount": d.mount,
                "total_gb": d.total_gb,
                "available_gb": d.available_gb,
                "used_pct": d.used_pct,
            })
        }).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&value).expect("disk list JSON serialization cannot fail")
}

fn format_json(snap: &SystemSnapshot) -> String {
    let procs: Vec<_> = snap.processes.iter().take(20).collect();
    let value = json!({
        "cpu_usage_pct": snap.cpu_usage_pct,
        "memory_used_mb": snap.memory_used_mb,
        "memory_total_mb": snap.memory_total_mb,
        "swap_used_mb": snap.swap_used_mb,
        "swap_total_mb": snap.swap_total_mb,
        "uptime_secs": snap.uptime_secs,
        "processes": procs.iter().map(|p| {
            json!({
                "pid": p.pid,
                "name": p.name,
                "cpu_pct": p.cpu_pct,
                "memory_mb": p.memory_mb,
            })
        }).collect::<Vec<_>>(),
        "disks": snap.disks.iter().map(|d| {
            json!({
                "mount": d.mount,
                "total_gb": d.total_gb,
                "available_gb": d.available_gb,
                "used_pct": d.used_pct,
            })
        }).collect::<Vec<_>>(),
    });

    serde_json::to_string_pretty(&value).expect("system snapshot JSON serialization cannot fail")
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> SystemSnapshot {
        SystemSnapshot {
            cpu_usage_pct: 12.5,
            memory_used_mb: 1024,
            memory_total_mb: 4096,
            swap_used_mb: 0,
            swap_total_mb: 0,
            uptime_secs: 3661,
            processes: vec![ProcessInfo {
                pid: 42,
                name: "proc \"quoted\"\u{7}".to_string(),
                cpu_pct: 3.5,
                memory_mb: 64,
                argv: None,
            }],
            disks: vec![DiskInfo {
                mount: "/Volumes/name \"quoted\"\u{7}".to_string(),
                total_gb: 100.0,
                available_gb: 40.0,
                used_pct: 60.0,
            }],
        }
    }

    #[test]
    fn status_json_serializes_valid_json_with_expected_keys() {
        let body = format_status(&sample_snapshot(), &OutputFormat::Json);
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");

        assert_eq!(value["cpu_usage_pct"], 12.5);
        assert_eq!(value["processes"][0]["name"], "proc \"quoted\"\u{7}");
        assert_eq!(value["disks"][0]["mount"], "/Volumes/name \"quoted\"\u{7}");
    }

    #[test]
    fn human_status_uses_non_emoji_indicators() {
        let body = format_status(&sample_snapshot(), &OutputFormat::Compact);

        assert!(body.contains("[ok] CPU"));
        assert!(!body.contains('🟢'));
        assert!(!body.contains('🟡'));
        assert!(!body.contains('🔴'));
    }

    #[test]
    fn top_json_serializes_processes_with_expected_keys() {
        let body = format_top_json(&sample_snapshot(), 10);
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");

        let processes = value["processes"].as_array().expect("processes array");
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0]["pid"], 42);
        assert_eq!(processes[0]["name"], "proc \"quoted\"\u{7}");
        assert_eq!(processes[0]["cpu_pct"], 3.5);
        assert_eq!(processes[0]["memory_mb"], 64);
        // argv was not captured (non-verbose snapshot), so the key is absent.
        assert!(processes[0].get("argv").is_none());
    }

    #[test]
    fn top_json_respects_process_limit_and_includes_argv_when_captured() {
        let mut snap = sample_snapshot();
        snap.processes = vec![
            ProcessInfo {
                pid: 1,
                name: "one".to_string(),
                cpu_pct: 9.0,
                memory_mb: 10,
                argv: Some(vec!["one".to_string(), "--flag".to_string()]),
            },
            ProcessInfo {
                pid: 2,
                name: "two".to_string(),
                cpu_pct: 1.0,
                memory_mb: 5,
                argv: None,
            },
        ];

        let body = format_top_json(&snap, 1);
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");

        let processes = value["processes"].as_array().expect("processes array");
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0]["argv"], serde_json::json!(["one", "--flag"]));
    }

    #[test]
    fn disk_json_serializes_disks_with_expected_keys() {
        let body = format_disk_json(&sample_snapshot());
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");

        let disks = value["disks"].as_array().expect("disks array");
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0]["mount"], "/Volumes/name \"quoted\"\u{7}");
        assert_eq!(disks[0]["total_gb"], 100.0);
        assert_eq!(disks[0]["available_gb"], 40.0);
        assert_eq!(disks[0]["used_pct"], 60.0);
    }
}
