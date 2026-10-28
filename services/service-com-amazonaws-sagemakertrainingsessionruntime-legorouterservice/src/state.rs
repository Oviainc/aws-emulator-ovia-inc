// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakertrainingsessionruntime-legorouterservice service.
//!
//! The service is the DATA PLANE of a SageMaker training session: a caller submits
//! asynchronous training and sampling operations against a session the control plane
//! created, then polls for each one's result. This module holds everything the six
//! handlers share, so no two operations grow their own copy of it:
//!
//! * the **session records** (`sessions`), materialised on first use of a session name
//!   because this runtime service models no create operation of its own;
//! * each session's **training queue** (`sequence_index` + `next_sequence_number`),
//!   which is what makes operations execute in sequence-number order;
//! * each session's **checkpoints**, written by `saveWeights` / `saveState` and read by
//!   `loadState`, `loadWeightsForSampling` and `SampleTrainingSession`;
//! * the **clock-owed transition** that moves a submitted operation out of its
//!   non-terminal status (see [`EXECUTION_SPAN`] and [`Session::advance`]);
//! * the **validation rules the model states in prose** rather than in a trait, which
//!   the boundary validator therefore does not refuse for us.
//!
//! No language model, tokenizer or training engine runs inside this emulator, so
//! nothing here invents model output and presents it as a forward pass, an optimizer
//! step or a generated sample: an operation whose execution needs one ends in the
//! service's own `Failed` status with `errorDetails` that say so (see
//! [`no_model_runtime`]), and the synchronous `SampleTrainingSession` answers a body
//! that says the same in plain terms. Everything around that — session and operation
//! records, sequence ordering, idempotency, checkpoints and their ARNs, status
//! progression, validation and the 404s — is real and computed from persisted state.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::{Document, Number};

use crate::ops::types::{
    LoadStateResult, LoadWeightsForSamplingResult, ModelInput, OperationConfig, OperationResult,
    SampleOperationConfig, SampleOperationResult, SaveStateResult, SaveWeightsResult,
};

/// The region a checkpoint ARN names when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after it starts executing an operation's non-terminal status settles.
///
/// AWS documents no duration for a training or sampling operation, so this span is
/// CHOSEN: half a second, inside this project's 250–750 ms window for a transition the
/// clock owes. It is never zero, so a caller that polls straight after submitting still
/// observes `InProgress`. The due instant is stored on the operation record
/// ([`TrainingOp::due_at`] / [`SamplingOp::due_at`]) and applied the next time any
/// handler runs; nothing sleeps.
pub const EXECUTION_SPAN: Duration = Duration::from_millis(500);

/// How many session records this emulator retains.
///
/// Sessions are materialised on first use, so without a bound a caller naming a fresh
/// session each call would grow the store without end. The oldest session is released
/// once the store passes this; it is far above any client working set, so eviction never
/// hides a session a caller is still polling.
pub const MAX_SESSIONS: usize = 256;

/// How many operations of one kind a single session retains. A session that has reached
/// it cannot accept another submit (`ConflictException`), which is a refusal rather than
/// silent eviction: an operation id a caller still holds must keep resolving.
pub const MAX_OPERATIONS_PER_SESSION: usize = 1_024;

/// The `RequestStatus` values, from the model's own enum.
pub const STATUS_PENDING: &str = "Pending";
/// See [`STATUS_PENDING`].
pub const STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`STATUS_PENDING`].
pub const STATUS_COMPLETED: &str = "Completed";
/// See [`STATUS_PENDING`].
pub const STATUS_FAILED: &str = "Failed";

/// Whether `status` is terminal: "Poll this operation until the status reaches a terminal
/// state (Completed, Failed, or Skipped)" — `GetTrainingOperationResult`.
pub fn is_terminal_status(status: &str) -> bool {
    matches!(status, STATUS_COMPLETED | STATUS_FAILED | "Skipped")
}

/// The id prefix of a training operation, and the namespace its ids are minted in.
pub const TRAINING_NAMESPACE: &str = "training";
/// The id prefix of a sampling operation. Sampling ids live in their own namespace, so a
/// training operation id never resolves on the sampling route or the other way round.
pub const SAMPLING_NAMESPACE: &str = "sampling";

/// The content type `SampleTrainingSession` answers with, and the only one it can serve.
pub const SAMPLE_RESPONSE_CONTENT_TYPE: &str = "application/json";

/// How deep a caller-supplied `Document` is walked when it is rendered for a request
/// fingerprint, keeping the recursion bound LOCAL to this crate rather than inherited
/// from a deserializer.
pub const MAX_DOCUMENT_DEPTH: usize = 32;

/// The keys `lossFnConfig` accepts, with the range each one's value must fall in.
///
/// From `ForwardBackwardConfig$lossFnConfig`: "Supported keys are `clip_low_threshold`
/// (valid range: 0 to 1) and `clip_high_threshold` (valid range: 1 to 10). Unsupported
/// keys are rejected." The same document member is declared on `ForwardConfig`.
pub const LOSS_FN_CONFIG_KEYS: &[(&str, f64, f64)] =
    &[("clip_low_threshold", 0.0, 1.0), ("clip_high_threshold", 1.0, 10.0)];

