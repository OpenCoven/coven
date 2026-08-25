use std::{
    collections::VecDeque,
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use async_trait::async_trait;
use coven_agents::{
    Agent, AgentId, BoxError, FileLoopJournal, GoalLoopRunner, InMemoryLoopJournal, LoopAttempt,
    LoopCheckpoint, LoopCheckpointStatus, LoopControl, LoopError, LoopEvaluator, LoopJournal,
    LoopOptions, LoopReconciler, LoopReconciliation, Model, ModelRequest, ModelResponse,
    RunOptions, RunResult, Runner,
};

struct QueueModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    calls: AtomicUsize,
}

impl QueueModel {
    fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Model<()> for QueueModel {
    async fn generate(
        &self,
        _request: ModelRequest,
        _context: &(),
    ) -> Result<ModelResponse, BoxError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| Box::new(io::Error::other("no queued response")) as BoxError)
    }
}

struct ExactOutputEvaluator {
    expected: &'static str,
}

#[async_trait]
impl LoopEvaluator<()> for ExactOutputEvaluator {
    async fn evaluate(
        &self,
        _iteration: usize,
        result: &RunResult,
        _context: &(),
    ) -> Result<LoopControl, BoxError> {
        if result.final_output == self.expected {
            Ok(LoopControl::Complete)
        } else {
            Ok(LoopControl::Continue {
                input: format!(
                    "The previous result was `{}`. Revise until it equals `{}`.",
                    result.final_output, self.expected
                ),
            })
        }
    }
}

fn goal_loop(model: Arc<QueueModel>, journal: Arc<dyn LoopJournal>) -> GoalLoopRunner<()> {
    let runner = Runner::new([Agent::new(
        "worker",
        "Worker",
        "Work until the exit criteria are met.",
        model,
    )])
    .unwrap();
    GoalLoopRunner::new(
        runner,
        Arc::new(ExactOutputEvaluator { expected: "done" }),
        journal,
        "test-process",
    )
}

#[tokio::test]
async fn loops_until_the_evaluator_declares_success() {
    let model = Arc::new(QueueModel::new([
        ModelResponse::final_output("not yet"),
        ModelResponse::final_output("done"),
    ]));
    let journal = Arc::new(InMemoryLoopJournal::default());
    let loop_runner = goal_loop(model.clone(), journal.clone());

    let result = loop_runner
        .run(
            "loop-1",
            "worker",
            "Produce the result.",
            &(),
            LoopOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);
    assert_eq!(result.result.unwrap().final_output, "done");
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        journal.checkpoint("loop-1").await.unwrap(),
        Some(LoopCheckpoint {
            loop_id: "loop-1".to_owned(),
            starting_agent: AgentId::from("worker"),
            completed_iterations: 2,
            status: LoopCheckpointStatus::Completed,
            next_input: None,
            active_attempt: None,
            blocked_reason: None,
        })
    );
}

struct StaticReconciler {
    action: LoopReconciliation,
    calls: AtomicUsize,
}

#[async_trait]
impl LoopReconciler<()> for StaticReconciler {
    async fn reconcile(
        &self,
        _checkpoint: &LoopCheckpoint,
        _context: &(),
    ) -> Result<LoopReconciliation, BoxError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.action.clone())
    }
}

#[tokio::test]
async fn file_journal_survives_reconstruction_and_lists_recoverable_work() {
    let directory = tempfile::tempdir().unwrap();
    let checkpoint = LoopCheckpoint {
        loop_id: "durable-loop".to_owned(),
        starting_agent: AgentId::from("worker"),
        completed_iterations: 2,
        status: LoopCheckpointStatus::Pending,
        next_input: Some("Resume after reboot.".to_owned()),
        active_attempt: None,
        blocked_reason: None,
    };
    let journal = FileLoopJournal::new(directory.path()).unwrap();
    assert!(journal
        .compare_and_set("durable-loop", None, &checkpoint)
        .await
        .unwrap());
    drop(journal);

    let reopened = FileLoopJournal::new(directory.path()).unwrap();
    assert_eq!(
        reopened.load("durable-loop").await.unwrap(),
        Some(checkpoint.clone())
    );
    assert_eq!(reopened.list().await.unwrap(), vec![checkpoint]);
}

