use clap::{Parser, Subcommand};

mod harness;
mod project;
mod store;

#[derive(Parser, Debug)]
#[command(name = "coven")]
#[command(about = "Project-scoped harness substrate for agent sessions")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Doctor,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor => {
            println!("coven doctor");
            for harness in harness::built_in_harnesses() {
                let status = if harness.available {
                    "available"
                } else {
                    "missing"
                };
                println!("- {} ({}): {status}", harness.label, harness.executable);
                if !harness.available {
                    println!("  {}", harness.install_hint);
                }
            }
        }
    }
    Ok(())
}