/// Why an operation whose execution needs a model runtime ends in `Failed`.
///
/// `kind` is the union member that was submitted. No language model, tokenizer or
/// training engine runs inside this emulator, so there is nothing honest to report as
/// `Completed` for a forward pass, an optimizer step, a generated sample or a
/// log-probability: the operation ends in the service's own modelled failure status with
/// a reason that says why.
pub fn no_model_runtime(kind: &str) -> String {
    format!(
        "This SageMaker training-session runtime emulator runs no model, tokenizer or \
         training engine, so the {kind} operation was not executed and produced no result."
    )
}

/// Why an operation that names a checkpoint this session does not hold ends in `Failed`.
pub fn checkpoint_not_in_session(session: &str, arn: &str) -> String {
    format!(
        "Training session {session} holds no checkpoint {arn}; a checkpoint must be \
         written by a saveWeights or saveState operation on this session before it can \
         be loaded."
    )
}

/// One checkpoint of a training session, as this emulator persists it.
///
/// Written when a `saveWeights` or `saveState` operation completes, and read by
/// `loadState`, `loadWeightsForSampling` and `SampleTrainingSession`.
#[derive(Debug, Clone)]
pub struct Checkpoint {
    /// The model package ARN the save operation reported, in the form
    /// `SaveStateResult$modelPackageArn` documents.
    pub model_package_arn: String,
    /// The model package version in that ARN: 1 for a session's first checkpoint.
    pub version: i32,
    pub created_by_operation_id: String,
    pub created_at: SystemTime,
    /// Whether this checkpoint carries optimizer state as well as weights. A
    /// `saveState` checkpoint does ("saving full training state, including model weights
    /// and optimizer state"); a `saveWeights` one does not.
    pub holds_optimizer_state: bool,
    /// `mergeWeights` as submitted: whether LoRA adapter weights were merged into the
    /// base model before saving.
    pub merged_weights: bool,
    pub description: Option<String>,
}

/// The checkpoint slot a save operation reserved when it was submitted.
///
/// The ARN is minted at SUBMIT time, from the region that request was signed for, so the
/// ARN a later poll reports does not depend on who polled; the operation publishes the
/// slot to [`Session::checkpoints`] when it completes, so a checkpoint only becomes
/// loadable once the save that writes it has finished.
#[derive(Debug, Clone)]
pub struct ReservedCheckpoint {
    pub model_package_arn: String,
    pub version: i32,
}

/// One submitted training operation.
#[derive(Debug, Clone)]
pub struct TrainingOp {
    pub operation_id: String,
    pub sequence_number: i32,
    pub client_token: Option<String>,
    /// Digest of the submitted parameters ([`training_request_fingerprint`]), compared
    /// when a `clientToken` is reused so an idempotent retry can be told from a reuse of
    /// the same token with different parameters.
    pub request_fingerprint: String,
    /// The configuration exactly as submitted. It is what decides which member of
    /// `OperationResult` a poll populates, and the record of what the caller asked for.
    pub config: OperationConfig,
    /// One of the `RequestStatus` values.
    pub status: &'static str,
    pub result: Option<OperationResult>,
    pub error_details: Option<String>,
    pub submitted_at: SystemTime,
    /// When this operation's transition out of `InProgress` falls due, set when it
    /// reaches the head of the session's sequence queue and starts executing.
    pub due_at: Option<SystemTime>,
    /// The checkpoint a `saveWeights` / `saveState` operation will publish.
    pub reserved: Option<ReservedCheckpoint>,
}

/// One submitted sampling operation. Sampling operations carry no sequence number, so
/// each one executes on its own rather than in a queue.
#[derive(Debug, Clone)]
pub struct SamplingOp {
    pub operation_id: String,
    pub config: SampleOperationConfig,
    /// One of the `RequestStatus` values.
    pub status: &'static str,
    pub result: Option<SampleOperationResult>,
    pub error_details: Option<String>,
    pub submitted_at: SystemTime,
    /// See [`TrainingOp::due_at`].
    pub due_at: Option<SystemTime>,
}

/// One training session's runtime state.
#[derive(Debug)]
pub struct Session {
    pub name: String,
    pub created_at: SystemTime,
    /// The sequence number of the next operation to execute. "The training session
    /// processes operations strictly in sequence number order. If a gap exists, the
    /// session blocks until the missing operation arrives" — so this only advances past
    /// a slot once the operation occupying it is terminal.
    pub next_sequence_number: i32,
    pub training_ops: BTreeMap<String, TrainingOp>,
    /// `sequenceNumber` -> `operationId`: the execution queue.
    pub sequence_index: BTreeMap<i32, String>,
    pub sampling_ops: BTreeMap<String, SamplingOp>,
    /// `clientToken` -> `operationId`, so a retry with the same token is ignored and
    /// answers the operation id the first call minted.
    pub client_tokens: HashMap<String, String>,
    /// Every checkpoint written on this session, keyed by its model package ARN.
    pub checkpoints: BTreeMap<String, Checkpoint>,
    /// The model package version the next reserved checkpoint carries.
    pub next_checkpoint_version: i32,
    /// The counter operation ids are minted from, so two submits never collide.
    pub next_operation_seq: u64,
    /// The instant the training queue last freed: the due instant of the operation that
    /// executed before the current head, so each operation's own due instant is chained
    /// from its predecessor's rather than from whenever somebody polled.
    pub queue_free_at: Option<SystemTime>,
    /// The checkpoint `loadState` last restored training state from.
    pub loaded_checkpoint_arn: Option<String>,
    /// The checkpoint `loadWeightsForSampling` last loaded for inference.
    pub sampling_checkpoint_arn: Option<String>,
}

