// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice service.
//!
//! The service's primary resource is a **trajectory**: the ordered turns of one
//! rollout inside a SageMaker job, keyed by the JOB the `JobArn` of a call
//! resolves to and the `TrajectoryId` it carries ([`trajectory_key`]). The
//! lifecycle the model documents is linear:
//!
//! * `Sample` / `SampleWithResponseStream` create the trajectory on its first
//!   turn and append a turn on every later call — "Each turn (prompt and
//!   response) is captured for later use".
//! * `CompleteRollout` seals it — "no further turns will be appended to the
//!   trajectory. After calling this operation, the trajectory is sealed and
//!   eligible for reward submission via the UpdateReward operation".
//! * `UpdateReward` records the reward scores and "transitions it to
//!   reward-received status".
//!
//! The job itself is NOT a resource of this store. This service is the data plane
//! of a job the SageMaker control plane created: its model binds no operation that
//! creates, lists or deletes a job, so the job a call names is resolved against the
//! SHARED job directory ([`::service_common::agentic_rft_jobs::JobDirectory`]) that
//! the control plane writes when it mints one — see [`job_directory`] and
//! [`job_not_found_message`]. A `JobArn` the member's own modelled constraints admit
//! (`@length` 1..256 and the ARN `@pattern`, which allow any partition, region,
//! account and job name) but that no create ever minted names no job, and EVERY
//! operation of this data plane refuses a call for it rather than serving it: a
//! sampling call because the turn can be routed to no model session, and
//! `CompleteRollout` and `UpdateReward` because a trajectory lives inside the job its
//! key names, so the pair names no trajectory of it either
//! ([`unresolved_job_trajectory_message`], which the rollout pair reaches through the
//! one resolution both of them serve a call by, [`serve_rollout_trajectory`], as the
//! sampling pair reaches its own through [`serve_sampling_turn`]). Beside that, a
//! sampling call is refused
//! on state this service owns: a trajectory `CompleteRollout` has sealed, and the
//! emulator bounds below.

use std::collections::HashMap;
use std::time::SystemTime;

use sha2::Digest;

/// How many turns one trajectory may hold.
///
/// An EMULATOR bound, not a published AWS quota: the turns of every trajectory
/// are kept in memory for the whole life of the process, so an unbounded
/// sampling loop would grow the store without limit. Reaching it is answered
/// with the modelled `ServiceQuotaExceededException`.
pub const MAX_TURNS_PER_TRAJECTORY: usize = 1_000;

/// How many trajectories the store keeps.
///
/// Same reason and same answer as [`MAX_TURNS_PER_TRAJECTORY`]: an emulator
/// bound on a store that is never garbage-collected.
pub const MAX_TRAJECTORIES: usize = 10_000;

/// How many bytes of inference payload ONE captured turn may retain.
///
/// An EMULATOR bound, not a published AWS quota, and the reason it exists is
/// different from the two counts above: a turn keeps the caller's payload
/// verbatim for the life of the process, so a count bound alone leaves the
/// RETAINED SIZE of the store chosen by the caller. A single oversized body is
/// refused rather than captured.
pub const MAX_TURN_BODY_BYTES: usize = 1 << 20;

/// How many bytes of payload the whole store may retain, across every turn of
/// every trajectory.
///
/// The store-wide half of the same bound: many in-bound bodies must not add up
/// to an unbounded store either, so the retained total is tracked
/// ([`ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState::retained_body_bytes`])
/// and a turn that would pass this ceiling is refused as a quota, with nothing
/// written.
pub const MAX_RETAINED_BODY_BYTES: usize = 64 << 20;

/// The wire value of the `Content-Type` the OpenAI-compatible, non-streaming
/// inference response is rendered in.
pub const JSON_CONTENT_TYPE: &str = "application/json";

/// The wire value of the `Content-Type` a streamed inference response is
/// rendered in: OpenAI-compatible streaming is server-sent events, one
/// `chat.completion.chunk` per `data:` line and a final `data: [DONE]`.
pub const EVENT_STREAM_CONTENT_TYPE: &str = "text/event-stream";

/// Where a trajectory is in the rollout lifecycle.
///
/// The two non-initial names come from the operation docs: `CompleteRollout`
/// seals the trajectory, and `UpdateReward` "transitions it to reward-received
/// status".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrajectoryStatus {
    /// Turns may still be appended.
    InProgress,
    /// `CompleteRollout` has sealed it; no further turns, reward submission is
    /// what comes next.
    Complete,
    /// `UpdateReward` has recorded the reward scores.
    RewardReceived,
}

impl TrajectoryStatus {
    /// The status as the service names it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in-progress",
            Self::Complete => "complete",
            Self::RewardReceived => "reward-received",
        }
    }
}

