use clap::{Parser, Subcommand};

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
            println!("coven doctor: ok");
        }
    }
    Ok(())
}