impl Session {
    /// A session materialised on first use of its name.
    pub fn new(name: String, now: SystemTime) -> Self {
        Self {
            name,
            created_at: now,
            next_sequence_number: 1,
            training_ops: BTreeMap::new(),
            sequence_index: BTreeMap::new(),
            sampling_ops: BTreeMap::new(),
            client_tokens: HashMap::new(),
            checkpoints: BTreeMap::new(),
            next_checkpoint_version: 1,
            next_operation_seq: 0,
            queue_free_at: None,
            loaded_checkpoint_arn: None,
            sampling_checkpoint_arn: None,
        }
    }

    /// Mint an operation id in `namespace`, within `OperationId`'s 1..64 length and
    /// `^[a-zA-Z0-9-]+$` pattern.
    ///
    /// Derived from this session's counter plus the submitting instant, so two submits
    /// never share an id and the id is not a guessable sequence; the loop skips an id the
    /// session already holds.
    pub fn mint_operation_id(&mut self, namespace: &str, now: SystemTime) -> String {
        use sha2::{Digest, Sha256};
        let millis = now
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or(0);
        loop {
            self.next_operation_seq = self.next_operation_seq.wrapping_add(1);
            let seed = format!(
                "{}:{namespace}:{}:{millis}",
                self.name, self.next_operation_seq
            );
            let digest = Sha256::digest(seed.as_bytes());
            let candidate = format!("{namespace}-{}", &hex::encode(digest)[..24]);
            let taken = self.training_ops.contains_key(&candidate)
                || self.sampling_ops.contains_key(&candidate);
            if !taken {
                return candidate;
            }
        }
    }

    /// Reserve the checkpoint a save operation will publish when it completes.
    ///
    /// The ARN has the form `SaveStateResult$modelPackageArn` documents:
    /// `arn:aws:sagemaker:REGION:ACCOUNT:model-package/PACKAGE_NAME/VERSION`.
    pub fn reserve_checkpoint(&mut self, region: &str) -> ReservedCheckpoint {
        let version = self.next_checkpoint_version;
        self.next_checkpoint_version = self.next_checkpoint_version.saturating_add(1);
        ReservedCheckpoint {
            model_package_arn: checkpoint_arn(region, &self.name, version),
            version,
        }
    }

    /// The checkpoint a sampling call runs against when the caller names none: the one
    /// last loaded for sampling, else the one training state was last loaded from, else
    /// the newest checkpoint this session wrote.
    pub fn current_checkpoint(&self) -> Option<&Checkpoint> {
        self.sampling_checkpoint_arn
            .as_deref()
            .or(self.loaded_checkpoint_arn.as_deref())
            .and_then(|arn| self.checkpoints.get(arn))
            .or_else(|| {
                self.checkpoints
                    .values()
                    .max_by_key(|checkpoint| checkpoint.version)
            })
    }

    /// Whether every operation this session holds has reached a terminal status, so
    /// releasing the session loses nothing a caller is still waiting on.
    pub fn is_settled(&self) -> bool {
        self.training_ops
            .values()
            .all(|op| is_terminal_status(op.status))
            && self
                .sampling_ops
                .values()
                .all(|op| is_terminal_status(op.status))
    }

    /// Whether this session's training queue is waiting on an operation nobody has
    /// submitted: the slot it would execute next holds nothing, while later slots do.
    ///
    /// Such a session executes nothing until a caller supplies the missing sequence
    /// number, so no instant the clock reaches settles it — which is what makes it the
    /// session to release when the store is full and nothing has settled.
    pub fn is_blocked_on_a_missing_operation(&self) -> bool {
        !self.sequence_index.is_empty()
            && !self.sequence_index.contains_key(&self.next_sequence_number)
    }

    /// Materialise every status transition the clock has already made due at `now`.
    ///
    /// A reporting operation calls this before it answers: each change was owed at the
    /// stored due instant whether or not anybody called, so applying it here is not a
    /// read with a side effect.
    pub fn advance(&mut self, now: SystemTime) {
        self.advance_training(now);
        self.advance_sampling(now);
    }

    /// Drive the training queue: operations execute strictly in sequence-number order,
    /// so the queue stops at the first slot that is empty (the session blocks on the gap)
    /// or still executing.
    fn advance_training(&mut self, now: SystemTime) {
        loop {
            let Some(op_id) = self.sequence_index.get(&self.next_sequence_number).cloned() else {
                return;
            };
            let Some(mut op) = self.training_ops.remove(&op_id) else {
                // The record behind the slot is gone, so nothing can execute it: release
                // the slot rather than block the session on it for ever.
                self.sequence_index.remove(&self.next_sequence_number);
                self.next_sequence_number = self.next_sequence_number.saturating_add(1);
                continue;
            };
            let terminal = self.start_or_settle_training(&mut op, now);
            self.training_ops.insert(op_id, op);
            if !terminal {
                return;
            }
            self.next_sequence_number = self.next_sequence_number.saturating_add(1);
        }
    }

