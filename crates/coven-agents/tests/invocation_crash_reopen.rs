//! Process termination at journal transaction boundaries. These tests prove
//! reopen behavior, not power-loss durability or authoritative fencing.
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use coven_agents::{
    Agent, AgentRef, BoxError, DurableInvocation, DurableInvocationError, DurableInvocationRunner,
    FileInvocationJournal, InvocationContext, InvocationId, InvocationJournal, InvocationRecord,
    InvocationRequest, InvocationSource, InvocationState, Model, ModelRequest, ModelResponse,
    RunControls, RunOptions, Runner,
};

const CRASH_EXIT: i32 = 86;

struct RecordedModel(PathBuf);

#[async_trait]
impl Model<()> for RecordedModel {
    async fn generate(&self, _: ModelRequest, _: &()) -> Result<ModelResponse, BoxError> {
        let mut evidence = OpenOptions::new().create(true).append(true).open(&self.0)?;
        evidence.write_all(b"model called\n")?;
        evidence.sync_all()?;
        Ok(ModelResponse::final_output("done"))
    }
}

struct CrashJournal {
    inner: FileInvocationJournal,
    transition: String,
    after: bool,
}

#[async_trait]
impl InvocationJournal for CrashJournal {
    async fn load(&self, id: InvocationId) -> Result<Option<InvocationRecord>, BoxError> {
        self.inner.load(id).await
    }

    async fn list(&self) -> Result<Vec<InvocationRecord>, BoxError> {
        self.inner.list().await
    }

    async fn compare_and_set(
        &self,
        id: InvocationId,
        expected: Option<&InvocationRecord>,
        next: &InvocationRecord,
    ) -> Result<bool, BoxError> {
        let transition = match next.state {
            InvocationState::Reserved {} => "reserve",
            InvocationState::Running {} => "adopt",
            _ => "settle",
        };
        let selected = transition == self.transition;
        if selected && !self.after {
            std::process::exit(CRASH_EXIT);
        }
        let result = self.inner.compare_and_set(id, expected, next).await?;
        if selected && self.after {
            assert!(result, "crash boundary must follow a committed write");
            std::process::exit(CRASH_EXIT);
        }
        Ok(result)
    }
}

fn owner(
    root: &std::path::Path,
    journal: Arc<dyn InvocationJournal>,
    process: &str,
) -> DurableInvocationRunner<()> {
    let model = Arc::new(RecordedModel(root.join("effects")));
    let runner = Arc::new(
        Runner::new([
            Agent::new("worker", "Worker", "work", model).with_revision("rev-1".parse().unwrap())
        ])
        .unwrap(),
    );
    DurableInvocationRunner::new(runner, journal, process)
}

async fn submit(
    owner: &DurableInvocationRunner<()>,
    request: InvocationRequest,
) -> Result<DurableInvocation, DurableInvocationError> {
    owner
        .run(
            request,
            "bounded task",
            &(),
            RunOptions::default(),
            RunControls::default(),
        )
        .await
}

#[tokio::test]
async fn crash_process_entry() {
    let Some(root) = std::env::var_os("COVEN_INVOCATION_CRASH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let request = serde_json::from_slice(&fs::read(root.join("request.json")).unwrap()).unwrap();
    let journal = Arc::new(CrashJournal {
        inner: FileInvocationJournal::new(root.join("journal")).unwrap(),
        transition: std::env::var("COVEN_INVOCATION_CRASH_TRANSITION").unwrap(),
        after: std::env::var("COVEN_INVOCATION_CRASH_SIDE").unwrap() == "after",
    });
    submit(&owner(&root, journal, "crashed-process"), request)
        .await
        .unwrap();
    panic!("selected crash boundary was not reached");
}

#[tokio::test]
async fn process_crashes_preserve_each_committed_transition_without_duplicate_effects() {
    for transition in ["reserve", "adopt", "settle"] {
        for after in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let request = InvocationRequest::new(
                InvocationContext::root(InvocationId::new()),
                InvocationSource::Caller,
                AgentRef::with_revision("worker", "rev-1").unwrap(),
            );
            fs::write(
                root.join("request.json"),
                serde_json::to_vec(&request).unwrap(),
            )
            .unwrap();
            let output_path = root.join("child-output");
            let output = fs::File::create(&output_path).unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "crash_process_entry", "--nocapture"])
                .env("COVEN_INVOCATION_CRASH_ROOT", root)
                .env("COVEN_INVOCATION_CRASH_TRANSITION", transition)
                .env(
                    "COVEN_INVOCATION_CRASH_SIDE",
                    if after { "after" } else { "before" },
                )
                .stdout(Stdio::from(output.try_clone().unwrap()))
                .stderr(Stdio::from(output))
                .spawn()
                .unwrap();
            let started = Instant::now();
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                // A hang guard, far above the six local journal operations;
                // no promptness or production deadline is being asserted.
                if started.elapsed() > Duration::from_secs(60) {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "{transition}/{after}: child wedged after {:?}: {}",
                        started.elapsed(),
                        fs::read_to_string(&output_path).unwrap()
                    );
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(
                status.code(),
                Some(CRASH_EXIT),
                "{transition}/{after}: {}",
                fs::read_to_string(&output_path).unwrap()
            );

            let journal = Arc::new(FileInvocationJournal::new(root.join("journal")).unwrap());
            let record = journal.load(request.invocation.id).await.unwrap();
            match (transition, after) {
                ("reserve", false) => assert!(record.is_none()),
                ("reserve", true) | ("adopt", false) => assert!(matches!(
                    record.unwrap().state,
                    InvocationState::Reserved {}
                )),
                ("adopt", true) | ("settle", false) => {
                    assert!(matches!(record.unwrap().state, InvocationState::Running {}))
                }
                ("settle", true) => assert!(matches!(
                    record.unwrap().state,
                    InvocationState::Completed { .. }
                )),
                _ => unreachable!(),
            }
            let recovery = owner(root, journal, "reopened-process");
            let uncertain = (transition == "adopt" && after) || (transition == "settle" && !after);
            for duplicate in 0..2 {
                let result = submit(&recovery, request.clone()).await;
                if uncertain {
                    assert!(
                        matches!(result, Err(DurableInvocationError::RecoveryRequired { .. })),
                        "{transition}/{after}: {result:?}"
                    );
                } else if transition == "settle" || duplicate > 0 {
                    assert!(
                        matches!(result, Ok(DurableInvocation::Replayed { .. })),
                        "{transition}/{after}: {result:?}"
                    );
                } else {
                    assert!(
                        matches!(result, Ok(DurableInvocation::Dispatched { ref outcome, .. }) if outcome.is_ok()),
                        "{transition}/{after}: {result:?}"
                    );
                }
            }
            let effects = match fs::read_to_string(root.join("effects")) {
                Ok(effects) => effects,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => panic!("cannot read effect evidence: {error}"),
            };
            let expected_calls = usize::from(!(transition == "adopt" && after));
            assert_eq!(
                effects.lines().count(),
                expected_calls,
                "{transition}/{after}: duplicate execution"
            );
        }
    }
}