/// The terminal status `CompleteRollout` seals a trajectory with.
///
/// The modelled `CompletionStatus` enum: `ready` — "Trajectory completed
/// successfully; sealed and eligible for reward submission" — and `failed` —
/// "Trajectory failed; terminal state, no further processing".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionStatus {
    /// `failed`: terminal, no further processing.
    Failed,
    /// `ready`: sealed and eligible for reward submission.
    Ready,
}

impl CompletionStatus {
    /// The wire value of this status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Ready => "ready",
        }
    }

    /// The status a wire value names, or `None` when the value is not one the
    /// model declares.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "failed" => Some(Self::Failed),
            "ready" => Some(Self::Ready),
            _ => None,
        }
    }

    /// Whether a trajectory sealed with this status accepts reward scores.
    ///
    /// Only `ready` does: it is the value whose documentation says the
    /// trajectory is "eligible for reward submission", where `failed` is a
    /// "terminal state, no further processing".
    #[must_use]
    pub const fn accepts_reward(self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// One captured turn of a trajectory: the prompt that was sent and the response
/// that was served for it.
#[derive(Debug, Clone)]
pub struct Turn {
    /// The raw inference request body, as the caller sent it.
    pub request_body: Vec<u8>,
    /// The `Content-Type` the caller labelled the request body with, when it
    /// sent one.
    pub request_content_type: Option<String>,
    /// The inference response body this emulator served for the turn.
    pub response_body: Vec<u8>,
    /// The `Content-Type` the response body was served with.
    pub response_content_type: Option<String>,
    /// Whether the turn was served by `SampleWithResponseStream` rather than
    /// `Sample`.
    pub streamed: bool,
    /// The turn's zero-based position in the trajectory.
    pub index: usize,
    /// When the turn was captured.
    pub at: SystemTime,
}

/// One rollout's trajectory: its turns, its lifecycle status and its rewards.
#[derive(Debug, Clone)]
pub struct Trajectory {
    /// The job the rollout belongs to, as the shared job directory identifies one: the
    /// `<category>/<name>` resource path of its ARN, which is also the job half of the
    /// key this trajectory is held under ([`trajectory_key`]).
    pub job: String,
    /// The trajectory id that groups the turns.
    pub trajectory_id: String,
    /// Where the trajectory is in the lifecycle.
    pub status: TrajectoryStatus,
    /// Every captured turn, in the order they were served.
    pub turns: Vec<Turn>,
    /// The terminal status `CompleteRollout` sealed it with, once sealed.
    pub completion_status: Option<CompletionStatus>,
    /// The reward scores `UpdateReward` recorded, once recorded.
    pub rewards: Option<Vec<f64>>,
    /// The `ClientToken` of the `CompleteRollout` call that sealed it.
    pub complete_rollout_token: Option<String>,
    /// The `ClientToken` of the `UpdateReward` call that rewarded it.
    pub update_reward_token: Option<String>,
    /// When the first turn created the trajectory.
    pub created_at: SystemTime,
    /// When the trajectory last changed.
    pub updated_at: SystemTime,
}

impl Trajectory {
    /// A fresh, empty, in-progress trajectory for `job_arn` / `trajectory_id`.
    #[must_use]
    pub fn new(job_arn: &str, trajectory_id: &str, now: SystemTime) -> Self {
        let (job, trajectory_id) = trajectory_key(job_arn, trajectory_id);
        Self {
            job,
            trajectory_id,
            status: TrajectoryStatus::InProgress,
            turns: Vec::new(),
            completion_status: None,
            rewards: None,
            complete_rollout_token: None,
            update_reward_token: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// The key a trajectory is stored under: the JOB it belongs to — as the shared directory
/// identifies one, NOT the ARN string a request spelled it with — and its id.
pub type TrajectoryKey = (String, String);

/// The key the trajectory of `job_arn` / `trajectory_id` is held under.
///
/// A trajectory belongs to the JOB a call resolves to, and this crate resolves a `JobArn`
/// against the shared directory, which identifies a job by the `<category>/<name>` resource
/// path of its ARN alone ([`::service_common::agentic_rft_jobs::job_identity_of_arn`]): the
/// partition, the Region and the account an ARN was addressed through identify no job, so
/// they cannot distinguish one job's trajectories either. Keying by the identity rather than
/// by the request's own string is what keeps the turns of one rollout in ONE trajectory
/// however its calls address the job, keeps the seal `CompleteRollout` writes in the way of
/// a later turn, and keeps the rollout-lifecycle pair resolving against the trajectory the
/// sampling calls built.
///
/// A value naming no job resource has no identity to key by and is held under itself; every
/// operation refuses such a call before it reaches the store, because no job can resolve for
/// it ([`job_exists`]).
#[must_use]
pub fn trajectory_key(job_arn: &str, trajectory_id: &str) -> TrajectoryKey {
    (job_identity(job_arn).to_string(), trajectory_id.to_string())
}

/// WHICH JOB the `JobArn` of a call resolves to, as the shared directory identifies one:
/// the `<category>/<name>` resource path of the ARN
/// ([`::service_common::agentic_rft_jobs::job_identity_of_arn`]).
///
/// This crate's ONE reading of a request's `JobArn` as the job it addresses, so nothing
/// this service keys, stores or REPORTS per job can be taken from the ARN string one call
/// happened to spell that job with: the partition, the Region and the account an ARN was
/// addressed through identify no job, so two calls of one rollout that differ only there
/// address one job and must agree about it. Everything per-job derives from here —
/// [`trajectory_key`] and [`Trajectory::new`] for where a turn is stored, [`completion_id`]
/// and [`completion_envelope`] for what a served turn reports.
///
/// A value naming no job resource has no identity to key or report by and stands for
/// itself; every operation refuses such a call before it reaches the store, because no job
/// can resolve for it ([`job_exists`]).
#[must_use]
pub fn job_identity(job_arn: &str) -> &str {
    ::service_common::agentic_rft_jobs::job_identity_of_arn(job_arn).unwrap_or(job_arn)
}

/// How this service reports a `(JobArn, TrajectoryId)` pair it holds no trajectory for.
///
/// Built from the request's own trajectory id and the account the call is served for —
/// the one account every emulated service presents
/// ([`::engine::emulator_account_id`]).
#[must_use]
pub fn trajectory_not_found_message(trajectory_id: &str) -> String {
    format!(
        "Trajectory {trajectory_id} not found for account {}",
        ::engine::emulator_account_id()
    )
}

/// Per-service state for com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState {
    /// Every trajectory this emulator has served a turn for, keyed by
    /// `(JobArn, TrajectoryId)`.
    pub trajectories: HashMap<TrajectoryKey, Trajectory>,
    /// How many payload bytes every captured turn of this store retains, summed over
    /// both the request and the response body of each turn.
    ///
    /// Maintained by [`Self::append_turn`], which is the only writer of a turn, and
    /// read through [`Self::retained_body_bytes`]; it is what makes
    /// [`MAX_RETAINED_BODY_BYTES`] a real ceiling rather than a comment.
    retained_body_bytes: usize,
}

impl Default for ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState {
    fn default() -> Self {
        Self {
            trajectories: HashMap::new(),
            retained_body_bytes: 0,
        }
    }
}

impl ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState {
    /// The trajectory the JOB `job_arn` resolves to holds under `trajectory_id`, if
    /// this emulator has one.
    #[must_use]
    pub fn trajectory(&self, job_arn: &str, trajectory_id: &str) -> Option<&Trajectory> {
        self.trajectories
            .get(&trajectory_key(job_arn, trajectory_id))
    }

    /// The trajectory the JOB `job_arn` resolves to holds under `trajectory_id`, for
    /// writing.
    pub fn trajectory_mut(
        &mut self,
        job_arn: &str,
        trajectory_id: &str,
    ) -> Option<&mut Trajectory> {
        self.trajectories
            .get_mut(&trajectory_key(job_arn, trajectory_id))
    }

    /// How many bytes of payload every captured turn of this store retains.
    #[must_use]
    pub const fn retained_body_bytes(&self) -> usize {
        self.retained_body_bytes
    }

    /// Capture one turn on the trajectory `request` names, creating the
    /// trajectory when this is its first turn, and answer what was served for
    /// it.
    ///
    /// `respond` is handed the turn's index and answers the response body and
    /// its `Content-Type`; it runs BEFORE the turn is recorded, so what the
    /// caller is served and what the trajectory holds are the same bytes, and
    /// before the store-wide byte ceiling is checked, so the check can measure
    /// the whole turn it admits. It renders a body and writes nothing, so a turn
    /// refused after it ran leaves the store exactly as it was. The answer is
    /// the [`ServedTurn`] — the response alone, never a copy of the caller's own
    /// payload, which is retained exactly once.
    ///
    /// `respond` runs while the store this method writes is LOCKED (the guard
    /// [`serve_sampling_turn`] takes), so it renders its bytes from what it was
    /// handed and must neither reach this store again ([`service_state`]) nor
    /// block.
    ///
    /// Refused — with nothing written — when the trajectory is already sealed
    /// ("no further turns will be appended to the trajectory") or when an
    /// emulator bound is reached, including the two BYTE bounds
    /// ([`MAX_TURN_BODY_BYTES`], [`MAX_RETAINED_BODY_BYTES`]) that keep the
    /// store's retained size from being the caller's choice.
    pub fn append_turn<F>(
        &mut self,
        request: &TurnRequest<'_>,
        respond: F,
    ) -> Result<ServedTurn, AppendRefusal>
    where
        F: FnOnce(usize) -> (Vec<u8>, String),
    {
        // The job a call names is the job its caller is running under, created in the
        // SageMaker control plane and resolved by the handler against the shared job
        // directory before it gets here (`crate::state::job_exists`): the trajectory is
        // keyed by the JOB that resolution identifies and the request's trajectory id
        // (`trajectory_key`), and created on that pair's first turn.
        let key = trajectory_key(request.job_arn, request.trajectory_id);
        if !self.trajectories.contains_key(&key) && self.trajectories.len() >= MAX_TRAJECTORIES {
            return Err(AppendRefusal::TrajectoryQuota);
        }
        if let Some(existing) = self.trajectories.get(&key) {
            if existing.status != TrajectoryStatus::InProgress {
                return Err(AppendRefusal::Sealed {
                    status: existing.status,
                    completion_status: existing.completion_status,
                });
            }
            if existing.turns.len() >= MAX_TURNS_PER_TRAJECTORY {
                return Err(AppendRefusal::TurnQuota);
            }
        }
        // The caller's payload is retained for the life of the process, so its
        // SIZE is bounded before anything is written: one body that is too big
        // to capture, and a turn that would pass the store-wide retained total,
        // are both refused here.
        let request_bytes = request.request_body.len();
        if request_bytes > MAX_TURN_BODY_BYTES {
            return Err(AppendRefusal::BodyTooLarge {
                bytes: request_bytes,
            });
        }
        // The position the turn would take is what `respond` is handed, and it is
        // known before anything is written — the turns the trajectory already holds —
        // so the response half is rendered, and MEASURED, before the turn is admitted.
        let index = self
            .trajectories
            .get(&key)
            .map_or(0, |trajectory| trajectory.turns.len());
        let (response_body, response_content_type) = respond(index);
        // A captured turn retains BOTH halves, and both are what the counter below is
        // advanced by, so both are what the ceiling is measured against: a turn whose
        // request half alone would fit is still refused when the pair would pass the
        // ceiling, and nothing is written for it.
        let turn_bytes = request_bytes.saturating_add(response_body.len());
        let retained_before = self.retained_body_bytes;
        if retained_before.saturating_add(turn_bytes) > MAX_RETAINED_BODY_BYTES {
            return Err(AppendRefusal::RetainedBytesQuota {
                bytes: turn_bytes,
                retained: retained_before,
            });
        }
        let served = ServedTurn {
            body: response_body.clone(),
            content_type: response_content_type.clone(),
            index,
        };
        let trajectory = self.trajectories.entry(key).or_insert_with(|| {
            Trajectory::new(request.job_arn, request.trajectory_id, request.at)
        });
        trajectory.turns.push(Turn {
            request_body: request.request_body.to_vec(),
            request_content_type: request.request_content_type(),
            response_body,
            response_content_type: Some(response_content_type),
            streamed: request.streamed,
            index,
            at: request.at,
        });
        trajectory.updated_at = request.at;
        self.retained_body_bytes = retained_before.saturating_add(turn_bytes);
        Ok(served)
    }
}

/// What one sampling call brings to a trajectory: which trajectory it is for,
/// the prompt it carries and the headers it labelled that prompt with, which
/// sampling operation served it, and when.
#[derive(Debug, Clone)]
pub struct TurnRequest<'a> {
    /// The job the rollout belongs to.
    pub job_arn: &'a str,
    /// The trajectory the turn is appended to.
    pub trajectory_id: &'a str,
    /// The raw inference request body.
    pub request_body: &'a [u8],
    /// The headers the sampling call arrived with, which is where everything a
    /// captured turn records about HOW the caller labelled its payload is read
    /// from ([`TurnRequest::request_content_type`]).
    pub headers: &'a ::http::HeaderMap,
    /// Whether `SampleWithResponseStream`, rather than `Sample`, serves it.
    pub streamed: bool,
    /// The instant the turn is served at.
    pub at: SystemTime,
}

impl TurnRequest<'_> {
    /// The `Content-Type` the caller labelled the request body with, when it
    /// sent one a header value can be read as text.
    ///
    /// Read HERE rather than at either handler: what a captured turn records
    /// about the request is the same for both sampling operations, so the read
    /// belongs to the one sampling path they share ([`serve_sampling_turn`]) and
    /// neither of them can drift from the other on how a turn of one trajectory
    /// labels its prompt.
    #[must_use]
    pub fn request_content_type(&self) -> Option<String> {
        self.headers
            .get(::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

/// What one captured turn served its caller: the response body and its
/// `Content-Type`, plus the turn's position in the trajectory.
///
/// The answer of
/// [`ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState::append_turn`].
/// Deliberately NOT the whole [`Turn`]: the caller's own payload is retained
/// once, in the store, and never copied back out into a response value.
#[derive(Debug, Clone)]
pub struct ServedTurn {
    /// The response body the turn was served with, byte for byte what the store
    /// holds for it.
    pub body: Vec<u8>,
    /// The `Content-Type` that body is served with.
    pub content_type: String,
    /// The turn's zero-based position in the trajectory.
    pub index: usize,
}

/// Why [`ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState::append_turn`]
/// refused to capture a turn. Each sampling operation renders this as one of its
/// OWN modelled errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendRefusal {
    /// `CompleteRollout` has already sealed the trajectory.
    Sealed {
        /// The lifecycle status it is sealed in.
        status: TrajectoryStatus,
        /// The terminal status it was sealed with.
        completion_status: Option<CompletionStatus>,
    },
    /// The payload is larger than [`MAX_TURN_BODY_BYTES`], so no single turn can
    /// retain it.
    BodyTooLarge {
        /// How many bytes the request carried.
        bytes: usize,
    },
    /// The trajectory already holds [`MAX_TURNS_PER_TRAJECTORY`] turns.
    TurnQuota,
    /// The store already holds [`MAX_TRAJECTORIES`] trajectories.
    TrajectoryQuota,
    /// Capturing the turn would pass [`MAX_RETAINED_BODY_BYTES`], the store-wide
    /// ceiling on retained payload.
    RetainedBytesQuota {
        /// How many bytes capturing the turn would retain, summed over both of its
        /// halves — the quantity the ceiling is measured against.
        bytes: usize,
        /// How many bytes the store already retains for the turns it holds.
        retained: usize,
    },
}

impl AppendRefusal {
    /// The sentence this refusal is reported with, naming the trajectory and
    /// what about its state stops the turn being captured.
    #[must_use]
    pub fn message(self, trajectory_id: &str) -> String {
        match self {
            Self::Sealed {
                status,
                completion_status,
            } => {
                let sealed_with = completion_status.map_or_else(
                    || String::from("completed"),
                    |status| format!("completed with Status {}", status.as_str()),
                );
                format!(
                    "Trajectory {trajectory_id} is {sealed_with} and is in {} status; no further \
                     turns can be appended to a completed trajectory.",
                    status.as_str()
                )
            }
            Self::BodyTooLarge { bytes } => format!(
                "Body carries {bytes} byte(s), more than the {MAX_TURN_BODY_BYTES} bytes one \
                 captured turn of trajectory {trajectory_id} can hold."
            ),
            Self::TurnQuota => format!(
                "Trajectory {trajectory_id} already holds the maximum of \
                 {MAX_TURNS_PER_TRAJECTORY} turns."
            ),
            Self::TrajectoryQuota => format!(
                "This emulator already holds the maximum of {MAX_TRAJECTORIES} trajectories, so \
                 trajectory {trajectory_id} cannot be created."
            ),
            Self::RetainedBytesQuota { bytes, retained } => format!(
                "Capturing a turn of {bytes} byte(s) would take this emulator past the maximum of \
                 {MAX_RETAINED_BODY_BYTES} bytes of captured payload it retains, of which \
                 {retained} byte(s) are already retained, so no further turn can be appended to \
                 trajectory {trajectory_id}."
            ),
        }
    }

    /// Whether the refusal is a quota being reached — reported as the modelled
    /// `ServiceQuotaExceededException` — rather than a request that conflicts
    /// with the trajectory's state or carries too much to capture.
    ///
    /// `BodyTooLarge` is deliberately NOT one: it is a property of the request the
    /// caller sent, so it is refused as invalid rather than as a quota.
    #[must_use]
    pub const fn is_quota(self) -> bool {
        matches!(
            self,
            Self::TurnQuota | Self::TrajectoryQuota | Self::RetainedBytesQuota { .. }
        )
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState>)
}

/// The SHARED job directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A job is created in the SageMaker control plane and this is its data plane: a request
/// here names a job by ARN in a header and carries no job definition, and this model
/// binds no operation that creates one. So the directory the control plane writes when it
/// mints a job ([`::service_common::agentic_rft_jobs::JobDirectory`]) is the only place a
/// `JobArn` can be resolved against. The engine keys state by TYPE, so a copy of its own
/// inside this service's state would be invisible to the writer.
///
/// `None` only when this service is mounted on an engine that holds no state at all,
/// which is the same condition [`service_state`] answers `None` for: a handler answers it
/// with a modelled server error, never with a fresh engine.
pub fn job_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::agentic_rft_jobs::JobDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::agentic_rft_jobs::JobDirectory>)
}

/// Whether `job_arn` names a job the SageMaker control plane created.
///
/// This is the ONE job resolution this data plane performs, so no two of its operations
/// can disagree about whether an ARN denotes a job: a job exists only because a create
/// minted one, and this API has no call that creates one. An engine holding no directory
/// at all holds no job either, so nothing resolves.
pub fn job_exists<Eng: Send + Sync + 'static>(engine: &Eng, job_arn: &str) -> bool {
    job_directory(engine)
        .is_some_and(|directory| directory.lock().get_by_arn(job_arn).is_some())
}