    /// Start the queue's head operation if it has not started, settle it if the clock has
    /// reached its due instant, and answer whether it is now terminal.
    ///
    /// An operation starts at the instant the slot ahead of it freed — not at the instant
    /// of whichever call happens to drive the queue — so the whole queue drains on
    /// instants that were owed whether or not anybody called.
    fn start_or_settle_training(&mut self, op: &mut TrainingOp, now: SystemTime) -> bool {
        if op.status == STATUS_PENDING {
            let started_at = self.queue_free_at.unwrap_or(op.submitted_at).max(op.submitted_at);
            if let Some(kind) = training_needs_model_runtime(&op.config) {
                // Nothing executes it, so it never enters `InProgress` and claims a
                // duration for work that did not run; the queue frees at the instant it
                // reached the head.
                op.status = STATUS_FAILED;
                op.error_details = Some(no_model_runtime(kind));
                self.queue_free_at = Some(started_at);
                return true;
            }
            op.status = STATUS_IN_PROGRESS;
            op.due_at = Some(started_at + EXECUTION_SPAN);
        }
        if op.status != STATUS_IN_PROGRESS {
            return true;
        }
        if op.due_at.is_none_or(|due| now < due) {
            return false;
        }
        self.queue_free_at = op.due_at;
        self.complete_training(op);
        true
    }

    /// Apply a due training operation's effect to the session and record its result.
    fn complete_training(&mut self, op: &mut TrainingOp) {
        let created_at = op.due_at.unwrap_or(op.submitted_at);
        match &op.config {
            OperationConfig::saveWeights(config) => {
                let description = config.description.clone();
                let merged = config.merge_weights.unwrap_or(false);
                self.publish_checkpoint(op, created_at, false, merged, description);
            }
            OperationConfig::saveState(config) => {
                let description = config.description.clone();
                self.publish_checkpoint(op, created_at, true, false, description);
            }
            OperationConfig::loadState(config) => {
                let arn = config.model_package_arn.clone();
                let load_optimizer = config.load_optimizer;
                match self.checkpoints.get(&arn) {
                    None => {
                        op.status = STATUS_FAILED;
                        op.error_details = Some(checkpoint_not_in_session(&self.name, &arn));
                    }
                    Some(checkpoint) if load_optimizer && !checkpoint.holds_optimizer_state => {
                        op.status = STATUS_FAILED;
                        op.error_details = Some(format!(
                            "Checkpoint {arn} was written by a saveWeights operation and \
                             carries no optimizer state, so loadOptimizer cannot be \
                             satisfied from it."
                        ));
                    }
                    Some(_) => {
                        self.loaded_checkpoint_arn = Some(arn);
                        op.status = STATUS_COMPLETED;
                        op.result = Some(OperationResult::loadState(LoadStateResult {}));
                    }
                }
            }
            // Every remaining member needs a model runtime, and was refused before it
            // reached `InProgress`.
            other => {
                op.status = STATUS_FAILED;
                op.error_details = Some(no_model_runtime(training_config_kind(other)));
            }
        }
    }

    /// Publish a save operation's reserved checkpoint and record its result.
    fn publish_checkpoint(
        &mut self,
        op: &mut TrainingOp,
        created_at: SystemTime,
        holds_optimizer_state: bool,
        merged_weights: bool,
        description: Option<String>,
    ) {
        let Some(reserved) = op.reserved.clone() else {
            // A save operation reserves its checkpoint at submit time, so this is
            // unreachable; report it as a failure of the operation rather than inventing
            // an ARN that names nothing.
            op.status = STATUS_FAILED;
            op.error_details = Some(
                "No checkpoint slot was reserved for this save operation, so no model \
                 package was written."
                    .to_string(),
            );
            return;
        };
        self.checkpoints.insert(
            reserved.model_package_arn.clone(),
            Checkpoint {
                model_package_arn: reserved.model_package_arn.clone(),
                version: reserved.version,
                created_by_operation_id: op.operation_id.clone(),
                created_at,
                holds_optimizer_state,
                merged_weights,
                description,
            },
        );
        op.status = STATUS_COMPLETED;
        op.result = Some(if holds_optimizer_state {
            OperationResult::saveState(SaveStateResult {
                model_package_arn: reserved.model_package_arn,
            })
        } else {
            OperationResult::saveWeights(SaveWeightsResult {
                model_package_arn: Some(reserved.model_package_arn),
            })
        });
    }

