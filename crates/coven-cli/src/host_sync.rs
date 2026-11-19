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
const MAX_QUEUED: usize = 32;
type Outcome = std::result::Result<Value, String>;

pub struct Worker {
    shared: Arc<Shared>,
}

struct Shared {
    home: PathBuf,
    state_dir: PathBuf,
    jobs: Mutex<Jobs>,
    wake: Condvar,
}

#[derive(Default)]
struct Jobs {
    running: bool,
    stopping: bool,
    configuring: bool,
    current: Option<Running>,
    pending: VecDeque<(Uuid, Task)>,
    last_sync: Option<Value>,
    completed: VecDeque<(Uuid, Outcome)>,
    watching_until: Option<Instant>,
    next_poll: Option<Instant>,
}

struct Configuration<'a>(&'a Worker);

impl Drop for Configuration<'_> {
    fn drop(&mut self) {
        let mut jobs = self.0.jobs();
        jobs.configuring = false;
        self.0.shared.wake.notify_one();
    }
}

#[derive(Copy, Clone)]
struct Running {
    id: Uuid,
    sync: bool,
}

enum Task {
    Sync,
    Resolve(HostResolve),
}

impl Task {
    fn perform(self, client: coven_host::Client) -> Result<Value> {
        let Self::Resolve(input) = self else {
            return client.sync();
        };
        match input.resolution {
            HostResolution::Local => client.resolve(&input.path, "local", None)?,
            HostResolution::Remote => client.resolve(&input.path, "remote", None)?,
            HostResolution::Merged(file) => client.resolve(&input.path, "merged", Some(&file))?,
        }
        Ok(json!({"ok": true}))
    }
}

impl Jobs {
    fn enqueue(&mut self, task: Task) -> Result<Uuid> {
        if matches!(task, Task::Sync) {
            // Sharing a sync across a queued resolution could acknowledge state
            // from before the caller's edit was applied.
            let share = match self.pending.back() {
                Some((id, Task::Sync)) => Some(*id),
                Some(_) => None,
                None => self.current.filter(|run| run.sync).map(|run| run.id),
            };
            if let Some(id) = share {
                return Ok(id);
            }
        }
        ensure!(
            self.pending.len() < MAX_QUEUED,
            "host operation queue is full; retry later"
        );
        let id = Uuid::new_v4();
        self.pending.push_back((id, task));
        Ok(id)
    }
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
            shared: Arc::new(Shared {
                home: home.to_owned(),
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
        Ok(coven_host::Client::for_profile(
            &self.shared.state_dir,
            &self.shared.home,
        ))
    }

    fn configure(&self, apply: impl FnOnce() -> Result<()>) -> Result<()> {
        let _configuration = {
            let mut jobs = self.jobs();
            ensure!(
                !jobs.stopping
                    && !jobs.configuring
                    && jobs.current.is_none()
                    && jobs.pending.is_empty(),
                "host operation in progress; retry afterward"
            );
            jobs.configuring = true;
            Configuration(self)
        };
        // Filesystem and database operations can block. Keep the queue mutex
        // available for ticket polling and the daemon's bounded shutdown.
        apply()?;
        let mut jobs = self.jobs();
        jobs.last_sync = None;
        jobs.next_poll = Some(Instant::now());
        Ok(())
    }

    fn schedule(&self, task: Option<Task>) -> Result<Value> {
        self.client()?;
        let now = Instant::now();
        let mut jobs = self.jobs();
        ensure!(!jobs.stopping, "daemon is stopping");
        let id = if let Some(task) = task {
            Some(jobs.enqueue(task)?)
        } else {
            if jobs.watching_until.is_none_or(|until| until <= now) {
                jobs.next_poll = Some(now);
            }
            jobs.watching_until = Some(now + WATCH_LEASE);
            None
        };
        if !jobs.running {
            let shared = Arc::clone(&self.shared);
            if let Err(err) = std::thread::Builder::new()
                .name("coven-host-sync".into())
                .spawn(move || reconcile(&shared))
            {
                jobs.pending.clear();
                jobs.watching_until = None;
                return Err(err.into());
            }
            jobs.running = true;
        }
        self.shared.wake.notify_one();
        Ok(match id {
            Some(id) => json!(HostRun { id: id.to_string() }),
            None => jobs
                .last_sync
                .clone()
                .unwrap_or_else(|| json!({"phase": "syncing"})),
        })
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
        if jobs.configuring {
            jobs = shared
                .wake
                .wait(jobs)
                .unwrap_or_else(|err| err.into_inner());
            continue;
        }
        let (id, task) = match jobs.pending.pop_front() {
            Some(job) => job,
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
                (Uuid::new_v4(), Task::Sync)
            }
            None => {
                jobs.running = false;
                return;
            }
        };
        let is_sync = matches!(task, Task::Sync);
        jobs.current = Some(Running { id, sync: is_sync });
        drop(jobs);
        let outcome = std::panic::catch_unwind(|| {
            task.perform(coven_host::Client::for_profile(
                &shared.state_dir,
                &shared.home,
            ))
        })
        .map_err(|_| "host worker panicked; inspect status before retrying".to_owned())
        .and_then(|result| result.map_err(|err| format!("{err:#}")));
        jobs = shared.jobs.lock().unwrap_or_else(|err| err.into_inner());
        jobs.current = None;
        if is_sync {
            let offline = outcome
                .as_ref()
                .is_ok_and(|value| value["phase"] == "offline");
            jobs.last_sync = Some(match &outcome {
                Ok(value) => value.clone(),
                Err(error) => json!({"phase":"needs attention","error":error}),
            });
            jobs.next_poll = Some(Instant::now() + if offline { backoff } else { POLL });
            backoff = if offline {
                (backoff * 2).min(Duration::from_secs(30))
            } else {
                POLL
            };
        }
        if !is_sync && outcome.is_ok() {
            jobs.last_sync = None;
            jobs.next_poll = Some(Instant::now());
        }
        if jobs.completed.len() == MAX_COMPLETED {
            jobs.completed.pop_front();
        }
        jobs.completed.push_back((id, outcome));
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
            let home = physical_path(&worker.shared.home)?;
            ensure!(
                !root.starts_with(&home) && !home.starts_with(&root),
                "keep the daemon home and Castle folder separate"
            );
            worker.configure(|| client.initialize(&input.root))?;
        }
        ("POST", "/host/connect") => {
            let input: HostConnect = decode(body)?;
            absolute(&input.token_file)?;
            worker.configure(|| client.connect(&input.url, &input.token_file))?;
        }
        ("POST", "/host/resolve") => {
            let input: HostResolve = decode(body)?;
            if let HostResolution::Merged(file) = &input.resolution {
                absolute(file)?;
            }
            coven_host::validate_path(&input.path)?;
            return worker.schedule(Some(Task::Resolve(input)));
        }
        ("POST", "/host/sync" | "/host/watch") => {
            decode::<Empty>(body)?;
            return worker.schedule((route == "/host/sync").then_some(Task::Sync));
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
        if jobs.current.is_some_and(|run| run.id == id)
            || jobs.pending.iter().any(|(pending, _)| *pending == id)
        {
            return json_response(202, &Value::Null);
        }
        return match jobs.completed.iter().find(|(previous, _)| *previous == id) {
            Some((_, Ok(outcome))) => json_response(200, outcome),
            Some((_, Err(error))) => api_error(409, "host_sync_failed", error, None),
            None => api_error(404, "not_found", "Unknown or expired host run.", None),
        };
    }
    match dispatch(worker, method, route, body) {
        Ok(value) => json_response(
            if matches!(route, "/host/sync" | "/host/resolve") {
                202
            } else {
                200
            },
            &value,
        ),
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
    fn castle_ownership_survives_daemon_restart() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("castle");
        let worker = |name: &str| {
            let home = base.join(name);
            Worker::with_state(&home, home.join("host"))
        };
        let init = |worker: &Worker| {
            let body = json!({"root": root}).to_string();
            dispatch(worker, "POST", "/host/init", Some(&body))
        };
        let owner = worker("owner");
        let competitor = worker("competitor");
        init(&owner).unwrap();
        assert!(
            init(&competitor).is_err(),
            "second profile claimed live root"
        );
        drop(owner);
        assert!(
            init(&competitor).is_err(),
            "second profile claimed stopped root"
        );
        init(&worker("owner")).unwrap();
    }

