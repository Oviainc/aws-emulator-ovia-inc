// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice service.
//!
//! The service's primary resource is a **trajectory**: the ordered turns of one
//! rollout inside a SageMaker job, keyed by the pair `(JobArn, TrajectoryId)`
//! that every operation of this service carries. The lifecycle the model
//! documents is linear:
//!
//! * `Sample` / `SampleWithResponseStream` create the trajectory on its first
//!   turn and append a turn on every later call — "Each turn (prompt and
//!   response) is captured for later use".
//! * `CompleteRollout` seals it — "no further turns will be appended to the
//!   trajectory. After calling this operation, the trajectory is sealed and
//!   eligible for reward submission via the UpdateReward operation".
//! * `UpdateReward` records the reward scores and "transitions it to
//!   reward-received status".

use std::collections::{BTreeSet, HashMap};
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
    /// The job the rollout belongs to.
    pub job_arn: String,
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
        Self {
            job_arn: job_arn.to_string(),
            trajectory_id: trajectory_id.to_string(),
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

/// The key a trajectory is stored under: the job it belongs to and its id.
pub type TrajectoryKey = (String, String);

/// The region this emulator presents in the ARNs and endpoints it hands clients
/// (`vendor/server/src/lib.rs` writes `region = us-east-1` into the shared config).
const SEED_REGION: &str = "us-east-1";

/// The AgenticRFT jobs this runtime starts with.
///
/// This service models no operation that creates a job: a job is created in the
/// SageMaker control plane and this runtime only serves rollouts of one that already
/// exists, which is why `Sample` answers "Could not find job with jobArn …" for a job
/// ARN it does not hold. Without a seed the registry would be empty, so the runtime
/// could not decide that ANY job exists and no rollout would be reachable at all. The
/// ARNs below are therefore a fixture standing in for that control-plane creation — two
/// of them, so a caller can keep two rollouts apart — built through
/// [`::engine::emulator_arn`] so they carry this emulator's canonical account. A test
/// registers its own job with
/// [`ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState::register_job`], and
/// when the SageMaker emulator grows an AgentRFT job store this seed is what it
/// replaces.
#[must_use]
pub fn seed_job_arns() -> Vec<String> {
    vec![
        ::engine::emulator_arn("sagemaker", SEED_REGION, "job/AgentRFT/agentic-rft-rollout-1"),
        ::engine::emulator_arn("sagemaker", SEED_REGION, "job/AgentRFT/agentic-rft-rollout-2"),
    ]
}

/// How this service reports a `Sample` or `SampleWithResponseStream` whose `JobArn`
/// names no job this runtime holds.
///
/// Built from the request's own `JobArn`, so the caller can see which job was looked
/// up.
#[must_use]
pub fn job_not_found_message(job_arn: &str) -> String {
    format!("Could not find job with jobArn {job_arn}")
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
    /// The ARNs of the jobs this runtime serves rollouts for. A sampling call for an
    /// ARN that is not here is refused: the job does not exist.
    pub jobs: BTreeSet<String>,
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
            jobs: seed_job_arns().into_iter().collect(),
            trajectories: HashMap::new(),
            retained_body_bytes: 0,
        }
    }
}

impl ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState {
    /// Whether this runtime holds the job `job_arn` names.
    #[must_use]
    pub fn knows_job(&self, job_arn: &str) -> bool {
        self.jobs.contains(job_arn)
    }

    /// Record `job_arn` as a job this runtime serves rollouts for, answering whether it
    /// was newly added. Stands in for the control-plane creation this service models no
    /// operation for (see [`seed_job_arns`]).
    pub fn register_job(&mut self, job_arn: &str) -> bool {
        self.jobs.insert(job_arn.to_string())
    }

    /// The trajectory `job_arn` / `trajectory_id` names, if this emulator has
    /// one.
    #[must_use]
    pub fn trajectory(&self, job_arn: &str, trajectory_id: &str) -> Option<&Trajectory> {
        self.trajectories
            .get(&(job_arn.to_string(), trajectory_id.to_string()))
    }