    /// Drive the sampling operations. They carry no sequence number, so each settles on
    /// its own due instant.
    fn advance_sampling(&mut self, now: SystemTime) {
        let pending: Vec<String> = self
            .sampling_ops
            .iter()
            .filter(|(_, op)| op.status == STATUS_PENDING || op.status == STATUS_IN_PROGRESS)
            .map(|(id, _)| id.clone())
            .collect();
        for id in pending {
            let Some(mut op) = self.sampling_ops.remove(&id) else {
                continue;
            };
            if op.status == STATUS_PENDING {
                if let Some(kind) = sampling_needs_model_runtime(&op.config) {
                    op.status = STATUS_FAILED;
                    op.error_details = Some(no_model_runtime(kind));
                } else {
                    op.status = STATUS_IN_PROGRESS;
                    // Sampling operations are not queued, so each one's due instant is
                    // chained from its own submit rather than from whenever this call
                    // happened to be made.
                    op.due_at = Some(op.submitted_at + EXECUTION_SPAN);
                }
            }
            if op.status == STATUS_IN_PROGRESS && op.due_at.is_some_and(|due| now >= due) {
                self.complete_sampling(&mut op);
            }
            self.sampling_ops.insert(id, op);
        }
    }

    /// Apply a due sampling operation's effect to the session and record its result.
    fn complete_sampling(&mut self, op: &mut SamplingOp) {
        match &op.config {
            SampleOperationConfig::loadWeightsForSampling(config) => {
                let arn = config.model_package_arn.clone();
                if self.checkpoints.contains_key(&arn) {
                    self.sampling_checkpoint_arn = Some(arn);
                    op.status = STATUS_COMPLETED;
                    op.result = Some(SampleOperationResult::loadWeightsForSampling(
                        LoadWeightsForSamplingResult {},
                    ));
                } else {
                    op.status = STATUS_FAILED;
                    op.error_details = Some(checkpoint_not_in_session(&self.name, &arn));
                }
            }
            other => {
                op.status = STATUS_FAILED;
                op.error_details = Some(no_model_runtime(sampling_config_kind(other)));
            }
        }
    }
}

/// Per-service state for com-amazonaws-sagemakertrainingsessionruntime-legorouterservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakertrainingsessionruntimeLegorouterserviceState {
    /// Every training session this emulator holds, keyed by `trainingSessionName`.
    /// Bounded by [`MAX_SESSIONS`].
    pub sessions: BTreeMap<String, Session>,
}

impl ComAmazonawsSagemakertrainingsessionruntimeLegorouterserviceState {
    /// The session `name` names, with every transition the clock already made due
    /// applied — for a REPORTING operation, so a name this emulator has never seen
    /// answers `None` rather than being created.
    pub fn advance_session(&mut self, name: &str, now: SystemTime) -> Option<&mut Session> {
        let session = self.sessions.get_mut(name)?;
        session.advance(now);
        Some(session)
    }

    /// The session `name` names, for a SUBMIT, materialising the record on first use.
    ///
    /// This runtime service models no create operation — sessions are created by the
    /// SageMaker control plane — so the first submit against a name is what brings the
    /// runtime record into existence.
    ///
    /// Answers `None` when the store holds [`MAX_SESSIONS`] and none of them can be
    /// released: a session is released only once nothing in it is still owed, so an
    /// operation a caller is still polling never vanishes into a 404 that cannot be told
    /// from "never existed".
    ///
    /// The oldest SETTLED session goes first, and each session is brought up to the
    /// instant the clock has already reached before that is decided — an operation the
    /// clock finished settles the session that holds it whether or not anybody polled it,
    /// so eviction does not depend on who read what. Where nothing has settled, the
    /// oldest session whose queue is waiting on an operation nobody submitted goes
    /// instead: it executes nothing until a caller supplies that sequence number, so
    /// keeping it would refuse every new session for as long as the caller never does.
    /// Every session that survives both passes has an operation the clock is going to
    /// settle, so the refusal below is one a retry gets past.
    pub fn session_for_submit(&mut self, name: &str, now: SystemTime) -> Option<&mut Session> {
        if !self.sessions.contains_key(name) {
            while self.sessions.len() >= MAX_SESSIONS {
                for session in self.sessions.values_mut() {
                    session.advance(now);
                }
                let releasable = self
                    .sessions
                    .iter()
                    .filter(|(_, session)| session.is_settled())
                    .min_by_key(|(key, session)| (session.created_at, (*key).clone()))
                    .map(|(key, _)| key.clone())
                    .or_else(|| {
                        self.sessions
                            .iter()
                            .filter(|(_, session)| session.is_blocked_on_a_missing_operation())
                            .min_by_key(|(key, session)| (session.created_at, (*key).clone()))
                            .map(|(key, _)| key.clone())
                    });
                // Every session still has an operation the clock has yet to settle; the
                // submit is refused rather than one of them being thrown away.
                let key = releasable?;
                self.sessions.remove(&key);
            }
        }
        let session = self
            .sessions
            .entry(name.to_string())
            .or_insert_with(|| Session::new(name.to_string(), now));
        session.advance(now);
        Some(session)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakertrainingsessionruntimeLegorouterserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakertrainingsessionruntimeLegorouterserviceState>)
}

/// The region a request is addressed to: the third field of its SigV4 credential scope,
/// falling back to [`DEFAULT_REGION`] for a request that carries none.
///
/// The value is interpolated into an ARN, so it is honoured only when it has an AWS
/// region's shape: lowercase alphanumeric segments joined by `-`.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:sagemaker:<region>:<account>:model-package/<session>/<version>` — the form
/// `SaveStateResult$modelPackageArn` and `LoadStateConfig$modelPackageArn` document.
pub fn checkpoint_arn(region: &str, session: &str, version: i32) -> String {
    ::engine::emulator_arn(
        "sagemaker",
        region,
        &format!("model-package/{session}/{version}"),
    )
}