/// How this service reports a `JobArn` that names no job of this account.
///
/// Built from the request's own `JobArn`: the ARN the caller asked to be routed to is
/// what the sentence names, so the refusal says which value could not be resolved.
#[must_use]
pub fn job_not_found_message(job_arn: &str) -> String {
    format!("Could not find job with jobArn {job_arn}")
}

/// The not-found sentence a trajectory-lifecycle call is refused with when the `JobArn`
/// it carries names no job, or `None` when the job resolves.
///
/// A trajectory exists only inside the job its key names: a sampling call of that job is
/// what creates it, and nothing of it outlives the job, so a `JobArn` [`job_exists`]
/// resolves nothing for names no trajectory of that job either, whatever this store still
/// holds under the pair — the pair is reported absent with the same sentence an id that
/// was never sampled gets ([`trajectory_not_found_message`]).
///
/// The job half of the rollout pair's resolution, which both of them reach through
/// [`serve_rollout_trajectory`], so neither can disagree with the sampling path about
/// whether an ARN denotes a job.
#[must_use]
pub fn unresolved_job_trajectory_message<Eng: Send + Sync + 'static>(
    engine: &Eng,
    job_arn: &str,
    trajectory_id: &str,
) -> Option<String> {
    if job_exists(engine, job_arn) {
        None
    } else {
        Some(trajectory_not_found_message(trajectory_id))
    }
}