#[tokio::test]
async fn resumes_pending_work_after_file_journal_reopens() {
    let directory = tempfile::tempdir().unwrap();
    let journal = FileLoopJournal::new(directory.path()).unwrap();
    journal
        .compare_and_set(
            "reboot-loop",
            None,
            &LoopCheckpoint {
                loop_id: "reboot-loop".to_owned(),
                starting_agent: AgentId::from("worker"),
                completed_iterations: 3,
                status: LoopCheckpointStatus::Pending,
                next_input: Some("Continue after restart.".to_owned()),
                active_attempt: None,
                blocked_reason: None,
            },
        )
        .await
        .unwrap();
    drop(journal);

    let model = Arc::new(QueueModel::new([ModelResponse::final_output("done")]));
    let reopened = Arc::new(FileLoopJournal::new(directory.path()).unwrap());
    let runner = goal_loop(model.clone(), reopened.clone());
    let result = runner
        .run(
            "reboot-loop",
            "worker",
            "Fresh input must not replace the checkpoint.",
            &(),
            LoopOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 4);
    assert_eq!(result.result.unwrap().final_output, "done");
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        reopened.load("reboot-loop").await.unwrap().unwrap().status,
        LoopCheckpointStatus::Completed
    );
}

#[tokio::test]
async fn offline_reconciliation_can_complete_without_replaying_the_agent() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("unused")]));
    let journal = Arc::new(InMemoryLoopJournal::default());
    journal
        .compare_and_set(
            "offline-complete",
            None,
            &LoopCheckpoint {
                loop_id: "offline-complete".to_owned(),
                starting_agent: AgentId::from("worker"),
                completed_iterations: 2,
                status: LoopCheckpointStatus::Running,
                next_input: None,
                active_attempt: Some(LoopAttempt {
                    id: "old-process:3:0".to_owned(),
                    process_instance_id: "old-process".to_owned(),
                }),
                blocked_reason: None,
            },
        )
        .await
        .unwrap();
    let reconciler = Arc::new(StaticReconciler {
        action: LoopReconciliation::Complete,
        calls: AtomicUsize::new(0),
    });
    let runner = goal_loop(model.clone(), journal.clone()).with_reconciler(reconciler.clone());

    let result = runner
        .run(
            "offline-complete",
            "worker",
            "Do not replay.",
            &(),
            LoopOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(result.result, None);
    assert_eq!(result.iterations, 3);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert_eq!(reconciler.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        journal
            .checkpoint("offline-complete")
            .await
            .unwrap()
            .unwrap()
            .status,
        LoopCheckpointStatus::Completed
    );
}

#[tokio::test]
async fn offline_reconciliation_can_confirm_an_iteration_and_continue() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("done")]));
    let journal = Arc::new(InMemoryLoopJournal::default());
    journal
        .compare_and_set(
            "offline-continue",
            None,
            &LoopCheckpoint {
                loop_id: "offline-continue".to_owned(),
                starting_agent: AgentId::from("worker"),
                completed_iterations: 1,
                status: LoopCheckpointStatus::Running,
                next_input: None,
                active_attempt: Some(LoopAttempt {
                    id: "old-process:2:0".to_owned(),
                    process_instance_id: "old-process".to_owned(),
                }),
                blocked_reason: None,
            },
        )
        .await
        .unwrap();
    let reconciler = Arc::new(StaticReconciler {
        action: LoopReconciliation::Continue {
            input: "External state confirms iteration two; run iteration three.".to_owned(),
        },
        calls: AtomicUsize::new(0),
    });
    let runner = goal_loop(model.clone(), journal.clone()).with_reconciler(reconciler);

    let result = runner
        .run(
            "offline-continue",
            "worker",
            "Do not use fresh input.",
            &(),
            LoopOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 3);
    assert_eq!(result.result.unwrap().final_output, "done");
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn live_iteration_owner_cannot_reconcile_its_own_claim() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("unused")]));
    let journal = Arc::new(InMemoryLoopJournal::default());
    journal
        .compare_and_set(
            "live-loop",
            None,
            &LoopCheckpoint {
                loop_id: "live-loop".to_owned(),
                starting_agent: AgentId::from("worker"),
                completed_iterations: 1,
                status: LoopCheckpointStatus::Running,
                next_input: None,
                active_attempt: Some(LoopAttempt {
                    id: "test-process:2:0".to_owned(),
                    process_instance_id: "test-process".to_owned(),
                }),
                blocked_reason: None,
            },
        )
        .await
        .unwrap();
    let reconciler = Arc::new(StaticReconciler {
        action: LoopReconciliation::Resume {
            input: "Unsafe duplicate.".to_owned(),
        },
        calls: AtomicUsize::new(0),
    });
    let runner = goal_loop(model.clone(), journal.clone()).with_reconciler(reconciler.clone());

    let error = runner
        .run(
            "live-loop",
            "worker",
            "Do not replay.",
            &(),
            LoopOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        LoopError::ActiveInFlight { ref loop_id } if loop_id == "live-loop"
    ));
    assert_eq!(reconciler.calls.load(Ordering::SeqCst), 0);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        journal
            .checkpoint("live-loop")
            .await
            .unwrap()
            .unwrap()
            .status,
        LoopCheckpointStatus::Running
    );
}