/// The `OperationConfig` member that was submitted, by its wire name.
pub fn training_config_kind(config: &OperationConfig) -> &'static str {
    match config {
        OperationConfig::Forward(_) => "forward",
        OperationConfig::forwardBackward(_) => "forwardBackward",
        OperationConfig::optimizerStep(_) => "optimizerStep",
        OperationConfig::saveWeights(_) => "saveWeights",
        OperationConfig::saveState(_) => "saveState",
        OperationConfig::loadState(_) => "loadState",
    }
}

/// The `SampleOperationConfig` member that was submitted, by its wire name.
pub fn sampling_config_kind(config: &SampleOperationConfig) -> &'static str {
    match config {
        SampleOperationConfig::Sample(_) => "sample",
        SampleOperationConfig::computeLogprobs(_) => "computeLogprobs",
        SampleOperationConfig::loadWeightsForSampling(_) => "loadWeightsForSampling",
    }
}

/// The member name when executing `config` would need a model, tokenizer or training
/// engine — none of which runs in this emulator — and `None` when it would not.
///
/// `saveWeights`, `saveState` and `loadState` move a checkpoint around, which this
/// emulator does for real; the other three members put data through the model.
pub fn training_needs_model_runtime(config: &OperationConfig) -> Option<&'static str> {
    match config {
        OperationConfig::Forward(_)
        | OperationConfig::forwardBackward(_)
        | OperationConfig::optimizerStep(_) => Some(training_config_kind(config)),
        OperationConfig::saveWeights(_)
        | OperationConfig::saveState(_)
        | OperationConfig::loadState(_) => None,
    }
}

/// The sampling counterpart of [`training_needs_model_runtime`]: `sample` and
/// `computeLogprobs` put data through the model, `loadWeightsForSampling` does not.
pub fn sampling_needs_model_runtime(config: &SampleOperationConfig) -> Option<&'static str> {
    match config {
        SampleOperationConfig::Sample(_) | SampleOperationConfig::computeLogprobs(_) => {
            Some(sampling_config_kind(config))
        }
        SampleOperationConfig::loadWeightsForSampling(_) => None,
    }
}

/// Refuse an `OperationConfig` the boundary validator accepted but the model's prose
/// rules out, answering the reason to put in a `ValidationException`.
pub fn validate_operation_config(config: &OperationConfig) -> Result<(), String> {
    let kind = training_config_kind(config);
    match config {
        OperationConfig::Forward(inner) => {
            validate_datum_list(kind, &inner.datum_list)?;
            validate_loss_fn_config(kind, inner.loss_fn_config.as_ref())
        }
        OperationConfig::forwardBackward(inner) => {
            validate_datum_list(kind, &inner.datum_list)?;
            validate_loss_fn_config(kind, inner.loss_fn_config.as_ref())
        }
        OperationConfig::optimizerStep(_) => Ok(()),
        OperationConfig::saveWeights(_) | OperationConfig::saveState(_) => Ok(()),
        OperationConfig::loadState(inner) => validate_model_package_arn(
            &format!("operationConfig.{kind}.modelPackageArn"),
            &inner.model_package_arn,
        ),
    }
}

/// The `SampleOperationConfig` counterpart of [`validate_operation_config`].
pub fn validate_sample_operation_config(config: &SampleOperationConfig) -> Result<(), String> {
    let kind = sampling_config_kind(config);
    match config {
        SampleOperationConfig::Sample(inner) => {
            validate_model_input(&format!("operationConfig.{kind}.prompt"), &inner.prompt)?;
            // "The number of top-k log-probabilities to return per prompt token",
            // `@default(0)`: a count, so a negative one names no response at all.
            if inner.topk_prompt_logprobs < 0 {
                return Err(format!(
                    "operationConfig.{kind}.topkPromptLogprobs must be at least 0, got {}",
                    inner.topk_prompt_logprobs
                ));
            }
            if let Some(arn) = inner.model_package_arn.as_deref() {
                validate_model_package_arn(&format!("operationConfig.{kind}.modelPackageArn"), arn)?;
            }
            Ok(())
        }
        SampleOperationConfig::computeLogprobs(inner) => {
            validate_model_input(&format!("operationConfig.{kind}.prompt"), &inner.prompt)?;
            if let Some(arn) = inner.model_package_arn.as_deref() {
                validate_model_package_arn(&format!("operationConfig.{kind}.modelPackageArn"), arn)?;
            }
            Ok(())
        }
        SampleOperationConfig::loadWeightsForSampling(inner) => validate_model_package_arn(
            &format!("operationConfig.{kind}.modelPackageArn"),
            &inner.model_package_arn,
        ),
    }
}

/// "Must contain at least one datum, and each datum must have a non-empty modelInput
/// with at least one chunk" — `ForwardBackwardConfig$datumList`.
fn validate_datum_list(kind: &str, datum_list: &[crate::ops::types::Datum]) -> Result<(), String> {
    if datum_list.is_empty() {
        return Err(format!(
            "operationConfig.{kind}.datumList must contain at least one datum"
        ));
    }
    for (index, datum) in datum_list.iter().enumerate() {
        validate_model_input(
            &format!("operationConfig.{kind}.datumList[{index}].modelInput"),
            &datum.model_input,
        )?;
    }
    Ok(())
}