// ---------------------------------------------------------------------------
// The rollout-lifecycle call, once
// ---------------------------------------------------------------------------

/// Resolve the `(JobArn, TrajectoryId)` pair one rollout-lifecycle call carries and hand
/// the stored trajectory to `apply`, or answer the sentence the call is refused with.
///
/// `CompleteRollout` and `UpdateReward` reach the SAME state through this, so neither can
/// drift from the other on which calls are refused, in which order, or with which
/// sentence: the `JobArn` is resolved against the jobs the SageMaker control plane minted
/// ([`unresolved_job_trajectory_message`]) BEFORE the store is locked, because a trajectory
/// lives inside the job its key names, and only then is the pair looked up in this
/// service's own map, a miss being reported with the sentence an id that was never sampled
/// gets ([`trajectory_not_found_message`]). What is left to each of them is what genuinely
/// differs: the modelled error constructor the sentence is reported through, the request
/// checks that operation owns, and what it does to the trajectory once it has it.
///
/// `apply` is handed the stored trajectory for writing and the instant the call is served
/// at, so both operations stamp one clock reading taken before the lock, and answers
/// whatever that operation owes its caller — including its own refusal, which is why the
/// answer is nested: the outer `Err` is the shared resolution's sentence, the inner value
/// is the operation's own.
pub fn serve_rollout_trajectory<Eng, T, F>(
    engine: &Eng,
    state: &::engine::ServiceState<ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState>,
    job_arn: &str,
    trajectory_id: &str,
    apply: F,
) -> Result<T, String>
where
    Eng: Send + Sync + 'static,
    F: FnOnce(&mut Trajectory, SystemTime) -> T,
{
    if let Some(message) = unresolved_job_trajectory_message(engine, job_arn, trajectory_id) {
        return Err(message);
    }
    let now = SystemTime::now();
    let mut guard = state.lock();
    let Some(trajectory) = guard.trajectory_mut(job_arn, trajectory_id) else {
        return Err(trajectory_not_found_message(trajectory_id));
    };
    Ok(apply(trajectory, now))
}

