// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakera2iruntime-amazonsagemakera2iruntime service.
//!
//! The service's one resource is the *human loop*: `StartHumanLoop` creates it,
//! `DescribeHumanLoop` / `ListHumanLoops` report it, `StopHumanLoop` moves its status and
//! `DeleteHumanLoop` removes it. Every handler reads and writes the single
//! [`human_loops`](ComAmazonawsSagemakera2iruntimeAmazonsagemakera2iruntimeState::human_loops)
//! map through [`service_state`], so one request's write is the next request's read.
//!
//! This module also owns the helpers more than one handler needs: the status enum and its
//! wire spelling, the clock-owed status transition, the epoch conversions the `date-time`
//! members are built from, and the opaque `ListHumanLoops` pagination token.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The region an ARN carries when the request is unsigned, so no SigV4 credential scope
/// names one. Every AWS SDK signs its requests, so this is the anonymous-request fallback.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a human loop reports `Stopping` before it settles on `Stopped`.
///
/// `StopHumanLoop` starts an asynchronous stop: A2I moves the loop to `Stopping` and then to
/// `Stopped`. AWS documents no duration for it, so this span is CHOSEN — the project's
/// 250-750 ms window for a clock-owed transition — and it is deliberately non-zero so a
/// caller that describes the loop immediately after stopping it still observes `Stopping`.
/// The due instant is stored on the record; nothing sleeps for it.
pub const STOP_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The largest page `ListHumanLoops` serves, and the page size it uses when the request
/// names no `MaxResults`: `ListHumanLoopsRequest$MaxResults` is modelled
/// `@range(min: 1, max: 100)`, so a request that omits it is served one bounded page plus a
/// `NextToken` rather than the whole store.
pub const HUMAN_LOOP_PAGE_LIMIT: usize = 100;

/// How many human loops the store holds before `StartHumanLoop` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// Amazon A2I publishes an adjustable quota on human loops, and this emulator models neither
/// Service Quotas nor an account's adjustments — so this is the emulator's OWN bound on the
/// store rather than a claim about any account's limit. It is set far above any workflow an
/// emulator serves, and it is what stops the state growing without end: every record it
/// holds is released by `DeleteHumanLoop`.
pub const HUMAN_LOOP_QUOTA: usize = 1_000;

/// The lifecycle status of a human loop.
///
/// The variants are exactly the `HumanLoopStatus` enum of the Smithy model (see
/// `crate::ops::types::HUMAN_LOOP_STATUS_VALUES`), and [`HumanLoopStatus::as_wire`] is the
/// spelling the model gives each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanLoopStatus {
    /// The loop is open and waiting on a human worker.
    InProgress,
    /// A stop was requested and has not settled yet.
    Stopping,
    /// The loop was stopped before a human completed it.
    Stopped,
    /// A human completed the loop.
    Completed,
    /// The loop failed; `failure_reason` / `failure_code` say why.
    Failed,
}

impl HumanLoopStatus {
    /// Every status, so a caller can enumerate the enum without naming the variants twice.
    pub const ALL: [Self; 5] = [
        Self::InProgress,
        Self::Stopping,
        Self::Stopped,
        Self::Completed,
        Self::Failed,
    ];

    /// The value the model gives this status on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "InProgress",
            Self::Stopping => "Stopping",
            Self::Stopped => "Stopped",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }

    /// Whether the loop can still change status on its own. `Stopping` is NOT terminal: the
    /// clock still owes it a move to `Stopped`.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Completed | Self::Failed)
    }
}

/// One persisted human loop: everything `StartHumanLoop` was given, plus the lifecycle the
/// other four operations read and move.
#[derive(Debug, Clone)]
pub struct HumanLoop {
    /// `HumanLoopName` — the map key as well, since A2I keys a loop by name.
    pub name: String,
    /// `HumanLoopArn`, minted once at creation and never recomputed.
    pub arn: String,
    /// `FlowDefinitionArn` the loop was started against; `ListHumanLoops` filters on it.
    pub flow_definition_arn: String,
    /// The SHA-256 digest of `HumanLoopInput.InputContent`, as
    /// [`input_content_digest`] computes it.
    ///
    /// The digest, not the content: no operation this service models returns the input
    /// content, and comparing digests is all the `ConflictException` check needs, so a
    /// record cannot be made to retain the megabytes one request may carry.
    pub input_digest: String,
    /// `DataAttributes.ContentClassifiers` as supplied; empty when the member was absent.
    pub content_classifiers: Vec<String>,
    /// `HumanLoopStatus`.
    pub status: HumanLoopStatus,
    /// `CreationTime`.
    pub creation_time: SystemTime,
    /// The status transition the clock owes this loop, and the instant it comes due.
    pub pending_status: Option<(SystemTime, HumanLoopStatus)>,
    /// `FailureReason`, set only when the loop failed.
    pub failure_reason: Option<String>,
    /// `FailureCode`, set only when the loop failed.
    pub failure_code: Option<String>,
    /// `HumanLoopOutput.OutputS3Uri`: the S3 object a completed human review is written to.
    /// Stays `None` while no human review can run in this emulator, so `DescribeHumanLoop`
    /// reports no output for work nothing performed.
    pub output_s3_uri: Option<String>,
}

