use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::io::{Read, Write};
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

pub fn daemon_status_path(coven_home: &Path) -> PathBuf {
    coven_home.join("daemon.json")
}

pub fn daemon_socket_path(coven_home: &Path) -> PathBuf {
    coven_home.join("coven.sock")
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
pub fn serve_next_connection(
    listener: UnixListener,
    coven_home: &Path,
    status: Option<DaemonStatus>,
) -> Result<()> {
    let (mut stream, _) = listener
        .accept()
        .context("failed to accept API connection")?;
    let mut request = String::new();
    stream
        .read_to_string(&mut request)
        .context("failed to read API request")?;
    let (method, path) = parse_request_line(&request)?;
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
fn parse_request_line(request: &str) -> Result<(&str, &str)> {
    let line = request.lines().next().context("empty API request")?;
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
        let server = thread::spawn(move || serve_next_connection(listener, &home, Some(status)));

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