/// A `ModelInput` is "the ordered list of encoded text chunks forming the input
/// sequence", and each chunk carries "the token IDs representing the encoded text": an
/// input with no chunks, or a chunk with no tokens, encodes nothing.
fn validate_model_input(member: &str, input: &ModelInput) -> Result<(), String> {
    if input.chunks.is_empty() {
        return Err(format!("{member}.chunks must contain at least one chunk"));
    }
    for (index, chunk) in input.chunks.iter().enumerate() {
        if chunk.tokens.is_empty() {
            return Err(format!(
                "{member}.chunks[{index}].tokens must contain at least one token id"
            ));
        }
    }
    Ok(())
}

/// Refuse a `lossFnConfig` document that carries a key the engine does not support, or a
/// supported key outside its range. See [`LOSS_FN_CONFIG_KEYS`] for the source.
fn validate_loss_fn_config(kind: &str, config: Option<&Document>) -> Result<(), String> {
    let Some(config) = config else {
        return Ok(());
    };
    let member = format!("operationConfig.{kind}.lossFnConfig");
    let Document::Object(entries) = config else {
        return Err(format!("{member} must be an object"));
    };
    for (key, value) in entries {
        let Some((_, min, max)) = LOSS_FN_CONFIG_KEYS
            .iter()
            .find(|(supported, _, _)| supported == key)
        else {
            let supported: Vec<&str> = LOSS_FN_CONFIG_KEYS.iter().map(|(key, _, _)| *key).collect();
            return Err(format!(
                "{member} does not support the key {key:?}; supported keys are {}",
                supported.join(", ")
            ));
        };
        let Some(number) = document_number(value) else {
            return Err(format!("{member}.{key} must be a number"));
        };
        if number < *min || number > *max {
            return Err(format!(
                "{member}.{key} must be between {min} and {max}, got {number}"
            ));
        }
    }
    Ok(())
}

/// The numeric value of a document node, or `None` when it is not a number.
fn document_number(value: &Document) -> Option<f64> {
    match value {
        Document::Number(Number::PosInt(value)) => Some(*value as f64),
        Document::Number(Number::NegInt(value)) => Some(*value as f64),
        Document::Number(Number::Float(value)) => Some(*value),
        _ => None,
    }
}

/// Refuse anything that is not a SageMaker model package ARN.
///
/// "Must be a valid SageMaker model package ARN in the format
/// `arn:aws:sagemaker:REGION:ACCOUNT:model-package/PACKAGE_NAME/VERSION`. Model package
/// group ARNs are not accepted." — `LoadStateConfig$modelPackageArn`.
pub fn validate_model_package_arn(member: &str, arn: &str) -> Result<(), String> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let malformed = |reason: &str| {
        Err(format!(
            "{member} must be a SageMaker model package ARN of the form \
             arn:aws:sagemaker:REGION:ACCOUNT:model-package/PACKAGE_NAME/VERSION ({reason}), \
             got {arn:?}"
        ))
    };
    if fields.len() != 6 || fields[0] != "arn" {
        return malformed("six colon-separated fields beginning with arn");
    }
    if !matches!(fields[1], "aws" | "aws-cn" | "aws-us-gov" | "aws-iso" | "aws-iso-b") {
        return malformed("an AWS partition");
    }
    if fields[2] != "sagemaker" {
        return malformed("the sagemaker service namespace");
    }
    if fields[3].is_empty() {
        return malformed("a region");
    }
    if fields[4].len() != 12 || !fields[4].chars().all(|c| c.is_ascii_digit()) {
        return malformed("a 12-digit account id");
    }
    let Some(resource) = fields[5].strip_prefix("model-package/") else {
        return malformed("a model-package resource, not a model package group");
    };
    let mut segments = resource.split('/');
    let name = segments.next().unwrap_or_default();
    let version = segments.next().unwrap_or_default();
    if segments.next().is_some() {
        return malformed("exactly one PACKAGE_NAME and one VERSION");
    }
    if name.is_empty() {
        return malformed("a package name");
    }
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit()) {
        return malformed("a numeric package version");
    }
    Ok(())
}

/// The body `SampleTrainingSession` answers with.
///
/// No model runs in this emulator, so this says so plainly rather than returning
/// synthesised inference output or the request's own body echoed back as if generated.
/// Besides the explanation it carries only facts about what was actually served: the
/// session, the checkpoint the request resolved to and how many bytes of request payload
/// arrived.
pub fn sample_response_body(session: &str, model_package_arn: &str, request_bytes: usize) -> Vec<u8> {
    let body = serde_json::json!({
        "message": "This SageMaker training-session runtime emulator runs no model, so no \
                    inference output was generated for this request.",
        "trainingSessionName": session,
        "modelPackageArn": model_package_arn,
        "requestBodyBytes": request_bytes,
    });
    serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec())
}