impl HumanLoop {
    /// Apply the pending status transition if `now` has made it due.
    ///
    /// This is the only write a reporting operation performs: it materialises a change the
    /// clock already owed, which would have happened at the same instant even if nobody had
    /// called.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some((due, next)) = self.pending_status {
            if now >= due {
                self.status = next;
                self.pending_status = None;
            }
        }
    }

    /// The `ListHumanLoops` sort key: creation time first (the member A2I sorts on), then the
    /// name, so loops created in the same millisecond still have one total order and a
    /// pagination token names exactly one position.
    pub fn sort_key(&self) -> (i64, String) {
        (epoch_millis(self.creation_time), self.name.clone())
    }
}

/// Per-service state for com-amazonaws-sagemakera2iruntime-amazonsagemakera2iruntime. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakera2iruntimeAmazonsagemakera2iruntimeState {
    /// Every live human loop, keyed by `HumanLoopName`. `DeleteHumanLoop` removes the entry,
    /// which is what frees the name and makes a later `DescribeHumanLoop` a 404. The map is
    /// bounded by [`HUMAN_LOOP_QUOTA`], which `StartHumanLoop` checks before it inserts.
    pub human_loops: BTreeMap<String, HumanLoop>,
}

impl ComAmazonawsSagemakera2iruntimeAmazonsagemakera2iruntimeState {
    /// Whether the store is full, i.e. holds [`HUMAN_LOOP_QUOTA`] records. `StartHumanLoop`
    /// asks before it inserts a new name, which is the one site that grows the store.
    pub fn is_full(&self) -> bool {
        self.human_loops.len() >= HUMAN_LOOP_QUOTA
    }
}

/// The digest a record keeps of one `HumanLoopInput.InputContent`.
///
/// SHA-256, hex-encoded: two requests carrying the same input content produce the same
/// digest, which is what `StartHumanLoop` compares, and no amount of input is retained.
pub fn input_content_digest(input_content: &str) -> String {
    use ::sha2::Digest as _;
    ::hex::encode(::sha2::Sha256::digest(input_content.as_bytes()))
}

/// Milliseconds since the Unix epoch, negative before it. The unit the sort key and the
/// pagination token use, and the resolution `date-time` members carry on the wire.
pub fn epoch_millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis())
            .map(|millis| -millis)
            .unwrap_or(i64::MIN),
    }
}

/// A `SystemTime` as the modelled `date-time` timestamp a response carries.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    #[allow(clippy::cast_precision_loss)]
    ::aws_smithy_types::DateTime::from_secs_f64(epoch_millis(time) as f64 / 1000.0)
}

/// A request's `date-time` timestamp in the same unit as [`epoch_millis`], so a filter bound
/// and a stored creation time are comparable.
///
/// Rounded to the nearest millisecond rather than truncated: a `date-time` on the wire and
/// the `f64` seconds a JSON body carries both land within a few hundred nanoseconds of the
/// millisecond they encode, so truncating would put a bound taken from a response's own
/// timestamp one millisecond below it and drop the record it names.
pub fn date_time_millis(timestamp: &::aws_smithy_types::DateTime) -> i64 {
    let subsec_millis = (i64::from(timestamp.subsec_nanos()) + 500_000) / 1_000_000;
    timestamp
        .secs()
        .saturating_mul(1000)
        .saturating_add(subsec_millis)
}

/// The separator inside a pagination token's decoded form. A unit separator cannot occur in a
/// `HumanLoopName` (the model's pattern admits only `[a-z0-9-]`), so the split is unambiguous.
const PAGE_TOKEN_SEPARATOR: char = '\u{1f}';

/// Encode the sort key of the last summary already returned as an opaque `NextToken`.
///
/// The token carries the position, not a server-side cursor, so pages stay stable with no
/// extra state and a token stays resolvable after the loop it names is deleted.
pub fn encode_page_token(key: &(i64, String)) -> String {
    ::hex::encode(format!("{}{PAGE_TOKEN_SEPARATOR}{}", key.0, key.1))
}

/// Decode a `NextToken` produced by [`encode_page_token`]. `None` when the token is not one
/// this service issued, which the handler reports as a `ValidationException`.
pub fn decode_page_token(token: &str) -> Option<(i64, String)> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (millis, name) = decoded.split_once(PAGE_TOKEN_SEPARATOR)?;
    Some((millis.parse().ok()?, name.to_string()))
}

/// The region this request was signed for, used to mint a resource's ARN.
///
/// Read from the SigV4 credential scope — the region the caller addressed — and falls back to
/// [`DEFAULT_REGION`] for an unsigned request or a scope whose region could not appear in an
/// ARN (the model's ARN patterns admit only `[a-z0-9-]`).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakera2iruntimeAmazonsagemakera2iruntimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakera2iruntimeAmazonsagemakera2iruntimeState>)
}
