use std::env;
use std::fs;
use std::io::{self, Read};
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() -> io::Result<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args == ["--descendant"] {
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
    if args == ["--version"] {
        println!("1.2.3");
        return Ok(());
    }

    if let Some(path) = env::var_os("COVEN_SETUP_PROBE_ARGS") {
        fs::write(path, args.join("\n"))?;
    }
    match env::var("COVEN_SETUP_PROBE_MODE").as_deref() {
        Ok("streams") => emit_hostile_streams(),
        Ok("timeout") => block_with_descendant(),
        Ok("failure") => std::process::exit(17),
        _ => Ok(()),
    }
}

fn emit_hostile_streams() -> io::Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let private_path = env::var("COVEN_SETUP_PROBE_PRIVATE").unwrap_or_default();
    println!(
        "provider-stdout:{} oauth account token model {}",
        input.trim(),
        private_path
    );
    eprintln!(
        "provider-stderr:\u{1b}[31m bearer authorization cookie {}",
        private_path
    );
    Ok(())
}

fn block_with_descendant() -> io::Result<()> {
    let child = Command::new(env::current_exe()?)
        .arg("--descendant")
        .spawn()?;
    let state_path = env::var_os("COVEN_SETUP_PROBE_STATE")
        .ok_or_else(|| io::Error::other("missing probe state path"))?;
    fs::write(
        state_path,
        format!(
            "{}\n{}\n{}\n{}\n",
            env::current_dir()?.display(),
            env::var_os("COVEN_HOME")
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
            std::process::id(),
            child.id()
        ),
    )?;
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