#[tokio::test]
async fn blocked_reconciliation_is_persisted_for_operator_recovery() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("unused")]));
    let journal = Arc::new(InMemoryLoopJournal::default());
    journal
        .compare_and_set(
            "blocked-loop",
            None,
            &LoopCheckpoint {
                loop_id: "blocked-loop".to_owned(),
                starting_agent: AgentId::from("worker"),
                completed_iterations: 1,
                status: LoopCheckpointStatus::Running,
                next_input: None,
                active_attempt: Some(LoopAttempt {
                    id: "old-process:2:0".to_owned(),
                    process_instance_id: "old-process".to_owned(),
                }),
                blocked_reason: None,
            },
        )
        .await
        .unwrap();
    let reconciler = Arc::new(StaticReconciler {
        action: LoopReconciliation::Blocked {
            reason: "remote state is inconclusive".to_owned(),
        },
        calls: AtomicUsize::new(0),
    });
    let runner = goal_loop(model.clone(), journal.clone()).with_reconciler(reconciler);

    let error = runner
        .run(
            "blocked-loop",
            "worker",
            "Do not replay.",
            &(),
            LoopOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        LoopError::ReconciliationBlocked {
            ref loop_id,
            ref reason
        } if loop_id == "blocked-loop" && reason == "remote state is inconclusive"
    ));
    let checkpoint = journal.checkpoint("blocked-loop").await.unwrap().unwrap();
    assert_eq!(checkpoint.status, LoopCheckpointStatus::Blocked);
    assert_eq!(
        checkpoint.blocked_reason.as_deref(),
        Some("remote state is inconclusive")
    );
    assert_eq!(checkpoint.active_attempt, None);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn resumes_from_a_pending_checkpoint_without_replaying_completed_iterations() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("done")]));
    let journal = Arc::new(InMemoryLoopJournal::default());
    journal
        .compare_and_set(
            "loop-2",
            None,
            &LoopCheckpoint {
                loop_id: "loop-2".to_owned(),
                starting_agent: AgentId::from("worker"),
                completed_iterations: 3,
                status: LoopCheckpointStatus::Pending,
                next_input: Some("Continue from the durable boundary.".to_owned()),
                active_attempt: None,
                blocked_reason: None,
            },
        )
        .await
        .unwrap();
    let loop_runner = goal_loop(model.clone(), journal.clone());

    let result = loop_runner
        .run(
            "loop-2",
            "worker",
            "This fresh input must be ignored on resume.",
            &(),
            LoopOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(result.iterations, 4);
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        journal.checkpoint("loop-2").await.unwrap().unwrap().status,
        LoopCheckpointStatus::Completed
    );
}