    /// The trajectory `job_arn` / `trajectory_id` names, for writing.
    pub fn trajectory_mut(
        &mut self,
        job_arn: &str,
        trajectory_id: &str,
    ) -> Option<&mut Trajectory> {
        self.trajectories
            .get_mut(&(job_arn.to_string(), trajectory_id.to_string()))
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
    /// caller is served and what the trajectory holds are the same bytes. The
    /// answer is the [`ServedTurn`] — the response alone, never a copy of the
    /// caller's own payload, which is retained exactly once.
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
        // A rollout belongs to a job: the trajectory of a job this runtime does not
        // hold cannot be sampled, whether or not a turn was ever captured for it.
        if !self.knows_job(request.job_arn) {
            return Err(AppendRefusal::UnknownJob);
        }
        let key = (
            request.job_arn.to_string(),
            request.trajectory_id.to_string(),
        );
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
        // to capture, and a body that would pass the store-wide retained total,
        // are both refused here.
        let request_bytes = request.request_body.len();
        if request_bytes > MAX_TURN_BODY_BYTES {
            return Err(AppendRefusal::BodyTooLarge {
                bytes: request_bytes,
            });
        }
        if self.retained_body_bytes.saturating_add(request_bytes) > MAX_RETAINED_BODY_BYTES {
            return Err(AppendRefusal::RetainedBytesQuota);
        }
        let retained_before = self.retained_body_bytes;
        let trajectory = self.trajectories.entry(key).or_insert_with(|| {
            Trajectory::new(request.job_arn, request.trajectory_id, request.at)
        });
        let index = trajectory.turns.len();
        let (response_body, response_content_type) = respond(index);
        let served = ServedTurn {
            body: response_body.clone(),
            content_type: response_content_type.clone(),
            index,
        };
        let response_bytes = response_body.len();
        trajectory.turns.push(Turn {
            request_body: request.request_body.to_vec(),
            request_content_type: request.request_content_type.clone(),
            response_body,
            response_content_type: Some(response_content_type),
            streamed: request.streamed,
            index,
            at: request.at,
        });
        trajectory.updated_at = request.at;
        self.retained_body_bytes = retained_before
            .saturating_add(request_bytes)
            .saturating_add(response_bytes);
        Ok(served)
    }
}

/// What one sampling call brings to a trajectory: which trajectory it is for,
/// the prompt it carries and how it was labelled, which sampling operation
/// served it, and when.
#[derive(Debug, Clone)]
pub struct TurnRequest<'a> {
    /// The job the rollout belongs to.
    pub job_arn: &'a str,
    /// The trajectory the turn is appended to.
    pub trajectory_id: &'a str,
    /// The raw inference request body.
    pub request_body: &'a [u8],
    /// The `Content-Type` the caller labelled the request body with.
    pub request_content_type: Option<String>,
    /// Whether `SampleWithResponseStream`, rather than `Sample`, serves it.
    pub streamed: bool,
    /// The instant the turn is served at.
    pub at: SystemTime,
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
    /// The `JobArn` names no job this runtime holds.
    UnknownJob,
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
    RetainedBytesQuota,
}

impl AppendRefusal {
    /// The sentence this refusal is reported with, naming the trajectory and
    /// what about its state stops the turn being captured.
    #[must_use]
    pub fn message(self, job_arn: &str, trajectory_id: &str) -> String {
        match self {
            Self::UnknownJob => job_not_found_message(job_arn),
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
            Self::RetainedBytesQuota => format!(
                "This emulator already retains the maximum of {MAX_RETAINED_BODY_BYTES} bytes of \
                 captured payload, so no further turn can be appended to trajectory \
                 {trajectory_id}."
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
            Self::TurnQuota | Self::TrajectoryQuota | Self::RetainedBytesQuota
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
/// Derived from the trajectory's identity and the turn's position, so the same
/// turn always reports the same id and two different turns do not collide. It
/// is not a random value: nothing about it changes between reads.
#[must_use]
pub fn completion_id(job_arn: &str, trajectory_id: &str, index: usize) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(job_arn.as_bytes());
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
        "model": job_arn,
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
