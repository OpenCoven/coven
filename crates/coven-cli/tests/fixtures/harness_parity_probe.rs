//! Native recorder behind the Windows parity suite's npm-style shims.
//! Compiled with rustc directly so fixtures need no Node/provider installation.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let settings = fs::read_to_string(std::env::current_exe()?.with_extension("settings"))?;
    let mut lines = settings.lines();
    let mode = lines.next().ok_or("missing mode or exit code")?;
    if mode == "git" {
        let status = std::process::Command::new(lines.next().ok_or("missing Git path")?)
            .args(std::env::args_os().skip(1))
            .status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
    let exit_code: i32 = mode.parse()?;
    let hold = lines.next().ok_or("missing hold setting")? == "true";
    let record = PathBuf::from(lines.next().ok_or("missing record path")?);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut output = OpenOptions::new().append(true).open(&record)?;
    for arg in &args {
        writeln!(output, "{arg}")?;
    }
    if args.first().is_some_and(|arg| arg == "exec")
        && args.last().is_some_and(|arg| arg == "-")
    {
        let mut prompt = String::new();
        std::io::stdin().read_to_string(&mut prompt)?;
        fs::write(record.with_extension("stdin"), prompt)?;
    }
    println!("fake harness ran");
    if hold {
        std::thread::sleep(std::time::Duration::from_secs(300));
    }
    std::process::exit(exit_code);
}