/// The fingerprint of an `ExecuteTrainingOperation` request's parameters: a SHA-256
/// digest of a canonical rendering of them.
///
/// `clientToken` makes a retry of "the same client token and the same parameters" a
/// no-op, so the handler needs to know whether a retry's parameters are in fact the
/// same. A fixed-size digest rather than the rendering itself, so the record keeps no
/// second copy of the caller's data; map keys are rendered in sorted order, so two
/// renderings of one request always agree.
pub fn training_request_fingerprint(sequence_number: i32, config: &OperationConfig) -> String {
    use sha2::{Digest, Sha256};
    let mut canonical = format!("sequenceNumber={sequence_number};");
    write_operation_config(&mut canonical, config);
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

fn write_operation_config(out: &mut String, config: &OperationConfig) {
    out.push_str(training_config_kind(config));
    out.push('{');
    match config {
        OperationConfig::Forward(inner) => write_pass(
            out,
            &inner.loss_fn_name,
            &inner.datum_list,
            inner.loss_fn_config.as_ref(),
        ),
        OperationConfig::forwardBackward(inner) => write_pass(
            out,
            &inner.loss_fn_name,
            &inner.datum_list,
            inner.loss_fn_config.as_ref(),
        ),
        OperationConfig::optimizerStep(inner) => {
            let adam = &inner.adam_params_config;
            out.push_str(&format!(
                "learningRate={};beta1={};beta2={};eps={};weightDecay={};gradClipNorm={};\
                 loraPlusLrRatio={:?}",
                adam.learning_rate,
                adam.beta1,
                adam.beta2,
                adam.eps,
                adam.weight_decay,
                adam.grad_clip_norm,
                adam.lora_plus_lr_ratio
            ));
        }
        OperationConfig::saveWeights(inner) => out.push_str(&format!(
            "mergeWeights={:?};description={:?}",
            inner.merge_weights, inner.description
        )),
        OperationConfig::saveState(inner) => {
            out.push_str(&format!("description={:?}", inner.description));
        }
        OperationConfig::loadState(inner) => out.push_str(&format!(
            "modelPackageArn={};loadOptimizer={}",
            inner.model_package_arn, inner.load_optimizer
        )),
    }
    out.push('}');
}

/// `ForwardConfig` and `ForwardBackwardConfig` carry the same three members, so one
/// renderer serves both.
fn write_pass(
    out: &mut String,
    loss_fn_name: &str,
    datum_list: &[crate::ops::types::Datum],
    loss_fn_config: Option<&Document>,
) {
    out.push_str("lossFnName=");
    out.push_str(loss_fn_name);
    out.push_str(";datumList=[");
    for datum in datum_list {
        write_model_input_canonical(out, &datum.model_input);
        out.push_str(";lossFnInputs=");
        match &datum.loss_fn_inputs {
            None => out.push_str("absent"),
            Some(inputs) => {
                let mut names: Vec<&String> = inputs.keys().collect();
                names.sort();
                out.push('{');
                for name in names {
                    out.push_str(name);
                    out.push('=');
                    if let Some(tensor) = inputs.get(name) {
                        out.push_str(&format!("{}/{:?}/", tensor.dtype, tensor.shape));
                        write_document_canonical(out, &tensor.data);
                    }
                    out.push(';');
                }
                out.push('}');
            }
        }
        out.push(',');
    }
    out.push_str("];lossFnConfig=");
    match loss_fn_config {
        None => out.push_str("absent"),
        Some(document) => write_document_canonical(out, document),
    }
}

fn write_model_input_canonical(out: &mut String, input: &ModelInput) {
    out.push_str("chunks=[");
    for chunk in &input.chunks {
        out.push_str(&format!("{:?}/{};", chunk.tokens, chunk.r#type));
    }
    out.push(']');
}

/// A document rendered with its object keys in sorted order, so the rendering does not
/// depend on the hash-map iteration order the deserializer happened to produce. The walk
/// stops at [`MAX_DOCUMENT_DEPTH`], so a deeply nested document cannot drive unbounded
/// recursion.
fn write_document_canonical(out: &mut String, document: &Document) {
    fn walk(out: &mut String, document: &Document, depth: usize) {
        if depth > MAX_DOCUMENT_DEPTH {
            out.push_str("...");
            return;
        }
        match document {
            Document::Object(members) => {
                let mut names: Vec<&String> = members.keys().collect();
                names.sort();
                out.push('{');
                for name in names {
                    out.push_str(name);
                    out.push('=');
                    if let Some(member) = members.get(name) {
                        walk(out, member, depth + 1);
                    }
                    out.push(';');
                }
                out.push('}');
            }
            Document::Array(items) => {
                out.push('[');
                for item in items {
                    walk(out, item, depth + 1);
                    out.push(',');
                }
                out.push(']');
            }
            Document::Number(Number::PosInt(value)) => out.push_str(&format!("u{value}")),
            Document::Number(Number::NegInt(value)) => out.push_str(&format!("i{value}")),
            Document::Number(Number::Float(value)) => out.push_str(&format!("f{value}")),
            Document::String(value) => out.push_str(&format!("s{value:?}")),
            Document::Bool(value) => out.push_str(&format!("b{value}")),
            Document::Null => out.push_str("null"),
        }
    }
    walk(out, document, 0)
}
