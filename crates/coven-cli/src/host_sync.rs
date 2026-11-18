//! The daemon owns one reconciler for its profile. Local IPC operations never
//! transport peer bytes, and waiting for a remote transfer never holds the IPC socket.
use crate::api_response::{api_error, json_response, ApiResponse};
use anyhow::{ensure, Context, Result};
use coven_client::host::{HostConnect, HostInit, HostResolution, HostResolve, HostRun};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    path::{Component, Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use uuid::Uuid;

const POLL: Duration = Duration::from_secs(2);
const WATCH_LEASE: Duration = Duration::from_secs(8);
const MAX_COMPLETED: usize = 32;
type Outcome = std::result::Result<Value, String>;

pub struct Worker {
    home: PathBuf,
    shared: Arc<Shared>,
}

struct Shared {
    state_dir: PathBuf,
    jobs: Mutex<Jobs>,
    wake: Condvar,
}

#[derive(Default)]
struct Jobs {
    running: bool,
    stopping: bool,
    current: Option<Uuid>,
    completed: VecDeque<(Uuid, Outcome)>,
    watching_until: Option<Instant>,
    next_poll: Option<Instant>,
}

fn state_dir(home: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os("COVEN_HOST_STATE").filter(|s| !s.is_empty()) {
        return PathBuf::from(dir);
    }
    let state = home.join("host");
    #[cfg(target_os = "linux")]
    if !state.join("client.sqlite").exists() {
        let default_owner = dirs_next::home_dir().filter(|owner| {
            owner
                .join(crate::paths::DEFAULT_COVEN_HOME_DIR)
                .canonicalize()
                .ok()
                .as_deref()
                == Some(home)
        });
        if let Some(legacy) =
            default_owner.map(|owner| owner.join(".local/state/coven-host/client"))
        {
            if legacy.join("client.sqlite").is_file() {
                return legacy;
            }
        }
    }
    state
}

impl Worker {
    pub fn new(home: &Path) -> Self {
        Self::with_state(home, state_dir(home))
    }

    fn with_state(home: &Path, state_dir: PathBuf) -> Self {
        Self {
            home: home.to_owned(),
            shared: Arc::new(Shared {
                state_dir,
                jobs: Mutex::default(),
                wake: Condvar::new(),
            }),
        }
    }

    fn jobs(&self) -> MutexGuard<'_, Jobs> {
        self.shared
            .jobs
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    fn client(&self) -> Result<coven_host::Client> {
        ensure!(
            self.shared.state_dir.is_absolute(),
            "daemon host state must be an absolute path"
        );
        Ok(coven_host::Client::new(&self.shared.state_dir))
    }

    fn schedule(&self, watch: bool) -> Result<Value> {
        self.client()?;
        let now = Instant::now();
        let mut jobs = self.jobs();
        ensure!(!jobs.stopping, "daemon is stopping");
        if watch {
            if jobs.watching_until.is_none_or(|until| until <= now) {
                jobs.next_poll = Some(now);
            }
            jobs.watching_until = Some(now + WATCH_LEASE);
        }
        let id = if watch {
            None
        } else {
            Some(*jobs.current.get_or_insert_with(Uuid::new_v4))
        };
        if !jobs.running {
            let shared = Arc::clone(&self.shared);
            if let Err(err) = std::thread::Builder::new()
                .name("coven-host-sync".into())
                .spawn(move || reconcile(&shared))
            {
                jobs.current = None;
                jobs.watching_until = None;
                return Err(err.into());
            }
            jobs.running = true;
        }
        self.shared.wake.notify_one();
        if let Some(id) = id {
            Ok(json!(HostRun { id: id.to_string() }))
        } else {
            Ok(match jobs.completed.back().map(|(_, value)| value) {
                Some(Ok(value)) => value.clone(),
                Some(Err(error)) => json!({"phase": "needs attention", "error": error}),
                None => json!({"phase": "syncing"}),
            })
        }
    }

    pub(crate) fn stop(&self) {
        self.jobs().stopping = true;
        self.shared.wake.notify_all();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn reconcile(shared: &Shared) {
    let mut backoff = POLL;
    let mut jobs = shared.jobs.lock().unwrap_or_else(|err| err.into_inner());
    loop {
        let now = Instant::now();
        if jobs.stopping {
            jobs.running = false;
            return;
        }
        let id = match jobs.current {
            Some(id) => id,
            None if jobs.watching_until.is_some_and(|until| until > now) => {
                let wait = jobs
                    .next_poll
                    .unwrap_or(now)
                    .min(jobs.watching_until.unwrap_or(now));
                if wait > now {
                    (jobs, _) = shared
                        .wake
                        .wait_timeout(jobs, wait - now)
                        .unwrap_or_else(|err| err.into_inner());
                    continue;
                }
                *jobs.current.get_or_insert_with(Uuid::new_v4)
            }
            None => {
                jobs.running = false;
                return;
            }
        };
        drop(jobs);
        let outcome =
            std::panic::catch_unwind(|| coven_host::Client::new(&shared.state_dir).sync())
                .map_err(|_| "host reconciler panicked; inspect status and retry".to_owned())
                .and_then(|result| result.map_err(|err| format!("{err:#}")));
        let offline = outcome
            .as_ref()
            .is_ok_and(|value| value["phase"] == "offline");
        jobs = shared.jobs.lock().unwrap_or_else(|err| err.into_inner());
        jobs.current = None;
        if jobs.completed.len() == MAX_COMPLETED {
            jobs.completed.pop_front();
        }
        jobs.completed.push_back((id, outcome));
        jobs.next_poll = Some(Instant::now() + if offline { backoff } else { POLL });
        backoff = if offline {
            (backoff * 2).min(Duration::from_secs(30))
        } else {
            POLL
        };
    }
}

fn decode<T: DeserializeOwned>(body: Option<&str>) -> Result<T> {
    Ok(serde_json::from_str(body.unwrap_or("{}"))?)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

fn absolute(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && !path.components().any(|p| p == Component::ParentDir),
        "expected an absolute path without parent traversal"
    );
    Ok(())
}

fn physical_path(path: &Path) -> Result<PathBuf> {
    absolute(path)?;
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().context("path has no existing parent")?;
            Ok(physical_path(parent)?.join(path.file_name().context("invalid file name")?))
        }
        Err(err) => Err(err.into()),
    }
}

fn dispatch(worker: &Worker, method: &str, route: &str, body: Option<&str>) -> Result<Value> {
    let client = worker.client()?;
    match (method, route) {
        ("POST", "/host/init") => {
            let input: HostInit = decode(body)?;
            let root = physical_path(&input.root)?;
            let home = physical_path(&worker.home)?;
            ensure!(
                !root.starts_with(&home) && !home.starts_with(&root),
                "keep the daemon home and Castle folder separate"
            );
            client.initialize(&input.root)?;
        }
        ("POST", "/host/connect") => {
            let input: HostConnect = decode(body)?;
            absolute(&input.token_file)?;
            client.connect(&input.url, &input.token_file)?;
        }
        ("POST", "/host/resolve") => {
            let input: HostResolve = decode(body)?;
            match input.resolution {
                HostResolution::Local => client.resolve(&input.path, "local", None)?,
                HostResolution::Remote => client.resolve(&input.path, "remote", None)?,
                HostResolution::Merged(file) => {
                    absolute(&file)?;
                    client.resolve(&input.path, "merged", Some(&file))?;
                }
            }
        }
        ("POST", "/host/sync" | "/host/watch") => {
            decode::<Empty>(body)?;
            return worker.schedule(route == "/host/watch");
        }
        ("GET", "/host/status") => {
            decode::<Empty>(body)?;
            return client.status();
        }
        _ => anyhow::bail!("unrecognized host operation"),
    }
    Ok(json!({"ok": true}))
}

pub(crate) fn route(
    worker: &Worker,
    method: &str,
    route: &str,
    body: Option<&str>,
) -> Result<ApiResponse> {
    let expected = match route {
        "/host/status" => "GET",
        "/host/init" | "/host/connect" | "/host/resolve" | "/host/sync" | "/host/watch"
        | "/host/result" => "POST",
        _ => return api_error(404, "not_found", "Unknown host operation.", None),
    };
    if method != expected {
        return api_error(
            405,
            "method_not_allowed",
            "Unsupported method for this host operation.",
            None,
        );
    }
    if route == "/host/result" {
        let Ok(id) = decode::<HostRun>(body).and_then(|run| Ok(Uuid::parse_str(&run.id)?)) else {
            return api_error(400, "invalid_request", "Expected a host run id.", None);
        };
        let jobs = worker.jobs();
        if jobs.current == Some(id) {
            return json_response(202, &Value::Null);
        }
        return match jobs.completed.iter().find(|(previous, _)| *previous == id) {
            Some((_, Ok(outcome))) => json_response(200, outcome),
            Some((_, Err(error))) => api_error(409, "host_sync_failed", error, None),
            None => api_error(404, "not_found", "Unknown or expired host run.", None),
        };
    }
    match dispatch(worker, method, route, body) {
        Ok(value) => json_response(if route == "/host/sync" { 202 } else { 200 }, &value),
        Err(error) if error.is::<serde_json::Error>() => {
            api_error(400, "invalid_request", &error.to_string(), None)
        }
        Err(error) => api_error(409, "host_sync_failed", &format!("{error:#}"), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_refuses_the_whole_host_route_family_before_opening_state() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("missing-profile");
        for (method, path) in [
            ("POST", "/api/v1/host/init"),
            ("POST", "/api/v1/host/connect"),
            ("POST", "/api/v1/host/sync"),
            ("POST", "/api/v1/host/result"),
            ("GET", "/api/v1/host/status"),
            ("POST", "/api/v1/host/watch"),
            ("POST", "/api/v1/host/resolve"),
        ] {
            let response = crate::api::handle_request_with_runtime_and_authority(
                method,
                path,
                &home,
                None,
                Some("{}"),
                &crate::api::NoopSessionRuntime,
                crate::api::RequestAuthority::Tcp,
            )
            .unwrap();
            assert_eq!(response.status, 403);
            assert!(response.body.contains("owner_local_required"));
        }
        assert!(!home.exists());
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_sync_tickets_share_the_inflight_run_and_return_its_result() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            os::unix::fs::PermissionsExt,
            sync::mpsc,
        };
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let home = base.join("daemon");
        std::fs::create_dir(&home).unwrap();
        let worker = Worker::with_state(&home, home.join("host"));
        let client = worker.client().unwrap();
        let root = base.join("folder");
        client.initialize(&root).unwrap();
        let token = base.join("access");
        std::fs::write(
            &token,
            format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
        )
        .unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        client
            .connect(
                &format!("http://{}", listener.local_addr().unwrap()),
                &token,
            )
            .unwrap();
        let (started, at_peer) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut first = [0u8; 3];
            stream.read_exact(&mut first).unwrap();
            assert_eq!(&first, b"GET");
            started.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(15)).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let first = worker.schedule(false).unwrap();
        at_peer.recv_timeout(Duration::from_secs(15)).unwrap();
        assert_eq!(worker.schedule(false).unwrap(), first);
        let body = first.to_string();
        assert_eq!(
            route(&worker, "POST", "/host/result", Some(&body))
                .unwrap()
                .status,
            202
        );
        release.send(()).unwrap();
        peer.join().unwrap();
        let start = Instant::now();
        loop {
            let result = route(&worker, "POST", "/host/result", Some(&body)).unwrap();
            if result.status != 202 {
                assert_eq!(result.status, 200, "{}", result.body);
                let value: Value = serde_json::from_str(&result.body).unwrap();
                assert_ne!(value["phase"], "synced");
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "run did not finish"
            );
            std::thread::yield_now();
        }
    }
}
