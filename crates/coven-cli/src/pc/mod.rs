pub mod diagnostics;
pub mod display;
pub mod relief;

use anyhow::Result;
use clap::Subcommand;

use crate::pc::display::OutputFormat;

#[derive(Subcommand, Debug)]
pub enum PcCommand {
    #[command(about = "One-line health summary")]
    Status {
        #[arg(long, help = "Output as JSON")]
        json: bool,
    },
    #[command(about = "Top processes by CPU and memory usage")]
    Top {
        #[arg(long, default_value = "10", help = "Number of processes to show")]
        n: usize,
        #[arg(long, help = "Include full command-line arguments")]
        verbose: bool,
        #[arg(long, help = "Output as JSON")]
        json: bool,
    },
    #[command(about = "Disk usage breakdown")]
    Disk {
        #[arg(long, help = "Output as JSON")]
        json: bool,
    },
    #[command(about = "Kill a process by PID (requires --confirm)")]
    Kill {
        #[arg(help = "Process ID to terminate")]
        pid: u32,
        #[arg(long, help = "Required: confirm you intend to terminate this process")]
        confirm: bool,
    },
    #[command(about = "Clear user and system caches (requires --confirm)")]
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    #[command(about = "Clear ~/Library/Caches and /Library/Caches (requires --confirm)")]
    Clear {
        #[arg(long, help = "Required: confirm you intend to clear caches")]
        confirm: bool,
    },
}

pub fn run_pc_command(command: Option<PcCommand>) -> Result<()> {
    match command {
        None => {
            // Full report
            let snap = diagnostics::snapshot(false)?;
            display::print_full(&snap, &OutputFormat::Compact);
        }
        Some(PcCommand::Status { json }) => {
            let snap = diagnostics::snapshot(false)?;
            let fmt = if json {
                OutputFormat::Json
            } else {
                OutputFormat::Compact
            };
            display::print_status(&snap, &fmt);
        }
        Some(PcCommand::Top { n, verbose, json }) => {
            let snap = diagnostics::snapshot(verbose)?;
            let fmt = if json {
                OutputFormat::Json
            } else if verbose {
                OutputFormat::Verbose
            } else {
                OutputFormat::Compact
            };
            display::print_top(&snap, n, &fmt);
        }
        Some(PcCommand::Disk { json }) => {
            let snap = diagnostics::snapshot(false)?;
            let fmt = if json {
                OutputFormat::Json
            } else {
                OutputFormat::Compact
            };
            display::print_disk_usage(&snap, &fmt);
        }
        Some(PcCommand::Kill { pid, confirm }) => {
            relief::kill_by_pid(pid, confirm)?;
        }
        Some(PcCommand::Cache {
            command: CacheCommand::Clear { confirm },
        }) => {
            relief::clear_caches(confirm)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kill_requires_confirm() {
        let err = relief::kill_by_pid(99999999, false).unwrap_err();
        assert!(err.to_string().contains("--confirm"));
    }

    #[test]
    fn test_cache_clear_requires_confirm() {
        let err = relief::clear_caches(false).unwrap_err();
        assert!(err.to_string().contains("--confirm"));
    }

    #[test]
    fn test_snapshot_returns_data() {
        let snap = diagnostics::snapshot(false).expect("snapshot should succeed");
        assert!(snap.memory_total_mb > 0);
    }
}