// ---------------------------------------------------------------------------
// The sampling call, once
// ---------------------------------------------------------------------------

/// How a sampling call whose `Body` carries no inference request is reported.
///
/// `Body` is `@required` and `@httpPayload`, so a request without one is already
/// refused at the boundary; a request WITH an empty payload gets this far and
/// carries no inference request to capture.
pub const EMPTY_BODY_MESSAGE: &str = "Body must carry the inference request payload; it was empty.";

/// Why a sampling call was not served, in the only terms the two sampling
/// operations differ on when they report it: whether a quota was reached, and
/// the sentence to report.
///
/// Each operation renders this as one of its OWN modelled errors — a quota as
/// its `ServiceQuotaExceededException`, anything else as its
/// `ValidationException` — so the decision lives here and only the error enum
/// is named at the handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingRefusal {
    message: String,
    quota: bool,
}

impl SamplingRefusal {
    /// A refusal of the request itself, or of what it conflicts with.
    fn invalid(message: String) -> Self {
        Self {
            message,
            quota: false,
        }
    }

    /// A refusal because a bound was reached.
    fn quota(message: String) -> Self {
        Self {
            message,
            quota: true,
        }
    }

    /// Whether a quota was reached, rather than the request being invalid or
    /// conflicting with the trajectory's state.
    #[must_use]
    pub const fn is_quota(&self) -> bool {
        self.quota
    }

