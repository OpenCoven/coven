use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::UnixListener;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub pid: u32,
    pub started_at: String,
    pub socket: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonSpawnSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub coven_home: PathBuf,
}

pub fn daemon_status_path(coven_home: &Path) -> PathBuf {
    coven_home.join("daemon.json")
}

pub fn daemon_socket_path(coven_home: &Path) -> PathBuf {
    coven_home.join("coven.sock")
}

pub fn background_server_spec(current_exe: &Path, coven_home: &Path) -> DaemonSpawnSpec {
    DaemonSpawnSpec {
        program: current_exe.to_path_buf(),
        args: vec!["daemon".to_string(), "serve".to_string()],
        coven_home: coven_home.to_path_buf(),
    }
}

pub fn start_background_server(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
) -> Result<DaemonStatus> {
    let spec = background_server_spec(current_exe, coven_home);
    std::fs::create_dir_all(coven_home)
        .with_context(|| format!("failed to create Coven home {}", coven_home.display()))?;
    let child = Command::new(&spec.program)
        .args(&spec.args)
        .env("COVEN_HOME", &spec.coven_home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start Coven daemon {}", spec.program.display()))?;
    let status = DaemonStatus {
        pid: child.id(),
        started_at,
        socket: daemon_socket_path(coven_home)
            .to_string_lossy()
            .into_owned(),
    };
    write_status(coven_home, &status)?;
    Ok(status)
}

pub fn write_status(coven_home: &Path, status: &DaemonStatus) -> Result<()> {
    std::fs::create_dir_all(coven_home)
        .with_context(|| format!("failed to create Coven home {}", coven_home.display()))?;
    let json = serde_json::to_string_pretty(status).context("failed to serialize daemon status")?;
    std::fs::write(daemon_status_path(coven_home), format!("{json}\n"))
        .context("failed to write daemon status")?;
    Ok(())
}

pub fn read_status(coven_home: &Path) -> Result<Option<DaemonStatus>> {
    let path = daemon_status_path(coven_home);
    if !path.exists() {
        return Ok(None);
    }

    let json = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read daemon status {}", path.display()))?;
    let status = serde_json::from_str(&json).context("failed to parse daemon status")?;
    Ok(Some(status))
}

pub fn clear_status(coven_home: &Path) -> Result<bool> {
    let path = daemon_status_path(coven_home);
    if !path.exists() {
        return Ok(false);
    }

    std::fs::remove_file(&path)
        .with_context(|| format!("failed to remove daemon status {}", path.display()))?;
    Ok(true)
}

pub fn stop_background_server(coven_home: &Path) -> Result<bool> {
    let status = read_status(coven_home)?;
    let Some(status) = status else {
        return Ok(false);
    };

    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(status.pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    clear_status(coven_home)?;
    let socket = daemon_socket_path(coven_home);
    if socket.exists() {
        std::fs::remove_file(&socket)
            .with_context(|| format!("failed to remove daemon socket {}", socket.display()))?;
    }
    Ok(true)
}

#[cfg(unix)]
pub fn bind_api_socket(coven_home: &Path) -> Result<UnixListener> {
    std::fs::create_dir_all(coven_home)
        .with_context(|| format!("failed to create Coven home {}", coven_home.display()))?;
    let socket_path = daemon_socket_path(coven_home);
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)
            .with_context(|| format!("failed to remove stale socket {}", socket_path.display()))?;
    }
    UnixListener::bind(&socket_path)
        .with_context(|| format!("failed to bind Coven API socket {}", socket_path.display()))
}

#[cfg(unix)]
pub fn serve_forever(coven_home: &Path, started_at: String) -> Result<()> {
    let status = DaemonStatus {
        pid: std::process::id(),
        started_at,
        socket: daemon_socket_path(coven_home)
            .to_string_lossy()
            .into_owned(),
    };
    write_status(coven_home, &status)?;
    let listener = bind_api_socket(coven_home)?;
    loop {
        serve_next_connection(&listener, coven_home, Some(status.clone()))?;
    }
}

#[cfg(unix)]
pub fn serve_next_connection(
    listener: &UnixListener,
    coven_home: &Path,
    status: Option<DaemonStatus>,
) -> Result<()> {
    let (stream, _) = listener
        .accept()
        .context("failed to accept API connection")?;
    let mut reader = BufReader::new(stream);
    let request_line = read_http_request_line(&mut reader)?;
    let mut header = String::new();
    loop {
        header.clear();
        let bytes = reader
            .read_line(&mut header)
            .context("failed to read API request header")?;
        if bytes == 0 || header == "\r\n" || header == "\n" {
            break;
        }
    }
    let mut stream = reader.into_inner();
    let (method, path) = parse_request_line(&request_line)?;
    let response = crate::api::handle_request(method, path, coven_home, status)?;
    let reason = match response.status {
        200 => "OK",
        202 => "Accepted",
        404 => "Not Found",
        _ => "OK",
    };
    let http = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        reason,
        response.content_type,
        response.body.len(),
        response.body
    );
    stream
        .write_all(http.as_bytes())
        .context("failed to write API response")?;
    Ok(())
}

#[cfg(unix)]
fn read_http_request_line<R: BufRead>(reader: &mut R) -> Result<String> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("failed to read API request line")?;
    if line.is_empty() {
        anyhow::bail!("empty API request");
    }
    Ok(line)
}

#[cfg(unix)]
fn parse_request_line(line: &str) -> Result<(&str, &str)> {
    let mut parts = line.split_whitespace();
    let method = parts.next().context("missing HTTP method")?;
    let path = parts.next().context("missing HTTP path")?;
    Ok((method, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_reads_and_clears_daemon_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: temp_dir
                .path()
                .join("coven.sock")
                .to_string_lossy()
                .into_owned(),
        };

        write_status(temp_dir.path(), &status)?;

        assert_eq!(read_status(temp_dir.path())?, Some(status));
        assert!(clear_status(temp_dir.path())?);
        assert_eq!(read_status(temp_dir.path())?, None);
        assert!(!clear_status(temp_dir.path())?);
        Ok(())
    }

    #[test]
    fn builds_background_server_spawn_spec() {
        let spec = background_server_spec(
            Path::new("/usr/local/bin/coven"),
            Path::new("/tmp/coven-home"),
        );

        assert_eq!(spec.program, PathBuf::from("/usr/local/bin/coven"));
        assert_eq!(spec.args, vec!["daemon".to_string(), "serve".to_string()]);
        assert_eq!(spec.coven_home, PathBuf::from("/tmp/coven-home"));
    }

    #[cfg(unix)]
    #[test]
    fn serves_health_over_unix_socket() -> Result<()> {
        use std::io::{Read, Write};
        use std::net::Shutdown;
        use std::os::unix::net::UnixStream;
        use std::thread;

        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        let listener = bind_api_socket(temp_dir.path())?;
        let home = temp_dir.path().to_path_buf();
        let server = thread::spawn(move || serve_next_connection(&listener, &home, Some(status)));

        let mut stream = UnixStream::connect(daemon_socket_path(temp_dir.path()))?;
        stream.write_all(b"GET /health HTTP/1.1\r\nHost: coven\r\n\r\n")?;
        stream.shutdown(Shutdown::Write)?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;

        server.join().expect("server thread panicked")?;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains(r#""ok":true"#));
        assert!(response.contains(r#""pid":12345"#));
        Ok(())
    }
}
