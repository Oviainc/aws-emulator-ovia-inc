// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pipes-pipes service.
//!
//! An EventBridge Pipes pipe is a CONFIGURATION record in this emulator: the control
//! plane (the record, its ARN, its lifecycle states and its timestamps) is real and
//! persisted here; nothing polls the source or delivers to the target.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    Pipe, PipeEnrichmentParameters, PipeLogConfiguration, PipeSourceParameters,
    PipeTargetParameters, ValidationException, ValidationExceptionField,
};

/// The `PipeState` members this slice reports. Source: the `PipeState` enum of
/// `smithy-model/com-amazonaws-pipes-pipes.json` (see `PIPE_STATE_VALUES`).
pub mod pipe_state {
    pub const CREATING: &str = "CREATING";
    pub const RUNNING: &str = "RUNNING";
    pub const STARTING: &str = "STARTING";
    pub const STOPPED: &str = "STOPPED";
    pub const STOPPING: &str = "STOPPING";
    pub const DELETING: &str = "DELETING";
    pub const UPDATING: &str = "UPDATING";
}

/// The `PipeState` values that mean "a move this pipe is already making has not landed
/// yet" — every state above that is not one a pipe rests in (`RUNNING`, `STOPPED`).
///
/// A pipe in one of these is mid-transition, so a call that would start ANOTHER
/// transition on it is refused rather than interleaved with the one in flight.
pub const TRANSIENT_PIPE_STATES: &[&str] = &[
    pipe_state::CREATING,
    pipe_state::STARTING,
    pipe_state::STOPPING,
    pipe_state::DELETING,
    pipe_state::UPDATING,
];

/// The states a caller may ask a pipe to be in (`RequestedPipeState`), plus the
/// `DELETED` that only a response reports (`RequestedPipeStateDescribeResponse`).
/// Source: those two enums in `smithy-model/com-amazonaws-pipes-pipes.json`.
pub mod requested_state {
    pub const RUNNING: &str = "RUNNING";
    pub const STOPPED: &str = "STOPPED";
    pub const DELETED: &str = "DELETED";
}

/// The `resourceType` this service names in a `ConflictException` / `NotFoundException`.
pub const PIPE_RESOURCE_TYPE: &str = "pipe";

/// The most pipe records this store holds, so a caller cannot grow the emulator's heap
/// without limit by creating distinct names.
///
/// This is the EMULATOR's bound, not an AWS number (AWS's own soft quota is larger): each
/// record is already size-bounded by the modelled `@length` constraints, so capping the
/// count bounds the store. `CreatePipe` refuses past it with the quota error its model
/// declares for exactly this case, `ServiceQuotaExceededException` (402).
pub const MAX_PIPES: usize = 100;

/// The most tags one pipe carries.
///
/// Source: the `TagResource` documentation in `smithy-model/com-amazonaws-pipes-pipes.json`
/// — "You can associate as many as 50 tags with a pipe." — the same bound as `TagMap`'s
/// `@length(max: 50)`. A single request past the bound is already refused by the boundary
/// validator; `TagResource` re-checks the bound against the tags ALREADY on the pipe, which
/// no per-request constraint can express.
pub const MAX_TAGS_PER_PIPE: usize = 50;

/// The length a tag key must be within: `@length(min: 1, max: 128)` on `TagKey`. Source:
/// the `TagMap` key member of `smithy-model/com-amazonaws-pipes-pipes.json`.
///
/// A constraint on a MAP KEY is not one the boundary validator checks (it walks the
/// collection's own `@length` and its values), so the writes that install tags enforce it
/// themselves — and refuse a key outside it with the `ValidationException` their models
/// declare, exactly as the boundary would have.
pub const TAG_KEY_LENGTH: std::ops::RangeInclusive<usize> = 1..=128;

/// The length a tag value must be within: `@length(min: 0, max: 256)` on `TagValue`.
/// Source: the `TagMap` value member of the same model. The empty string is therefore a
/// value a tag can hold, not a way of removing one.
pub const TAG_VALUE_LENGTH: std::ops::RangeInclusive<usize> = 0..=256;