    /// The sentence this refusal is reported with, consuming the refusal.
    #[must_use]
    pub fn into_message(self) -> String {
        self.message
    }
}

/// Serve one sampling call: the pre-flight and the capture that `Sample` and
/// `SampleWithResponseStream` share, answering what was served or why it was
/// refused.
///
/// The two operations reach the SAME state through this, so one trajectory can
/// mix turns served by either of them and neither can drift from the other on
/// which calls are refused, in which order, or with which sentence. What is
/// left to each of them is what genuinely differs: the response body it renders
/// (`respond`), the `Content-Type` that body carries, the `streamed` flag the
/// captured turn records, and the modelled error enum it reports a
/// [`SamplingRefusal`] through. Everything a captured turn records about the
/// REQUEST — including the `Content-Type` the caller labelled its payload with
/// ([`TurnRequest::request_content_type`]) — is derived here, from the request
/// this path is handed.
///
/// In order: a request carrying no inference request payload is refused; then
/// the `JobArn` is resolved, because it names the job the turn belongs to,
/// created in the SageMaker control plane — this service's model binds no
/// operation that creates, lists or deletes one, so the job is resolved against
/// the directory that control plane writes ([`job_exists`]). The member's
/// modelled constraints (`@length` 1..256 and the ARN `@pattern`) are already
/// enforced by the time a handler runs, and they admit any partition, region,
/// account and job name: an ARN no create ever minted is well formed and still
/// names no job, so the request cannot be routed to a model session and is
/// refused rather than served an inference. Only then is the turn captured, with
/// [`ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState::append_turn`]
/// refusing it — writing nothing — on the trajectory's own state and the
/// emulator's bounds.
pub fn serve_sampling_turn<Eng, F>(
    engine: &Eng,
    state: &::engine::ServiceState<ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState>,
    request: &TurnRequest<'_>,
    respond: F,
) -> Result<ServedTurn, SamplingRefusal>
where
    Eng: Send + Sync + 'static,
    F: FnOnce(usize) -> (Vec<u8>, String),
{
    if request.request_body.is_empty() {
        return Err(SamplingRefusal::invalid(EMPTY_BODY_MESSAGE.to_string()));
    }
    if !job_exists(engine, request.job_arn) {
        return Err(SamplingRefusal::invalid(job_not_found_message(
            request.job_arn,
        )));
    }
    state
        .lock()
        .append_turn(request, respond)
        .map_err(|refusal| {
            let message = refusal.message(request.trajectory_id);
            if refusal.is_quota() {
                SamplingRefusal::quota(message)
            } else {
                // Neither sampling operation models a `ConflictException`, so a
                // request that conflicts with the trajectory's sealed state is
                // refused with the client error they do model.
                SamplingRefusal::invalid(message)
            }
        })
}