    #[test]
    fn shutdown_stays_available_while_host_configuration_is_blocked() {
        use std::sync::mpsc;

        let temp = tempfile::tempdir().unwrap();
        let worker = Arc::new(Worker::new(temp.path()));
        let (entered, configuring) = mpsc::channel();
        let (resume, blocked) = mpsc::channel();
        let config_worker = Arc::clone(&worker);
        let config = std::thread::spawn(move || {
            config_worker.configure(|| {
                entered.send(()).unwrap();
                blocked.recv().unwrap();
                Ok(())
            })
        });
        configuring.recv().unwrap();

        let (shutdown, stopped) = mpsc::channel();
        let stop = std::thread::spawn(move || {
            worker.stop();
            shutdown.send(()).unwrap();
        });
        // The daemon has a two-second shutdown budget even when file I/O stalls.
        let completed = stopped.recv_timeout(Duration::from_secs(2));
        resume.send(()).unwrap();
        config.join().unwrap().unwrap();
        stop.join().unwrap();
        assert!(
            completed.is_ok(),
            "host configuration blocked daemon shutdown"
        );
    }

    #[test]
    fn resolutions_are_distinct_and_fence_sync_coalescing() {
        let active = Uuid::new_v4();
        let mut jobs = Jobs {
            current: Some(Running {
                id: active,
                sync: true,
            }),
            ..Jobs::default()
        };
        assert_eq!(jobs.enqueue(Task::Sync).unwrap(), active);
        let first = jobs
            .enqueue(Task::Resolve(HostResolve {
                path: "files/note".into(),
                resolution: HostResolution::Local,
            }))
            .unwrap();
        let second = jobs
            .enqueue(Task::Resolve(HostResolve {
                path: "files/note".into(),
                resolution: HostResolution::Remote,
            }))
            .unwrap();
        assert_ne!(first, second);
        let final_sync = jobs.enqueue(Task::Sync).unwrap();
        assert_ne!(final_sync, active);
        assert_eq!(final_sync, jobs.enqueue(Task::Sync).unwrap());
        assert_eq!(
            jobs.pending.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [first, second, final_sync]
        );
    }

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
        let first = worker.schedule(Some(Task::Sync)).unwrap();
        at_peer.recv_timeout(Duration::from_secs(15)).unwrap();
        assert_eq!(worker.schedule(Some(Task::Sync)).unwrap(), first);
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