/// The tag-key prefix AWS reserves for its own tags: "You cannot edit or delete tag keys
/// or values with this prefix. Tags with this prefix do not count against your tags per
/// resource limit." (AWS General Reference, "Tag naming limits and requirements"), so a
/// caller-supplied key carrying it is refused rather than stored. Matched
/// case-insensitively, the way the reservation is stated.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Why a tag the request carries is not one this service accepts, worded for the
/// `ValidationException` the writes that install tags answer with.
///
/// `@length` counts CHARACTERS, not bytes (the framework's own constraint checker counts
/// `chars()`), so a unicode key or value is measured the same way AWS measures it.
pub fn tag_violation(key: &str, value: &str) -> Option<String> {
    let key_length = key.chars().count();
    if !TAG_KEY_LENGTH.contains(&key_length) {
        return Some(format!(
            "tag key must have length between {} and {} characters; got {key_length}",
            TAG_KEY_LENGTH.start(),
            TAG_KEY_LENGTH.end(),
        ));
    }
    if key
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
    {
        return Some(format!(
            "tag key '{key}' uses the reserved prefix '{RESERVED_TAG_KEY_PREFIX}'",
        ));
    }
    let value_length = value.chars().count();
    if !TAG_VALUE_LENGTH.contains(&value_length) {
        return Some(format!(
            "the value of tag key '{key}' must have length between {} and {} characters; \
             got {value_length}",
            TAG_VALUE_LENGTH.start(),
            TAG_VALUE_LENGTH.end(),
        ));
    }
    None
}

/// The refusal a write that installs tags answers with when [`tag_violation`] names one:
/// the modelled `ValidationException`, carrying the violation as the `fieldList` entry for
/// `member` (the input member the tags arrived on, which the operations name differently)
/// and a message derived FROM that list, so the count it states cannot drift from it.
pub fn tag_violation_error(member: &str, violation: String) -> ValidationException {
    let field_list = vec![ValidationExceptionField {
        name: member.to_string(),
        message: violation,
    }];
    ValidationException {
        message: Some(format!(
            "{} validation error detected: {}",
            field_list.len(),
            field_list
                .iter()
                .map(|field| field.message.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        )),
        field_list: Some(field_list),
    }
}

/// The region an ARN carries when the request names none.
const DEFAULT_REGION: &str = "us-east-1";

/// How long after a lifecycle call a pipe settles out of its transient state
/// (`CREATING`, `STARTING`, `STOPPING`, `UPDATING`) into the state the caller asked for.
///
/// AWS does not publish this duration, so it is CHOSEN, not cited: long enough that a
/// caller acting straight after the call still observes the transient state, short
/// enough to be waited out. It is never zero and nothing ever sleeps for it — the due
/// instant is stored and the move is materialised by whichever later call reads the
/// pipe (see [`ComAmazonawsPipesPipesState::settle_due_transitions`]).
pub const STATE_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// A state move the clock owes a pipe: at `due_at` its `CurrentState` becomes `to`.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The monotonic instant the move became (or becomes) due.
    pub due_at: Instant,
    /// The `PipeState` the pipe settles into.
    pub to: String,
}

/// The move owed to the clock when a lifecycle call starts a transition into `to`.
pub fn pending_transition(to: &str) -> PendingTransition {
    PendingTransition {
        due_at: Instant::now() + STATE_SETTLE_DELAY,
        to: to.to_string(),
    }
}

/// One persisted pipe. Every member `CreatePipe` accepts is stored here and read back
/// by `DescribePipe`, so the record — not the request — is what every response is
/// computed from.
#[derive(Debug, Clone)]
pub struct PipeRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub desired_state: String,
    pub current_state: String,
    pub state_reason: Option<String>,
    pub source: String,
    pub source_parameters: Option<PipeSourceParameters>,
    pub enrichment: Option<String>,
    pub enrichment_parameters: Option<PipeEnrichmentParameters>,
    pub target: String,
    pub target_parameters: Option<PipeTargetParameters>,
    pub role_arn: String,
    /// Tags of this pipe, keyed by tag key. They live on the record (reachable from the
    /// pipe ARN) so the later `TagResource`/`UntagResource`/`ListTagsForResource` slice
    /// reads the same copy.
    pub tags: BTreeMap<String, String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub log_configuration: Option<PipeLogConfiguration>,
    pub kms_key_identifier: Option<String>,
    /// The move the clock owes this pipe, if any. See [`STATE_SETTLE_DELAY`].
    pub pending: Option<PendingTransition>,
}