// ---------------------------------------------------------------------------
// Inference bodies
// ---------------------------------------------------------------------------

/// The field names an OpenAI-compatible inference request body carries prompt
/// text under.
///
/// Source: the OpenAI API request bodies this service's `Body` member is
/// documented to be in ("The raw inference request body in OpenAI-compatible
/// JSON format") — chat completions bind their text to
/// `messages[].content` (a string, or `content[].text` blocks), the
/// completions endpoint to `prompt`, and the responses/embeddings endpoints to
/// `input`.
pub const PROMPT_TEXT_FIELDS: [&str; 4] = ["content", "prompt", "input", "text"];

/// How deep the prompt-text walk descends before it stops, so a deeply nested
/// request body cannot drive unbounded recursion.
const MAX_BODY_DEPTH: usize = 32;

/// Every string the body binds under a [`PROMPT_TEXT_FIELDS`] name, joined by
/// newlines in key order.
fn prompt_text_of_json(value: &serde_json::Value) -> String {
    fn walk(value: &serde_json::Value, field: Option<&str>, depth: usize, out: &mut Vec<String>) {
        if depth > MAX_BODY_DEPTH {
            return;
        }
        match value {
            serde_json::Value::String(text) => {
                if field.is_some_and(|name| PROMPT_TEXT_FIELDS.contains(&name)) {
                    out.push(text.clone());
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, field, depth + 1, out);
                }
            }
            serde_json::Value::Object(members) => {
                for (name, member) in members {
                    walk(member, Some(name.as_str()), depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let mut parts = Vec::new();
    walk(value, None, 0, &mut parts);
    parts.join("\n")
}

/// The prompt text an inference request body carries.
///
/// A JSON body contributes the strings it binds under a prompt-bearing field
/// name; a body that is not JSON, or that binds no such field, contributes its
/// own bytes as text, so no request is counted as zero tokens just because this
/// emulator does not know the field it used.
#[must_use]
pub fn prompt_text_of_body(body: &[u8]) -> String {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
        let text = prompt_text_of_json(&value);
        if !text.trim().is_empty() {
            return text;
        }
    }
    String::from_utf8_lossy(body).into_owned()
}

/// How many tokens `text` is, counted as whitespace-separated pieces.
///
/// An approximation this emulator computes itself — no tokenizer of any model
/// family runs here — so it is the same count for the same text on every call,
/// which is what makes a reported `usage` reproducible.
#[must_use]
pub fn count_text_tokens(text: &str) -> u64 {
    u64::try_from(text.split_whitespace().count()).unwrap_or(u64::MAX)
}

/// The opaque, stable id of one turn's completion.
///
/// Derived from the trajectory's identity — the JOB the call resolves to and the
/// trajectory id, the pair the turn is stored under ([`trajectory_key`]) — and the
/// turn's position, so the same turn always reports the same id however its call
/// addressed the job, and two different turns do not collide. It is not a random
/// value: nothing about it changes between reads.
#[must_use]
pub fn completion_id(job_arn: &str, trajectory_id: &str, index: usize) -> String {
    let (job, trajectory_id) = trajectory_key(job_arn, trajectory_id);
    let mut hasher = sha2::Sha256::new();
    hasher.update(job.as_bytes());
    hasher.update([0u8]);
    hasher.update(trajectory_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(index.to_be_bytes());
    format!("chatcmpl-{}", hex::encode(&hasher.finalize()[..12]))
}

/// The assistant text a turn of this emulator answers with.
///
/// No language model runs in this emulator, so this says so, and carries only
/// facts about the turn that was actually served: which turn of the trajectory
/// it is and how many tokens of prompt it carried. It is never the request's
/// own text echoed back as if it had been generated.
#[must_use]
pub fn emulated_turn_text(turn_number: usize, prompt_tokens: u64) -> String {
    format!(
        "No language model runs in this SageMaker AgenticRFT runtime emulator, so no text was \
         generated for turn {turn_number} of this trajectory. The prompt carried {prompt_tokens} \
         token(s) and the turn has been captured on the trajectory."
    )
}

/// The OpenAI-compatible chat-completion envelope for one captured turn.
///
/// Shape per the OpenAI chat-completions response object, which is the format
/// this service's `Body` members are documented to carry: `id`, `object`,
/// `created`, `model`, `choices[].message`, `choices[].finish_reason` and
/// `usage`.
///
/// Both of the per-job values the envelope carries — the turn's `id` and the
/// `model` that served it — are derived from the JOB the call resolves to
/// ([`job_identity`]), never from the ARN string the request addressed that job
/// through, so every retained turn of ONE trajectory reports one job, whichever
/// partition, Region or account its own call spelled the ARN with. No language
/// model runs here ([`emulated_turn_text`]), so the job the turn was routed to is
/// what `model` reports.
#[must_use]
pub fn completion_envelope(
    job_arn: &str,
    trajectory_id: &str,
    index: usize,
    prompt: &[u8],
    created: SystemTime,
    streaming: bool,
) -> serde_json::Value {
    let prompt_tokens = count_text_tokens(&prompt_text_of_body(prompt));
    let text = emulated_turn_text(index + 1, prompt_tokens);
    let completion_tokens = count_text_tokens(&text);
    let created_secs = created
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let message_key = if streaming { "delta" } else { "message" };
    serde_json::json!({
        "id": completion_id(job_arn, trajectory_id, index),
        "object": if streaming { "chat.completion.chunk" } else { "chat.completion" },
        "created": created_secs,
        "model": job_identity(job_arn),
        "choices": [{
            "index": 0,
            message_key: { "role": "assistant", "content": text },
            "finish_reason": "stop",
        }],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        },
    })
}

/// The non-streaming response body for one captured turn.
#[must_use]
pub fn sample_response_body(
    job_arn: &str,
    trajectory_id: &str,
    index: usize,
    prompt: &[u8],
    created: SystemTime,
) -> Vec<u8> {
    let envelope = completion_envelope(job_arn, trajectory_id, index, prompt, created, false);
    serde_json::to_vec(&envelope).unwrap_or_default()
}

/// The streamed response body for one captured turn: the same completion, in
/// the server-sent-event framing an OpenAI-compatible stream uses, terminated
/// by the `[DONE]` sentinel.
#[must_use]
pub fn sample_stream_response_body(
    job_arn: &str,
    trajectory_id: &str,
    index: usize,
    prompt: &[u8],
    created: SystemTime,
) -> Vec<u8> {
    let envelope = completion_envelope(job_arn, trajectory_id, index, prompt, created, true);
    let chunk = serde_json::to_string(&envelope).unwrap_or_default();
    format!("data: {chunk}\n\ndata: [DONE]\n\n").into_bytes()
}
