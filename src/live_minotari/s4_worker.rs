use super::*;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

const WORKER_SCHEMA_VERSION: u32 = 1;
const WORKER_PASSWORD_ENV: &str = "WALLET_BENCH_MODE2_S4_WORKER_PASSWORD";

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct WorkerInput {
    schema_version: u32,
    db_path: PathBuf,
    base_node_url: String,
    recipients: Vec<String>,
    amount_microtari: u64,
    fee_rate_microtari: u64,
    seconds_to_lock: u64,
    confirmation_window: u64,
    request_timeout_ms: u64,
    tip_height_at_broadcast: Option<u64>,
    arm_started_at_unix_ms: u128,
    deadline_at_unix_ms: u128,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct WireIdentity {
    tx_id: String,
    fee_microtari: u64,
    construction_ms: u128,
    broadcast_start_offset_ms: u128,
}

impl From<&OneSidedSubmissionIdentity> for WireIdentity {
    fn from(identity: &OneSidedSubmissionIdentity) -> Self {
        Self {
            tx_id: identity.tx_id.clone(),
            fee_microtari: identity.fee_microtari,
            construction_ms: identity.construction_ms,
            broadcast_start_offset_ms: identity.broadcast_start_offset_ms,
        }
    }
}

impl From<WireIdentity> for OneSidedSubmissionIdentity {
    fn from(identity: WireIdentity) -> Self {
        Self {
            tx_id: identity.tx_id,
            fee_microtari: identity.fee_microtari,
            construction_ms: identity.construction_ms,
            broadcast_start_offset_ms: identity.broadcast_start_offset_ms,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct WireOutcome {
    tx_id: String,
    fee_microtari: u64,
    accepted: bool,
    is_synced: bool,
    rejection_reason: Option<String>,
    construction_ms: u128,
    broadcast_to_mempool_ms: Option<u128>,
    broadcast_start_offset_ms: Option<u128>,
    recipient: String,
}

impl From<OneSidedSendOutcome> for WireOutcome {
    fn from(outcome: OneSidedSendOutcome) -> Self {
        Self {
            tx_id: outcome.tx_id,
            fee_microtari: outcome.fee_microtari,
            accepted: outcome.accepted,
            is_synced: outcome.is_synced,
            rejection_reason: outcome.rejection_reason,
            construction_ms: outcome.construction_ms,
            broadcast_to_mempool_ms: outcome.broadcast_to_mempool_ms,
            broadcast_start_offset_ms: outcome.broadcast_start_offset_ms,
            recipient: outcome.recipient,
        }
    }
}

impl From<WireOutcome> for OneSidedSendOutcome {
    fn from(outcome: WireOutcome) -> Self {
        Self {
            tx_id: outcome.tx_id,
            fee_microtari: outcome.fee_microtari,
            accepted: outcome.accepted,
            is_synced: outcome.is_synced,
            rejection_reason: outcome.rejection_reason,
            construction_ms: outcome.construction_ms,
            broadcast_to_mempool_ms: outcome.broadcast_to_mempool_ms,
            broadcast_start_offset_ms: outcome.broadcast_start_offset_ms,
            broadcasted_at: None,
            recipient: outcome.recipient,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct WireFailure {
    identity: Option<WireIdentity>,
    submission_started: bool,
    submission_elapsed_ms: Option<u128>,
    failure_class: ObservationFailureClass,
    message: String,
}

impl WireFailure {
    fn from_error(error: &anyhow::Error) -> Self {
        if let Some(failure) = error.downcast_ref::<OneSidedSubmissionFailure>() {
            return Self {
                identity: failure.identity.as_ref().map(WireIdentity::from),
                submission_started: failure.submission_started,
                submission_elapsed_ms: failure.submission_elapsed_ms,
                failure_class: failure.failure_class,
                message: failure.message.clone(),
            };
        }
        Self {
            identity: None,
            submission_started: false,
            submission_elapsed_ms: None,
            failure_class: ObservationFailureClass::Wallet,
            message: format!("{error:#}"),
        }
    }

    fn into_error(self) -> anyhow::Error {
        anyhow::Error::new(OneSidedSubmissionFailure {
            identity: self.identity.map(Into::into),
            submission_started: self.submission_started,
            submission_elapsed_ms: self.submission_elapsed_ms,
            failure_class: self.failure_class,
            message: self.message,
        })
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum WorkerEventKind {
    Started,
    Prepared {
        identity: WireIdentity,
    },
    SubmissionStarted {
        identity: WireIdentity,
    },
    Completed {
        construction_complete_offset_ms: Option<u128>,
        outcome: Option<Box<WireOutcome>>,
        failure: Option<Box<WireFailure>>,
    },
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct WorkerEvent {
    schema_version: u32,
    attempt: u32,
    recipient: String,
    event_offset_ms: u128,
    #[serde(flatten)]
    kind: WorkerEventKind,
}

#[derive(Clone)]
struct EventJournal {
    file: Arc<Mutex<File>>,
}

impl EventJournal {
    fn create(path: &Path) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
            .with_context(|| format!("creating Mode 2 S4 event journal {}", path.display()))?;
        restrict_owner_only(path, false)?;
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
        })
    }

    fn append(&self, event: &WorkerEvent) -> anyhow::Result<()> {
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');
        let mut file = self
            .file
            .lock()
            .map_err(|_| anyhow::anyhow!("Mode 2 S4 event journal lock poisoned"))?;
        file.write_all(&line)?;
        file.flush()?;
        Ok(())
    }
}

#[derive(Default)]
struct AttemptReplay {
    submit_offset_ms: Option<u128>,
    construction_complete_offset_ms: Option<u128>,
    identity: Option<WireIdentity>,
    submission_started_offset_ms: Option<u128>,
    completed: Option<Result<WireOutcome, WireFailure>>,
}

pub(super) async fn run_worker(request_path: &Path, events_path: &Path) -> anyhow::Result<()> {
    let input: WorkerInput =
        serde_json::from_slice(&fs::read(request_path).with_context(|| {
            format!("reading Mode 2 S4 worker input {}", request_path.display())
        })?)
        .with_context(|| format!("decoding Mode 2 S4 worker input {}", request_path.display()))?;
    if input.schema_version != WORKER_SCHEMA_VERSION
        || input.recipients.is_empty()
        || input.deadline_at_unix_ms <= input.arm_started_at_unix_ms
    {
        bail!("invalid Mode 2 S4 worker input");
    }
    let password = std::env::var(WORKER_PASSWORD_ENV)
        .context("Mode 2 S4 worker password environment variable is missing")?;
    let journal = EventJournal::create(events_path)?;
    let deadline = instant_from_unix_deadline(input.deadline_at_unix_ms);
    let mut tasks = JoinSet::new();

    for (index, recipient) in input.recipients.into_iter().enumerate() {
        let attempt = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let submit_offset_ms = arm_offset_ms(input.arm_started_at_unix_ms);
        journal.append(&WorkerEvent {
            schema_version: WORKER_SCHEMA_VERSION,
            attempt,
            recipient: recipient.clone(),
            event_offset_ms: submit_offset_ms,
            kind: WorkerEventKind::Started,
        })?;
        let journal_for_progress = journal.clone();
        let progress_recipient = recipient.clone();
        let arm_started_at_unix_ms = input.arm_started_at_unix_ms;
        let observer: Arc<OneSidedSubmissionObserver> = Arc::new(move |progress, identity| {
            let kind = match progress {
                OneSidedSubmissionProgress::Prepared => WorkerEventKind::Prepared {
                    identity: WireIdentity::from(identity),
                },
                OneSidedSubmissionProgress::SubmissionStarted => {
                    WorkerEventKind::SubmissionStarted {
                        identity: WireIdentity::from(identity),
                    }
                }
            };
            journal_for_progress.append(&WorkerEvent {
                schema_version: WORKER_SCHEMA_VERSION,
                attempt,
                recipient: progress_recipient.clone(),
                event_offset_ms: arm_offset_ms(arm_started_at_unix_ms),
                kind,
            })
        });
        let request = OwnedOneSidedSendRequest {
            db_path: input.db_path.clone(),
            password: password.clone(),
            base_node_url: input.base_node_url.clone(),
            recipient: recipient.clone(),
            amount: MicroMinotari(input.amount_microtari),
            fee_rate: MicroMinotari(input.fee_rate_microtari),
            seconds_to_lock: input.seconds_to_lock,
            confirmation_window: input.confirmation_window,
            request_timeout: Duration::from_millis(input.request_timeout_ms),
            tip_height_at_broadcast: input.tip_height_at_broadcast,
            prepared_transaction_binding: None,
        };
        let completion_journal = journal.clone();
        tasks.spawn(async move {
            let construction_request = request.clone();
            let prepared = tokio::task::spawn_blocking(move || {
                prepare_one_sided_transaction(&construction_request.as_borrowed())
            })
            .await
            .context("Mode 2 S4 blocking construction task failed")
            .and_then(|result| result);
            let construction_complete_offset_ms = prepared
                .as_ref()
                .ok()
                .map(|_| arm_offset_ms(arm_started_at_unix_ms));
            let result = match prepared {
                Ok(prepared) => {
                    finalize_prepared_one_sided_and_broadcast_without_retry_before_observed(
                        prepared,
                        request.as_borrowed(),
                        deadline,
                        observer.as_ref(),
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            let (outcome, failure) = match result {
                Ok(outcome) => (Some(Box::new(WireOutcome::from(outcome))), None),
                Err(error) => (None, Some(Box::new(WireFailure::from_error(&error)))),
            };
            completion_journal.append(&WorkerEvent {
                schema_version: WORKER_SCHEMA_VERSION,
                attempt,
                recipient,
                event_offset_ms: arm_offset_ms(arm_started_at_unix_ms),
                kind: WorkerEventKind::Completed {
                    construction_complete_offset_ms,
                    outcome,
                    failure,
                },
            })
        });
    }

    while let Some(result) = tasks.join_next().await {
        result.context("Mode 2 S4 worker task panicked")??;
    }
    Ok(())
}

pub(super) async fn run_arm_process(
    label: &str,
    recipients: Vec<String>,
    request: OwnedOneSidedSendRequest,
    deadline: time::Instant,
    arm_started_at_unix_ms: u128,
    seeds: &crate::config::SeedConfig,
) -> ScenarioSendSummary {
    let attempts = u32::try_from(recipients.len()).unwrap_or(u32::MAX);
    let mut summary = ScenarioSendSummary {
        attempted: attempts,
        ..ScenarioSendSummary::default()
    };
    summary.tip_start_height =
        match time::timeout_at(deadline, base_node_tip_height(&request.base_node_url)).await {
            Ok(Ok(height)) => Some(height),
            _ => None,
        };
    let checkpoint_ids_before = match checkpoint_tx_ids(&request.db_path) {
        Ok(ids) => ids,
        Err(error) => {
            return record_unstarted_process_failures(
                summary,
                label,
                recipients,
                arm_started_at_unix_ms,
                format!("could not snapshot prepared checkpoints before the arm: {error:#}"),
            );
        }
    };
    let worker_dir = match tempfile::Builder::new()
        .prefix("wallet-bench-mode2-s4-")
        .tempdir()
    {
        Ok(directory) => directory,
        Err(error) => {
            return record_unstarted_process_failures(
                summary,
                label,
                recipients,
                arm_started_at_unix_ms,
                format!("could not create worker directory: {error}"),
            );
        }
    };
    if let Err(error) = restrict_owner_only(worker_dir.path(), true) {
        return record_unstarted_process_failures(
            summary,
            label,
            recipients,
            arm_started_at_unix_ms,
            format!("could not restrict worker directory permissions: {error:#}"),
        );
    }
    let request_path = worker_dir.path().join("request.json");
    let events_path = worker_dir.path().join("events.jsonl");
    let deadline_at_unix_ms = unix_time_ms().saturating_add(
        deadline
            .saturating_duration_since(time::Instant::now())
            .as_millis(),
    );
    let request_timeout_ms = u64::try_from(request.request_timeout.as_millis()).unwrap_or(u64::MAX);
    let input = WorkerInput {
        schema_version: WORKER_SCHEMA_VERSION,
        db_path: request.db_path.clone(),
        base_node_url: request.base_node_url.clone(),
        recipients: recipients.clone(),
        amount_microtari: request.amount.0,
        fee_rate_microtari: request.fee_rate.0,
        seconds_to_lock: request.seconds_to_lock,
        confirmation_window: request.confirmation_window,
        request_timeout_ms,
        tip_height_at_broadcast: request.tip_height_at_broadcast,
        arm_started_at_unix_ms,
        deadline_at_unix_ms,
    };
    let input_bytes = match serde_json::to_vec(&input) {
        Ok(bytes) => bytes,
        Err(error) => {
            return record_unstarted_process_failures(
                summary,
                label,
                recipients,
                arm_started_at_unix_ms,
                format!("could not serialize worker input: {error}"),
            );
        }
    };
    if let Err(error) = fs::write(&request_path, input_bytes) {
        return record_unstarted_process_failures(
            summary,
            label,
            recipients,
            arm_started_at_unix_ms,
            format!("could not persist worker input: {error}"),
        );
    }
    if let Err(error) = restrict_owner_only(&request_path, false) {
        return record_unstarted_process_failures(
            summary,
            label,
            recipients,
            arm_started_at_unix_ms,
            format!("could not restrict worker input permissions: {error:#}"),
        );
    }

    let spawn_offset_ms = arm_offset_ms(arm_started_at_unix_ms);
    let mut command = match std::env::current_exe() {
        Ok(executable) => Command::new(executable),
        Err(error) => {
            return record_unstarted_process_failures(
                summary,
                label,
                recipients,
                arm_started_at_unix_ms,
                format!("could not resolve current executable: {error}"),
            );
        }
    };
    crate::managed_process::remove_benchmark_seed_envs(&mut command, seeds);
    command
        .arg("__mode2-s4-worker")
        .arg("--request")
        .arg(&request_path)
        .arg("--events")
        .arg(&events_path)
        .env(WORKER_PASSWORD_ENV, &request.password)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return record_unstarted_process_failures(
                summary,
                label,
                recipients,
                arm_started_at_unix_ms,
                format!("could not spawn worker process: {error}"),
            );
        }
    };
    let worker_pid = child.id();
    let mut deadline_expired = false;
    let mut force_kill_sent = false;
    let (exit_status, cleanup_ms) = match time::timeout_at(deadline, child.wait()).await {
        Ok(status) => (status, 0),
        Err(_) => {
            deadline_expired = true;
            let cleanup_start = Instant::now();
            force_kill_sent = child.start_kill().is_ok();
            (child.wait().await, cleanup_start.elapsed().as_millis())
        }
    };
    let actual_wall_ms = arm_offset_ms(arm_started_at_unix_ms);
    let (events, mut journal_errors) = read_events(&events_path);
    if let Err(error) = &exit_status {
        journal_errors.push(format!("worker wait failed: {error}"));
    }
    let worker_exit_success = exit_status
        .as_ref()
        .ok()
        .is_some_and(std::process::ExitStatus::success);
    let worker_reaped = exit_status.is_ok();
    if !deadline_expired && !worker_exit_success {
        journal_errors.push(format!(
            "worker exited before the deadline with status {}",
            exit_status
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|error| format!("unavailable ({error})"))
        ));
    }
    let checkpoint_ids_after = checkpoint_tx_ids(&request.db_path).unwrap_or_else(|error| {
        journal_errors.push(format!(
            "could not snapshot prepared checkpoints after the arm: {error:#}"
        ));
        checkpoint_ids_before.clone()
    });
    let new_checkpoint_ids = checkpoint_ids_after
        .difference(&checkpoint_ids_before)
        .cloned()
        .collect::<BTreeSet<_>>();
    let replay = replay_events(
        &mut summary,
        &recipients,
        events,
        &new_checkpoint_ids,
        deadline_expired,
        spawn_offset_ms,
        actual_wall_ms,
        &mut journal_errors,
    );
    summary.wall_ms = actual_wall_ms;
    let observed_deadline_expired = deadline_expired
        || summary.tx_timings.iter().any(|timing| {
            timing
                .get("failure_class")
                .and_then(serde_json::Value::as_str)
                == Some("arm_deadline")
        });
    let journal_error_count = journal_errors.len();
    summary.errors.extend(journal_errors);
    summary.extra_metrics.insert(
        "s4_dispatch_quiescence".to_string(),
        serde_json::json!({
            "deadline_expired": observed_deadline_expired,
            "pending_tasks_at_deadline": replay.pending_tasks_at_deadline,
            "post_deadline_drained_tasks": 0,
            "post_deadline_drain_ms": cleanup_ms,
            "quiesced_offset_ms": actual_wall_ms,
            "late_broadcasts_prevented": replay.late_broadcasts_prevented,
            "unstructured_tasks_after_quiescence": if worker_reaped { 0 } else { replay.pending_tasks_at_deadline },
            "tasks_quiesced_before_return": worker_reaped,
            "worker_pid": worker_pid,
            "worker_force_kill_sent": force_kill_sent,
            "worker_reaped": worker_reaped,
            "worker_exit_success": worker_exit_success,
            "worker_completed_attempts_replayed": replay.completed_attempts,
            "worker_preexisting_checkpoint_count": checkpoint_ids_before.len(),
            "worker_new_checkpoint_count": new_checkpoint_ids.len(),
            "worker_journal_error_count": journal_error_count,
            "request_policy": "one directly linked worker process owns the concurrent arm; the parent sends SIGKILL/start_kill at the absolute deadline and reaps it before replaying completed events and newly created durable checkpoints; no retry or resubmission"
        }),
    );
    summary
}

#[derive(Default)]
struct ReplayEvidence {
    completed_attempts: usize,
    pending_tasks_at_deadline: usize,
    late_broadcasts_prevented: u32,
}

#[allow(clippy::too_many_arguments)]
fn replay_events(
    summary: &mut ScenarioSendSummary,
    recipients: &[String],
    events: Vec<WorkerEvent>,
    new_checkpoint_ids: &BTreeSet<String>,
    deadline_expired: bool,
    default_submit_offset_ms: u128,
    arm_elapsed_ms: u128,
    journal_errors: &mut Vec<String>,
) -> ReplayEvidence {
    let mut attempts = BTreeMap::<u32, AttemptReplay>::new();
    for event in events {
        if event.schema_version != WORKER_SCHEMA_VERSION
            || event.attempt == 0
            || event.attempt as usize > recipients.len()
            || recipients[event.attempt as usize - 1] != event.recipient
        {
            journal_errors.push(format!(
                "ignored invalid worker event for attempt {}",
                event.attempt
            ));
            continue;
        }
        let replay = attempts.entry(event.attempt).or_default();
        match event.kind {
            WorkerEventKind::Started => {
                replay.submit_offset_ms.get_or_insert(event.event_offset_ms);
            }
            WorkerEventKind::Prepared { identity } => {
                if replay
                    .identity
                    .as_ref()
                    .is_some_and(|existing| existing.tx_id != identity.tx_id)
                {
                    journal_errors.push(format!(
                        "ignored conflicting prepared identity for attempt {}",
                        event.attempt
                    ));
                } else if new_checkpoint_ids.contains(&identity.tx_id) {
                    replay.identity = Some(identity);
                    replay.construction_complete_offset_ms = Some(event.event_offset_ms);
                } else {
                    journal_errors.push(format!(
                        "ignored prepared event for attempt {} without a checkpoint created by this arm",
                        event.attempt
                    ));
                }
            }
            WorkerEventKind::SubmissionStarted { identity } => {
                if replay
                    .identity
                    .as_ref()
                    .is_some_and(|existing| existing.tx_id != identity.tx_id)
                {
                    journal_errors.push(format!(
                        "ignored conflicting submission identity for attempt {}",
                        event.attempt
                    ));
                } else if new_checkpoint_ids.contains(&identity.tx_id) {
                    replay.identity = Some(identity);
                    replay.submission_started_offset_ms = Some(event.event_offset_ms);
                } else {
                    journal_errors.push(format!(
                        "ignored submission-started event for attempt {} without a checkpoint created by this arm",
                        event.attempt
                    ));
                }
            }
            WorkerEventKind::Completed {
                construction_complete_offset_ms,
                outcome,
                mut failure,
            } => {
                if replay.completed.is_some() {
                    journal_errors.push(format!(
                        "ignored duplicate completed event for attempt {}",
                        event.attempt
                    ));
                    continue;
                }
                replay.construction_complete_offset_ms =
                    construction_complete_offset_ms.or(replay.construction_complete_offset_ms);
                if let Some(failure) = &mut failure {
                    if failure.identity.is_none() {
                        failure.identity = replay.identity.clone();
                    }
                    if let Some(submission_started_offset_ms) = replay.submission_started_offset_ms
                    {
                        failure.submission_started = true;
                        failure.submission_elapsed_ms.get_or_insert(
                            event
                                .event_offset_ms
                                .saturating_sub(submission_started_offset_ms),
                        );
                    }
                }
                let outcome = outcome.and_then(|outcome| {
                    let outcome = *outcome;
                    let identity_matches = replay
                        .identity
                        .as_ref()
                        .is_none_or(|identity| identity.tx_id == outcome.tx_id);
                    if outcome.recipient == event.recipient
                        && identity_matches
                        && new_checkpoint_ids.contains(&outcome.tx_id)
                    {
                        Some(outcome)
                    } else {
                        journal_errors.push(format!(
                            "ignored completed outcome for attempt {} with inconsistent recipient, identity, or arm-created checkpoint",
                            event.attempt
                        ));
                        None
                    }
                });
                if let Some(failure) = &mut failure
                    && let Some(identity) = &failure.identity
                    && (!new_checkpoint_ids.contains(&identity.tx_id)
                        || replay
                            .identity
                            .as_ref()
                            .is_some_and(|prepared| prepared.tx_id != identity.tx_id))
                {
                    journal_errors.push(format!(
                        "removed completed failure identity for attempt {} because its checkpoint was not created by this arm",
                        event.attempt
                    ));
                    failure.identity = None;
                    failure.submission_started = false;
                    failure.submission_elapsed_ms = None;
                }
                replay.completed = match (outcome, failure) {
                    (Some(outcome), None) => Some(Ok(outcome)),
                    (None, Some(failure)) => Some(Err(*failure)),
                    _ => {
                        journal_errors.push(format!(
                            "ignored malformed completed event for attempt {}",
                            event.attempt
                        ));
                        None
                    }
                };
            }
        }
    }

    let mut evidence = ReplayEvidence::default();
    for (index, recipient) in recipients.iter().enumerate() {
        let attempt = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let replay = attempts.remove(&attempt).unwrap_or_default();
        let submit_offset_ms = replay.submit_offset_ms.unwrap_or(default_submit_offset_ms);
        let construction_complete_offset_ms = replay.construction_complete_offset_ms;
        let result = if let Some(completed) = replay.completed {
            evidence.completed_attempts += 1;
            completed.map(Into::into).map_err(WireFailure::into_error)
        } else {
            if deadline_expired {
                evidence.pending_tasks_at_deadline += 1;
            }
            let identity = replay.identity.map(Into::into);
            let submission_started = replay.submission_started_offset_ms.is_some();
            if identity.is_some() && !submission_started && deadline_expired {
                evidence.late_broadcasts_prevented =
                    evidence.late_broadcasts_prevented.saturating_add(1);
            }
            let failure_class = if deadline_expired {
                ObservationFailureClass::ArmDeadline
            } else {
                ObservationFailureClass::Process
            };
            let submission_elapsed_ms = replay
                .submission_started_offset_ms
                .map(|started| arm_elapsed_ms.saturating_sub(started));
            Err(anyhow::Error::new(OneSidedSubmissionFailure {
                identity,
                submission_started,
                submission_elapsed_ms,
                failure_class,
                message: if deadline_expired {
                    "Mode 2 S4 arm deadline force-terminated the worker before this attempt produced a terminal result"
                        .to_string()
                } else {
                    "Mode 2 S4 worker exited before this attempt produced a terminal result"
                        .to_string()
                },
            }))
        };
        if let Some(completed) = construction_complete_offset_ms {
            summary.construction_complete_ms.push(completed);
        }
        summary.record_attempt(
            attempt,
            submit_offset_ms,
            construction_complete_offset_ms,
            recipient.clone(),
            result,
        );
    }
    evidence
}

fn record_unstarted_process_failures(
    mut summary: ScenarioSendSummary,
    label: &str,
    recipients: Vec<String>,
    arm_started_at_unix_ms: u128,
    message: String,
) -> ScenarioSendSummary {
    let offset_ms = arm_offset_ms(arm_started_at_unix_ms);
    for (index, recipient) in recipients.into_iter().enumerate() {
        summary.record_attempt(
            u32::try_from(index + 1).unwrap_or(u32::MAX),
            offset_ms,
            None,
            recipient,
            Err(anyhow::Error::new(OneSidedSubmissionFailure {
                identity: None,
                submission_started: false,
                submission_elapsed_ms: None,
                failure_class: ObservationFailureClass::Process,
                message: format!("{label}: {message}"),
            })),
        );
    }
    summary.wall_ms = offset_ms;
    summary.extra_metrics.insert(
        "s4_dispatch_quiescence".to_string(),
        serde_json::json!({
            "deadline_expired": false,
            "pending_tasks_at_deadline": 0,
            "post_deadline_drained_tasks": 0,
            "post_deadline_drain_ms": 0,
            "quiesced_offset_ms": offset_ms,
            "late_broadcasts_prevented": 0,
            "unstructured_tasks_after_quiescence": 0,
            "tasks_quiesced_before_return": true,
            "worker_force_kill_sent": false,
            "worker_reaped": true,
            "worker_exit_success": false,
            "worker_completed_attempts_replayed": 0,
            "worker_preexisting_checkpoint_count": 0,
            "worker_new_checkpoint_count": 0,
            "worker_journal_error_count": 1,
            "request_policy": "worker process did not start; no submission or retry occurred"
        }),
    );
    summary
}

fn checkpoint_tx_ids(db_path: &Path) -> anyhow::Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for path in prepared_transaction_checkpoint_paths(db_path)? {
        let checkpoint = load_prepared_transaction_checkpoint_at(&path)?
            .with_context(|| format!("checkpoint disappeared at {}", path.display()))?;
        ids.insert(checkpoint.tx_id);
    }
    Ok(ids)
}

fn read_events(path: &Path) -> (Vec<WorkerEvent>, Vec<String>) {
    let Ok(bytes) = fs::read(path) else {
        return (Vec::new(), Vec::new());
    };
    let mut events = Vec::new();
    let mut errors = Vec::new();
    let mut start = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let line = &bytes[start..index];
        start = index + 1;
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice(line) {
            Ok(event) => events.push(event),
            Err(error) => errors.push(format!("ignored malformed worker journal row: {error}")),
        }
    }
    if start < bytes.len() {
        errors.push("ignored partial trailing worker journal row after termination".to_string());
    }
    (events, errors)
}

fn instant_from_unix_deadline(deadline_at_unix_ms: u128) -> time::Instant {
    time::Instant::now()
        + Duration::from_millis(
            u64::try_from(deadline_at_unix_ms.saturating_sub(unix_time_ms())).unwrap_or(u64::MAX),
        )
}

fn arm_offset_ms(arm_started_at_unix_ms: u128) -> u128 {
    unix_time_ms().saturating_sub(arm_started_at_unix_ms)
}

pub(super) fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(unix)]
fn restrict_owner_only(path: &Path, directory: bool) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = if directory { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("setting {:o} permissions on {}", mode, path.display()))
}

#[cfg(not(unix))]
fn restrict_owner_only(_path: &Path, _directory: bool) -> anyhow::Result<()> {
    // tempfile uses a per-user temporary namespace on Windows. The worker request
    // contains no password or seed; the password is passed only in the child env.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_recipients() -> Vec<String> {
        vec!["recipient-1".to_string(), "recipient-2".to_string()]
    }

    fn identity(tx_id: &str) -> WireIdentity {
        WireIdentity {
            tx_id: tx_id.to_string(),
            fee_microtari: 7,
            construction_ms: 5,
            broadcast_start_offset_ms: 6,
        }
    }

    #[test]
    fn partial_journal_replay_preserves_completed_and_prepared_attempts_once() {
        let recipients = input_recipients();
        let events = vec![
            WorkerEvent {
                schema_version: 1,
                attempt: 1,
                recipient: recipients[0].clone(),
                event_offset_ms: 2,
                kind: WorkerEventKind::Started,
            },
            WorkerEvent {
                schema_version: 1,
                attempt: 1,
                recipient: recipients[0].clone(),
                event_offset_ms: 7,
                kind: WorkerEventKind::Completed {
                    construction_complete_offset_ms: Some(5),
                    outcome: Some(Box::new(WireOutcome {
                        tx_id: "accepted".to_string(),
                        fee_microtari: 7,
                        accepted: true,
                        is_synced: true,
                        rejection_reason: None,
                        construction_ms: 3,
                        broadcast_to_mempool_ms: Some(2),
                        broadcast_start_offset_ms: Some(3),
                        recipient: recipients[0].clone(),
                    })),
                    failure: None,
                },
            },
            WorkerEvent {
                schema_version: 1,
                attempt: 2,
                recipient: recipients[1].clone(),
                event_offset_ms: 3,
                kind: WorkerEventKind::Started,
            },
            WorkerEvent {
                schema_version: 1,
                attempt: 2,
                recipient: recipients[1].clone(),
                event_offset_ms: 8,
                kind: WorkerEventKind::Prepared {
                    identity: identity("prepared"),
                },
            },
        ];
        let mut summary = ScenarioSendSummary {
            attempted: 2,
            ..ScenarioSendSummary::default()
        };
        let mut errors = Vec::new();
        let evidence = replay_events(
            &mut summary,
            &recipients,
            events,
            &BTreeSet::from(["accepted".to_string(), "prepared".to_string()]),
            true,
            1,
            20,
            &mut errors,
        );

        assert!(errors.is_empty());
        assert_eq!(evidence.completed_attempts, 1);
        assert_eq!(evidence.pending_tasks_at_deadline, 1);
        assert_eq!(evidence.late_broadcasts_prevented, 1);
        assert_eq!(summary.tx_timings.len(), 2);
        assert_eq!(summary.tx_ids, vec!["accepted"]);
        assert_eq!(summary.tx_timings[1]["tx_id"], "prepared");
        assert_eq!(summary.tx_timings[1]["failure_class"], "arm_deadline");
        assert!(summary.tx_timings[1]["api_accepted"].is_null());
        assert_eq!(summary.tx_timings[1]["submission_started"], false);
    }

    #[test]
    fn deadline_replay_retains_ambiguous_identity_without_resubmission() {
        let recipients = vec!["recipient".to_string()];
        let prepared = identity("ambiguous");
        let events = vec![
            WorkerEvent {
                schema_version: 1,
                attempt: 1,
                recipient: recipients[0].clone(),
                event_offset_ms: 1,
                kind: WorkerEventKind::Prepared {
                    identity: prepared.clone(),
                },
            },
            WorkerEvent {
                schema_version: 1,
                attempt: 1,
                recipient: recipients[0].clone(),
                event_offset_ms: 9,
                kind: WorkerEventKind::SubmissionStarted { identity: prepared },
            },
        ];
        let mut summary = ScenarioSendSummary {
            attempted: 1,
            ..ScenarioSendSummary::default()
        };
        let mut errors = Vec::new();
        replay_events(
            &mut summary,
            &recipients,
            events,
            &BTreeSet::from(["ambiguous".to_string()]),
            true,
            0,
            15,
            &mut errors,
        );

        assert!(errors.is_empty());
        assert_eq!(summary.tx_ids, vec!["ambiguous"]);
        assert_eq!(summary.tx_timings[0]["submission_started"], true);
        assert_eq!(summary.tx_timings[0]["submission_attempt_ms"], 6);
        assert!(summary.tx_timings[0]["api_accepted"].is_null());
        assert_eq!(summary.tx_timings[0]["failure_class"], "arm_deadline");
    }

    #[test]
    fn completed_generic_failure_inherits_preceding_submission_identity() {
        let recipients = vec!["recipient".to_string()];
        let prepared = identity("ambiguous");
        let events = vec![
            WorkerEvent {
                schema_version: 1,
                attempt: 1,
                recipient: recipients[0].clone(),
                event_offset_ms: 3,
                kind: WorkerEventKind::Prepared {
                    identity: prepared.clone(),
                },
            },
            WorkerEvent {
                schema_version: 1,
                attempt: 1,
                recipient: recipients[0].clone(),
                event_offset_ms: 9,
                kind: WorkerEventKind::SubmissionStarted { identity: prepared },
            },
            WorkerEvent {
                schema_version: 1,
                attempt: 1,
                recipient: recipients[0].clone(),
                event_offset_ms: 12,
                kind: WorkerEventKind::Completed {
                    construction_complete_offset_ms: Some(3),
                    outcome: None,
                    failure: Some(Box::new(WireFailure {
                        identity: None,
                        submission_started: false,
                        submission_elapsed_ms: None,
                        failure_class: ObservationFailureClass::Database,
                        message: "local status persistence failed".to_string(),
                    })),
                },
            },
        ];
        let mut summary = ScenarioSendSummary {
            attempted: 1,
            ..ScenarioSendSummary::default()
        };
        let mut errors = Vec::new();
        replay_events(
            &mut summary,
            &recipients,
            events,
            &BTreeSet::from(["ambiguous".to_string()]),
            false,
            0,
            12,
            &mut errors,
        );

        assert!(errors.is_empty());
        assert_eq!(summary.tx_ids, vec!["ambiguous"]);
        assert_eq!(summary.tx_timings[0]["submission_started"], true);
        assert_eq!(summary.tx_timings[0]["submission_attempt_ms"], 3);
        assert_eq!(summary.tx_timings[0]["failure_class"], "database");
        assert!(summary.tx_timings[0]["api_accepted"].is_null());
    }

    #[tokio::test]
    async fn start_kill_force_terminates_and_reaps_a_stuck_worker() {
        #[cfg(unix)]
        let mut command = {
            let mut command = Command::new("sh");
            command.arg("-c").arg("exec sleep 30");
            command
        };
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("powershell.exe");
            command.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
            command
        };
        command.kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let started = Instant::now();
        let deadline = time::Instant::now() + Duration::from_millis(50);
        assert!(time::timeout_at(deadline, child.wait()).await.is_err());
        child.start_kill().unwrap();
        let status = child.wait().await.unwrap();
        assert!(!status.success());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(child.try_wait().unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn event_journal_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let journal = EventJournal::create(&path).unwrap();
        journal
            .append(&WorkerEvent {
                schema_version: WORKER_SCHEMA_VERSION,
                attempt: 1,
                recipient: "recipient".to_string(),
                event_offset_ms: 3,
                kind: WorkerEventKind::Started,
            })
            .unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let (events, errors) = read_events(&path);
        assert!(errors.is_empty());
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].kind, WorkerEventKind::Started));
    }
}