impl PipeRecord {
    /// Put the pipe into `transient` now and owe it the move to `settled`.
    pub fn begin_transition(&mut self, transient: &str, settled: &str) {
        self.current_state = transient.to_string();
        self.pending = Some(pending_transition(settled));
    }

    /// Materialise the move the clock has ALREADY made due, if it has. A no-op while the
    /// move is still in the future, so a caller acting immediately still sees the
    /// transient state.
    pub fn settle(&mut self, now: Instant) {
        let settled = match &self.pending {
            Some(pending) if now >= pending.due_at => pending.to.clone(),
            _ => return,
        };
        self.current_state = settled;
        self.pending = None;
    }

    /// The transient state this pipe is in, or `None` when it is resting in a settled
    /// one. Call [`ComAmazonawsPipesPipesState::settle_due_transitions`] first, so a move
    /// the clock has already made due is not mistaken for one still in flight.
    pub fn transient_state(&self) -> Option<&str> {
        TRANSIENT_PIPE_STATES
            .iter()
            .find(|state| **state == self.current_state)
            .copied()
    }

    /// This record as the `Pipe` summary `ListPipes` returns.
    pub fn summary(&self) -> Pipe {
        Pipe {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            desired_state: Some(self.desired_state.clone()),
            current_state: Some(self.current_state.clone()),
            state_reason: self.state_reason.clone(),
            creation_time: Some(timestamp(self.creation_time)),
            last_modified_time: Some(timestamp(self.last_modified_time)),
            source: Some(self.source.clone()),
            target: Some(self.target.clone()),
            enrichment: self.enrichment.clone(),
        }
    }

    /// The stored tags as the modelled `TagMap`, or `None` when the pipe carries none:
    /// `TagMap` is `@length(min: 1)`, so an empty map is not a legal value for it.
    pub fn tag_map(&self) -> Option<std::collections::HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(
            self.tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }
}

/// Per-service state for com-amazonaws-pipes-pipes. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPipesPipesState {
    /// Every pipe this account holds, keyed by name. A `BTreeMap` so `ListPipes` and its
    /// `NextToken` paging enumerate in a single deterministic order.
    pub pipes: BTreeMap<String, PipeRecord>,
}

impl ComAmazonawsPipesPipesState {
    /// The stored pipe whose ARN is `arn`, if this account holds one.
    ///
    /// The tagging operations address a pipe by its `PipeArn` rather than by name, so the
    /// lookup is over the ARN each record minted at create time. A pipe that was never
    /// created — or that `DeletePipe` has removed — is simply absent here, which is the
    /// 404 those operations model.
    pub fn pipe_by_arn(&self, arn: &str) -> Option<&PipeRecord> {
        self.pipes.values().find(|pipe| pipe.arn == arn)
    }

    /// [`Self::pipe_by_arn`] for the operations that write the record's tags.
    pub fn pipe_by_arn_mut(&mut self, arn: &str) -> Option<&mut PipeRecord> {
        self.pipes.values_mut().find(|pipe| pipe.arn == arn)
    }

    /// Apply every state move the clock has already made due. Call this after locking,
    /// before reading any pipe's `CurrentState`: it writes only what was owed at this
    /// instant even if nobody had called, so a reporting operation may do it too.
    pub fn settle_due_transitions(&mut self) {
        let now = Instant::now();
        for pipe in self.pipes.values_mut() {
            pipe.settle(now);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPipesPipesState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPipesPipesState>)
}

/// The region this request is signed for, for the ARNs a handler mints.
///
/// Read from the SigV4 credential scope of the `Authorization` header (the same scope
/// the router already routes on); an unsigned request falls back to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the pipe named `name`: `arn:aws:pipes:<region>:<account>:pipe/<name>`.
pub fn pipe_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("pipes", region, &format!("pipe/{name}"))
}

/// A stored instant as the modelled `Timestamp`.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Encode the exclusive start key of the next `ListPipes` page as an opaque `NextToken`.
///
/// Hex rather than the bare name: `NextToken` is an opaque continuation token to its
/// caller, who must pass it back unchanged rather than build one.
pub fn encode_next_token(last_returned_name: &str) -> String {
    ::hex::encode(last_returned_name.as_bytes())
}

/// The exclusive start key a `NextToken` carries, or `None` when the token was not
/// produced by [`encode_next_token`].
pub fn decode_next_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}