#[tokio::test]
async fn refuses_to_replay_an_iteration_that_may_have_external_side_effects() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("done")]));
    let journal = Arc::new(InMemoryLoopJournal::default());
    journal
        .compare_and_set(
            "loop-3",
            None,
            &LoopCheckpoint {
                loop_id: "loop-3".to_owned(),
                starting_agent: AgentId::from("worker"),
                completed_iterations: 1,
                status: LoopCheckpointStatus::Running,
                next_input: None,
                active_attempt: Some(LoopAttempt {
                    id: "old-process:2:0".to_owned(),
                    process_instance_id: "old-process".to_owned(),
                }),
                blocked_reason: None,
            },
        )
        .await
        .unwrap();
    let loop_runner = goal_loop(model.clone(), journal);

    let error = loop_runner
        .run(
            "loop-3",
            "worker",
            "Do not replay.",
            &(),
            LoopOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        LoopError::AmbiguousInFlight { ref loop_id } if loop_id == "loop-3"
    ));
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejects_a_pending_checkpoint_without_resume_input() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("done")]));
    let journal = Arc::new(InMemoryLoopJournal::default());
    journal
        .compare_and_set(
            "loop-missing-input",
            None,
            &LoopCheckpoint {
                loop_id: "loop-missing-input".to_owned(),
                starting_agent: AgentId::from("worker"),
                completed_iterations: 1,
                status: LoopCheckpointStatus::Pending,
                next_input: None,
                active_attempt: None,
                blocked_reason: None,
            },
        )
        .await
        .unwrap();
    let loop_runner = goal_loop(model.clone(), journal);

    let error = loop_runner
        .run(
            "loop-missing-input",
            "worker",
            "Do not replace missing durable state.",
            &(),
            LoopOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        LoopError::MissingNextInput { ref loop_id } if loop_id == "loop-missing-input"
    ));
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exhausts_the_loop_at_the_configured_iteration_limit() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("not done")]));
    let journal = Arc::new(InMemoryLoopJournal::default());
    let loop_runner = goal_loop(model, journal.clone());

    let error = loop_runner
        .run(
            "loop-4",
            "worker",
            "Try once.",
            &(),
            LoopOptions {
                max_iterations: 1,
                run: RunOptions::default(),
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        LoopError::MaxIterationsExceeded {
            ref loop_id,
            limit: 1
        } if loop_id == "loop-4"
    ));
    assert_eq!(
        journal.checkpoint("loop-4").await.unwrap().unwrap().status,
        LoopCheckpointStatus::Exhausted
    );
}

#[tokio::test]
async fn rejects_a_checkpoint_owned_by_another_starting_agent() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("done")]));
    let journal = Arc::new(InMemoryLoopJournal::default());
    journal
        .compare_and_set(
            "loop-5",
            None,
            &LoopCheckpoint {
                loop_id: "loop-5".to_owned(),
                starting_agent: AgentId::from("reviewer"),
                completed_iterations: 0,
                status: LoopCheckpointStatus::Pending,
                next_input: Some("Review.".to_owned()),
                active_attempt: None,
                blocked_reason: None,
            },
        )
        .await
        .unwrap();
    let loop_runner = goal_loop(model.clone(), journal);

    let error = loop_runner
        .run(
            "loop-5",
            "worker",
            "Do not cross identities.",
            &(),
            LoopOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        LoopError::StartingAgentMismatch {
            ref loop_id,
            ref expected,
            ref actual
        } if loop_id == "loop-5"
            && expected.as_str() == "worker"
            && actual.as_str() == "reviewer"
    ));
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn journal_compare_and_set_allows_only_one_iteration_claim() {
    let journal = Arc::new(InMemoryLoopJournal::default());
    let pending = LoopCheckpoint {
        loop_id: "loop-6".to_owned(),
        starting_agent: AgentId::from("worker"),
        completed_iterations: 0,
        status: LoopCheckpointStatus::Pending,
        next_input: Some("Run once.".to_owned()),
        active_attempt: None,
        blocked_reason: None,
    };
    assert!(journal
        .compare_and_set("loop-6", None, &pending)
        .await
        .unwrap());
    let running = LoopCheckpoint {
        status: LoopCheckpointStatus::Running,
        next_input: None,
        active_attempt: Some(LoopAttempt {
            id: "test-process:1:0".to_owned(),
            process_instance_id: "test-process".to_owned(),
        }),
        ..pending.clone()
    };

    let left = {
        let journal = journal.clone();
        let pending = pending.clone();
        let running = running.clone();
        tokio::spawn(async move {
            journal
                .compare_and_set("loop-6", Some(&pending), &running)
                .await
                .unwrap()
        })
    };
    let right = {
        let journal = journal.clone();
        let pending = pending.clone();
        let running = running.clone();
        tokio::spawn(async move {
            journal
                .compare_and_set("loop-6", Some(&pending), &running)
                .await
                .unwrap()
        })
    };

    let claims = [left.await.unwrap(), right.await.unwrap()];
    assert_eq!(claims.into_iter().filter(|claimed| *claimed).count(), 1);
}
