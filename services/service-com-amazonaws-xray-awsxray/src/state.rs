// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-xray-awsxray service.
//!
//! X-Ray's ingest -> query -> read loop is entirely state-derived:
//! `PutTraceSegments` stores each accepted segment document VERBATIM under its
//! trace, and every reader (`BatchGetTraces`, `GetTraceSummaries`) recomputes
//! its answer from those stored documents — nothing is echoed from the request
//! that stored them. Groups are a keyed map plus an ARN index, so `GetGroup`
//! resolves a group by either selector; the account-level `Default` group is
//! DERIVED rather than stored ([`default_group`]), because it exists in every
//! region without being created and cannot be created or deleted. Sampling
//! rules are a map keyed by `RuleName`, holding what `CreateSamplingRule`
//! stored; `GetSamplingRules` lists that map and `DeleteSamplingRule` removes
//! from it, by either selector. The account's service-managed
//! [`DEFAULT_SAMPLING_RULE_NAME`] rule is DERIVED rather than stored
//! ([`default_sampling_rule`]), exactly as the `Default` group is, so a fresh
//! store already lists it; only an `UpdateSamplingRule` of it ever stores a
//! record under that name, and no delete may remove it. Indexing
//! rules are a fixed, seeded keyed map: the account's `Default` rule exists
//! before the first call, and `UpdateIndexingRule` rewrites its configuration in
//! place for `GetIndexingRules` to report.
//!
//! [`filter_expression`] is the `FilterExpression` grammar `CreateGroup` and
//! `UpdateGroup` validate against before either writes anything.
//!
//! A group's tags are keyed by tag key ON its record, so `CreateGroup`,
//! `TagResource`, `UntagResource` and `ListTagsForResource` read and write ONE
//! set per resource rather than a tag store beside it, and the restrictions the
//! model documents for a tag are written down once, in
//! [`validate_tag`]/[`validate_tag_key`].
//!
//! The segment-document schema this module parses is the one `PutTraceSegments`
//! documents (`smithy_query(query="doc",shape="PutTraceSegments")`): `name`,
//! `id`, `trace_id` and `start_time` are required, `end_time`/`in_progress` say
//! whether the segment is complete, and `error`/`fault`/`throttle`, `http`,
//! `parent_id`, `annotations`, `cause` and `subsegments` are optional. Two more
//! members come from the X-Ray Segment Documents reference that
//! `PutTraceSegments` links to, because the service graph is derived from them:
//! `type`, which is `subsegment` on a subsegment sent as its own document, and
//! `namespace`, which is `aws` for an AWS SDK call and `remote` for any other
//! call that left the process.
//!
//! Two things a reader reports are DERIVED from those documents rather than
//! carried by them, and both are derived here so that `BatchGetTraces` and
//! `GetTraceSummaries` cannot disagree:
//!
//! * a subsegment that names a downstream service (`namespace: "remote"`) gets
//!   an INFERRED top-level segment synthesised for that downstream service
//!   ([`inferred_segments`]), which is the service X-Ray attributes the call to;
//! * every entity (a segment, or a subsegment inside one) records an
//!   [`Outcome`], read from its own `fault`/`error`/`throttle` fields and
//!   `http.response.status`: the one [`Classification`] a COUNTER splits it
//!   into (a fault outranks a throttle, which outranks an error), and whether it
//!   recorded a 400-series error at all — which a throttle's own 429 status also
//!   is, because the model's `HasError` ("a 400 series error") and `HasThrottle`
//!   ("a 429 throttling error") are not exclusive. The trace's
//!   `HasFault`/`HasError`/`HasThrottle` flags and its root causes are read off
//!   those outcomes.
//!
//! Daemon telemetry (`PutTelemetryRecords`, "used by the Amazon Web Services
//! X-Ray daemon to upload telemetry") is kept the same way: each submitted
//! `TelemetryRecord` is stored VERBATIM under the reporting source the request
//! identifies, beside the running totals accumulated over every record that
//! source ever had accepted — including records that have since fallen out of
//! its [`MAX_TELEMETRY_RECORDS_PER_SOURCE`] window. No modelled operation reads
//! telemetry back — `PutTelemetryRecordsResult` carries no members and X-Ray
//! publishes no telemetry reader — so the store is what the ingest asserts, and
//! it is bounded like the trace store is.
//!
//! A trace retrieval is the same loop over a longer clock: `StartTraceRetrieval`
//! stores one [`TraceRetrievalRecord`] per accepted call under the token it
//! minted, and every reader of that token (`ListRetrievedTraces`,
//! `GetRetrievedTracesGraph`) recomputes BOTH its `RetrievalStatus` — from the
//! instants stored on the record, through
//! [`TraceRetrievalRecord::status`] — and its traces — from the stored segment
//! documents of the requested ids inside the requested window, through
//! [`ComAmazonawsXrayAwsxrayState::retrieved_traces`]. `CancelTraceRetrieval` is
//! the one call that moves a retrieval's status by hand.
//!
//! Every instant a segment document carries is checked where the document is
//! read ([`MAX_SEGMENT_EPOCH_SECONDS`]), so no stored `start_time` / `end_time`
//! can be a value a reader cannot convert to the modelled `Timestamp`; the
//! conversion itself goes through [`timestamp_of`].

pub mod filter;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use crate::ops::types::{
    Edge, EdgeStatistics, EncryptionConfig, ErrorStatistics, FaultStatistics, Group, GroupSummary,
    HistogramEntry, IndexingRule, IndexingRuleValue, Insight, InsightsConfiguration,
    ProbabilisticRuleValue, SamplingRateBoost, SamplingRule, SamplingRuleRecord, Service,
    ServiceStatistics, TelemetryRecord,
};

/// The region an ARN carries when the request's SigV4 scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, from the model's `awsService.arnNamespace`
/// (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "xray";

/// `UnprocessedTraceSegment$ErrorCode` for a document that cannot be read as a
/// JSON segment object at all.
///
/// The model documents `ErrorCode` only as "The error that caused processing to
/// fail" and publishes no value set, so the VALUES are X-Ray's own, as its
/// recorded `PutTraceSegments` responses state them: a document that cannot be
/// read is `ParseError`, and a required member that is absent is `Missing` plus
/// that member's name in PascalCase (`MissingName` for an absent `name`). A
/// member that is present but unusable — wrong JSON type, or a `trace_id` that
/// is not a well-formed trace id — follows the same construction as `Invalid`
/// plus the member name, and a document refused because its trace is full takes
/// [`SEGMENT_ERROR_LIMIT_EXCEEDED`]. The `Message` beside the code RESTATES it,
/// which is the one wording X-Ray uses for every per-document rejection:
/// `Invalid segment. ErrorCode: <ErrorCode>`. So the code is where the fault is
/// named, and [`segment_member_code`] is the single place a member's code is
/// built.
pub const SEGMENT_ERROR_PARSE: &str = "ParseError";

// ---------------------------------------------------------------------------
// Encryption configuration
//
// `PutEncryptionConfig` replaces the whole document and `GetEncryptionConfig`
// reports what is stored, so both read one record ([`EncryptionConfigRecord`])
// rather than keeping a copy each.
// ---------------------------------------------------------------------------

/// `EncryptionConfig$Type` for encryption with a caller-selected KMS key, from
/// [`crate::ops::types::ENCRYPTION_TYPE_VALUES`].
pub const ENCRYPTION_TYPE_KMS: &str = "KMS";

/// `EncryptionConfig$Type` for X-Ray's default encryption, from
/// [`crate::ops::types::ENCRYPTION_TYPE_VALUES`].
pub const ENCRYPTION_TYPE_NONE: &str = "NONE";

/// `EncryptionConfig$Status` while a submitted configuration is being applied,
/// from [`crate::ops::types::ENCRYPTION_STATUS_VALUES`].
pub const ENCRYPTION_STATUS_UPDATING: &str = "UPDATING";

/// `EncryptionConfig$Status` once the stored configuration is in force, from
/// [`crate::ops::types::ENCRYPTION_STATUS_VALUES`].
pub const ENCRYPTION_STATUS_ACTIVE: &str = "ACTIVE";

/// How long a configuration reports `UPDATING` before the clock owes it
/// `ACTIVE`.
///
/// CHOSEN, not documented: X-Ray publishes no duration for applying a new
/// encryption configuration, so this is a value inside the project's 250–750 ms
/// band for a transition the clock owes. It is deliberately non-zero, because
/// `EncryptionConfig$Status` documents a state a caller can observe — "While the
/// status is UPDATING, X-Ray may encrypt data with a combination of the new and
/// old settings" — so a read taken straight after the put must still see it.
pub const ENCRYPTION_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The number of digits in an Amazon Web Services account id, which the account
/// field of an ARN carries.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The number of dash-separated groups in the id KMS mints for a single-region
/// key.
const KEY_ID_GROUPS: usize = 5;

/// The number of hexadecimal digits after the `mrk-` prefix of the id KMS mints
/// for a multi-region key.
const MULTI_REGION_KEY_ID_DIGITS: usize = 32;

/// Whether `candidate` is a KMS key id (`PutEncryptionConfig$KeyId`: "Key ID -
/// The KMS key ID of the key").
///
/// The emulator resolves no KMS key, so only the FORM is judged, and it is judged
/// no more narrowly than the id's two shapes: [`KEY_ID_GROUPS`] non-empty groups
/// of hexadecimal digits for a single-region key — which covers both the
/// canonical 8-4-4-4-12 grouping and the differently grouped example that
/// member's own documentation gives — or `mrk-` and
/// [`MULTI_REGION_KEY_ID_DIGITS`] hexadecimal digits for a multi-region key.
fn is_kms_key_id(candidate: &str) -> bool {
    let hexadecimal = |group: &str| {
        !group.is_empty() && group.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    if let Some(digits) = candidate.strip_prefix("mrk-") {
        return digits.len() == MULTI_REGION_KEY_ID_DIGITS && hexadecimal(digits);
    }
    let mut groups = 0usize;
    for group in candidate.split('-') {
        if !hexadecimal(group) {
            return false;
        }
        groups += 1;
    }
    groups == KEY_ID_GROUPS
}

/// Whether `candidate` is a KMS alias path: `alias/` followed by a non-empty
/// name of the characters KMS's alias name allows (`^[a-zA-Z0-9:/_-]+$`), as in
/// `PutEncryptionConfig$KeyId`'s "Alias - The name of the key. For example,
/// alias/MyKey".
fn is_kms_alias(candidate: &str) -> bool {
    candidate.strip_prefix("alias/").is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":/_-".contains(&byte))
    })
}

/// The six colon-delimited fields of an ARN —
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — or `None` when
/// `candidate` is not in that form at all.
///
/// One definition of "split an ARN" for this module: the callers differ only in
/// which fields they then require.
fn arn_fields(candidate: &str) -> Option<[&str; 6]> {
    let mut split = candidate.splitn(6, ':');
    let fields = [
        split.next()?,
        split.next()?,
        split.next()?,
        split.next()?,
        split.next()?,
        split.next()?,
    ];
    (fields[0] == "arn").then_some(fields)
}

/// Whether `candidate` is the full ARN of a KMS key or alias
/// (`arn:<partition>:kms:<region>:<account>:key/<key id>` or `…:alias/<name>`),
/// the third form `PutEncryptionConfig$KeyId` documents.
fn is_kms_arn(candidate: &str) -> bool {
    let Some([_, partition, service, region, account, resource]) = arn_fields(candidate) else {
        return false;
    };
    service == "kms"
        && !partition.is_empty()
        && !region.is_empty()
        && account.len() == ACCOUNT_ID_DIGITS
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && (is_kms_alias(resource) || resource.strip_prefix("key/").is_some_and(is_kms_key_id))
}

/// Whether `key_id` selects a KMS key in one of the three forms
/// `PutEncryptionConfig$KeyId` documents — an alias, a key id, or the full ARN of
/// either. Anything else is refused: the emulator holds no KMS keys, so the form
/// of the selector is all there is to judge.
pub fn is_kms_key_selector(key_id: &str) -> bool {
    is_kms_alias(key_id) || is_kms_key_id(key_id) || is_kms_arn(key_id)
}

/// The encryption configuration this service reports: what `PutEncryptionConfig`
/// last stored, and when the clock owes it `ACTIVE`.
#[derive(Debug, Clone)]
pub struct EncryptionConfigRecord {
    /// `EncryptionConfig$Type`: [`ENCRYPTION_TYPE_KMS`] or
    /// [`ENCRYPTION_TYPE_NONE`].
    pub encryption_type: String,
    /// `EncryptionConfig$KeyId`: the key the caller selected, kept in the form it
    /// was given (the emulator resolves no KMS key, so it invents no other form);
    /// `None` under [`ENCRYPTION_TYPE_NONE`].
    pub key_id: Option<String>,
    /// When `UPDATING` -> `ACTIVE` falls due; `None` once the configuration is
    /// active. See [`ENCRYPTION_SETTLE_AFTER`].
    pub active_due: Option<Instant>,
}

impl Default for EncryptionConfigRecord {
    /// Until a configuration is put, X-Ray encrypts with its own key: `Type`
    /// `NONE`, `Status` `ACTIVE`, and no `KeyId`.
    fn default() -> Self {
        Self {
            encryption_type: ENCRYPTION_TYPE_NONE.to_string(),
            key_id: None,
            active_due: None,
        }
    }
}

impl EncryptionConfigRecord {
    /// Materialise the `UPDATING` -> `ACTIVE` transition the clock owes by `now`.
    pub fn settle(&mut self, now: Instant) {
        if self.active_due.is_some_and(|due| due <= now) {
            self.active_due = None;
        }
    }

    /// This record as the modelled `EncryptionConfig`, as of `now`.
    pub fn to_config(&self, now: Instant) -> EncryptionConfig {
        let status = if self.active_due.is_some_and(|due| due > now) {
            ENCRYPTION_STATUS_UPDATING
        } else {
            ENCRYPTION_STATUS_ACTIVE
        };
        EncryptionConfig {
            key_id: self.key_id.clone(),
            status: Some(status.to_string()),
            r#type: Some(self.encryption_type.clone()),
        }
    }
}

/// `UnprocessedTraceSegment$ErrorCode` for a document refused because its trace
/// already holds [`MAX_SEGMENTS_PER_TRACE`] segments.
pub const SEGMENT_ERROR_LIMIT_EXCEEDED: &str = "LimitExceeded";

/// The `ErrorCode` for one segment member at fault: `<prefix><Member>`, the
/// member's snake_case document name rendered in PascalCase, as `MissingName`
/// is built from `name`.
fn segment_member_code(prefix: &str, member: &str) -> String {
    let mut code = prefix.to_string();
    for word in member.split('_') {
        let mut characters = word.chars();
        if let Some(first) = characters.next() {
            code.extend(first.to_uppercase());
            code.push_str(characters.as_str());
        }
    }
    code
}

/// Whether `value` is a well-formed X-Ray trace id: `1-<8 hex>-<24 hex>`, the
/// form `PutTraceSegments` documents for a segment's `trace_id` and the only
/// form a stored trace is indexed under (`TraceId` is 1..=35 characters, which
/// this form occupies exactly).
pub fn is_trace_id(value: &str) -> bool {
    let mut fields = value.split('-');
    let (Some("1"), Some(epoch), Some(unique), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    let hex = |field: &str, width: usize| {
        field.len() == width && field.chars().all(|c| c.is_ascii_hexdigit())
    };
    hex(epoch, 8) && hex(unique, 24)
}

// ---------------------------------------------------------------------------
// Bounds on caller-driven work and growth
//
// Every limit below exists so that no request, and no sequence of requests, can
// make this service do or hold unbounded work: the per-call document count, the
// segments one trace accumulates, the number of traces and groups kept, and the
// number of segment documents one response carries are each capped, and the
// trace store evicts its least recently ingested trace once full (real X-Ray
// expires traces too, so the cap is also closer to the service).
// ---------------------------------------------------------------------------

/// The most segment documents one `PutTraceSegments` call accepts. A larger
/// batch is refused outright rather than reported document by document, so a
/// single request can neither drive unbounded parsing nor amplify into an
/// unbounded response.
pub const MAX_SEGMENT_DOCUMENTS_PER_CALL: usize = 500;

/// The most segments one trace holds. A further NEW segment is refused as an
/// `UnprocessedTraceSegment` and marks the trace
/// [`limit_exceeded`](TraceRecord::limit_exceeded), which is what `Trace$LimitExceeded`
/// reports.
pub const MAX_SEGMENTS_PER_TRACE: usize = 1_000;

/// The most subsegments one segment document contributes, counting every level
/// of nesting, so one document can neither drive unbounded parsing nor hold
/// unbounded state. Further ones are dropped.
pub const MAX_SUBSEGMENTS_PER_SEGMENT: usize = 1_000;

/// The most traces the store keeps. Ingesting a new trace past this evicts the
/// least recently ingested one.
pub const MAX_TRACES: usize = 1_000;

/// The most groups the store keeps; `CreateGroup` past this is refused.
pub const MAX_GROUPS: usize = 250;
/// The most telemetry records one `PutTelemetryRecords` call accepts.
///
/// `TelemetryRecordList` carries no modelled length bound, so the batch is
/// bounded here — as `PutTraceSegments`'s is — rather than letting one call
/// drive unbounded work and growth.
pub const MAX_TELEMETRY_RECORDS_PER_CALL: usize = 500;

/// The most telemetry records kept per reporting source: a rolling window of the
/// most recently submitted ones. Older records fall out of the window, while the
/// source's running [`TelemetryTotals`] keep counting every record accepted.
pub const MAX_TELEMETRY_RECORDS_PER_SOURCE: usize = 1_000;

/// The most reporting sources the telemetry store keeps. A report from a new
/// source past this evicts the one that has gone longest without reporting.
pub const MAX_TELEMETRY_SOURCES: usize = 500;

/// The most user-applied tags one taggable resource holds.
///
/// Published by the service, not chosen here: the `Tag` shape's own
/// documentation (`smithy_query(query="shape",shape="Tag")`) says "Maximum
/// number of user-applied tags per resource: 50". A `TagResource` call that
/// would carry a resource past it is refused with `TooManyTagsException`, and a
/// `CreateGroup` with `InvalidRequestException` (the only 400 it models).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The prefix reserved for Amazon Web Services' own tag keys.
///
/// From `TagResource$Tags`' documentation: "Don't use aws: as a prefix for
/// keys; it's reserved for Amazon Web Services use. You cannot edit or delete
/// system tags." So a caller may neither apply nor remove a key carrying it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

// ---------------------------------------------------------------------------
// Trace segment destination (Transaction Search)
// ---------------------------------------------------------------------------

/// The destination an account sends `PutTraceSegments` and OTLP data to before
/// `UpdateTraceSegmentDestination` ever ran: `TraceSegmentDestination$XRay`.
///
/// The `CloudWatchLogs` destination is what Transaction Search requires
/// (`smithy_query(query="doc",shape="UpdateTraceSegmentDestination")`), and
/// Transaction Search is opt-in, so an account that never asked for it reports
/// the X-Ray destination.
pub const DEFAULT_TRACE_SEGMENT_DESTINATION: &str = "XRay";

/// `TraceSegmentDestination$CloudWatchLogs`: the destination Transaction Search
/// requires, and the only one a `CloudWatchLogsConfiguration` applies to (that
/// member's documentation: "Specify this only when `Destination` is
/// `CloudWatchLogs`").
pub const CLOUD_WATCH_LOGS_DESTINATION: &str = "CloudWatchLogs";

/// `TraceSegmentDestinationStatus$ACTIVE`: the configured destination is in
/// effect.
pub const DESTINATION_STATUS_ACTIVE: &str = "ACTIVE";

/// `TraceSegmentDestinationStatus$PENDING`: a destination change was accepted
/// and is not in effect yet.
pub const DESTINATION_STATUS_PENDING: &str = "PENDING";

/// How long a destination change reports `PENDING` before the clock owes it
/// `ACTIVE`.
///
/// Neither the model nor the AWS documentation states a duration for the
/// switch, so this span is chosen: non-zero, so a caller reading straight back
/// still observes the `PENDING` the real service reports, and short enough that
/// nothing has to wait long for `ACTIVE`.
pub const DESTINATION_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// The indexing rule every account already holds, before any call.
///
/// Transaction Search ships one rule, named `Default`, and this service's model
/// publishes no operation that creates or deletes an indexing rule
/// (`smithy_query(query="list_operations")` has only `GetIndexingRules` and
/// `UpdateIndexingRule`), so this is the only name [`IndexingRuleStore`] holds.
pub const DEFAULT_INDEXING_RULE_NAME: &str = "Default";

/// The `DesiredSamplingPercentage` the [`DEFAULT_INDEXING_RULE_NAME`] rule
/// carries until `UpdateIndexingRule` rewrites it: the 1% of spans Transaction
/// Search indexes out of the box.
pub const DEFAULT_INDEXING_SAMPLING_PERCENTAGE: f64 = 1.0;

/// The largest `DesiredSamplingPercentage` an indexing rule accepts.
///
/// `ProbabilisticRuleValueUpdate$DesiredSamplingPercentage` targets
/// `NullableDouble` and the model states no range, but the member is documented
/// as a "sampling percentage of traceIds": a percentage outside 0..=100 names no
/// sampling rate, so it is refused rather than stored.
pub const MAX_SAMPLING_PERCENTAGE: f64 = 100.0;

/// The most trace ids one `BatchGetTraces` call accepts.
///
/// `TraceIdList` carries no modelled length bound, so the bound is the
/// service's: X-Ray's `BatchGetTraces` "retrieves a list of traces specified by
/// ID" five at a time, and a longer list is refused as an invalid request
/// rather than partly served.
pub const MAX_TRACE_IDS_PER_CALL: usize = 5;

/// The most segment documents one `BatchGetTraces` response carries. A trace
/// that does not fit in what is left of the budget is deferred to the next page
/// rather than truncated, so a page always makes progress and a whole trace is
/// always readable ([`MAX_SEGMENTS_PER_TRACE`] segments fit the budget exactly).
pub const MAX_SEGMENTS_PER_BATCH_RESPONSE: usize = MAX_SEGMENTS_PER_TRACE;

/// The deepest `subsegments` nesting one segment document may carry.
///
/// Every reader walks that tree RECURSIVELY — the classifications a summary
/// unions, the frames a root cause names, the entity index an `edge(…)` filter
/// resolves through — so its depth is caller-driven work on the read path, and
/// the one kind a reader cannot recover from: exhausting the stack aborts the
/// process instead of failing the request. A document nested deeper than this is
/// therefore refused at ingest, like any other member it cannot use, and every
/// walk is bounded by this constant by construction. Deep enough for the nesting
/// an instrumented call tree produces (an SDK subsegment inside a handler
/// subsegment inside a segment is three).
pub const MAX_SUBSEGMENT_DEPTH: usize = 16;

/// The most root causes one trace reports per class
/// ([`TraceRecord::root_causes`]).
///
/// A root cause costs a walk of the trace, so without a cap a trace holding
/// [`MAX_SEGMENTS_PER_TRACE`] failing segments would make one read do work in
/// the square of its size. Past the cap the answer is the first causes derived,
/// which degrades the answer rather than the service.
pub const MAX_ROOT_CAUSES_PER_TRACE: usize = 32;

/// The most sampling rules the store keeps; `CreateSamplingRule` past this is
/// refused with the modelled `RuleLimitExceededException`.
///
/// The model documents no quota, so this is the emulator's bound on growth, in
/// the same spirit as [`MAX_GROUPS`].
pub const MAX_SAMPLING_RULES: usize = 500;

/// The most trace retrievals the store keeps. A new retrieval past this evicts
/// the one that started longest ago (expired ones are dropped first).
pub const MAX_TRACE_RETRIEVALS: usize = 250;

/// How long a started trace retrieval stays readable.
///
/// "Retrievals will time out after 60 minutes" — the `StartTraceRetrieval`
/// documentation (`smithy_query(query="doc",shape="StartTraceRetrieval")`), so
/// the value is the service's, not this emulator's choice.
pub const TRACE_RETRIEVAL_TIMEOUT_SECONDS: f64 = 60.0 * 60.0;

/// How many `StartTraceRetrieval` calls may arrive back-to-back before the
/// account's request budget is empty and the next one is throttled.
///
/// `ThrottledException` is "the request exceeds the maximum number of requests
/// per second" and the model publishes no number, so the budget below is this
/// emulator's: it bounds how much retrieval work one caller can start at once,
/// and refills at [`TRACE_RETRIEVAL_RATE_PER_SECOND`] so a steady caller is
/// never throttled.
pub const TRACE_RETRIEVAL_BURST: u32 = 3;

/// How fast the `StartTraceRetrieval` budget refills, in requests per second.
///
/// Slower than the burst, so a caller that fires several retrievals at once runs
/// the budget down and is told so, while a caller that starts one retrieval at a
/// time is never throttled.
pub const TRACE_RETRIEVAL_RATE_PER_SECOND: f64 = 3.0;

/// `RetrievalStatus` for a retrieval that was accepted but has not begun
/// reading, from
/// [`RETRIEVAL_STATUS_VALUES`](crate::ops::types::RETRIEVAL_STATUS_VALUES).
pub const RETRIEVAL_STATUS_SCHEDULED: &str = "SCHEDULED";

/// `RetrievalStatus` while a retrieval is reading, from
/// [`RETRIEVAL_STATUS_VALUES`](crate::ops::types::RETRIEVAL_STATUS_VALUES).
pub const RETRIEVAL_STATUS_RUNNING: &str = "RUNNING";

/// `RetrievalStatus` once a retrieval's results are readable, from
/// [`RETRIEVAL_STATUS_VALUES`](crate::ops::types::RETRIEVAL_STATUS_VALUES).
pub const RETRIEVAL_STATUS_COMPLETE: &str = "COMPLETE";

/// `RetrievalStatus` for a retrieval `CancelTraceRetrieval` stopped, from
/// [`RETRIEVAL_STATUS_VALUES`](crate::ops::types::RETRIEVAL_STATUS_VALUES).
pub const RETRIEVAL_STATUS_CANCELLED: &str = "CANCELLED";

/// How long a started retrieval reports `SCHEDULED` before the clock owes it
/// `RETRIEVAL_STATUS_RUNNING`.
///
/// CHOSEN, not documented: X-Ray publishes no duration for scheduling a
/// retrieval, so this is a value inside the project's 250–750 ms band for a
/// transition the clock owes. It is deliberately non-zero because
/// `GetRetrievedTracesGraph` documents the states a caller can observe — "Retry
/// the request after the status changes from RUNNING or SCHEDULED to COMPLETE" —
/// so a read taken straight after the start must still see the retrieval before
/// it runs.
pub const TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS: f64 = 0.25;

/// How long a retrieval reports `RETRIEVAL_STATUS_RUNNING` before the clock owes
/// it `RETRIEVAL_STATUS_COMPLETE`.
///
/// CHOSEN for the same reason as [`TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS`], and
/// non-zero for the same one: "the API will return an empty response" while the
/// status is not `COMPLETE` (`ListRetrievedTraces`), which a caller can only
/// observe if the running state lasts.
pub const TRACE_RETRIEVAL_RUNNING_SPAN_SECONDS: f64 = 0.25;

/// `TraceFormatType` for the X-Ray segment documents this service ingests, from
/// [`TRACE_FORMAT_TYPE_VALUES`](crate::ops::types::TRACE_FORMAT_TYPE_VALUES).
pub const TRACE_FORMAT_XRAY: &str = "XRAY";

/// `TraceFormatType` for OpenTelemetry spans, from
/// [`TRACE_FORMAT_TYPE_VALUES`](crate::ops::types::TRACE_FORMAT_TYPE_VALUES).
pub const TRACE_FORMAT_OTEL: &str = "OTEL";

/// How far past the present a `[StartTime, EndTime]` bound may land before the
/// window is refused as not naming a time the data could have.
///
/// A whole day is far more than any clock skew, which is deliberate: the bound
/// exists to refuse a value that cannot be a timestamp of recorded traffic at
/// all — a millisecond-scale epoch value read as seconds lands thousands of
/// years out — not to second-guess a caller asking for a window that reaches a
/// little past now.
pub const MAX_WINDOW_FUTURE_SKEW_SECONDS: f64 = 24.0 * 60.0 * 60.0;

/// The most entries one `Histogram` in a service graph carries. The model
/// publishes no bound, so this caps the response a caller can amplify out of
/// widely-spread segment durations.
pub const MAX_HISTOGRAM_ENTRIES: usize = 50;

/// How far from the epoch an instant a segment document carries may be, in
/// seconds.
///
/// A `start_time` or `end_time` beyond this — and any value that is not finite
/// — is not a time a trace could have been recorded at (a millisecond-scale
/// epoch value read as seconds already lands past it, the same mistake
/// [`validate_window`] refuses for a request window) and is not a value every
/// reader's modelled `Timestamp` can carry, so the document is refused at
/// ingest rather than stored and converted later. 1e12 seconds is about 31,000
/// years, which covers every instant in seconds of epoch time and no
/// millisecond-scale one.
pub const MAX_SEGMENT_EPOCH_SECONDS: f64 = 1e12;

/// Per-service state for com-amazonaws-xray-awsxray. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsXrayAwsxrayState {
    /// Ingested traces, keyed by `trace_id`, at most [`MAX_TRACES`] of them.
    pub traces: BTreeMap<String, TraceRecord>,
    /// Groups, keyed by [`GroupKey`] — the region they live in and their
    /// case-sensitive `GroupName` — at most [`MAX_GROUPS`] of them.
    ///
    /// The reserved [`DEFAULT_GROUP_NAME`] group is NOT here: it is derived
    /// ([`default_group`]), so a fresh store already answers for it, and only an
    /// `UpdateGroup` of it ever stores a record under that name.
    pub groups: BTreeMap<GroupKey, GroupRecord>,
    /// `GroupARN` -> the [`GroupKey`] it names, so a reader can resolve a group
    /// by either selector. A group ARN embeds its region and carries a unique
    /// suffix, so one ARN names at most one record anywhere in this store.
    pub group_arn_to_name: BTreeMap<String, GroupKey>,
    /// Feeds the unique suffix of a minted group ARN; never reset, so two groups
    /// of the same name created in sequence still get distinct ARNs.
    pub group_seq: u64,
    /// Ingest counter, stamped on a trace so the oldest can be evicted.
    pub ingest_seq: u64,
    /// Sampling rules, keyed by their case-sensitive `RuleName`, at most
    /// [`MAX_SAMPLING_RULES`] of them. A rule's ARN is derived from its name, so
    /// no second index is needed to resolve one by ARN.
    pub sampling_rules: BTreeMap<String, StoredSamplingRule>,
    /// Started trace retrievals, keyed by the `RetrievalToken` that was minted
    /// for them, at most [`MAX_TRACE_RETRIEVALS`] of them.
    pub retrievals: BTreeMap<String, TraceRetrievalRecord>,
    /// Feeds the unique prefix of a minted `RetrievalToken`; never reset, so two
    /// retrievals of the same traces still get distinct tokens.
    pub retrieval_seq: u64,
    /// The account's `StartTraceRetrieval` request budget.
    pub retrieval_rate: RateLimiter,
    /// Insights, keyed by their `InsightId`.
    ///
    /// X-Ray MINTS insights itself, from anomaly detection over ingested traces
    /// ("When fault rates go outside of the expected range, X-Ray creates an
    /// insight" — `smithy_query(query="shape",shape="Insight")`); no operation in
    /// the API creates one. This emulator does not run that analysis, so this is
    /// the map an insight generator would write and `GetInsight` /
    /// `GetInsightSummaries` read: an `InsightId` it does not hold names no
    /// insight, which is exactly what `GetInsight` refuses.
    pub insights: BTreeMap<String, Insight>,
    /// Daemon telemetry, keyed by the reporting source the request identified,
    /// at most [`MAX_TELEMETRY_SOURCES`] of them.
    pub telemetry: BTreeMap<TelemetrySource, TelemetryReporter>,
    /// Report counter, stamped on a reporter so the least recently reporting one
    /// can be evicted; never reset.
    pub telemetry_seq: u64,
    /// The encryption configuration `PutEncryptionConfig` stores and
    /// `GetEncryptionConfig` reports; X-Ray's default until one is put.
    pub encryption: EncryptionConfigRecord,
    /// Where `PutTraceSegments` and OTLP data go, as last set by
    /// `UpdateTraceSegmentDestination`; its [`Default`] is the account default
    /// every reader answers until something changes it.
    pub trace_segment_destination: TraceSegmentDestinationRecord,
    /// The account's indexing rules, which `GetIndexingRules` reports and
    /// `UpdateIndexingRule` rewrites.
    pub indexing_rules: IndexingRuleStore,
}

/// Who reported telemetry: the `PutTelemetryRecords` request's own reporter
/// identity (`EC2InstanceId`, `Hostname`, `ResourceARN`), which is what
/// distinguishes one daemon's counters from another's.
///
/// A member carrying the empty string identifies nothing, so it is held as
/// absent: each of the three is modelled `len 0..N`
/// (`smithy_query(query="operation",operation="PutTelemetryRecords")`), so a
/// daemon that sends `""` for a field it does not know reports as the same
/// source as one that omits it.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TelemetrySource {
    /// `PutTelemetryRecordsRequest$EC2InstanceId`.
    pub ec2_instance_id: Option<String>,
    /// `PutTelemetryRecordsRequest$Hostname`.
    pub hostname: Option<String>,
    /// `PutTelemetryRecordsRequest$ResourceARN`.
    pub resource_arn: Option<String>,
}

impl TelemetrySource {
    /// The source a request identifies, with an empty-string member held as
    /// absent.
    pub fn new(
        ec2_instance_id: Option<&str>,
        hostname: Option<&str>,
        resource_arn: Option<&str>,
    ) -> Self {
        let identifier = |value: Option<&str>| {
            value
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        Self {
            ec2_instance_id: identifier(ec2_instance_id),
            hostname: identifier(hostname),
            resource_arn: identifier(resource_arn),
        }
    }
}

/// The account's trace segment destination, as stored.
///
/// `GetTraceSegmentDestination` answers from this record and
/// `UpdateTraceSegmentDestination` is the only thing that changes it, so the
/// reported destination is always the one that was last configured — never a
/// value echoed from the request being answered.
#[derive(Debug, Clone)]
pub struct TraceSegmentDestinationRecord {
    /// The configured destination: one of
    /// [`TRACE_SEGMENT_DESTINATION_VALUES`](crate::ops::types::TRACE_SEGMENT_DESTINATION_VALUES).
    pub destination: String,
    /// When the accepted change becomes effective, i.e. when the clock owes the
    /// `PENDING` -> `ACTIVE` move. `None` once the destination is in effect.
    pub active_at: Option<Instant>,
    /// `CloudWatchLogsConfiguration$SpansLogGroupKmsKeyId`, as last supplied
    /// with a `CloudWatchLogs` destination, and dropped when the service is
    /// pointed anywhere else. Kept so the configuration survives the call that
    /// set it; no modelled output returns it, so
    /// [`ComAmazonawsXrayAwsxrayState::set_trace_segment_destination`]'s own
    /// test is what pins it.
    pub spans_log_group_kms_key_id: Option<String>,
}

impl Default for TraceSegmentDestinationRecord {
    fn default() -> Self {
        Self {
            destination: DEFAULT_TRACE_SEGMENT_DESTINATION.to_string(),
            active_at: None,
            spans_log_group_kms_key_id: None,
        }
    }
}

impl TraceSegmentDestinationRecord {
    /// The `TraceSegmentDestinationStatus` this record reports: `PENDING` while
    /// an accepted change is still owed, `ACTIVE` once it is in effect.
    pub fn status(&self) -> &'static str {
        if self.active_at.is_some() {
            DESTINATION_STATUS_PENDING
        } else {
            DESTINATION_STATUS_ACTIVE
        }
    }
}

/// The indexing rules this service holds, keyed by rule name.
///
/// The set of rules is FIXED: X-Ray models no create and no delete for one, so
/// the store is seeded with [`DEFAULT_INDEXING_RULE_NAME`] at construction and
/// only ever has a rule's configuration rewritten under it. That is why a read
/// never has to materialise anything — the `Default` rule is there before the
/// first call, exactly as it is on a fresh account.
#[derive(Debug)]
pub struct IndexingRuleStore {
    rules: BTreeMap<String, IndexingRuleRecord>,
}

impl Default for IndexingRuleStore {
    fn default() -> Self {
        let record = IndexingRuleRecord {
            name: DEFAULT_INDEXING_RULE_NAME.to_string(),
            modified_at_secs: now_epoch_secs(),
            desired_sampling_percentage: DEFAULT_INDEXING_SAMPLING_PERCENTAGE,
        };
        Self {
            rules: BTreeMap::from([(record.name.clone(), record)]),
        }
    }
}

/// One reporting source's telemetry: the records it submitted, how much it has
/// submitted, and the totals recomputed from every record accepted.
#[derive(Debug, Clone, Default)]
pub struct TelemetryReporter {
    /// The source these records came from.
    pub source: TelemetrySource,
    /// The most recent [`MAX_TELEMETRY_RECORDS_PER_SOURCE`] records, in
    /// submission order, each stored exactly as submitted.
    pub records: Vec<TelemetryRecord>,
    /// How many records this source has had accepted, including any that have
    /// since fallen out of [`records`](Self::records).
    pub records_accepted: u64,
    /// How many `PutTelemetryRecords` calls this source has made, counting one
    /// that carried an empty batch.
    pub reports: u64,
    /// The sums of every accepted record's counters.
    pub totals: TelemetryTotals,
    /// When this source last reported, in [`ComAmazonawsXrayAwsxrayState::telemetry_seq`]
    /// ticks. Orders eviction once the telemetry store is full.
    pub last_report_seq: u64,
}

/// The running sums of a reporting source's telemetry counters: one field per
/// `NullableInteger` member of `TelemetryRecord` and of its
/// `BackendConnectionErrors`
/// (`smithy_query(query="closure",operation="PutTelemetryRecords")`).
///
/// Held as `i64` and added saturatingly, so no sequence of calls can overflow a
/// total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TelemetryTotals {
    /// Sum of `TelemetryRecord$SegmentsReceivedCount`.
    pub segments_received: i64,
    /// Sum of `TelemetryRecord$SegmentsSentCount`.
    pub segments_sent: i64,
    /// Sum of `TelemetryRecord$SegmentsSpilloverCount`.
    pub segments_spillover: i64,
    /// Sum of `TelemetryRecord$SegmentsRejectedCount`.
    pub segments_rejected: i64,
    /// Sum of `BackendConnectionErrors$TimeoutCount`.
    pub backend_timeouts: i64,
    /// Sum of `BackendConnectionErrors$ConnectionRefusedCount`.
    pub backend_connection_refused: i64,
    /// Sum of `BackendConnectionErrors$HTTPCode4XXCount`.
    pub backend_http_code_4xx: i64,
    /// Sum of `BackendConnectionErrors$HTTPCode5XXCount`.
    pub backend_http_code_5xx: i64,
    /// Sum of `BackendConnectionErrors$UnknownHostCount`.
    pub backend_unknown_host: i64,
    /// Sum of `BackendConnectionErrors$OtherCount`.
    pub backend_other: i64,
}

impl TelemetryTotals {
    /// Add one record's counters. An absent counter contributes nothing, which
    /// is what `NullableInteger` being optional means.
    fn add(&mut self, record: &TelemetryRecord) {
        let accumulate = |total: &mut i64, count: Option<i32>| {
            if let Some(count) = count {
                *total = total.saturating_add(i64::from(count));
            }
        };
        accumulate(&mut self.segments_received, record.segments_received_count);
        accumulate(&mut self.segments_sent, record.segments_sent_count);
        accumulate(
            &mut self.segments_spillover,
            record.segments_spillover_count,
        );
        accumulate(&mut self.segments_rejected, record.segments_rejected_count);
        if let Some(errors) = record.backend_connection_errors.as_ref() {
            accumulate(&mut self.backend_timeouts, errors.timeout_count);
            accumulate(
                &mut self.backend_connection_refused,
                errors.connection_refused_count,
            );
            accumulate(&mut self.backend_http_code_4xx, errors.http_code4_xx_count);
            accumulate(&mut self.backend_http_code_5xx, errors.http_code5_xx_count);
            accumulate(&mut self.backend_unknown_host, errors.unknown_host_count);
            accumulate(&mut self.backend_other, errors.other_count);
        }
    }
}

impl IndexingRuleStore {
    /// Every stored rule, in rule-name order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &IndexingRuleRecord> {
        self.rules.values()
    }

    /// The stored rules from `name` onward, in rule-name order — what a page
    /// resumed from a `NextToken` starts at.
    pub fn iter_from(&self, name: &str) -> impl Iterator<Item = &IndexingRuleRecord> {
        self.rules
            .range::<str, _>((
                ::std::ops::Bound::Included(name),
                ::std::ops::Bound::Unbounded,
            ))
            .map(|(_, rule)| rule)
    }

    /// Whether a rule of this exact name is stored.
    pub fn contains(&self, name: &str) -> bool {
        self.rules.contains_key(name)
    }

    /// Rewrite the named rule's probabilistic configuration and restamp its
    /// `ModifiedAt`, answering the stored record — or `None` when no rule of
    /// that name exists, which is what `ResourceNotFoundException` reports.
    ///
    /// The store neither grows nor shrinks here: an unknown name is refused
    /// rather than created, because X-Ray has no operation that adds a rule.
    pub fn update_probabilistic(
        &mut self,
        name: &str,
        desired_sampling_percentage: f64,
    ) -> Option<&IndexingRuleRecord> {
        let record = self.rules.get_mut(name)?;
        record.desired_sampling_percentage = desired_sampling_percentage;
        record.modified_at_secs = now_epoch_secs();
        Some(record)
    }
}

/// One indexing rule, as stored. `GetIndexingRules` and `UpdateIndexingRule`
/// both answer from this record, never from the request that wrote it.
#[derive(Debug, Clone)]
pub struct IndexingRuleRecord {
    /// The rule's name.
    pub name: String,
    /// When the rule was last modified, in whole seconds of epoch time —
    /// `IndexingRule$ModifiedAt` is documented "in Unix time seconds".
    pub modified_at_secs: i64,
    /// The configured `DesiredSamplingPercentage`.
    pub desired_sampling_percentage: f64,
}

impl IndexingRuleRecord {
    /// This record as the modelled `IndexingRule`.
    ///
    /// `ActualSamplingPercentage` is "applied sampling percentage of traceIds",
    /// which AWS may hold below the configured one "to ensure completeness of
    /// data". This emulator ingests through `PutTraceSegments` and applies no
    /// such limit, so the applied percentage IS the configured one.
    pub fn to_indexing_rule(&self) -> IndexingRule {
        IndexingRule {
            name: Some(self.name.clone()),
            modified_at: Some(::aws_smithy_types::DateTime::from_secs(
                self.modified_at_secs,
            )),
            rule: Some(IndexingRuleValue::Probabilistic(ProbabilisticRuleValue {
                desired_sampling_percentage: self.desired_sampling_percentage,
                actual_sampling_percentage: Some(self.desired_sampling_percentage),
            })),
        }
    }
}

/// The current instant in whole seconds of epoch time.
///
/// The crate keeps ONE clock: this is [`now_epoch_seconds`] truncated, for the
/// members X-Ray documents "in Unix time seconds".
pub fn now_epoch_secs() -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let seconds = now_epoch_seconds().trunc() as i64;
    seconds
}

/// One trace: every segment document submitted under a single `trace_id`.
#[derive(Debug, Clone, Default)]
pub struct TraceRecord {
    /// The trace's `trace_id`.
    pub id: String,
    /// Its segments, in first-submission order.
    pub segments: Vec<SegmentRecord>,
    /// Segment id -> its index in [`segments`](Self::segments), so ingesting a
    /// document is a lookup rather than a scan of everything stored so far.
    segment_index: BTreeMap<String, usize>,
    /// The earliest `start_time` across [`segments`](Self::segments).
    pub first_start: f64,
    /// The latest `end_time` across [`segments`](Self::segments), taking an
    /// in-progress segment's `start_time` as its latest known instant.
    pub last_end: f64,
    /// Set once a segment was refused because this trace reached
    /// [`MAX_SEGMENTS_PER_TRACE`]; reported as `Trace$LimitExceeded`.
    pub limit_exceeded: bool,
    /// When this trace was last ingested into, in [`ComAmazonawsXrayAwsxrayState::ingest_seq`]
    /// ticks. Orders eviction once the store is full.
    pub ingest_seq: u64,
}

/// One segment document, with the fields a reader needs lifted out of it and the
/// document itself kept byte-for-byte as submitted.
#[derive(Debug, Clone, Default)]
pub struct SegmentRecord {
    /// The segment's `id`.
    pub id: String,
    /// The segment's `parent_id`; `None` makes this a root segment.
    pub parent_id: Option<String>,
    /// The segment's `name` — the service that handled the request.
    pub name: String,
    /// The submitted document, verbatim. `BatchGetTraces` returns exactly this.
    pub document: String,
    /// `start_time`, in floating point seconds of epoch time.
    pub start_time: f64,
    /// `end_time`, absent while the segment is in progress.
    pub end_time: Option<f64>,
    /// `in_progress`: the segment was started but is not complete.
    pub in_progress: bool,
    /// `error`: the segment recorded a client-side (400 series) error.
    pub error: bool,
    /// `fault`: the segment recorded a server-side (500 series) fault.
    pub fault: bool,
    /// `throttle`: the segment recorded a throttling (429) error.
    pub throttle: bool,
    /// The segment's `http` block, when it carries one.
    pub http: Option<HttpRecord>,
    /// The segment's `annotations`, as submitted.
    pub annotations: BTreeMap<String, serde_json::Value>,
    /// The segment's `origin` — the resource type that ran the service.
    pub origin: Option<String>,
    /// The segment's `account_id`, when it names one.
    pub account_id: Option<String>,
    /// The segment's `user` — the authenticated user the request came from
    /// (`TraceSummary$Users`).
    pub user: Option<String>,
    /// The segment's `resource_arn` (`TraceSummary$ResourceARNs`).
    pub resource_arn: Option<String>,
    /// The document's `type`. `Some("subsegment")` marks a SUBSEGMENT sent as
    /// its own document: it records work inside its parent's service, so it
    /// reports no service of its own.
    pub segment_type: Option<String>,
    /// The document's `namespace`: `aws` for an AWS SDK call, `remote` for any
    /// other call that left the process, absent for in-process work.
    pub namespace: Option<String>,
    /// The exceptions the segment's `cause` block carries.
    pub exceptions: Vec<CauseException>,
    /// The `subsegments` this document carries inline, recursively — the
    /// entities inside it that a root cause walks — at most
    /// [`MAX_SUBSEGMENTS_PER_SEGMENT`] of them and at most
    /// [`MAX_SUBSEGMENT_DEPTH`] levels deep.
    pub subsegments: Vec<SubsegmentRecord>,
    /// Whether this segment was SYNTHESISED by the service for a downstream
    /// service a stored document only called (`namespace: "remote"`), rather than
    /// submitted. Its document carries `"inferred": true`.
    pub inferred: bool,
}

/// One subsegment, as carried inline in its segment's `subsegments` array, with
/// the same fields a root cause and a filter expression read off a segment.
///
/// A subsegment records work done INSIDE the segment's service, so — unlike a
/// segment — it names a service of its own only when its `namespace` says the
/// work left the process.
#[derive(Debug, Clone, Default)]
pub struct SubsegmentRecord {
    /// The subsegment's `id`, when it carries one.
    pub id: Option<String>,
    /// The subsegment's `name` — the operation, or the downstream service it
    /// called.
    pub name: String,
    /// The subsegment's `namespace`: `aws`, `remote`, or absent.
    pub namespace: Option<String>,
    /// `start_time`, when the subsegment carries one.
    pub start_time: Option<f64>,
    /// `end_time`, absent while the subsegment is in progress.
    pub end_time: Option<f64>,
    /// `in_progress`: started but not complete.
    pub in_progress: bool,
    /// `error`: a client-side (400 series) error.
    pub error: bool,
    /// `fault`: a server-side (500 series) fault.
    pub fault: bool,
    /// `throttle`: a throttling (429) error.
    pub throttle: bool,
    /// The subsegment's `http` block, when it carries one.
    pub http: Option<HttpRecord>,
    /// The subsegment's `annotations`, as submitted.
    pub annotations: BTreeMap<String, serde_json::Value>,
    /// The exceptions this subsegment's `cause` block carries.
    pub exceptions: Vec<CauseException>,
    /// The subsegments nested under this one.
    pub subsegments: Vec<SubsegmentRecord>,
}

/// One exception from an entity's `cause.exceptions` — the `type` X-Ray reports
/// as `RootCauseException$Name`, and its `message`.
#[derive(Debug, Clone, Default)]
pub struct CauseException {
    /// The exception's `type`.
    pub name: Option<String>,
    /// The exception's `message`.
    pub message: Option<String>,
}

/// What one entity's outcome was. EXCLUSIVE: an entity is a fault, a throttle,
/// an error or none of the three, never two at once — X-Ray reclassifies rather
/// than unions, so a document carrying both `throttle` and `error` is a
/// throttle, and one carrying `fault` is a fault. A trace's
/// `HasFault`/`HasError`/`HasThrottle` are the union of its entities'
/// classifications, so a trace can still report two of them when two entities
/// were classified differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    /// Neither a fault, a throttle nor an error.
    Ok,
    /// A client-side (400 series) error that is not a throttle.
    Error,
    /// A throttling (429) error.
    Throttle,
    /// A server-side (500 series) fault.
    Fault,
}

/// Classify one entity from its own flags and recorded HTTP status.
///
/// The flags come first and the status stands in for a flag the document did not
/// set, which is what [`recorded_fault`] / [`recorded_throttle`] /
/// [`recorded_error`] each state for one outcome; this is where the three become
/// EXCLUSIVE, in the order X-Ray reclassifies them.
fn classify(fault: bool, error: bool, throttle: bool, status: Option<i32>) -> Classification {
    if recorded_fault(fault, status) {
        Classification::Fault
    } else if recorded_throttle(throttle, status) {
        Classification::Throttle
    } else if recorded_error(error, throttle, status) {
        Classification::Error
    } else {
        Classification::Ok
    }
}

/// The parts of a segment's `http` block a `TraceSummary` reports.
#[derive(Debug, Clone, Default)]
pub struct HttpRecord {
    /// `http.request.client_ip`.
    pub client_ip: Option<String>,
    /// `http.request.method`.
    pub http_method: Option<String>,
    /// `http.request.url`.
    pub url: Option<String>,
    /// `http.request.user_agent`.
    pub user_agent: Option<String>,
    /// `http.response.status`.
    pub response_status: Option<i32>,
}

/// A stored group. `GetGroup` / `GetGroups` answer from this record, never from
/// the request that created it.
#[derive(Debug, Clone, Default)]
pub struct GroupRecord {
    /// The region this group lives in. Part of the record, not context around
    /// it, so [`ComAmazonawsXrayAwsxrayState::key`] can rebuild the exact key a
    /// lookup resolved and a write can never address another region's record.
    pub region: String,
    /// The group's case-sensitive name.
    pub group_name: String,
    /// The ARN minted when the group was created, and persisted since.
    pub group_arn: String,
    /// The group's filter expression, when it has one.
    pub filter_expression: Option<String>,
    /// The group's insights configuration, when it has one.
    pub insights_configuration: Option<InsightsConfigurationRecord>,
    /// The group's tags, keyed by tag key — so applying a key already present
    /// REPLACES its value rather than storing the key twice. Tags live here, on
    /// the resource record, rather than in a tag store beside it, so a group has
    /// exactly one set of tags whichever operation reads or writes them.
    pub tags: BTreeMap<String, String>,
}

/// A group's `InsightsConfiguration`, as stored.
#[derive(Debug, Clone, Default)]
pub struct InsightsConfigurationRecord {
    /// Whether insights are enabled for the group.
    pub insights_enabled: Option<bool>,
    /// Whether insight notifications are enabled for the group.
    pub notifications_enabled: Option<bool>,
}

/// Why a `(RuleName, RuleARN)` pair names no stored sampling rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplingRuleLookupError {
    /// Neither selector was supplied.
    NoSelector,
    /// Both selectors were supplied: `SamplingRule$RuleName` and
    /// `SamplingRule$RuleARN` are documented as "either name or ARN, but not
    /// both".
    BothSelectors,
    /// A selector was supplied but no rule matches it.
    NotFound,
}

impl SamplingRuleLookupError {
    /// What to tell the caller. Lives here, beside
    /// [`ComAmazonawsXrayAwsxrayState::resolve_sampling_rule_name`], so every
    /// operation that selects a rule words one refusal the same way, and never
    /// renders an internal Rust type.
    pub fn message(self) -> &'static str {
        match self {
            Self::NoSelector => "either RuleName or RuleARN must be specified",
            Self::BothSelectors => {
                "specify a sampling rule by either RuleName or RuleARN, but not both"
            }
            Self::NotFound => SAMPLING_RULE_NOT_FOUND_MESSAGE,
        }
    }
}

/// What X-Ray answers when a selector names no sampling rule.
///
/// A FIXED wording that names neither selector, as the service's recorded
/// `DeleteSamplingRule` / `UpdateSamplingRule` refusals state it: the refusal
/// reports that there is no such rule, not what the request asked for.
pub const SAMPLING_RULE_NOT_FOUND_MESSAGE: &str = "Sampling rule does not exist";

/// What X-Ray answers when a created rule's `RuleName` is already in use — the
/// same fixed shape of wording, from its recorded `CreateSamplingRule`
/// refusals.
pub const SAMPLING_RULE_EXISTS_MESSAGE: &str = "Sampling rule already exists";

/// What identifies one group in the store: the region it lives in and its
/// case-sensitive `GroupName`.
///
/// An X-Ray group is a REGIONAL resource — its ARN carries the region, and the
/// same name in another region is another group — so the region is part of the
/// key, not context around it. Every lookup and every write goes through this
/// one key type, so a caller addressing a group in its own region can neither
/// read nor remove a record belonging to another.
pub type GroupKey = (String, String);

/// The name of the group every X-Ray account and region holds without anyone
/// creating it.
///
/// `CreateGroupRequest$GroupName` says so in the model
/// (`smithy_query(query="shape",shape="CreateGroupRequest")`): "Default is a
/// reserved name and names must be unique".
pub const DEFAULT_GROUP_NAME: &str = "Default";

/// `arn:aws:xray:<region>:<account>:group/<GroupName>/<12 hex digits>` — the ONE
/// place a group ARN's shape is written, so a minted ARN and the reserved
/// group's derived one cannot drift apart.
///
/// X-Ray's `GroupARN` is "generated based on the GroupName"; `seed`
/// distinguishes two ARNs carrying the same name, and is mixed with the name
/// into the 48-bit suffix (exactly 12 hexadecimal digits) by `std`'s default
/// hasher, whose algorithm `std` does not promise to keep stable across
/// releases — so the suffix is an opaque unique id rather than a value to depend
/// on. No clock enters the value.
///
/// Seed `0` is reserved for the derived [`DEFAULT_GROUP_NAME`] group and
/// [`ComAmazonawsXrayAwsxrayState::mint_group_arn`] only ever seeds from a
/// counter it has already advanced past zero, so no minted ARN can collide with
/// the reserved group's.
fn group_arn(region: &str, group_name: &str, seed: u64) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    group_name.hash(&mut hasher);
    seed.hash(&mut hasher);
    let suffix = hasher.finish() & 0xffff_ffff_ffff;
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("group/{group_name}/{suffix:012x}"),
    )
}

/// The reserved [`DEFAULT_GROUP_NAME`] group, as a caller in `region` reads it.
///
/// DERIVED, never stored: the group exists in every account and region without
/// being created, so there is no record to insert and no request can remove it.
/// Its ARN is a pure function of the reserved name and the region, which is what
/// keeps it stable across reads — two `GetGroups` calls answer the same ARN —
/// while a caller signing for another region reads that region's ARN.
pub fn default_group(region: &str) -> GroupRecord {
    GroupRecord {
        region: region.to_string(),
        group_name: DEFAULT_GROUP_NAME.to_string(),
        group_arn: group_arn(region, DEFAULT_GROUP_NAME, 0),
        // The reserved group selects every trace, so nothing is configured on
        // it: no filter expression and no insights configuration. Neither is
        // invented here.
        filter_expression: None,
        insights_configuration: None,
        tags: BTreeMap::new(),
    }
}

/// The refusal X-Ray answers when a request supplies BOTH group selectors.
///
/// A group is identified by exactly one of `GroupName` or `GroupARN`, so a
/// request carrying both states two identities at once; the service refuses it
/// with this fixed wording rather than servicing one of them.
pub const CONFLICTING_GROUP_SELECTORS_MESSAGE: &str = "Provide either GroupName or GroupARN";

/// The reserved [`DEFAULT_GROUP_NAME`] group `resource_arn` names, when it names
/// that group's derived ARN.
///
/// The region comes out of the ARN itself (`arn:aws:xray:<region>:…`) and the
/// whole ARN is then re-derived and compared, so only the exact ARN this
/// emulator would answer for that region's reserved group matches.
fn default_group_for_arn(resource_arn: &str) -> Option<GroupRecord> {
    let region = resource_arn.split(':').nth(3)?;
    let reserved = default_group(region);
    (reserved.group_arn == resource_arn).then_some(reserved)
}

/// Why a `(GroupName, GroupARN)` pair names no single stored group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupLookupError {
    /// Neither selector was supplied.
    NoSelector,
    /// BOTH selectors were supplied, so the request states two identities.
    ConflictingSelectors,
    /// A selector was supplied but no group matches it.
    NotFound,
}

impl GroupLookupError {
    /// What to tell the caller. Lives here, beside
    /// [`ComAmazonawsXrayAwsxrayState::find_group`], so `GetGroup` and
    /// `DeleteGroup` cannot drift into two wordings of one refusal.
    ///
    /// A lookup that matched nothing is answered with the fixed text X-Ray uses,
    /// `Group not found`: it names neither selector, so no value from the
    /// request — and nothing about how this emulator represents one — reaches
    /// the caller.
    pub fn message(self) -> &'static str {
        match self {
            Self::NoSelector => "either GroupName or GroupARN must be specified",
            Self::ConflictingSelectors => CONFLICTING_GROUP_SELECTORS_MESSAGE,
            Self::NotFound => "Group not found",
        }
    }
}

/// Why a tag a request carries cannot be applied to, or removed from, a
/// resource. Becomes the `InvalidRequestException` message of whichever tagging
/// operation carried it, so `CreateGroup`, `TagResource` and `UntagResource`
/// cannot drift into three readings of one documented restriction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagRejection {
    /// The key carries [`RESERVED_TAG_KEY_PREFIX`].
    ReservedKeyPrefix {
        /// The refused key.
        key: String,
    },
    /// A key or a value carries a character outside the documented set.
    InvalidCharacter {
        /// `"key"` or `"value"` — which half of the tag is at fault.
        member: &'static str,
        /// The refused text.
        text: String,
        /// The first character of it that is not allowed.
        character: char,
    },
}

impl TagRejection {
    /// What to tell the caller: the member at fault and the restriction it broke.
    pub fn message(&self) -> String {
        match self {
            Self::ReservedKeyPrefix { key } => format!(
                "tag key `{key}` uses the `{RESERVED_TAG_KEY_PREFIX}` prefix, which is reserved \
                 for Amazon Web Services use"
            ),
            Self::InvalidCharacter {
                member,
                text,
                character,
            } => format!(
                "tag {member} `{text}` carries {character:?}, which is not one of the letters, \
                 numbers, space and `_ . : / = + - @` a tag {member} may use"
            ),
        }
    }
}

/// Whether `c` is a character a tag key or a tag value may carry.
///
/// `TagResource$Tags` publishes the set — "Valid values for key and value: a-z,
/// A-Z, 0-9, space, and the following characters: _ . : / = + - and @" — and the
/// `Tag` shape widens its alphanumeric half to "Unicode letters, numbers",
/// which is what [`char::is_alphanumeric`] answers.
fn is_tag_character(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, ' ' | '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@')
}

/// Refuse a tag key a caller may not name.
///
/// The key's length bound (1..=128 characters) is modelled on `TagKey`, so the
/// request boundary refuses an empty or over-long key before any handler runs;
/// what is left is the character set and the reserved prefix.
pub fn validate_tag_key(key: &str) -> Result<(), TagRejection> {
    if let Some(character) = key.chars().find(|c| !is_tag_character(*c)) {
        return Err(TagRejection::InvalidCharacter {
            member: "key",
            text: key.to_string(),
            character,
        });
    }
    if key.starts_with(RESERVED_TAG_KEY_PREFIX) {
        return Err(TagRejection::ReservedKeyPrefix {
            key: key.to_string(),
        });
    }
    Ok(())
}

/// Refuse a whole tag whose key or value is outside what the model documents.
pub fn validate_tag(key: &str, value: &str) -> Result<(), TagRejection> {
    validate_tag_key(key)?;
    if let Some(character) = value.chars().find(|c| !is_tag_character(*c)) {
        return Err(TagRejection::InvalidCharacter {
            member: "value",
            text: value.to_string(),
            character,
        });
    }
    Ok(())
}

/// Why a submitted segment document could not be stored. Becomes one
/// `UnprocessedTraceSegment` in the `PutTraceSegments` response.
#[derive(Debug, Clone)]
pub struct SegmentRejection {
    /// The rejected document's `id`, when it carried a usable one.
    pub id: Option<String>,
    /// [`SEGMENT_ERROR_PARSE`], [`SEGMENT_ERROR_LIMIT_EXCEEDED`], or a member
    /// code built by [`segment_member_code`].
    pub error_code: String,
    /// The code, restated: `Invalid segment. ErrorCode: <ErrorCode>`.
    pub message: String,
}

impl SegmentRejection {
    /// A rejection naming `error_code`, with the `Message` X-Ray states beside
    /// every per-document rejection — the code, restated.
    fn new(id: Option<&str>, error_code: String) -> Self {
        Self {
            id: id.map(str::to_string),
            message: format!("Invalid segment. ErrorCode: {error_code}"),
            error_code,
        }
    }

    /// A document that could not be read as a JSON segment object.
    fn unparseable() -> Self {
        Self::new(None, SEGMENT_ERROR_PARSE.to_string())
    }

    /// A document missing a documented required member.
    fn missing(id: Option<&str>, member: &str) -> Self {
        Self::new(id, segment_member_code("Missing", member))
    }

    /// A document whose member is present but unusable — the wrong JSON type, a
    /// value that is not of the form the member documents, or an instant no
    /// trace could have been recorded at.
    fn invalid(id: Option<&str>, member: &str) -> Self {
        Self::new(id, segment_member_code("Invalid", member))
    }

    /// A document refused because its trace is full.
    fn trace_full(id: &str) -> Self {
        Self::new(Some(id), SEGMENT_ERROR_LIMIT_EXCEEDED.to_string())
    }
}

/// A JSON object's member, read as a string.
type JsonObject = serde_json::Map<String, serde_json::Value>;

/// A required string field, distinguishing "absent" from "wrong type".
fn required_string(
    object: &JsonObject,
    member: &str,
    id: Option<&str>,
) -> Result<String, SegmentRejection> {
    match object.get(member) {
        None | Some(serde_json::Value::Null) => Err(SegmentRejection::missing(id, member)),
        Some(serde_json::Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(SegmentRejection::invalid(id, member)),
    }
}

/// A required epoch-seconds field, distinguishing "absent" from "wrong type"
/// and from an instant no trace could carry.
fn required_seconds(
    object: &JsonObject,
    member: &str,
    id: Option<&str>,
) -> Result<f64, SegmentRejection> {
    match object.get(member) {
        None | Some(serde_json::Value::Null) => Err(SegmentRejection::missing(id, member)),
        Some(serde_json::Value::Number(number)) => {
            let seconds = number
                .as_f64()
                .ok_or_else(|| SegmentRejection::invalid(id, member))?;
            epoch_instant(id, member, seconds)
        }
        Some(_) => Err(SegmentRejection::invalid(id, member)),
    }
}

/// One instant a segment document carries, refused when it is not a time a
/// trace could have been recorded at.
///
/// Checked HERE, where the document is read, so no unusable instant is ever
/// stored: every reader converts a stored instant to the modelled `Timestamp`,
/// and a value that is not finite, or is further from the epoch than
/// [`MAX_SEGMENT_EPOCH_SECONDS`], has no `Timestamp` to convert to. It is the
/// same per-document refusal as any other unusable member, so it takes the same
/// `Invalid<Member>` code.
fn epoch_instant(id: Option<&str>, member: &str, seconds: f64) -> Result<f64, SegmentRejection> {
    if !seconds.is_finite() || seconds.abs() > MAX_SEGMENT_EPOCH_SECONDS {
        return Err(SegmentRejection::invalid(id, member));
    }
    Ok(seconds)
}

/// An optional string field; a wrong-typed value is ignored rather than fatal,
/// because none of these fields is documented as required.
fn optional_string(object: &JsonObject, member: &str) -> Option<String> {
    object
        .get(member)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// An optional boolean flag, absent meaning `false`.
fn flag(object: &JsonObject, member: &str) -> bool {
    object
        .get(member)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Parse one submitted segment document into the trace it belongs to and the
/// record stored under it. `document` is kept verbatim on the record.
pub fn parse_segment_document(document: &str) -> Result<(String, SegmentRecord), SegmentRejection> {
    let value: serde_json::Value =
        serde_json::from_str(document).map_err(|_| SegmentRejection::unparseable())?;
    let object = value
        .as_object()
        .ok_or_else(SegmentRejection::unparseable)?;

    // The segment's own id first: a rejection reports it when the document
    // carried one, so a client can tell which document failed.
    let reported_id = object
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let id_ref = reported_id.as_deref();

    // The four fields `PutTraceSegments` documents as required.
    let trace_id = required_string(object, "trace_id", id_ref)?;
    let id = required_string(object, "id", id_ref)?;
    let name = required_string(object, "name", id_ref)?;
    let start_time = required_seconds(object, "start_time", id_ref)?;

    // A `trace_id` that is not of the documented form names no trace X-Ray
    // could index, so the document is refused per-document — the rest of the
    // batch still lands — rather than stored under an id no reader can ask for.
    if !is_trace_id(&trace_id) {
        return Err(SegmentRejection::invalid(id_ref, "trace_id"));
    }

    // "Specify either an end_time or in_progress" — a document with neither is
    // treated as in progress rather than refused, so a partial trace is still
    // readable.
    let end_time = match object.get("end_time") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(number)) => match number.as_f64() {
            Some(seconds) => Some(epoch_instant(id_ref, "end_time", seconds)?),
            None => None,
        },
        Some(_) => return Err(SegmentRejection::invalid(id_ref, "end_time")),
    };
    let in_progress = flag(object, "in_progress") || end_time.is_none();

    let segment = SegmentRecord {
        id,
        parent_id: optional_string(object, "parent_id"),
        name,
        document: document.to_string(),
        start_time,
        end_time,
        in_progress,
        error: flag(object, "error"),
        fault: flag(object, "fault"),
        throttle: flag(object, "throttle"),
        http: parse_http(object.get("http")),
        annotations: parse_annotations(object),
        origin: optional_string(object, "origin"),
        account_id: optional_string(object, "account_id"),
        user: optional_string(object, "user"),
        resource_arn: optional_string(object, "resource_arn"),
        segment_type: optional_string(object, "type"),
        namespace: optional_string(object, "namespace"),
        exceptions: parse_cause(object.get("cause")),
        subsegments: parse_subsegments(id_ref, object.get("subsegments"), 1, &mut 0)?,
        // NOT read from the document: `inferred` is service-assigned, and the
        // segment schema `PutTraceSegments` documents has no such member, so a
        // submitted document cannot claim to be a service the trace only
        // called. [`inferred_segment`] is the one place it is set.
        inferred: false,
    };
    Ok((trace_id, segment))
}

/// An entity's `annotations`, as submitted.
fn parse_annotations(object: &JsonObject) -> BTreeMap<String, serde_json::Value> {
    object
        .get("annotations")
        .and_then(serde_json::Value::as_object)
        .map(|annotations| {
            annotations
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The exceptions an entity's `cause` block carries. `cause` is either an object
/// with an `exceptions` array or a bare cause id, and only the former names
/// exceptions.
fn parse_cause(value: Option<&serde_json::Value>) -> Vec<CauseException> {
    let Some(exceptions) = value
        .and_then(serde_json::Value::as_object)
        .and_then(|cause| cause.get("exceptions"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    exceptions
        .iter()
        .filter_map(serde_json::Value::as_object)
        .map(|exception| CauseException {
            // X-Ray reports the exception's `type` as the root cause
            // exception's `Name` (`RootCauseException$Name`).
            name: optional_string(exception, "type"),
            message: optional_string(exception, "message"),
        })
        .collect()
}

/// An entity's `subsegments`, recursively, at `depth` levels below the segment.
///
/// Lenient about one subsegment where the document itself is strict: a
/// subsegment is part of an already accepted document, so an entry that is not
/// an object, or carries no usable `name`, is dropped rather than refusing the
/// whole document — only the segment's own members are documented as required.
/// The COUNT one document contributes is bounded by
/// [`MAX_SUBSEGMENTS_PER_SEGMENT`] (`budget`, shared across the whole nesting),
/// and a tree DEEPER than [`MAX_SUBSEGMENT_DEPTH`] is refused per document,
/// which is what bounds every later walk of it: the readers recurse once per
/// level, and a stack this deep is the one caller-driven cost a reader cannot
/// recover from.
fn parse_subsegments(
    id: Option<&str>,
    value: Option<&serde_json::Value>,
    depth: usize,
    budget: &mut usize,
) -> Result<Vec<SubsegmentRecord>, SegmentRejection> {
    let Some(array) = value.and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    if depth > MAX_SUBSEGMENT_DEPTH {
        return Err(SegmentRejection::invalid(id, "subsegments"));
    }
    let mut out = Vec::new();
    for object in array.iter().filter_map(serde_json::Value::as_object) {
        if *budget >= MAX_SUBSEGMENTS_PER_SEGMENT {
            break;
        }
        let Some(name) = optional_string(object, "name") else {
            continue;
        };
        *budget += 1;
        let end_time = object.get("end_time").and_then(serde_json::Value::as_f64);
        out.push(SubsegmentRecord {
            id: optional_string(object, "id"),
            name,
            namespace: optional_string(object, "namespace"),
            start_time: object.get("start_time").and_then(serde_json::Value::as_f64),
            end_time,
            in_progress: flag(object, "in_progress") || end_time.is_none(),
            error: flag(object, "error"),
            fault: flag(object, "fault"),
            throttle: flag(object, "throttle"),
            http: parse_http(object.get("http")),
            annotations: parse_annotations(object),
            exceptions: parse_cause(object.get("cause")),
            subsegments: parse_subsegments(id, object.get("subsegments"), depth + 1, budget)?,
        });
    }
    Ok(out)
}

/// Lift the `http.request` / `http.response` fields a `TraceSummary` reports.
fn parse_http(value: Option<&serde_json::Value>) -> Option<HttpRecord> {
    let http = value?.as_object()?;
    let request = http.get("request").and_then(serde_json::Value::as_object);
    let response = http.get("response").and_then(serde_json::Value::as_object);
    let request_field = |member: &str| request.and_then(|object| optional_string(object, member));
    Some(HttpRecord {
        client_ip: request_field("client_ip"),
        http_method: request_field("method"),
        url: request_field("url"),
        user_agent: request_field("user_agent"),
        response_status: response
            .and_then(|object| object.get("status"))
            .and_then(serde_json::Value::as_i64)
            .and_then(|status| i32::try_from(status).ok()),
    })
}

/// The `namespace` that makes a subsegment a call OUT of the service it sits in,
/// so the work it covers is a downstream service's.
pub const REMOTE_NAMESPACE: &str = "remote";

/// The id of the segment synthesised for the remote subsegment `remote_id` of
/// `trace_id`: 16 hexadecimal digits, as a segment id is.
///
/// Service-assigned and derived only from what it identifies, so it is the same
/// on every read and distinct per remote subsegment — re-submitting the same
/// document replaces the inferred segment it already derived instead of adding a
/// second one. The mixing is [`service_common::stable_hex_id`], the one
/// deterministic id helper the service crates share: this id travels on the
/// wire, so it has to be reproducible across builds as well as across reads,
/// which `std`'s default hasher does not promise. It is still an opaque id, not
/// a value to depend on.
fn inferred_segment_id(trace_id: &str, remote_id: &str) -> String {
    ::service_common::stable_hex_id(&["inferred", trace_id, remote_id])
}

/// The segments X-Ray synthesises for the downstream services `segment` only
/// CALLED: one per subsegment in its tree whose `namespace` is
/// [`REMOTE_NAMESPACE`] and that carries an id to parent the synthesised segment
/// on.
///
/// Each is a top-level segment of the same trace whose `parent_id` is the remote
/// subsegment's id and whose document carries `"inferred": true`, so a reader
/// building a service map sees the downstream node the caller recorded. Its
/// times, `http` block, outcome flags and `cause` are the subsegment's: the
/// remote subsegment is the only record of what the downstream service did.
pub fn inferred_segments(trace_id: &str, segment: &SegmentRecord) -> Vec<SegmentRecord> {
    fn walk(
        trace_id: &str,
        subsegments: &[SubsegmentRecord],
        timed_from: f64,
        out: &mut Vec<SegmentRecord>,
    ) {
        for subsegment in subsegments {
            if let (Some(remote_id), true) = (
                subsegment.id.as_deref(),
                subsegment.namespace.as_deref() == Some(REMOTE_NAMESPACE),
            ) {
                out.push(inferred_segment(trace_id, remote_id, subsegment, timed_from));
            }
            walk(trace_id, &subsegment.subsegments, timed_from, out);
        }
    }

    let mut out = Vec::new();
    if segment.inferred {
        // A synthesised segment never derives further segments from itself. It
        // carries no subsegments either, so this is belt and braces: only
        // `inferred_segment` sets the flag, never a submitted document.
        return out;
    }
    walk(
        trace_id,
        &segment.subsegments,
        segment.start_time,
        &mut out,
    );
    out
}

/// One inferred segment: the record, and the document a reader returns for it.
///
/// `timed_from` is the calling segment's own `start_time`, which is when a
/// remote subsegment that carries no `start_time` of its own is timed from: it
/// is the only instant known for it.
fn inferred_segment(
    trace_id: &str,
    remote_id: &str,
    remote: &SubsegmentRecord,
    timed_from: f64,
) -> SegmentRecord {
    let start_time = remote.start_time.unwrap_or(timed_from);
    let id = inferred_segment_id(trace_id, remote_id);
    let mut document = serde_json::Map::new();
    document.insert("id".to_string(), serde_json::json!(id));
    document.insert("name".to_string(), serde_json::json!(remote.name));
    document.insert("trace_id".to_string(), serde_json::json!(trace_id));
    document.insert("parent_id".to_string(), serde_json::json!(remote_id));
    document.insert("start_time".to_string(), serde_json::json!(start_time));
    match remote.end_time {
        Some(end_time) => {
            document.insert("end_time".to_string(), serde_json::json!(end_time));
        }
        None => {
            document.insert("in_progress".to_string(), serde_json::json!(true));
        }
    }
    document.insert("inferred".to_string(), serde_json::json!(true));
    for (member, set) in [
        ("fault", remote.fault),
        ("error", remote.error),
        ("throttle", remote.throttle),
    ] {
        if set {
            document.insert(member.to_string(), serde_json::json!(true));
        }
    }
    if let Some(http) = &remote.http {
        document.insert("http".to_string(), http_document(http));
    }
    if !remote.exceptions.is_empty() {
        document.insert("cause".to_string(), cause_document(&remote.exceptions));
    }

    SegmentRecord {
        id,
        parent_id: Some(remote_id.to_string()),
        name: remote.name.clone(),
        document: serde_json::Value::Object(document).to_string(),
        start_time,
        end_time: remote.end_time,
        in_progress: remote.in_progress,
        error: remote.error,
        fault: remote.fault,
        throttle: remote.throttle,
        http: remote.http.clone(),
        annotations: BTreeMap::new(),
        origin: None,
        account_id: None,
        user: None,
        resource_arn: None,
        // An inferred segment stands for a downstream SERVICE, not for work
        // inside one, so it is no `type: "subsegment"` document; and it carries
        // no `namespace` of its own — the namespace that produced it was the
        // remote subsegment's.
        segment_type: None,
        namespace: None,
        exceptions: remote.exceptions.clone(),
        subsegments: Vec::new(),
        inferred: true,
    }
}

/// An `http` block, as a segment document carries it.
fn http_document(http: &HttpRecord) -> serde_json::Value {
    let mut request = serde_json::Map::new();
    for (member, value) in [
        ("client_ip", &http.client_ip),
        ("method", &http.http_method),
        ("url", &http.url),
        ("user_agent", &http.user_agent),
    ] {
        if let Some(value) = value {
            request.insert(member.to_string(), serde_json::json!(value));
        }
    }
    let mut block = serde_json::Map::new();
    if !request.is_empty() {
        block.insert("request".to_string(), serde_json::Value::Object(request));
    }
    if let Some(status) = http.response_status {
        block.insert(
            "response".to_string(),
            serde_json::json!({ "status": status }),
        );
    }
    serde_json::Value::Object(block)
}

/// A `cause` block, as a segment document carries it.
fn cause_document(exceptions: &[CauseException]) -> serde_json::Value {
    let exceptions: Vec<serde_json::Value> = exceptions
        .iter()
        .map(|exception| {
            let mut entry = serde_json::Map::new();
            if let Some(name) = &exception.name {
                entry.insert("type".to_string(), serde_json::json!(name));
            }
            if let Some(message) = &exception.message {
                entry.insert("message".to_string(), serde_json::json!(message));
            }
            serde_json::Value::Object(entry)
        })
        .collect();
    serde_json::json!({ "exceptions": exceptions })
}

/// One entity's recorded outcome.
///
/// Two facts, because the model reports two: the EXCLUSIVE class the counters
/// split a call into ([`Classification`]), and whether the entity recorded a
/// 400-series error at all — which a throttle's own 429 status also is. They
/// disagree exactly where X-Ray does: a throttled request is counted under
/// `ErrorStatistics$ThrottleCount` rather than `$OtherCount`, and still reported
/// by `TraceSummary$HasError` when its status says 429.
#[derive(Debug, Clone, Copy)]
pub struct Outcome {
    /// The one class this entity is counted as.
    pub class: Classification,
    /// Whether it recorded a 400-series error ([`recorded_error`]).
    pub error_400: bool,
}

impl SubsegmentRecord {
    /// The `http.response.status` this subsegment recorded, if any.
    pub fn http_status(&self) -> Option<i32> {
        self.http.as_ref().and_then(|http| http.response_status)
    }

    /// This subsegment's outcome, from its own flags and recorded status.
    pub fn classification(&self) -> Classification {
        classify(self.fault, self.error, self.throttle, self.http_status())
    }

    /// Whether this subsegment recorded a 400-series error.
    pub fn is_error(&self) -> bool {
        recorded_error(self.error, self.throttle, self.http_status())
    }

    /// This subsegment's recorded outcome: its exclusive class, and whether it
    /// recorded a 400-series error.
    pub fn outcome(&self) -> Outcome {
        Outcome {
            class: self.classification(),
            error_400: self.is_error(),
        }
    }

    /// Whether this subsegment recorded a 500-series fault.
    pub fn is_fault(&self) -> bool {
        recorded_fault(self.fault, self.http_status())
    }

    /// Whether this subsegment recorded a throttling error.
    pub fn is_throttle(&self) -> bool {
        recorded_throttle(self.throttle, self.http_status())
    }

    /// Whether this subsegment is a call OUT of the service it sits in.
    pub fn is_remote(&self) -> bool {
        self.namespace.as_deref() == Some(REMOTE_NAMESPACE)
    }

    /// This subsegment as one frame of a root cause's entity path.
    fn entity(&self) -> RootCauseEntityView {
        RootCauseEntityView {
            name: self.name.clone(),
            exceptions: self.exceptions.clone(),
            remote: self.is_remote(),
        }
    }

    /// Every entity in this subsegment's tree, itself first.
    ///
    /// Recurses once per level of nesting, which the ingest bounds at
    /// [`MAX_SUBSEGMENT_DEPTH`] ([`parse_subsegments`]). `depth` re-states that
    /// bound HERE, so the walk's stack is bounded by this module rather than by
    /// what a request managed to store: past the bound the walk stops instead of
    /// recursing, which no stored tree can reach.
    fn walk<'a>(&'a self, depth: usize, out: &mut Vec<&'a Self>) {
        out.push(self);
        if depth >= MAX_SUBSEGMENT_DEPTH {
            return;
        }
        for child in &self.subsegments {
            child.walk(depth + 1, out);
        }
    }

    /// The entity path from this subsegment down to the deepest entity inside it
    /// classified `class`, or `None` when its tree holds none.
    ///
    /// A REMOTE subsegment is not walked: what happened on the other side of a
    /// call out belongs to the downstream service, which is the inferred segment
    /// synthesised for it ([`inferred_segments`]) and carries its own root cause.
    ///
    /// `depth` bounds the recursion at [`MAX_SUBSEGMENT_DEPTH`], the same bound
    /// the ingest refuses a deeper document with.
    fn root_cause_path(
        &self,
        class: Classification,
        depth: usize,
    ) -> Option<Vec<RootCauseEntityView>> {
        if self.is_remote() {
            return None;
        }
        if depth < MAX_SUBSEGMENT_DEPTH {
            for child in &self.subsegments {
                if let Some(mut path) = child.root_cause_path(class, depth + 1) {
                    path.insert(0, self.entity());
                    return Some(path);
                }
            }
        }
        (self.classification() == class).then(|| vec![self.entity()])
    }
}

/// Whether a recorded outcome is a 400-series error: the `error` flag the
/// document set, or an `http.response.status` in the 400 series — which the 429
/// of a throttled request is one of.
///
/// `throttle` is consulted for ONE case, the only one a recorded response pins:
/// a document whose FLAGS say `throttle: true, error: true` and that recorded no
/// status comes back as a throttle and NOT as an error (X-Ray reclassifies a
/// declared throttle rather than unioning it). A 400-series STATUS stands on its
/// own, because the model words `TraceSummary$HasError` as "The root segment
/// document has a 400 series error" and `$HasThrottle` as "One or more of the
/// segment documents has a 429 throttling error"
/// (`smithy_query(query="doc",shape="TraceSummary",member="HasError")`) without
/// making the two exclusive: a 429 IS a 400-series status, so a request the
/// service throttled is reported as both.
fn recorded_error(error: bool, throttle: bool, status: Option<i32>) -> bool {
    (error && !throttle) || status.is_some_and(|status| (400..500).contains(&status))
}

/// Whether a recorded outcome is a 500-series fault: the `fault` flag, or an
/// `http.response.status` of 500 or above.
fn recorded_fault(fault: bool, status: Option<i32>) -> bool {
    fault || status.is_some_and(|status| status >= 500)
}

/// Whether a recorded outcome is a throttling error: the `throttle` flag, or an
/// `http.response.status` of 429.
fn recorded_throttle(throttle: bool, status: Option<i32>) -> bool {
    throttle || status == Some(429)
}

impl SegmentRecord {
    /// The `http.response.status` this segment recorded, if any.
    pub fn http_status(&self) -> Option<i32> {
        self.http.as_ref().and_then(|http| http.response_status)
    }

    /// This segment's own outcome, from its own flags and recorded status.
    pub fn classification(&self) -> Classification {
        classify(self.fault, self.error, self.throttle, self.http_status())
    }

    /// How long this segment took, in seconds; `None` while it is in progress.
    pub fn duration(&self) -> Option<f64> {
        self.end_time.map(|end| end - self.start_time)
    }

    /// The latest instant this segment is known to have reached: its `end_time`
    /// when it has one, else its `start_time`.
    pub fn latest(&self) -> f64 {
        self.end_time.unwrap_or(self.start_time)
    }

    /// The outcomes recorded anywhere in this segment's document — the segment
    /// itself and every subsegment inside it.
    pub fn outcomes(&self) -> Vec<Outcome> {
        let mut entities = Vec::new();
        for subsegment in &self.subsegments {
            subsegment.walk(1, &mut entities);
        }
        let mut out = vec![self.outcome()];
        out.extend(entities.iter().map(|entity| entity.outcome()));
        out
    }

    /// Those outcomes' exclusive classes, which is what a counter reads.
    pub fn classifications(&self) -> Vec<Classification> {
        self.outcomes()
            .into_iter()
            .map(|outcome| outcome.class)
            .collect()
    }

    /// Whether this segment recorded a 400-series error — its `error` flag, or
    /// an `http.response.status` in the 400 series.
    pub fn is_error(&self) -> bool {
        recorded_error(self.error, self.throttle, self.http_status())
    }

    /// This segment's recorded outcome: its exclusive class, and whether it
    /// recorded a 400-series error.
    pub fn outcome(&self) -> Outcome {
        Outcome {
            class: self.classification(),
            error_400: self.is_error(),
        }
    }

    /// Whether this segment recorded a 500-series fault — its `fault` flag, or
    /// an `http.response.status` of 500 or above.
    pub fn is_fault(&self) -> bool {
        recorded_fault(self.fault, self.http_status())
    }

    /// Whether this segment recorded a throttling error — its `throttle` flag,
    /// or an `http.response.status` of 429.
    pub fn is_throttle(&self) -> bool {
        recorded_throttle(self.throttle, self.http_status())
    }

    /// Whether this document is a SUBSEGMENT sent on its own, which the X-Ray
    /// segment-document schema marks with `type: "subsegment"`.
    ///
    /// Such a document records work INSIDE its parent's service — a function
    /// call, a query, a request that left the process — so it is not a service
    /// of the graph in its own right.
    pub fn is_subsegment(&self) -> bool {
        self.segment_type.as_deref() == Some("subsegment")
    }

    /// This segment as one frame of a root cause's entity path. An inferred
    /// segment stands for a service this trace only called, so its frame is
    /// remote.
    fn entity(&self) -> RootCauseEntityView {
        RootCauseEntityView {
            name: self.name.clone(),
            exceptions: self.exceptions.clone(),
            remote: self.inferred,
        }
    }

    /// This segment's service identity, as a root cause reports it.
    fn service(&self, entity_path: Vec<RootCauseEntityView>) -> RootCauseServiceView {
        RootCauseServiceView {
            name: self.name.clone(),
            r#type: self.origin.clone(),
            account_id: self.account_id.clone(),
            inferred: self.inferred,
            entity_path,
        }
    }

    /// The entity path from this segment down to the deepest entity inside it
    /// classified `class`, or `None` when it holds none.
    ///
    /// The walk is bounded by [`MAX_SUBSEGMENT_DEPTH`], the depth the ingest
    /// refuses a deeper document with, so the stack it costs is this module's
    /// choice rather than the request's.
    fn root_cause_path(&self, class: Classification) -> Option<Vec<RootCauseEntityView>> {
        for child in &self.subsegments {
            if let Some(mut path) = child.root_cause_path(class, 1) {
                path.insert(0, self.entity());
                return Some(path);
            }
        }
        (self.classification() == class).then(|| vec![self.entity()])
    }

    /// The frames from this segment down to the entity with id `entity_id`, when
    /// its tree holds one.
    ///
    /// Bounded by [`MAX_SUBSEGMENT_DEPTH`] the same way.
    fn frames_to(&self, entity_id: &str) -> Option<Vec<RootCauseEntityView>> {
        fn frames(
            subsegments: &[SubsegmentRecord],
            entity_id: &str,
            depth: usize,
        ) -> Option<Vec<RootCauseEntityView>> {
            if depth > MAX_SUBSEGMENT_DEPTH {
                return None;
            }
            for subsegment in subsegments {
                if subsegment.id.as_deref() == Some(entity_id) {
                    return Some(vec![subsegment.entity()]);
                }
                if let Some(mut path) = frames(&subsegment.subsegments, entity_id, depth + 1) {
                    path.insert(0, subsegment.entity());
                    return Some(path);
                }
            }
            None
        }

        if self.id == entity_id {
            return Some(vec![self.entity()]);
        }
        frames(&self.subsegments, entity_id, 1).map(|mut path| {
            path.insert(0, self.entity());
            path
        })
    }
}

/// One service of a root cause: the chain of them is the path through the
/// service graph the fault travelled (`FaultRootCause$Services`).
#[derive(Debug, Clone)]
pub struct RootCauseServiceView {
    /// The service's name.
    pub name: String,
    /// Its `origin`, which is what X-Ray reports as the service type.
    pub r#type: Option<String>,
    /// Its `account_id`, when the document named one.
    pub account_id: Option<String>,
    /// Whether this service was inferred from a call to it rather than
    /// instrumented itself.
    pub inferred: bool,
    /// The frames inside this service the root cause ran through.
    pub entity_path: Vec<RootCauseEntityView>,
}

/// One frame of a root cause's entity path.
#[derive(Debug, Clone)]
pub struct RootCauseEntityView {
    /// The entity's name.
    pub name: String,
    /// The exceptions this entity's own `cause` block carried.
    pub exceptions: Vec<CauseException>,
    /// Whether this entity is a call out of its service (a dependency frame)
    /// rather than work inside it (a pipeline frame).
    pub remote: bool,
}

/// Which root causes a reader is asking for. X-Ray reports three lists on a
/// `TraceSummary`; the two below are the ones a submitted document states an
/// outcome for, and `ResponseTimeRootCauses` is derived from a response-time
/// analysis this emulator does not perform, so it names no kind here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootCauseKind {
    /// `TraceSummary$FaultRootCauses`.
    Fault,
    /// `TraceSummary$ErrorRootCauses`.
    Error,
}

impl RootCauseKind {
    /// The entity classification this kind of root cause is built from.
    pub fn classification(self) -> Classification {
        match self {
            Self::Fault => Classification::Fault,
            Self::Error => Classification::Error,
        }
    }
}

/// One root cause: which services and entities the outcome travelled through,
/// and whether the trace's client saw it.
#[derive(Debug, Clone)]
pub struct RootCauseView {
    /// Root first, the service that recorded the outcome last.
    pub services: Vec<RootCauseServiceView>,
    /// Whether the trace's own root segment recorded the same outcome, which is
    /// what makes a root cause client-impacting.
    pub client_impacting: bool,
}

impl TraceRecord {
    /// Widen the trace's bounds to include one newly appended segment. O(1), so
    /// an ingest never re-walks what is already stored.
    fn extend_bounds(&mut self, segment: &SegmentRecord) {
        let latest = segment.end_time.unwrap_or(segment.start_time);
        if self.segments.is_empty() {
            self.first_start = segment.start_time;
            self.last_end = latest;
        } else {
            self.first_start = self.first_start.min(segment.start_time);
            self.last_end = self.last_end.max(latest);
        }
    }

    /// Recompute the trace's time bounds from its segments.
    ///
    /// Only a REPLACED segment needs this — replacing can move a bound inwards,
    /// which [`extend_bounds`](Self::extend_bounds) cannot — and the walk is
    /// bounded by [`MAX_SEGMENTS_PER_TRACE`].
    fn recompute_bounds(&mut self) {
        self.first_start = self
            .segments
            .iter()
            .map(|segment| segment.start_time)
            .fold(f64::INFINITY, f64::min);
        self.last_end = self
            .segments
            .iter()
            .map(|segment| segment.end_time.unwrap_or(segment.start_time))
            .fold(f64::NEG_INFINITY, f64::max);
        if self.segments.is_empty() {
            self.first_start = 0.0;
            self.last_end = 0.0;
        }
    }

    /// "The length of time in seconds between the start time of the earliest
    /// segment that started and the end time of the last segment that
    /// completed" (`Trace$Duration` / `TraceSummary$Duration`).
    pub fn duration(&self) -> Option<f64> {
        (!self.segments.is_empty()).then_some(self.last_end - self.first_start)
    }

    /// The trace's root segment: the first one with no `parent_id`.
    pub fn root(&self) -> Option<&SegmentRecord> {
        self.segments
            .iter()
            .find(|segment| segment.parent_id.is_none())
    }

    /// The segment stored under `id`, in O(log n) through the id index rather
    /// than a scan \u2014 which is how a graph resolves a segment's `parent_id`.
    pub fn segment(&self, id: &str) -> Option<&SegmentRecord> {
        self.segment_index
            .get(id)
            .and_then(|at| self.segments.get(*at))
    }

    /// "The length of time in seconds between the start and end times of the
    /// root segment" (`TraceSummary$ResponseTime`).
    pub fn response_time(&self) -> Option<f64> {
        let root = self.root()?;
        Some(root.end_time? - root.start_time)
    }

    /// Whether any entity of this trace was classified `class`.
    ///
    /// An entity is a segment or a subsegment inside one, and each is classified
    /// EXCLUSIVELY ([`Classification`]), so a document carrying both `throttle`
    /// and `error` contributes a throttle and no error.
    pub fn has(&self, class: Classification) -> bool {
        self.segments
            .iter()
            .flat_map(SegmentRecord::classifications)
            .any(|recorded| recorded == class)
    }

    /// Whether the ROOT segment document recorded `class` — the root segment
    /// itself, or an entity inside the document that carried it.
    ///
    /// The scope matters, and it is the model's: `TraceSummary$HasFault` is
    /// documented as "The root segment document has a 500 series error" and
    /// `$HasError` as "…a 400 series error"
    /// (`smithy_query(query="doc",shape="TraceSummary",member="HasFault")`),
    /// while `$HasThrottle` is "One or more of the segment documents has a 429
    /// throttling error" — so the first two are read off the root document and
    /// only the third is a union over the trace.
    pub fn root_document_has(&self, class: Classification) -> bool {
        self.root()
            .is_some_and(|root| root.classifications().contains(&class))
    }

    /// Whether the root segment document recorded a 400-series error
    /// (`TraceSummary$HasError`): the root segment itself, or an entity inside
    /// its document.
    ///
    /// Read off each entity's [`Outcome::error_400`] rather than its exclusive
    /// class, because the two differ for a throttled request: a 429 is a
    /// 400-series status, so the request is reported here AND by
    /// [`has_throttle`](Self::has_throttle), while the counters still split it
    /// under `ErrorStatistics$ThrottleCount` ([`recorded_error`]).
    pub fn has_error(&self) -> bool {
        self.root().is_some_and(|root| {
            root.outcomes().iter().any(|outcome| outcome.error_400)
        })
    }

    /// Whether the root segment document recorded a 500-series fault
    /// (`TraceSummary$HasFault`).
    pub fn has_fault(&self) -> bool {
        self.root_document_has(Classification::Fault)
    }

    /// Whether ANY segment document recorded a throttling error
    /// (`TraceSummary$HasThrottle`).
    pub fn has_throttle(&self) -> bool {
        self.has(Classification::Throttle)
    }

    /// Whether any segment is still in progress (`TraceSummary$IsPartial`).
    pub fn is_partial(&self) -> bool {
        self.segments.iter().any(|segment| segment.in_progress)
    }

    /// The root causes of `kind` this trace recorded
    /// (`TraceSummary$FaultRootCauses` / `$ErrorRootCauses`).
    ///
    /// One per segment whose own tree recorded the outcome: its `Services` are
    /// the chain of services from the trace's root down to that segment, each
    /// carrying the frames inside it the outcome ran through — the frame that
    /// called out (a dependency frame, `Remote`) or the work it did itself (a
    /// pipeline frame) — and the deepest frame carries the exceptions its own
    /// `cause` block named.
    ///
    /// Bounded in both dimensions a trace can grow in: at most
    /// [`MAX_ROOT_CAUSES_PER_TRACE`] causes are derived, and each chain walks
    /// each of the trace's segments at most once, resolving every hop through
    /// one entity index built here rather than by re-scanning the segments per
    /// hop. A trace that recorded more outcomes than the cap therefore reports
    /// the first of them rather than making one read do super-linear work.
    pub fn root_causes(&self, kind: RootCauseKind) -> Vec<RootCauseView> {
        let class = kind.classification();
        let client_impacting = self
            .root()
            .is_some_and(|root| root.classification() == class);
        let owners = self.entity_owners();
        let mut causes = Vec::new();
        for (index, segment) in self.segments.iter().enumerate() {
            if causes.len() >= MAX_ROOT_CAUSES_PER_TRACE {
                break;
            }
            if let Some(path) = segment.root_cause_path(class) {
                causes.push(RootCauseView {
                    services: self.service_chain(index, path, &owners),
                    client_impacting,
                });
            }
        }
        causes
    }

    /// Every entity id in this trace — each segment's own, and each id in its
    /// `subsegments` tree — mapped to the segment that holds it.
    ///
    /// Built once per derivation, so resolving a `parent_id` to the service it
    /// belongs to is a lookup rather than a scan of every segment's tree. The
    /// filter language's `edge(…)` scope reads the same index for the same
    /// reason ([`filter`]).
    ///
    /// The tree walk is bounded by [`MAX_SUBSEGMENT_DEPTH`], the depth the
    /// ingest refuses a deeper document with, so no stored tree can cost this
    /// read more stack than this module allows.
    pub fn entity_owners(&self) -> BTreeMap<&str, usize> {
        fn walk<'a>(
            at: usize,
            subsegments: &'a [SubsegmentRecord],
            depth: usize,
            out: &mut BTreeMap<&'a str, usize>,
        ) {
            if depth > MAX_SUBSEGMENT_DEPTH {
                return;
            }
            for subsegment in subsegments {
                if let Some(id) = subsegment.id.as_deref() {
                    out.entry(id).or_insert(at);
                }
                walk(at, &subsegment.subsegments, depth + 1, out);
            }
        }

        let mut owners = BTreeMap::new();
        for (at, segment) in self.segments.iter().enumerate() {
            owners.entry(segment.id.as_str()).or_insert(at);
            walk(at, &segment.subsegments, 1, &mut owners);
        }
        owners
    }

    /// The services a root cause recorded at `index` travelled through, root
    /// first: each ancestor segment contributes the frames from itself down to
    /// the entity the next segment hangs off, and `path` is the frames inside the
    /// segment that recorded the outcome.
    fn service_chain(
        &self,
        index: usize,
        path: Vec<RootCauseEntityView>,
        owners: &BTreeMap<&str, usize>,
    ) -> Vec<RootCauseServiceView> {
        let mut chain = vec![self.segments[index].service(path)];
        let mut seen = BTreeSet::from([index]);
        let mut current = index;
        // One hop per segment at most: a hop that lands on a segment already
        // walked ends the chain, so a `parent_id` cycle cannot loop here, and
        // each segment's subsegment tree is walked at most once per chain.
        while let Some(parent_id) = self.segments[current].parent_id.as_deref() {
            let Some(&at) = owners.get(parent_id) else {
                break;
            };
            if !seen.insert(at) {
                break;
            }
            let Some(frames) = self.segments[at].frames_to(parent_id) else {
                break;
            };
            chain.push(self.segments[at].service(frames));
            current = at;
        }
        chain.reverse();
        chain
    }

    /// This trace with only the segments whose `start_time` falls in the
    /// INCLUSIVE interval `[start, end]`, with its id index and time bounds
    /// recomputed over what is left.
    ///
    /// The index is rebuilt rather than carried over, so a reader that resolves
    /// a `parent_id` through the trimmed trace — a derived graph does — can
    /// never reach a segment the window excluded.
    fn within(&self, start: f64, end: f64) -> Self {
        let segments: Vec<SegmentRecord> = self
            .segments
            .iter()
            .filter(|segment| segment.start_time >= start && segment.start_time <= end)
            .cloned()
            .collect();
        let segment_index = segments
            .iter()
            .enumerate()
            .map(|(at, segment)| (segment.id.clone(), at))
            .collect();
        let mut trimmed = Self {
            id: self.id.clone(),
            segments,
            segment_index,
            first_start: 0.0,
            last_end: 0.0,
            limit_exceeded: self.limit_exceeded,
            ingest_seq: self.ingest_seq,
        };
        trimmed.recompute_bounds();
        trimmed
    }
}

impl GroupRecord {
    /// This record as the modelled `Group`.
    pub fn to_group(&self) -> Group {
        Group {
            group_name: Some(self.group_name.clone()),
            group_arn: Some(self.group_arn.clone()),
            filter_expression: self.filter_expression.clone(),
            insights_configuration: self
                .insights_configuration
                .as_ref()
                .map(InsightsConfigurationRecord::to_configuration),
        }
    }

    /// This record as the modelled `GroupSummary`.
    pub fn to_summary(&self) -> GroupSummary {
        GroupSummary {
            group_name: Some(self.group_name.clone()),
            group_arn: Some(self.group_arn.clone()),
            filter_expression: self.filter_expression.clone(),
            insights_configuration: self
                .insights_configuration
                .as_ref()
                .map(InsightsConfigurationRecord::to_configuration),
        }
    }
}

impl InsightsConfigurationRecord {
    /// This record as the modelled `InsightsConfiguration`.
    pub fn to_configuration(&self) -> InsightsConfiguration {
        InsightsConfiguration {
            insights_enabled: self.insights_enabled,
            notifications_enabled: self.notifications_enabled,
        }
    }

    /// Store a submitted `InsightsConfiguration`.
    pub fn from_configuration(configuration: &InsightsConfiguration) -> Self {
        Self {
            insights_enabled: configuration.insights_enabled,
            notifications_enabled: configuration.notifications_enabled,
        }
    }
}

impl ComAmazonawsXrayAwsxrayState {
    /// Replace the encryption configuration and start its `UPDATING` span,
    /// returning the document to report — which is the stored one, as of `now`.
    pub fn put_encryption_config(
        &mut self,
        encryption_type: &str,
        key_id: Option<String>,
        now: Instant,
    ) -> EncryptionConfig {
        self.encryption = EncryptionConfigRecord {
            encryption_type: encryption_type.to_string(),
            key_id,
            active_due: Some(now + ENCRYPTION_SETTLE_AFTER),
        };
        self.encryption.to_config(now)
    }

    /// The stored encryption configuration as of `now`, materialising the
    /// `UPDATING` -> `ACTIVE` transition the clock already owed.
    pub fn encryption_config(&mut self, now: Instant) -> EncryptionConfig {
        self.encryption.settle(now);
        self.encryption.to_config(now)
    }

    /// Store one parsed segment under its trace, together with the segments
    /// X-Ray infers from it for the downstream services it called
    /// ([`inferred_segments`]).
    ///
    /// Bounded in both directions: a trace holds at most
    /// [`MAX_SEGMENTS_PER_TRACE`] segments (a further new one is refused, and
    /// marks the trace `limit_exceeded`), and the store holds at most
    /// [`MAX_TRACES`] traces (a new trace past that evicts the least recently
    /// ingested one).
    ///
    /// An inferred segment that does not fit marks the trace `limit_exceeded`
    /// like any other refused segment, but is not reported as an
    /// `UnprocessedTraceSegment`: the caller submitted no such document.
    pub fn ingest_segment(
        &mut self,
        trace_id: &str,
        segment: SegmentRecord,
    ) -> Result<(), SegmentRejection> {
        let inferred = inferred_segments(trace_id, &segment);
        self.store_segment(trace_id, segment)?;
        for extra in inferred {
            // The rejection is dropped deliberately: `store_segment` has
            // already recorded it on the trace as `limit_exceeded`.
            let _ = self.store_segment(trace_id, extra);
        }
        Ok(())
    }

    /// Store one segment under its trace, replacing a segment already held under
    /// the same id — which is how an in-progress segment is overwritten by the
    /// completed one, and how a re-submitted document replaces the inferred
    /// segment it already derived rather than adding a second one.
    fn store_segment(
        &mut self,
        trace_id: &str,
        segment: SegmentRecord,
    ) -> Result<(), SegmentRejection> {
        if !self.traces.contains_key(trace_id) && self.traces.len() >= MAX_TRACES {
            self.evict_least_recently_ingested_trace();
        }
        self.ingest_seq += 1;
        let seq = self.ingest_seq;
        let trace = self
            .traces
            .entry(trace_id.to_string())
            .or_insert_with(|| TraceRecord {
                id: trace_id.to_string(),
                ..TraceRecord::default()
            });
        trace.ingest_seq = seq;
        match trace.segment_index.get(&segment.id).copied() {
            Some(at) => {
                trace.segments[at] = segment;
                trace.recompute_bounds();
            }
            None => {
                if trace.segments.len() >= MAX_SEGMENTS_PER_TRACE {
                    trace.limit_exceeded = true;
                    return Err(SegmentRejection::trace_full(&segment.id));
                }
                trace
                    .segment_index
                    .insert(segment.id.clone(), trace.segments.len());
                trace.extend_bounds(&segment);
                trace.segments.push(segment);
            }
        }
        Ok(())
    }

    /// Drop the trace that has gone longest without an ingest, to keep the store
    /// at [`MAX_TRACES`]. Walked only when the store is full.
    fn evict_least_recently_ingested_trace(&mut self) {
        let oldest = self
            .traces
            .iter()
            .min_by_key(|(_, trace)| trace.ingest_seq)
            .map(|(id, _)| id.clone());
        if let Some(id) = oldest {
            self.traces.remove(&id);
        }
    }

    /// Mint a group ARN for a newly created group, in the shape
    /// [`group_arn`] writes.
    ///
    /// The suffix is unique per minted group: it is seeded from the never-reset
    /// [`group_seq`](Self::group_seq), advanced before use, so two groups never
    /// share an ARN even when one replaces the other by name, and none can
    /// collide with the reserved group's seed of `0`.
    pub fn mint_group_arn(&mut self, region: &str, group_name: &str) -> String {
        self.group_seq += 1;
        group_arn(region, group_name, self.group_seq)
    }

    /// The [`GroupKey`] a `(region, GroupName)` pair makes. The ONE place a
    /// group key is built, so a lookup and the write that follows it cannot
    /// disagree about which record is addressed.
    fn key(region: &str, group_name: &str) -> GroupKey {
        (region.to_string(), group_name.to_string())
    }

    /// Insert a group and index its ARN, returning the stored record, or `None`
    /// when the store already holds [`MAX_GROUPS`] groups.
    ///
    /// Inserting over a name that already exists IN THAT REGION replaces that
    /// record, and drops its superseded ARN from the index in the same step — so
    /// the index never grows past `groups`, and a superseded ARN resolves to
    /// nothing rather than to whatever group later took the name. A group of the
    /// same name in another region is a different group and is untouched.
    pub fn insert_group(&mut self, region: &str, record: GroupRecordDraft) -> Option<&GroupRecord> {
        let key = Self::key(region, &record.group_name);
        match self.groups.remove(&key) {
            Some(superseded) => {
                self.group_arn_to_name.remove(&superseded.group_arn);
            }
            None if self.groups.len() >= MAX_GROUPS => return None,
            None => {}
        }
        let group_arn = self.mint_group_arn(region, &record.group_name);
        self.group_arn_to_name
            .insert(group_arn.clone(), key.clone());
        self.groups.insert(
            key.clone(),
            GroupRecord {
                region: region.to_string(),
                group_name: record.group_name,
                group_arn,
                filter_expression: record.filter_expression,
                insights_configuration: record.insights_configuration,
                tags: record.tags,
            },
        );
        Some(&self.groups[&key])
    }

    /// The group a `(GroupName, GroupARN)` pair selects, for a caller in
    /// `region`.
    ///
    /// Either selector resolves the same record, but a request supplies EXACTLY
    /// ONE: `GroupName` is "the case-sensitive name of the group" and `GroupARN`
    /// "the ARN that was generated upon creation"
    /// (`smithy_query(query="shape",shape="UpdateGroupRequest")`), two names for
    /// one identity with no operation that re-points either at another group.
    /// So a pair that supplies NEITHER names nothing
    /// ([`GroupLookupError::NoSelector`]) and a pair that supplies BOTH states
    /// two identities at once and is refused unresolved
    /// ([`GroupLookupError::ConflictingSelectors`]) — whether or not the two
    /// happen to agree — rather than one side being preferred silently.
    /// Otherwise the single selector resolves the record, by name or through the
    /// ARN index, and last the reserved [`DEFAULT_GROUP_NAME`] group, which is
    /// derived rather than stored (see [`default_group`]) and so is resolvable by
    /// name and by ARN without ever appearing in `groups`.
    ///
    /// Both selectors are scoped to `region`: a group lives in one region, so an
    /// ARN minted in another region resolves to nothing here, exactly as a name
    /// that was never used in this one does.
    pub fn find_group(
        &self,
        region: &str,
        group_name: Option<&str>,
        group_arn: Option<&str>,
    ) -> Result<GroupRecord, GroupLookupError> {
        if group_name.is_none() && group_arn.is_none() {
            return Err(GroupLookupError::NoSelector);
        }
        if group_name.is_some() && group_arn.is_some() {
            return Err(GroupLookupError::ConflictingSelectors);
        }
        if let Some(record) = group_name
            .map(|name| Self::key(region, name))
            .and_then(|key| self.groups.get(&key))
        {
            return Ok(record.clone());
        }
        if let Some(record) = group_arn
            .and_then(|arn| self.group_arn_to_name.get(arn))
            .filter(|(arn_region, _)| arn_region == region)
            .and_then(|key| self.groups.get(key))
        {
            return Ok(record.clone());
        }
        let default = default_group(region);
        if group_name == Some(DEFAULT_GROUP_NAME) || group_arn == Some(default.group_arn.as_str()) {
            return Ok(default);
        }
        Err(GroupLookupError::NotFound)
    }

    /// Every group a caller in `region` sees, in `GroupName` order: the reserved
    /// [`DEFAULT_GROUP_NAME`] group plus every one stored IN THAT REGION.
    ///
    /// This is what `GetGroups` lists, so the reserved group is visible on a
    /// fresh emulator exactly as it is in a fresh account, and another region's
    /// groups are not. Bounded by [`MAX_GROUPS`] + 1.
    pub fn visible_groups(&self, region: &str) -> Vec<GroupRecord> {
        // `groups` is a `BTreeMap` keyed (region, name), so one region's records
        // are a contiguous run in `GroupName` order.
        let mut visible: Vec<GroupRecord> = self
            .groups
            .range(Self::key(region, "")..)
            .take_while(|((stored_region, _), _)| stored_region == region)
            .map(|(_, record)| record.clone())
            .collect();
        // The reserved group is stored only once an `UpdateGroup` has
        // materialised it, in which case it is already in `groups`; this guard
        // is what keeps the listing from carrying it twice.
        if !self.groups.contains_key(&Self::key(region, DEFAULT_GROUP_NAME)) {
            visible.push(default_group(region));
        }
        visible.sort_by(|left, right| left.group_name.cmp(&right.group_name));
        visible
    }

    /// Whether `group_name` already names a group in `region`, the reserved
    /// [`DEFAULT_GROUP_NAME`] one included — `CreateGroup`'s uniqueness check.
    pub fn name_is_taken(&self, region: &str, group_name: &str) -> bool {
        group_name == DEFAULT_GROUP_NAME
            || self.groups.contains_key(&Self::key(region, group_name))
    }

    /// The stored record for `group`, for an operation that MUTATES it.
    ///
    /// Addressed by the key `group` itself carries — the record a lookup
    /// resolved — never by a string from the request, so a write cannot reach a
    /// record the lookup did not resolve.
    ///
    /// The reserved [`DEFAULT_GROUP_NAME`] group is derived rather than stored
    /// ([`default_group`]), so the first write to it is what gives it a stored
    /// record — carrying the SAME derived ARN, so its ARN stays stable across
    /// that write and every selector that resolved it before still does. `None`
    /// means there is no record to write and no room to materialise one.
    fn group_mut(&mut self, group: GroupRecord) -> Option<&mut GroupRecord> {
        let key = Self::key(&group.region, &group.group_name);
        if !self.groups.contains_key(&key) {
            if group.group_name != DEFAULT_GROUP_NAME || self.groups.len() >= MAX_GROUPS {
                return None;
            }
            self.group_arn_to_name
                .insert(group.group_arn.clone(), key.clone());
            self.groups.insert(key.clone(), group);
        }
        self.groups.get_mut(&key)
    }

    /// Store one `PutTelemetryRecords` batch under the source that reported it.
    ///
    /// Appending, not replacing: a source's records accumulate across calls, and
    /// its [`TelemetryTotals`] are the sums over every record ever accepted from
    /// it. The call itself is counted too, so a batch that carried no records is
    /// still a report from a known source.
    ///
    /// Bounded in both directions: a source keeps at most
    /// [`MAX_TELEMETRY_RECORDS_PER_SOURCE`] records (the oldest fall out of the
    /// window first), and the store holds at most [`MAX_TELEMETRY_SOURCES`]
    /// sources (a report from a new source past that evicts the least recently
    /// reporting one).
    pub fn record_telemetry(&mut self, source: TelemetrySource, records: &[TelemetryRecord]) {
        if !self.telemetry.contains_key(&source) && self.telemetry.len() >= MAX_TELEMETRY_SOURCES {
            self.evict_least_recently_reporting_source();
        }
        self.telemetry_seq += 1;
        let seq = self.telemetry_seq;
        let reporter = self
            .telemetry
            .entry(source.clone())
            .or_insert_with(|| TelemetryReporter {
                source,
                ..TelemetryReporter::default()
            });
        reporter.last_report_seq = seq;
        reporter.reports += 1;
        for record in records {
            reporter.totals.add(record);
            reporter.records_accepted += 1;
            reporter.records.push(record.clone());
        }
        if reporter.records.len() > MAX_TELEMETRY_RECORDS_PER_SOURCE {
            let overflow = reporter.records.len() - MAX_TELEMETRY_RECORDS_PER_SOURCE;
            reporter.records.drain(..overflow);
        }
    }

    /// Drop the reporting source that has gone longest without a report, to keep
    /// the telemetry store at [`MAX_TELEMETRY_SOURCES`]. Walked only when the
    /// store is full.
    fn evict_least_recently_reporting_source(&mut self) {
        let oldest = self
            .telemetry
            .iter()
            .min_by_key(|(_, reporter)| reporter.last_report_seq)
            .map(|(source, _)| source.clone());
        if let Some(source) = oldest {
            self.telemetry.remove(&source);
        }
    }

    /// Apply an `UpdateGroup` to the group `group` names, returning the record
    /// as it now stands, or `None` when there is no record to write (see
    /// [`group_mut`](Self::group_mut)).
    ///
    /// Each member the update SUPPLIES replaces the stored one; each member it
    /// OMITS leaves the stored one as it is. The group's identity — its
    /// case-sensitive name and the ARN that "was generated upon creation"
    /// (`UpdateGroupRequest$GroupARN`) — and its tags are never touched by an
    /// update, so neither the keyed map nor the ARN index needs maintenance
    /// here and every selector that resolved the group before still does.
    pub fn apply_group_update(
        &mut self,
        group: GroupRecord,
        update: GroupUpdate,
    ) -> Option<&GroupRecord> {
        let record = self.group_mut(group)?;
        if let Some(filter_expression) = update.filter_expression {
            record.filter_expression = Some(filter_expression);
        }
        if let Some(insights_configuration) = update.insights_configuration {
            record.insights_configuration = Some(insights_configuration);
        }
        Some(record)
    }

    /// Remove the group `group` IS — the record a lookup resolved — and its ARN
    /// index entry.
    ///
    /// Takes the resolved record rather than a name from the request, and
    /// addresses it by the key that record carries, so the removal can only ever
    /// reach the record the lookup returned: a caller in one region cannot
    /// remove a same-named group in another. `None` means there was nothing
    /// under that key.
    pub fn remove_group(&mut self, group: &GroupRecord) -> Option<GroupRecord> {
        let record = self
            .groups
            .remove(&Self::key(&group.region, &group.group_name))?;
        self.group_arn_to_name.remove(&record.group_arn);
        Some(record)
    }

    /// The tags held by the resource `resource_arn` names, or `None` when it
    /// names no resource this store holds.
    ///
    /// X-Ray tags attach to a group or a sampling rule, so both branches are
    /// here: a group resolves through the ARN index (or, for the reserved
    /// [`DEFAULT_GROUP_NAME`] group, through the ARN it is read with), and a
    /// sampling rule through the name its ARN embeds
    /// ([`sampling_rule_name_in_arn`]), so the tags `CreateSamplingRule`
    /// validated and stored on a rule are the ones its ARN reports.
    ///
    /// No region argument: a group ARN carries the region it was minted in and a
    /// suffix unique to the group, so it names at most one record in the whole
    /// store — the ARN IS the key, and resolving one cannot reach a resource in
    /// another region by accident.
    ///
    /// Until something tags the reserved group there is no record to read from,
    /// and the group carries no tags, so the answer is the empty map.
    pub fn tags_for_resource(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        /// The tags a resource that holds none reports. `static`, so the read
        /// path borrows it rather than materialising a record.
        static NO_TAGS: BTreeMap<String, String> = BTreeMap::new();

        if let Some(rule_name) = sampling_rule_name_in_arn(resource_arn) {
            // The service-managed rule exists whether or not a record has been
            // materialised for it ([`default_sampling_rule`]), and until
            // something tags it, it carries no tags.
            let derived_default = rule_name == DEFAULT_SAMPLING_RULE_NAME
                && !self.sampling_rules.contains_key(rule_name);
            if derived_default {
                return Some(&NO_TAGS);
            }
            return Some(&self.sampling_rules.get(rule_name)?.tags);
        }
        if let Some(key) = self.group_arn_to_name.get(resource_arn) {
            return Some(&self.groups.get(key)?.tags);
        }
        default_group_for_arn(resource_arn).map(|_| &NO_TAGS)
    }

    /// The same lookup, for a tagging operation that writes.
    ///
    /// Writing to the reserved group materialises its derived record first —
    /// with the ARN it is already read with — the same way `UpdateGroup` does,
    /// because a tag has to be stored somewhere to be read back.
    pub fn tags_for_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        if let Some(rule_name) = sampling_rule_name_in_arn(resource_arn) {
            let rule_name = rule_name.to_string();
            // Tagging the service-managed rule materialises its derived record
            // first — with the members and the ARN it is already read with —
            // the same way `UpdateSamplingRule` does, because a tag has to be
            // stored somewhere to be read back. The region is the one the ARN
            // itself names.
            let region = arn_fields(resource_arn).map_or_else(
                || DEFAULT_REGION.to_string(),
                |[_, _, _, region, _, _]| region.to_string(),
            );
            return Some(&mut self.sampling_rule_mut(&region, &rule_name)?.tags);
        }
        if let Some(key) = self.group_arn_to_name.get(resource_arn).cloned() {
            return Some(&mut self.groups.get_mut(&key)?.tags);
        }
        let reserved = default_group_for_arn(resource_arn)?;
        Some(&mut self.group_mut(reserved)?.tags)
    }

    /// Materialise the destination change the clock already owes at `now`.
    ///
    /// Called by the reporting operation as well as the mutating one: the move
    /// to `ACTIVE` was due at a fixed instant whether or not anybody called, so
    /// applying it on a read shows nothing a later caller would not have seen.
    pub fn settle_trace_segment_destination(&mut self, now: Instant) {
        if self
            .trace_segment_destination
            .active_at
            .is_some_and(|due| now >= due)
        {
            self.trace_segment_destination.active_at = None;
        }
    }

    /// Point the service at `destination`, and record `spans_log_group_kms_key_id`
    /// when one was supplied.
    ///
    /// A change becomes effective [`DESTINATION_ACTIVATION_SPAN`] after `now`, so
    /// it reports `PENDING` until then. A request that names the destination
    /// already configured changes nothing — there is no switch to apply — and so
    /// leaves the reported status as it stands. A destination other than
    /// [`CLOUD_WATCH_LOGS_DESTINATION`] takes no `CloudWatchLogsConfiguration`,
    /// so pointing the service there drops the stored key rather than keeping a
    /// key for a log group nothing writes to.
    pub fn set_trace_segment_destination(
        &mut self,
        destination: &str,
        spans_log_group_kms_key_id: Option<&str>,
        now: Instant,
    ) -> TraceSegmentDestinationRecord {
        let current = &mut self.trace_segment_destination;
        if current.destination != destination {
            current.destination = destination.to_string();
            current.active_at = Some(now + DESTINATION_ACTIVATION_SPAN);
        }
        if destination == CLOUD_WATCH_LOGS_DESTINATION {
            if let Some(key_id) = spans_log_group_kms_key_id {
                current.spans_log_group_kms_key_id = Some(key_id.to_string());
            }
        } else {
            current.spans_log_group_kms_key_id = None;
        }
        current.clone()
    }
}

/// The `RuleName` a sampling rule ARN embeds, when `value` is one this service
/// could have minted ([`sampling_rule_arn`]).
///
/// A tagging operation takes an ARN rather than a name, and the ARN is derived
/// from the region and the name, so reading the name back out of it is how a
/// sampling rule is resolved from one. The partition, service, account and
/// resource prefix all have to match.
///
/// The REGION the ARN names is not checked here, and that is deliberate: a
/// tagging operation has no other selector, so the ARN is the caller's whole
/// statement of which resource it means, and this one store serves every
/// regional endpoint. An operation that can also select by name
/// ([`ComAmazonawsXrayAwsxrayState::resolve_sampling_rule_name`], used by
/// `DeleteSamplingRule`) holds an ARN selector to the region of the endpoint it
/// was called on instead, which is the stricter reading of a request that names
/// a region explicitly.
pub fn sampling_rule_name_in_arn(value: &str) -> Option<&str> {
    let [_, partition, service, _region, account, resource] = arn_fields(value)?;
    if partition != "aws" || service != ARN_NAMESPACE || account != ::emulator_server::emulator_account_id() {
        return None;
    }
    let name = resource.strip_prefix("sampling-rule/")?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

/// What `CreateGroup` supplies; the ARN is minted by [`ComAmazonawsXrayAwsxrayState::insert_group`].
#[derive(Debug, Clone, Default)]
pub struct GroupRecordDraft {
    /// The requested group name.
    pub group_name: String,
    /// The requested filter expression.
    pub filter_expression: Option<String>,
    /// The requested insights configuration.
    pub insights_configuration: Option<InsightsConfigurationRecord>,
    /// The requested tags, keyed by tag key.
    pub tags: BTreeMap<String, String>,
}

/// X-Ray's filter-expression language: the ONE place this crate reads it.
///
/// Every surface that takes a `FilterExpression` goes through [`parse`] — the
/// grammar, the catalog of fields X-Ray indexes, the bounds on an expression's
/// length and nesting, and the wording of every refusal that follows from them
/// live here and nowhere else. A surface that only STORES an expression
/// (`CreateGroup`, `UpdateGroup`) calls [`validate`], which is `parse` with the
/// tree thrown away; `GetTraceSummaries` hands the tree to
/// [`crate::state::filter`], which compiles it into what it can answer from the
/// stored segment documents and evaluates it. So an expression is parsed once,
/// by one parser, however it arrives.
///
/// The language is proprietary to X-Ray — no crates.io crate parses it — so the
/// grammar is written here as a tokenizer plus a recursive-descent parser. Both
/// the grammar and the keyword catalog come from AWS's "Using filter
/// expressions" reference for X-Ray; the model carries no field list.
pub mod filter_expression {
    /// The longest expression this parser accepts, in CHARACTERS — the unit the
    /// tokenizer walks, so the bound and the parsed unit agree whatever the
    /// encoding. `FilterExpression` carries no modelled length bound, so this is
    /// the emulator's own: it keeps one request from driving unbounded parsing.
    pub const MAX_LENGTH: usize = 4_096;

    /// The deepest nesting of `(…)` / `{…}` this parser accepts, for the same
    /// reason — a bound on caller-driven work, not a published limit.
    pub const MAX_DEPTH: usize = 32;

    /// The fields a filter expression may address, from AWS's X-Ray "Using
    /// filter expressions" keyword reference: the boolean request keywords
    /// (`ok`, `error`, `fault`, `throttle`, `partial`), the numeric ones
    /// (`duration`, `responsetime`, `http.status`, `coverage`), the string ones
    /// (the rest of `http.*`, `user`, `resource.arn`, `trace_id`), the complex
    /// keywords that take arguments (`service`, `edge`, `id`, `group`) and the
    /// members addressable inside one of their blocks (`name`, `type`,
    /// `message`, `account.id`, `service.*`, `edge.*`).
    ///
    /// A field NOT reachable from this catalog is not indexed by X-Ray, and an
    /// expression addressing one is refused — `metadata.<key>` is the ordinary
    /// case: a segment's metadata is stored but never indexed, so no filter can
    /// select on it.
    const KEYWORDS: &[&str] = &[
        "account.id",
        "coverage",
        "duration",
        "edge",
        "edge.destination",
        "edge.source",
        "error",
        "fault",
        "group",
        "http.clientip",
        "http.method",
        "http.status",
        "http.url",
        "http.useragent",
        "id",
        "message",
        "name",
        "ok",
        "partial",
        "resource.arn",
        "responsetime",
        "service",
        "service.account.id",
        "service.name",
        "service.type",
        "throttle",
        "trace_id",
        "type",
        "user",
    ];

    /// Keyword roots that take a caller-chosen suffix: an annotation key
    /// (`annotation.<key>`) and a root-cause path (`rootcause.<…>`).
    const KEYWORD_PREFIXES: &[&str] = &["annotation.", "rootcause."];

    /// The comparison operators written as words, from the same reference's
    /// string operators. They sit beside `=`, `!=`, `<`, `<=`, `>` and `>=`.
    const WORD_COMPARISONS: &[(&str, Operator)] = &[
        ("contains", Operator::Contains),
        ("beginswith", Operator::BeginsWith),
        ("startswith", Operator::BeginsWith),
        ("endswith", Operator::EndsWith),
    ];

    /// The operator `word` names, when it names one of the word comparisons.
    fn word_comparison(word: &str) -> Option<Operator> {
        WORD_COMPARISONS
            .iter()
            .find(|(name, _)| word.eq_ignore_ascii_case(name))
            .map(|(_, operator)| *operator)
    }

    /// Whether `path` names a field a filter expression may address.
    #[must_use]
    pub fn is_indexed_field(path: &str) -> bool {
        KEYWORDS.contains(&path)
            || KEYWORD_PREFIXES
                .iter()
                .any(|prefix| path.len() > prefix.len() && path.starts_with(prefix))
    }

    /// Why a `FilterExpression` was refused.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Error {
        /// The expression was absent, empty or only whitespace. X-Ray treats an
        /// omitted `FilterExpression` and an empty one identically.
        Empty,
        /// The expression does not parse.
        Syntax(String),
        /// The expression addresses a field X-Ray does not index.
        UnknownField(String),
    }

    impl Error {
        /// What to tell the caller. The empty case carries X-Ray's own fixed
        /// wording; the other two are built from the request, naming the field
        /// or the token at fault so the caller can fix the expression.
        #[must_use]
        pub fn message(&self) -> String {
            match self {
                Self::Empty => "Filter Expression is empty".to_string(),
                Self::Syntax(detail) => format!("Invalid filter expression: {detail}"),
                Self::UnknownField(path) => {
                    format!("Invalid filter expression: `{path}` is not an indexed field")
                }
            }
        }
    }

    /// One lexical item.
    #[derive(Debug, Clone, PartialEq)]
    enum Token {
        /// A bare word: a keyword path, or a label inside a keyword's arguments.
        Field(String),
        /// A quoted string literal.
        Text(String),
        /// A numeric literal.
        Number(f64),
        /// `true` / `false`.
        Boolean(bool),
        And,
        Or,
        Not,
        /// One of `=`, `!=`, `<`, `<=`, `>`, `>=`, or one of the string
        /// operators the same reference lists (`CONTAINS`, `BEGINSWITH`,
        /// `STARTSWITH`, `ENDSWITH`). WHICH comparison it is does not change
        /// whether the expression parses, but it does change what the
        /// expression MEANS, so the operator travels on the token for whoever
        /// evaluates the parsed tree.
        Comparison(Operator),
        OpenParen,
        CloseParen,
        OpenBrace,
        CloseBrace,
        Comma,
        Colon,
    }

    /// Which comparison a predicate makes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Operator {
        /// `=`
        Equal,
        /// `!=`
        NotEqual,
        /// `<`
        Less,
        /// `<=`
        LessOrEqual,
        /// `>`
        Greater,
        /// `>=`
        GreaterOrEqual,
        /// `CONTAINS`
        Contains,
        /// `BEGINSWITH` / `STARTSWITH` — the same operator under two names.
        BeginsWith,
        /// `ENDSWITH`
        EndsWith,
    }

    /// One value a comparison or an argument carries.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Operand {
        /// A quoted string.
        Text(String),
        /// A number.
        Number(f64),
        /// `true` / `false`.
        Boolean(bool),
        /// Another indexed field, as `id(name: …)`'s arguments may name.
        Field(String),
    }

    /// One argument of a complex keyword, labelled as in `id(name: "svc")` or
    /// positional as in `service("svc", "AWS::EC2::Instance")`.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Argument {
        /// The member the argument names, when it was labelled.
        pub label: Option<String>,
        /// The value it carries.
        pub value: Operand,
    }

    /// A parsed expression.
    ///
    /// This is the ONE tree the crate parses this language into: `validate`
    /// answers from it, and `crate::state::filter` evaluates it. `AND` binds
    /// tighter than `OR`, and each associates left to right.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Node {
        /// `left AND right`.
        And(Box<Node>, Box<Node>),
        /// `left OR right`.
        Or(Box<Node>, Box<Node>),
        /// `NOT inner`.
        Not(Box<Node>),
        /// `keyword [ ( arguments ) ] [ { block } | comparison operand ]`.
        Predicate {
            /// The keyword's path, which [`is_indexed_field`] accepted.
            path: String,
            /// Its arguments, when it is a complex keyword.
            arguments: Vec<Argument>,
            /// The expression its `{ … }` block scopes, when it carries one.
            block: Option<Box<Node>>,
            /// The comparison it makes, when it makes one.
            comparison: Option<(Operator, Operand)>,
        },
    }

    /// How a token reads in a refusal message.
    fn describe(token: &Token) -> String {
        match token {
            Token::Field(path) => format!("`{path}`"),
            Token::Text(text) => format!("the string `{text}`"),
            Token::Number(number) => format!("the number `{number}`"),
            Token::Boolean(value) => format!("`{value}`"),
            Token::And => "`AND`".to_string(),
            Token::Or => "`OR`".to_string(),
            Token::Not => "`NOT`".to_string(),
            Token::Comparison(_) => "a comparison operator".to_string(),
            Token::OpenParen => "`(`".to_string(),
            Token::CloseParen => "`)`".to_string(),
            Token::OpenBrace => "`{`".to_string(),
            Token::CloseBrace => "`}`".to_string(),
            Token::Comma => "`,`".to_string(),
            Token::Colon => "`:`".to_string(),
        }
    }

    /// Split `expression` into tokens, refusing anything that is not one.
    fn tokenize(expression: &str) -> Result<Vec<Token>, Error> {
        let characters: Vec<char> = expression.chars().collect();
        let mut tokens = Vec::new();
        let mut at = 0;
        while at < characters.len() {
            let character = characters[at];
            match character {
                whitespace if whitespace.is_whitespace() => at += 1,
                '"' | '\'' => {
                    at += 1;
                    let mut text = String::new();
                    loop {
                        match characters.get(at) {
                            None => {
                                return Err(Error::Syntax(format!(
                                    "unterminated string literal, expected a closing {character}"
                                )))
                            }
                            Some('\\') => {
                                let Some(escaped) = characters.get(at + 1) else {
                                    return Err(Error::Syntax(
                                        "the expression ends in a trailing `\\`".to_string(),
                                    ));
                                };
                                text.push(*escaped);
                                at += 2;
                            }
                            Some(&closing) if closing == character => {
                                at += 1;
                                break;
                            }
                            Some(&other) => {
                                text.push(other);
                                at += 1;
                            }
                        }
                    }
                    tokens.push(Token::Text(text));
                }
                '(' => {
                    tokens.push(Token::OpenParen);
                    at += 1;
                }
                ')' => {
                    tokens.push(Token::CloseParen);
                    at += 1;
                }
                '{' => {
                    tokens.push(Token::OpenBrace);
                    at += 1;
                }
                '}' => {
                    tokens.push(Token::CloseBrace);
                    at += 1;
                }
                ',' => {
                    tokens.push(Token::Comma);
                    at += 1;
                }
                ':' => {
                    tokens.push(Token::Colon);
                    at += 1;
                }
                '=' => {
                    tokens.push(Token::Comparison(Operator::Equal));
                    at += 1;
                }
                '<' | '>' => {
                    let inclusive = characters.get(at + 1) == Some(&'=');
                    at += usize::from(inclusive) + 1;
                    tokens.push(Token::Comparison(match (character, inclusive) {
                        ('<', false) => Operator::Less,
                        ('<', true) => Operator::LessOrEqual,
                        (_, false) => Operator::Greater,
                        (_, true) => Operator::GreaterOrEqual,
                    }));
                }
                '!' => {
                    if characters.get(at + 1) != Some(&'=') {
                        return Err(Error::Syntax(
                            "`!` is only valid as part of `!=`".to_string(),
                        ));
                    }
                    tokens.push(Token::Comparison(Operator::NotEqual));
                    at += 2;
                }
                digit
                    if digit.is_ascii_digit()
                        || (digit == '-'
                            && characters.get(at + 1).is_some_and(char::is_ascii_digit)) =>
                {
                    let start = at;
                    at += 1;
                    while characters
                        .get(at)
                        .is_some_and(|next| next.is_ascii_digit() || *next == '.')
                    {
                        at += 1;
                    }
                    let literal: String = characters[start..at].iter().collect();
                    let number = literal.parse::<f64>().map_err(|_| {
                        Error::Syntax(format!("`{literal}` is not a number"))
                    })?;
                    tokens.push(Token::Number(number));
                }
                word if word.is_alphabetic() || word == '_' => {
                    let start = at;
                    while characters
                        .get(at)
                        .is_some_and(|next| next.is_alphanumeric() || "_.-".contains(*next))
                    {
                        at += 1;
                    }
                    let word: String = characters[start..at].iter().collect();
                    tokens.push(if word.eq_ignore_ascii_case("and") {
                        Token::And
                    } else if word.eq_ignore_ascii_case("or") {
                        Token::Or
                    } else if word.eq_ignore_ascii_case("not") {
                        Token::Not
                    } else if word.eq_ignore_ascii_case("true") {
                        Token::Boolean(true)
                    } else if word.eq_ignore_ascii_case("false") {
                        Token::Boolean(false)
                    } else if let Some(operator) = word_comparison(&word) {
                        Token::Comparison(operator)
                    } else {
                        Token::Field(word)
                    });
                }
                other => {
                    return Err(Error::Syntax(format!("unexpected character `{other}`")));
                }
            }
        }
        Ok(tokens)
    }

    /// The recursive-descent walk over the tokens.
    ///
    /// ```text
    /// expression := term (("AND" | "OR") term)*
    /// term       := "NOT" term | "(" expression ")" | predicate
    /// predicate  := keyword ["(" arguments ")"] ["{" expression "}" | comparison operand]
    /// arguments  := argument ("," argument)*
    /// argument   := [label ":"] operand
    /// operand    := text | number | boolean | keyword
    /// ```
    struct Parser {
        tokens: Vec<Token>,
        at: usize,
    }

    impl Parser {
        fn peek(&self) -> Option<&Token> {
            self.tokens.get(self.at)
        }

        fn take(&mut self) -> Option<Token> {
            let token = self.tokens.get(self.at).cloned();
            if token.is_some() {
                self.at += 1;
            }
            token
        }

        fn eat(&mut self, token: &Token) -> bool {
            if self.peek() == Some(token) {
                self.at += 1;
                return true;
            }
            false
        }

        /// What the walk is looking at, for a refusal message.
        fn found(&self) -> String {
            match self.peek() {
                Some(token) => format!(", found {}", describe(token)),
                None => ", found the end of the expression".to_string(),
            }
        }

        fn expect(&mut self, token: &Token, what: &str) -> Result<(), Error> {
            if self.eat(token) {
                return Ok(());
            }
            Err(Error::Syntax(format!("expected {what}{}", self.found())))
        }

        fn expression(&mut self, depth: usize) -> Result<Node, Error> {
            // The terms and the operators between them, folded below so `AND`
            // binds tighter than `OR` while the WALK stays exactly what it was:
            // which expressions parse does not depend on the fold.
            let mut terms = vec![self.term(depth)?];
            let mut operators: Vec<Token> = Vec::new();
            while matches!(self.peek(), Some(Token::And | Token::Or)) {
                let operator = self.take().expect("the token was just peeked");
                operators.push(operator);
                terms.push(self.term(depth)?);
            }
            Ok(fold(terms, &operators))
        }

        fn term(&mut self, depth: usize) -> Result<Node, Error> {
            if depth > MAX_DEPTH {
                return Err(Error::Syntax(format!(
                    "nested more than {MAX_DEPTH} levels deep"
                )));
            }
            match self.take() {
                Some(Token::Not) => Ok(Node::Not(Box::new(self.term(depth + 1)?))),
                Some(Token::OpenParen) => {
                    let inner = self.expression(depth + 1)?;
                    self.expect(&Token::CloseParen, "a closing `)`")?;
                    Ok(inner)
                }
                Some(Token::Field(path)) => self.predicate(path, depth),
                Some(other) => Err(Error::Syntax(format!(
                    "expected a keyword, found {}",
                    describe(&other)
                ))),
                None => Err(Error::Syntax(
                    "expected a keyword, found the end of the expression".to_string(),
                )),
            }
        }

        fn predicate(&mut self, path: String, depth: usize) -> Result<Node, Error> {
            if !is_indexed_field(&path) {
                return Err(Error::UnknownField(path));
            }
            let arguments = if self.eat(&Token::OpenParen) {
                self.arguments()?
            } else {
                Vec::new()
            };
            if self.eat(&Token::OpenBrace) {
                let block = self.expression(depth + 1)?;
                self.expect(&Token::CloseBrace, "a closing `}`")?;
                return Ok(Node::Predicate {
                    path,
                    arguments,
                    block: Some(Box::new(block)),
                    comparison: None,
                });
            }
            let comparison = match self.peek() {
                Some(Token::Comparison(operator)) => {
                    let operator = *operator;
                    self.at += 1;
                    Some((operator, self.operand()?))
                }
                _ => None,
            };
            Ok(Node::Predicate {
                path,
                arguments,
                block: None,
                comparison,
            })
        }

        fn arguments(&mut self) -> Result<Vec<Argument>, Error> {
            let mut arguments = Vec::new();
            loop {
                // A labelled argument, as in `id(name: "svc", type: "AWS::EC2")`.
                // The label is a member of the keyword, not a field of its own,
                // so it is not looked up in the catalog.
                let mut label = None;
                if let (Some(Token::Field(name)), Some(&Token::Colon)) =
                    (self.peek(), self.tokens.get(self.at + 1))
                {
                    label = Some(name.clone());
                    self.at += 2;
                }
                arguments.push(Argument {
                    label,
                    value: self.operand()?,
                });
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect(&Token::CloseParen, "a closing `)`")?;
            Ok(arguments)
        }

        fn operand(&mut self) -> Result<Operand, Error> {
            match self.take() {
                Some(Token::Text(text)) => Ok(Operand::Text(text)),
                Some(Token::Number(number)) => Ok(Operand::Number(number)),
                Some(Token::Boolean(value)) => Ok(Operand::Boolean(value)),
                Some(Token::Field(path)) => {
                    if is_indexed_field(&path) {
                        return Ok(Operand::Field(path));
                    }
                    Err(Error::UnknownField(path))
                }
                Some(other) => Err(Error::Syntax(format!(
                    "expected a value, found {}",
                    describe(&other)
                ))),
                None => Err(Error::Syntax(
                    "expected a value, found the end of the expression".to_string(),
                )),
            }
        }
    }

    /// Fold a flat `term (AND|OR term)*` sequence into a tree in which `AND`
    /// binds tighter than `OR`, each associating left to right.
    fn fold(terms: Vec<Node>, operators: &[Token]) -> Node {
        let mut disjuncts: Vec<Node> = Vec::new();
        let mut conjunct: Option<Node> = None;
        for (at, term) in terms.into_iter().enumerate() {
            match at.checked_sub(1).map(|before| &operators[before]) {
                // The first term opens the first conjunction.
                None => conjunct = Some(term),
                Some(Token::And) => {
                    conjunct = Some(match conjunct.take() {
                        Some(left) => Node::And(Box::new(left), Box::new(term)),
                        None => term,
                    });
                }
                // An `OR` closes the conjunction built so far.
                _ => {
                    if let Some(left) = conjunct.take() {
                        disjuncts.push(left);
                    }
                    conjunct = Some(term);
                }
            }
        }
        if let Some(last) = conjunct {
            disjuncts.push(last);
        }
        let mut walk = disjuncts.into_iter();
        let first = walk
            .next()
            .expect("a parsed expression holds at least one term");
        walk.fold(first, |left, right| {
            Node::Or(Box::new(left), Box::new(right))
        })
    }

    /// Parse `expression` into the one tree this crate reads it as, answering
    /// `Ok` only for an expression X-Ray would accept.
    ///
    /// An ABSENT expression is the same refusal as an empty one — X-Ray treats
    /// the two identically — so a handler passes `unwrap_or_default()` here
    /// rather than skipping the check when the member is `None`.
    pub fn parse(expression: &str) -> Result<Node, Error> {
        if expression.trim().is_empty() {
            return Err(Error::Empty);
        }
        if expression.chars().count() > MAX_LENGTH {
            return Err(Error::Syntax(format!(
                "longer than the {MAX_LENGTH} characters this emulator parses"
            )));
        }
        let mut parser = Parser {
            tokens: tokenize(expression)?,
            at: 0,
        };
        let node = parser.expression(0)?;
        if let Some(trailing) = parser.peek() {
            return Err(Error::Syntax(format!(
                "unexpected trailing {}",
                describe(trailing)
            )));
        }
        Ok(node)
    }

    /// Whether `expression` is one X-Ray would accept, for a surface that only
    /// STORES it: [`parse`] with the tree thrown away.
    pub fn validate(expression: &str) -> Result<(), Error> {
        parse(expression).map(drop)
    }
}

/// What `UpdateGroup` supplies: the members of a group a caller may change.
///
/// A `None` member is one the request OMITTED and so leaves alone — which is
/// why this is not a [`GroupRecordDraft`]: a draft states a whole group, an
/// update states only the parts of one.
#[derive(Debug, Clone, Default)]
pub struct GroupUpdate {
    /// The updated filter expression, when the request carried one.
    pub filter_expression: Option<String>,
    /// The updated insights configuration, when the request carried one.
    pub insights_configuration: Option<InsightsConfigurationRecord>,
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// Now, in floating point seconds of epoch time — the unit every stored instant
/// in this service uses, so a handler never converts between two clocks.
pub fn now_epoch_seconds() -> f64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// A modelled timestamp as floating point seconds of epoch time.
pub fn epoch_seconds(timestamp: &::aws_smithy_types::DateTime) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let seconds = timestamp.secs() as f64;
    seconds + f64::from(timestamp.subsec_nanos()) / 1_000_000_000.0
}

// ---------------------------------------------------------------------------
// [StartTime, EndTime] windows
// ---------------------------------------------------------------------------

/// Why a `[StartTime, EndTime]` window names no interval of recorded traffic.
///
/// Lives here, beside [`validate_window`], so every operation that takes a
/// required window refuses the same windows with the same wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowError {
    /// `EndTime` is before `StartTime`, so the window contains no instant.
    Inverted,
    /// A bound lands further than [`MAX_WINDOW_FUTURE_SKEW_SECONDS`] past now,
    /// so it names no time any recorded segment could carry — which is what a
    /// millisecond-scale epoch value read as seconds looks like.
    Future,
}

impl WindowError {
    /// What to tell the caller, naming the members at fault and the values that
    /// broke the rule.
    pub fn message(self, start: f64, end: f64) -> String {
        match self {
            Self::Inverted => format!(
                "EndTime ({end}) must be at or after StartTime ({start}), in seconds of epoch time"
            ),
            Self::Future => format!(
                "StartTime ({start}) and EndTime ({end}) must be within {MAX_WINDOW_FUTURE_SKEW_SECONDS} seconds of the present, in seconds of epoch time"
            ),
        }
    }
}

/// Refuse a `[StartTime, EndTime]` window that names no interval of recorded
/// traffic: one running backwards, or one whose bounds are not times the data
/// could have.
///
/// Nothing is refused for being OLD: this emulator's trace store keeps whatever
/// was ingested until [`MAX_TRACES`] forces an eviction, so there is no
/// retention horizon a window could fall off the far side of.
pub fn validate_window(start: f64, end: f64, now: f64) -> Result<(), WindowError> {
    if !start.is_finite() || !end.is_finite() {
        return Err(WindowError::Future);
    }
    if end < start {
        return Err(WindowError::Inverted);
    }
    if start > now + MAX_WINDOW_FUTURE_SKEW_SECONDS || end > now + MAX_WINDOW_FUTURE_SKEW_SECONDS {
        return Err(WindowError::Future);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The per-account request budget
// ---------------------------------------------------------------------------

/// A request budget: [`burst`](Self::burst) requests may arrive back-to-back,
/// and the budget refills at [`per_second`](Self::per_second) requests a second.
///
/// A token bucket rather than a counter per fixed window, so a caller that has
/// just exhausted the budget is not locked out for the rest of a window: the
/// next request is served as soon as the refill has earned one request back.
#[derive(Debug)]
pub struct RateLimiter {
    /// How many requests may arrive back-to-back.
    burst: f64,
    /// How fast the budget refills, in requests per second.
    per_second: f64,
    /// What is left of the budget.
    tokens: f64,
    /// When the budget was last refilled.
    last_refill: ::std::time::Instant,
}

impl RateLimiter {
    /// A full budget of `burst` requests, refilling at `per_second`.
    pub fn new(burst: u32, per_second: f64) -> Self {
        Self {
            burst: f64::from(burst),
            per_second,
            tokens: f64::from(burst),
            last_refill: ::std::time::Instant::now(),
        }
    }

    /// Spend one request of the budget, answering whether there was one to
    /// spend. `false` is what a `ThrottledException` reports.
    pub fn try_acquire(&mut self) -> bool {
        let now = ::std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.tokens = (self.tokens + elapsed * self.per_second).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// `StartTraceRetrieval` is the one operation in this service with a request
/// budget, so the default is its: see [`TRACE_RETRIEVAL_BURST`].
impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(TRACE_RETRIEVAL_BURST, TRACE_RETRIEVAL_RATE_PER_SECOND)
    }
}

// ---------------------------------------------------------------------------
// Trace retrievals
// ---------------------------------------------------------------------------

/// One started trace retrieval: what `StartTraceRetrieval` was asked for, under
/// the `RetrievalToken` it minted for it, plus the instants the clock owes its
/// `RetrievalStatus` transitions at.
#[derive(Debug, Clone, Default)]
pub struct TraceRetrievalRecord {
    /// The minted, opaque `RetrievalToken`.
    pub retrieval_token: String,
    /// The trace ids the retrieval was started for.
    pub trace_ids: Vec<String>,
    /// The requested window's start, in seconds of epoch time.
    pub start_time: f64,
    /// The requested window's end, in seconds of epoch time.
    pub end_time: f64,
    /// When the retrieval was started, in seconds of epoch time.
    pub started_at: f64,
    /// When the retrieval stops reporting `SCHEDULED` and starts reporting
    /// `RUNNING`, in seconds of epoch time.
    pub running_at: f64,
    /// When the retrieval's results become readable and it starts reporting
    /// `COMPLETE`, in seconds of epoch time.
    pub complete_at: f64,
    /// When `CancelTraceRetrieval` stopped this retrieval, in seconds of epoch
    /// time; `None` while it was never cancelled.
    pub cancelled_at: Option<f64>,
    /// When the retrieval times out, [`TRACE_RETRIEVAL_TIMEOUT_SECONDS`] after
    /// it started.
    pub expires_at: f64,
}

impl TraceRetrievalRecord {
    /// This retrieval's `RetrievalStatus` at `now`, computed from the instants
    /// stored on it — never from a status written at the time of some earlier
    /// call, so two readers at the same instant always agree.
    ///
    /// `FAILED` and `TIMEOUT` are never reported: no retrieval here can fail
    /// (the results are derived from traces already stored), and every one
    /// becomes readable within [`TRACE_RETRIEVAL_RUNNING_SPAN_SECONDS`] of
    /// running — far inside the documented 60-minute timeout, so nothing ever
    /// runs out of it.
    pub fn status(&self, now: f64) -> &'static str {
        if self.cancelled_at.is_some() {
            RETRIEVAL_STATUS_CANCELLED
        } else if now >= self.complete_at {
            RETRIEVAL_STATUS_COMPLETE
        } else if now >= self.running_at {
            RETRIEVAL_STATUS_RUNNING
        } else {
            RETRIEVAL_STATUS_SCHEDULED
        }
    }

    /// Whether this retrieval is still the "ongoing trace retrieval job"
    /// `CancelTraceRetrieval` cancels: one that has not reached a status it can
    /// no longer leave.
    pub fn is_ongoing(&self, now: f64) -> bool {
        matches!(
            self.status(now),
            RETRIEVAL_STATUS_SCHEDULED | RETRIEVAL_STATUS_RUNNING
        )
    }
}

impl ComAmazonawsXrayAwsxrayState {
    /// Start a trace retrieval and return the stored record, whose
    /// `retrieval_token` is what the caller gets back.
    ///
    /// Bounded: retrievals the clock has already timed out are dropped first,
    /// and if the store is still at [`MAX_TRACE_RETRIEVALS`] the one that started
    /// longest ago makes room.
    pub fn start_trace_retrieval(
        &mut self,
        trace_ids: Vec<String>,
        start_time: f64,
        end_time: f64,
        now: f64,
    ) -> &TraceRetrievalRecord {
        self.expire_trace_retrievals(now);
        if self.retrievals.len() >= MAX_TRACE_RETRIEVALS {
            let oldest = self
                .retrievals
                .iter()
                .min_by(|(_, left), (_, right)| left.started_at.total_cmp(&right.started_at))
                .map(|(token, _)| token.clone());
            if let Some(token) = oldest {
                self.retrievals.remove(&token);
            }
        }
        self.retrieval_seq += 1;
        let retrieval_token = self.mint_retrieval_token(&trace_ids, start_time, end_time);
        let running_at = now + TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS;
        self.retrievals.insert(
            retrieval_token.clone(),
            TraceRetrievalRecord {
                retrieval_token: retrieval_token.clone(),
                trace_ids,
                start_time,
                end_time,
                started_at: now,
                running_at,
                complete_at: running_at + TRACE_RETRIEVAL_RUNNING_SPAN_SECONDS,
                cancelled_at: None,
                expires_at: now + TRACE_RETRIEVAL_TIMEOUT_SECONDS,
            },
        );
        &self.retrievals[&retrieval_token]
    }

    /// The retrieval `retrieval_token` names, once the retrievals the clock has
    /// already timed out are gone.
    ///
    /// `None` is a token this service never issued, or one whose retrieval has
    /// timed out — which is what a reader answers `ResourceNotFoundException`
    /// for. The expiry it materialises was owed at `now` whether or not anybody
    /// called, so a reader may do it too.
    pub fn trace_retrieval(
        &mut self,
        retrieval_token: &str,
        now: f64,
    ) -> Option<&TraceRetrievalRecord> {
        self.expire_trace_retrievals(now);
        self.retrievals.get(retrieval_token)
    }

    /// Stop an ongoing retrieval, so every later read of its token reports
    /// `RETRIEVAL_STATUS_CANCELLED`.
    ///
    /// `Ok` carries the cancelled retrieval. `Err(Some(status))` is a retrieval
    /// that is no longer ongoing — there is no job left to cancel — and
    /// `Err(None)` a token that names no retrieval at all.
    pub fn cancel_trace_retrieval(
        &mut self,
        retrieval_token: &str,
        now: f64,
    ) -> Result<&TraceRetrievalRecord, Option<&'static str>> {
        self.expire_trace_retrievals(now);
        let Some(retrieval) = self.retrievals.get_mut(retrieval_token) else {
            return Err(None);
        };
        let status = retrieval.status(now);
        if !retrieval.is_ongoing(now) {
            return Err(Some(status));
        }
        retrieval.cancelled_at = Some(now);
        Ok(retrieval)
    }

    /// The traces one retrieval holds: every requested trace id that names a
    /// stored trace, each trimmed to the segments whose `start_time` falls
    /// inside the retrieval's window, in the order the retrieval asked for them.
    ///
    /// The window is INCLUSIVE at both ends — "The range is inclusive, so the
    /// specified start time is included in the query" /"the specified end time
    /// is included", `StartTraceRetrieval`'s own documentation for `StartTime`
    /// and `EndTime` — unlike the half-open interval `GetTraceSummaries`
    /// selects on.
    ///
    /// A requested id that names nothing stored, a repeat of one already
    /// answered, and a trace whose every segment started outside the window each
    /// contribute no trace: a retrieval reports the traffic that was recorded,
    /// never the ids that were asked about.
    pub fn retrieved_traces(&self, retrieval: &TraceRetrievalRecord) -> Vec<TraceRecord> {
        let mut answered: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let mut traces = Vec::new();
        for trace_id in &retrieval.trace_ids {
            if !answered.insert(trace_id.as_str()) {
                continue;
            }
            let Some(trace) = self.traces.get(trace_id) else {
                continue;
            };
            let trimmed = trace.within(retrieval.start_time, retrieval.end_time);
            if !trimmed.segments.is_empty() {
                traces.push(trimmed);
            }
        }
        traces
    }

    /// Drop every retrieval the clock has already timed out.
    ///
    /// This is work the clock owed at `now` whether or not anybody called, which
    /// is why a reader may materialise it too.
    pub fn expire_trace_retrievals(&mut self, now: f64) {
        self.retrievals
            .retain(|_, retrieval| retrieval.expires_at > now);
    }

    /// Mint an opaque `RetrievalToken`: 32 hexadecimal digits, the first half the
    /// never-reset [`retrieval_seq`](Self::retrieval_seq) and the second a mix of
    /// what was asked for, so no two retrievals share a token even when they ask
    /// for the same traces and no clock enters the value.
    fn mint_retrieval_token(&self, trace_ids: &[String], start_time: f64, end_time: f64) -> String {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.retrieval_seq.hash(&mut hasher);
        trace_ids.hash(&mut hasher);
        start_time.to_bits().hash(&mut hasher);
        end_time.to_bits().hash(&mut hasher);
        format!("{:016x}{:016x}", self.retrieval_seq, hasher.finish())
    }
}

// ---------------------------------------------------------------------------
// Sampling rules
// ---------------------------------------------------------------------------

/// A stored sampling rule: the rule as it was accepted — its `RuleName` and the
/// `RuleARN` minted for it filled in — plus when it was created and last
/// modified. `CreateSamplingRule` and `GetSamplingRules` both answer from this.
#[derive(Debug, Clone)]
pub struct StoredSamplingRule {
    /// The accepted rule, with `RuleName` and `RuleARN` as stored.
    pub rule: SamplingRule,
    /// When the rule was created, in seconds of epoch time.
    pub created_at: f64,
    /// When the rule was last modified, in seconds of epoch time.
    pub modified_at: f64,
    /// The tags the rule carries, keyed by tag key — what `TagResource` and
    /// `ListTagsForResource` reach through
    /// [`tags_for_resource`](ComAmazonawsXrayAwsxrayState::tags_for_resource).
    pub tags: BTreeMap<String, String>,
}

impl StoredSamplingRule {
    /// This record as the modelled `SamplingRuleRecord`.
    ///
    /// The ONE place a stored rule becomes a wire record, so every operation
    /// that answers a `SamplingRuleRecord` — `CreateSamplingRule`,
    /// `GetSamplingRules`, `UpdateSamplingRule`, `DeleteSamplingRule` — answers
    /// the same shape for the same rule.
    pub fn to_record(&self) -> SamplingRuleRecord {
        let mut rule = self.rule.clone();
        // `SamplingRule$Attributes` targets `AttributeMap`, modelled
        // `@length(min: 0, max: 5)`
        // (`smithy_query(query="shape",shape="AttributeMap")`): a map whose
        // minimum length is zero, so the EMPTY map is a legal value of the
        // member rather than an absent member. A rule that matches on no
        // attribute therefore serializes `Attributes` as `{}`, which is what an
        // attribute-matching client iterates, instead of omitting it.
        let attributes = rule.attributes.take().unwrap_or_default();
        rule.attributes = Some(attributes);
        SamplingRuleRecord {
            sampling_rule: Some(rule),
            created_at: Some(::aws_smithy_types::DateTime::from_secs_f64(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from_secs_f64(self.modified_at)),
        }
    }
}

/// Why a submitted `SamplingRule` cannot be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamplingRuleRejection {
    /// The rule is refused for what it says: a bad selector, or a member value
    /// the service does not accept. A client-input refusal.
    Invalid(String),
    /// The store already holds [`MAX_SAMPLING_RULES`] rules. This is the one
    /// rejection `RuleLimitExceededException` reports.
    LimitExceeded(String),
}

impl SamplingRuleRejection {
    /// What to tell the caller.
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(message) | Self::LimitExceeded(message) => message,
        }
    }
}

/// The `RuleName` a submitted rule is to be stored under, refusing a rule whose
/// members cannot describe one.
///
/// The rules come from the member documentation
/// (`smithy_query(query="shape",shape="SamplingRule")`): a rule is specified
/// "by either name or ARN, but not both", `Version` is "the version of the
/// sampling rule format (`1`)", and `ResourceARN` "matches the ARN of the Amazon
/// Web Services resource on which the service runs" — an ARN, or `*` for any.
/// Every modelled `@length` / `@range` bound is refused at the boundary before a
/// handler sees the request, so none of them is re-checked here.
pub fn sampling_rule_name(rule: &SamplingRule) -> Result<&str, SamplingRuleRejection> {
    let invalid =
        |message: String| -> SamplingRuleRejection { SamplingRuleRejection::Invalid(message) };
    let name = match (rule.rule_name.as_deref(), rule.rule_arn.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(invalid(
                "SamplingRule must specify a rule by either RuleName or RuleARN, but not both"
                    .to_string(),
            ))
        }
        (None, _) => {
            return Err(invalid(
                "SamplingRule.RuleName is required to create a sampling rule; its RuleARN is minted by X-Ray".to_string(),
            ))
        }
        (Some(name), None) => name,
    };
    if rule.version != SAMPLING_RULE_FORMAT_VERSION {
        return Err(invalid(format!(
            "SamplingRule.Version must be {SAMPLING_RULE_FORMAT_VERSION}, the version of the sampling rule format, not {}",
            rule.version
        )));
    }
    validate_resource_arn(&rule.resource_arn)?;
    Ok(name)
}

/// Check a submitted `ResourceARN`: "matches the ARN of the Amazon Web Services
/// resource on which the service runs"
/// (`smithy_query(query="shape",shape="SamplingRule")`) — so an ARN, or
/// [`ANY_RESOURCE_ARN`] for any resource. Written once, because a rule may not
/// reach a value through `UpdateSamplingRule` that `CreateSamplingRule` refuses.
pub fn validate_resource_arn(resource_arn: &str) -> Result<(), SamplingRuleRejection> {
    if resource_arn != ANY_RESOURCE_ARN && !resource_arn.starts_with("arn:") {
        return Err(SamplingRuleRejection::Invalid(format!(
            "ResourceARN must be an ARN or `{ANY_RESOURCE_ARN}`, not `{resource_arn}`"
        )));
    }
    Ok(())
}

/// The bounds the modelled `SamplingRule` members carry, as
/// `smithy_query(query="shape",shape="Priority,FixedRate,ReservoirSize")` states
/// them: `Priority` `@range(min: 1, max: 9999)`, `FixedRate` `@range(min: 0,
/// max: 1)`, `ReservoirSize` `@range(min: 0)`.
///
/// They are re-checked HERE for `UpdateSamplingRule`, whose own members target
/// the unconstrained `NullableInteger` / `NullableDouble`, so the boundary
/// refuses none of them: a rule may not reach through an update a value it could
/// not have been created with.
pub const SAMPLING_RULE_PRIORITY_MIN: i32 = 1;
/// The modelled maximum of `SamplingRule$Priority`.
pub const SAMPLING_RULE_PRIORITY_MAX: i32 = 9999;
/// The modelled minimum of `SamplingRule$FixedRate`.
pub const SAMPLING_RULE_FIXED_RATE_MIN: f64 = 0.0;
/// The modelled maximum of `SamplingRule$FixedRate`.
pub const SAMPLING_RULE_FIXED_RATE_MAX: f64 = 1.0;
/// The modelled minimum of `SamplingRule$ReservoirSize`.
pub const SAMPLING_RULE_RESERVOIR_SIZE_MIN: i32 = 0;

/// Check an update's values against those bounds, before anything is written.
pub fn validate_sampling_rule_update(
    update: &SamplingRuleUpdateDraft,
) -> Result<(), SamplingRuleRejection> {
    let invalid =
        |message: String| -> SamplingRuleRejection { SamplingRuleRejection::Invalid(message) };
    if let Some(resource_arn) = update.resource_arn.as_deref() {
        validate_resource_arn(resource_arn)?;
    }
    if let Some(priority) = update.priority {
        if !(SAMPLING_RULE_PRIORITY_MIN..=SAMPLING_RULE_PRIORITY_MAX).contains(&priority) {
            return Err(invalid(format!(
                "Priority must be between {SAMPLING_RULE_PRIORITY_MIN} and {SAMPLING_RULE_PRIORITY_MAX}, not {priority}"
            )));
        }
    }
    if let Some(fixed_rate) = update.fixed_rate {
        if !(SAMPLING_RULE_FIXED_RATE_MIN..=SAMPLING_RULE_FIXED_RATE_MAX).contains(&fixed_rate) {
            return Err(invalid(format!(
                "FixedRate must be between {SAMPLING_RULE_FIXED_RATE_MIN} and {SAMPLING_RULE_FIXED_RATE_MAX}, not {fixed_rate}"
            )));
        }
    }
    if let Some(reservoir_size) = update.reservoir_size {
        if reservoir_size < SAMPLING_RULE_RESERVOIR_SIZE_MIN {
            return Err(invalid(format!(
                "ReservoirSize must be at least {SAMPLING_RULE_RESERVOIR_SIZE_MIN}, not {reservoir_size}"
            )));
        }
    }
    Ok(())
}

/// The only `SamplingRule$Version` X-Ray accepts: "the version of the sampling
/// rule format (`1`)".
pub const SAMPLING_RULE_FORMAT_VERSION: i32 = 1;

/// The `SamplingRule$ResourceARN` that matches any resource.
pub const ANY_RESOURCE_ARN: &str = "*";

/// The `SamplingRule` matcher value that matches anything: the wildcard every
/// matcher of the service-managed catch-all rule carries.
pub const ANY_MATCHER: &str = "*";

/// The name of the sampling rule every X-Ray account holds without anyone
/// creating it.
///
/// X-Ray seeds each account with one service-managed catch-all rule, which
/// `GetSamplingRules` always lists and which no `DeleteSamplingRule` removes;
/// every X-Ray SDK reads it at startup as its fallback sampling policy. The
/// model does not describe it — `GetSamplingRulesResult$SamplingRuleRecords` is
/// just "rule definitions and metadata" — so the rule is recorded service
/// behaviour, written down here once.
pub const DEFAULT_SAMPLING_RULE_NAME: &str = "Default";

/// The service-managed rule's `Priority`, deliberately PAST the `@range(min: 1,
/// max: 9999)` a caller may set on a rule of its own
/// (`smithy_query(query="shape",shape="Priority")`), so no created rule can ever
/// sort behind the account's catch-all.
pub const DEFAULT_SAMPLING_RULE_PRIORITY: i32 = 10_000;

/// The service-managed rule's `FixedRate`: it instruments 5% of the requests
/// that reach it once its reservoir is spent.
pub const DEFAULT_SAMPLING_RULE_FIXED_RATE: f64 = 0.05;

/// The service-managed rule's `ReservoirSize`: one request per second is
/// instrumented before the fixed rate applies.
pub const DEFAULT_SAMPLING_RULE_RESERVOIR_SIZE: i32 = 1;

/// The account's service-managed [`DEFAULT_SAMPLING_RULE_NAME`] rule, as a
/// caller in `region` reads it.
///
/// DERIVED, never seeded into the store: the rule exists in every account
/// without being created, so there is no record to insert and no request can
/// remove it. Its ARN is a pure function of the reserved name and the region —
/// `sampling-rule/Default`, with none of the suffixes a minted id carries — so
/// two reads answer the same ARN while a caller signing for another region reads
/// that region's. Its `CreatedAt`/`ModifiedAt` are the epoch rather than a
/// reading of the clock, because nothing created it and nothing has modified it:
/// a derived rule that reported "now" would change on every read.
pub fn default_sampling_rule(region: &str) -> StoredSamplingRule {
    StoredSamplingRule {
        rule: SamplingRule {
            rule_name: Some(DEFAULT_SAMPLING_RULE_NAME.to_string()),
            rule_arn: Some(sampling_rule_arn(region, DEFAULT_SAMPLING_RULE_NAME)),
            resource_arn: ANY_RESOURCE_ARN.to_string(),
            priority: DEFAULT_SAMPLING_RULE_PRIORITY,
            fixed_rate: DEFAULT_SAMPLING_RULE_FIXED_RATE,
            reservoir_size: DEFAULT_SAMPLING_RULE_RESERVOIR_SIZE,
            service_name: ANY_MATCHER.to_string(),
            service_type: ANY_MATCHER.to_string(),
            host: ANY_MATCHER.to_string(),
            http_method: ANY_MATCHER.to_string(),
            url_path: ANY_MATCHER.to_string(),
            version: SAMPLING_RULE_FORMAT_VERSION,
            // The catch-all matches on no attribute; the empty map is what a
            // reader sees ([`StoredSamplingRule::to_record`]).
            attributes: None,
            sampling_rate_boost: None,
        },
        created_at: 0.0,
        modified_at: 0.0,
        tags: BTreeMap::new(),
    }
}

/// What `UpdateSamplingRule` supplies: the members of a sampling rule a caller
/// may change.
///
/// A `None` member is one the request OMITTED and so leaves alone — "a document
/// specifying CHANGES to a sampling rule's configuration"
/// (`smithy_query(query="shape",shape="SamplingRuleUpdate")`). The rule's
/// identity (`RuleName`, `RuleARN`) and its `Version` are not here: the update
/// shape carries no `Version` and its two selectors NAME the rule rather than
/// re-point it.
#[derive(Debug, Clone, Default)]
pub struct SamplingRuleUpdateDraft {
    /// The updated `ResourceARN`, when the request carried one.
    pub resource_arn: Option<String>,
    /// The updated `Priority`, when the request carried one.
    pub priority: Option<i32>,
    /// The updated `FixedRate`, when the request carried one.
    pub fixed_rate: Option<f64>,
    /// The updated `ReservoirSize`, when the request carried one.
    pub reservoir_size: Option<i32>,
    /// The updated `ServiceName`, when the request carried one.
    pub service_name: Option<String>,
    /// The updated `ServiceType`, when the request carried one.
    pub service_type: Option<String>,
    /// The updated `Host`, when the request carried one.
    pub host: Option<String>,
    /// The updated `HTTPMethod`, when the request carried one.
    pub http_method: Option<String>,
    /// The updated `URLPath`, when the request carried one.
    pub url_path: Option<String>,
    /// The updated `Attributes`, when the request carried them.
    pub attributes: Option<std::collections::HashMap<String, String>>,
    /// The updated `SamplingRateBoost`, when the request carried one.
    pub sampling_rate_boost: Option<SamplingRateBoost>,
}

impl ComAmazonawsXrayAwsxrayState {
    /// Create one sampling rule, keyed by its `RuleName`.
    ///
    /// The collision check and the insert are ONE step on the locked store, so N
    /// concurrent calls for one name have exactly one winner: every loser sees
    /// the winner's record already present.
    pub fn insert_sampling_rule(
        &mut self,
        region: &str,
        mut rule: SamplingRule,
        tags: BTreeMap<String, String>,
        now: f64,
    ) -> Result<&StoredSamplingRule, SamplingRuleRejection> {
        let name = sampling_rule_name(&rule)?.to_string();
        if self.sampling_rule_name_is_taken(&name) {
            return Err(SamplingRuleRejection::Invalid(
                SAMPLING_RULE_EXISTS_MESSAGE.to_string(),
            ));
        }
        if self.created_sampling_rule_count() >= MAX_SAMPLING_RULES {
            return Err(SamplingRuleRejection::LimitExceeded(format!(
                "this emulator holds the maximum of {MAX_SAMPLING_RULES} sampling rules"
            )));
        }
        rule.rule_arn = Some(sampling_rule_arn(region, &name));
        rule.rule_name = Some(name.clone());
        self.sampling_rules.insert(
            name.clone(),
            StoredSamplingRule {
                rule,
                created_at: now,
                modified_at: now,
                tags,
            },
        );
        Ok(&self.sampling_rules[&name])
    }

    /// The stored `RuleName` a `(RuleName, RuleARN)` pair selects, for a request
    /// addressed to `region`.
    ///
    /// `RuleName` and `RuleARN` are documented as "either name or ARN, but not
    /// both", so supplying neither or both is a refusal rather than a lookup. An
    /// ARN selects the rule whose ARN under THIS request's region it equals — an
    /// ARN naming another region names another region's rule — and it is matched
    /// by walking the stored rules, bounded by [`MAX_SAMPLING_RULES`], rather
    /// than through a second index, so there is only ever one copy of a rule.
    pub fn resolve_sampling_rule_name(
        &self,
        rule_name: Option<&str>,
        rule_arn: Option<&str>,
        region: &str,
    ) -> Result<String, SamplingRuleLookupError> {
        match (rule_name, rule_arn) {
            (None, None) => return Err(SamplingRuleLookupError::NoSelector),
            (Some(_), Some(_)) => return Err(SamplingRuleLookupError::BothSelectors),
            _ => {}
        }
        if let Some(name) = rule_name {
            if self.sampling_rules.contains_key(name) {
                return Ok(name.to_string());
            }
        }
        if let Some(arn) = rule_arn {
            if let Some(name) = self
                .sampling_rules
                .keys()
                .find(|name| sampling_rule_arn(region, name) == arn)
            {
                return Ok(name.clone());
            }
        }
        // Last the account's service-managed rule, which is derived rather than
        // stored ([`default_sampling_rule`]) and so is resolvable by name and by
        // its region's ARN without ever appearing in `sampling_rules`.
        if rule_name == Some(DEFAULT_SAMPLING_RULE_NAME)
            || rule_arn
                .is_some_and(|arn| arn == sampling_rule_arn(region, DEFAULT_SAMPLING_RULE_NAME))
        {
            return Ok(DEFAULT_SAMPLING_RULE_NAME.to_string());
        }
        Err(SamplingRuleLookupError::NotFound)
    }

    /// Whether `rule_name` already names a rule in the account, the
    /// service-managed [`DEFAULT_SAMPLING_RULE_NAME`] one included —
    /// `CreateSamplingRule`'s uniqueness check.
    pub fn sampling_rule_name_is_taken(&self, rule_name: &str) -> bool {
        rule_name == DEFAULT_SAMPLING_RULE_NAME || self.sampling_rules.contains_key(rule_name)
    }

    /// How many of the account's [`MAX_SAMPLING_RULES`] are spent: the rules it
    /// CREATED.
    ///
    /// A materialised record for the service-managed
    /// [`DEFAULT_SAMPLING_RULE_NAME`] rule is not one of them — nobody created
    /// it — so writing to that rule does not cost the account a rule it could
    /// otherwise create.
    pub fn created_sampling_rule_count(&self) -> usize {
        self.sampling_rules.len()
            - usize::from(self.sampling_rules.contains_key(DEFAULT_SAMPLING_RULE_NAME))
    }

    /// The account's [`DEFAULT_SAMPLING_RULE_NAME`] rule as a caller in `region`
    /// reads it: the configuration the stored record holds once a write has
    /// materialised one, else the derived rule — and in both cases the ARN
    /// DERIVED for `region`.
    ///
    /// The rule exists in every region without being created, so its ARN stays a
    /// pure function of the reserved name and the reader's region even after a
    /// write: the ARN a reader is answered is exactly the one that selects the
    /// rule for that reader
    /// ([`resolve_sampling_rule_name`](Self::resolve_sampling_rule_name)), which
    /// a stored ARN from whichever region wrote first would not be.
    pub fn default_sampling_rule_record(&self, region: &str) -> StoredSamplingRule {
        let mut record = self
            .sampling_rules
            .get(DEFAULT_SAMPLING_RULE_NAME)
            .cloned()
            .unwrap_or_else(|| default_sampling_rule(region));
        record.rule.rule_arn = Some(sampling_rule_arn(region, DEFAULT_SAMPLING_RULE_NAME));
        record
    }

    /// Every sampling rule a caller in `region` sees: the rules the account
    /// created, in `RuleName` order, and then the service-managed
    /// [`DEFAULT_SAMPLING_RULE_NAME`] rule.
    ///
    /// This is what `GetSamplingRules` lists, so the catch-all is visible on a
    /// fresh emulator exactly as it is in a fresh account, and an SDK reading
    /// the listing always finds a rule that matches every request. It is listed
    /// LAST, after the created rules, whether or not an update has materialised
    /// a record for it — which is also the order of its `Priority`, past every
    /// priority a created rule may carry. Bounded by [`MAX_SAMPLING_RULES`] + 1.
    pub fn visible_sampling_rules(&self, region: &str) -> Vec<StoredSamplingRule> {
        let mut visible: Vec<StoredSamplingRule> = self
            .sampling_rules
            .iter()
            .filter(|(name, _)| name.as_str() != DEFAULT_SAMPLING_RULE_NAME)
            .map(|(_, stored)| stored.clone())
            .collect();
        visible.push(self.default_sampling_rule_record(region));
        visible
    }

    /// The stored record for the rule named `rule_name`, for an operation that
    /// MUTATES it.
    ///
    /// The service-managed [`DEFAULT_SAMPLING_RULE_NAME`] rule is derived rather
    /// than stored, so the first write to it is what gives it a stored record —
    /// carrying the SAME derived members, so every selector that resolved it
    /// before still does. It is not a rule the account created, so materialising
    /// it takes none of the [`MAX_SAMPLING_RULES`] the account may create: the
    /// limit a caller observes is the same before and after that write. `None`
    /// means there is no record under that name to write to.
    fn sampling_rule_mut(
        &mut self,
        region: &str,
        rule_name: &str,
    ) -> Option<&mut StoredSamplingRule> {
        if !self.sampling_rules.contains_key(rule_name) {
            if rule_name != DEFAULT_SAMPLING_RULE_NAME {
                return None;
            }
            self.sampling_rules.insert(
                DEFAULT_SAMPLING_RULE_NAME.to_string(),
                default_sampling_rule(region),
            );
        }
        self.sampling_rules.get_mut(rule_name)
    }

    /// Apply an `UpdateSamplingRule` to the rule `rule_name` names, answering
    /// the record as it now stands, or `None` when there is no record to write
    /// (see [`sampling_rule_mut`](Self::sampling_rule_mut)).
    ///
    /// Each member the update SUPPLIES replaces the stored one; each member it
    /// OMITS leaves the stored one as it is. The write lands in the STORE, so
    /// every later read of the rule — `GetSamplingRules`, `DeleteSamplingRule`,
    /// another update — reports the updated values rather than the ones it was
    /// created with, and `ModifiedAt` moves to `now` while `CreatedAt` does not.
    /// A rule's identity (its name and the ARN derived from it) and its tags are
    /// never touched, so no index needs maintenance here.
    pub fn apply_sampling_rule_update(
        &mut self,
        region: &str,
        rule_name: &str,
        update: SamplingRuleUpdateDraft,
        now: f64,
    ) -> Option<&StoredSamplingRule> {
        let stored = self.sampling_rule_mut(region, rule_name)?;
        let rule = &mut stored.rule;
        if let Some(resource_arn) = update.resource_arn {
            rule.resource_arn = resource_arn;
        }
        if let Some(priority) = update.priority {
            rule.priority = priority;
        }
        if let Some(fixed_rate) = update.fixed_rate {
            rule.fixed_rate = fixed_rate;
        }
        if let Some(reservoir_size) = update.reservoir_size {
            rule.reservoir_size = reservoir_size;
        }
        if let Some(service_name) = update.service_name {
            rule.service_name = service_name;
        }
        if let Some(service_type) = update.service_type {
            rule.service_type = service_type;
        }
        if let Some(host) = update.host {
            rule.host = host;
        }
        if let Some(http_method) = update.http_method {
            rule.http_method = http_method;
        }
        if let Some(url_path) = update.url_path {
            rule.url_path = url_path;
        }
        if let Some(attributes) = update.attributes {
            rule.attributes = Some(attributes);
        }
        if let Some(sampling_rate_boost) = update.sampling_rate_boost {
            rule.sampling_rate_boost = Some(sampling_rate_boost);
        }
        stored.modified_at = now;
        Some(stored)
    }

    /// Remove one sampling rule, answering the record that was removed.
    ///
    /// "Deletes a sampling rule" is the whole of the operation: the rule is gone
    /// from the store, so no later listing carries it and no later selector
    /// resolves it. The service-managed [`DEFAULT_SAMPLING_RULE_NAME`] rule is
    /// not removable — `DeleteSamplingRule` refuses it before reaching here —
    /// and removing its materialised record would only put the derived rule back
    /// in the listing.
    pub fn remove_sampling_rule(&mut self, rule_name: &str) -> Option<StoredSamplingRule> {
        self.sampling_rules.remove(rule_name)
    }
}

/// A sampling rule's ARN, `arn:aws:xray:<region>:<account>:sampling-rule/<RuleName>`.
///
/// Derived from the name rather than minted with a unique suffix, because a
/// rule's name is unique in the account for as long as the rule exists — so an
/// ARN resolves back to a name without a second index.
pub fn sampling_rule_arn(region: &str, rule_name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("sampling-rule/{rule_name}"),
    )
}

// ---------------------------------------------------------------------------
// Service graphs
// ---------------------------------------------------------------------------

/// `Service$State` for a node that has at least one completed segment in the
/// graph's window: the service answered.
pub const SERVICE_STATE_ACTIVE: &str = "active";

/// `Service$State` for a node whose every segment in the window is still in
/// progress: nothing is known about how it answered. An INFERRED node — one no
/// segment reported, such as a `client` — is always in this state.
pub const SERVICE_STATE_UNKNOWN: &str = "unknown";

/// `Service$Type` for the node that "represents the clients that sent requests
/// to a root service".
///
/// The clients report no segment of their own, so the node carries the root
/// service's name and the requests that service received.
pub const SERVICE_TYPE_CLIENT: &str = "client";

/// `Service$Type` for "a downstream service of indeterminate type", which is
/// what a `remote`-namespace subsegment records.
pub const SERVICE_TYPE_REMOTE: &str = "remote";

/// The most inline subsegments one derivation walks, across every trace and
/// segment it covers.
///
/// A trace holds at most [`MAX_SEGMENTS_PER_TRACE`] segments and a segment at
/// most [`MAX_SUBSEGMENTS_PER_SEGMENT`] subsegments, so capping each factor
/// still leaves their product to grow; this caps the product. Past it a graph
/// reports the services its segments named, which is the whole topology a trace
/// whose services are instrumented has.
pub const MAX_GRAPH_SUBSEGMENT_VISITS: usize = 50_000;

/// One recorded call, as a graph counts it: a segment the service reported, or
/// a subsegment it recorded inside that segment. The two carry the same timing
/// and the same outcome flags, so the accumulators below take this rather than
/// one shape of document.
#[derive(Debug, Clone, Copy)]
struct Call {
    start_time: f64,
    end_time: Option<f64>,
    error: bool,
    fault: bool,
    throttle: bool,
}

impl Call {
    /// The call a stored segment records.
    fn of_segment(segment: &SegmentRecord) -> Self {
        Self {
            start_time: segment.start_time,
            end_time: segment.end_time,
            error: segment.is_error(),
            fault: segment.is_fault(),
            throttle: segment.is_throttle(),
        }
    }

    /// The call an inline subsegment records. A subsegment that carries no
    /// `start_time` is timed from its segment, which is the only instant known
    /// for it.
    fn of_subsegment(subsegment: &SubsegmentRecord, segment: &SegmentRecord) -> Self {
        Self {
            start_time: subsegment.start_time.unwrap_or(segment.start_time),
            end_time: subsegment.end_time,
            error: subsegment.is_error(),
            fault: subsegment.is_fault(),
            throttle: subsegment.is_throttle(),
        }
    }

    /// How long the call took, in seconds; `None` while it is in progress.
    fn duration(&self) -> Option<f64> {
        self.end_time.map(|end| end - self.start_time)
    }

    /// The latest instant this call is known to have reached.
    fn latest(&self) -> f64 {
        self.end_time.unwrap_or(self.start_time)
    }
}

/// The time span a node or an edge covers, widened call by call.
#[derive(Debug, Default)]
struct Span {
    start: Option<f64>,
    end: Option<f64>,
}

impl Span {
    /// Widen this span to include one call.
    fn widen(&mut self, call: Call) {
        self.start = Some(
            self.start
                .map_or(call.start_time, |start| start.min(call.start_time)),
        );
        self.end = Some(self.end.map_or(call.latest(), |end| end.max(call.latest())));
    }

    /// The span's start as a modelled timestamp.
    fn start_time(&self) -> Option<::aws_smithy_types::DateTime> {
        self.start.map(timestamp_of)
    }

    /// The span's end as a modelled timestamp.
    fn end_time(&self) -> Option<::aws_smithy_types::DateTime> {
        self.end.map(timestamp_of)
    }
}

/// One stored instant as the modelled `Timestamp`.
///
/// Ingest refuses an instant that is not finite or further from the epoch than
/// [`MAX_SEGMENT_EPOCH_SECONDS`], so this conversion is always in range; the
/// bounding here is the second belt, so no stored value can make a READ end the
/// request instead of answering it.
pub fn timestamp_of(seconds: f64) -> ::aws_smithy_types::DateTime {
    let bounded = if seconds.is_nan() {
        0.0
    } else {
        seconds.clamp(-MAX_SEGMENT_EPOCH_SECONDS, MAX_SEGMENT_EPOCH_SECONDS)
    };
    ::aws_smithy_types::DateTime::from_secs_f64(bounded)
}

/// The counts `ServiceStatistics` and `EdgeStatistics` both carry, accumulated
/// over the segments a node or an edge covers.
#[derive(Debug, Default)]
struct CallStatistics {
    ok_count: i64,
    throttle_count: i64,
    error_other_count: i64,
    fault_count: i64,
    total_count: i64,
    total_response_time: f64,
}

impl CallStatistics {
    /// Count one call. A fault outranks an error (a 500-series answer is not
    /// also a 400-series one), a throttled error is counted as a throttle rather
    /// than as "other", and a call that recorded neither is OK — which is
    /// exactly the exclusive [`Classification`] every other reader of this
    /// state uses, read here rather than recomputed.
    fn record(&mut self, call: Call) {
        self.total_count += 1;
        // The call's flags already fold in the recorded `http.response.status`
        // ([`Call::of_segment`] / [`Call::of_subsegment`] read them through
        // `is_fault`/`is_throttle`/`is_error`), so the status is not consulted
        // a second time here.
        match classify(call.fault, call.error, call.throttle, None) {
            Classification::Fault => self.fault_count += 1,
            Classification::Throttle => self.throttle_count += 1,
            Classification::Error => self.error_other_count += 1,
            Classification::Ok => self.ok_count += 1,
        }
        self.total_response_time += call.duration().unwrap_or(0.0);
    }

    /// The 400-series counts.
    fn error_statistics(&self) -> ErrorStatistics {
        ErrorStatistics {
            throttle_count: Some(self.throttle_count),
            other_count: Some(self.error_other_count),
            total_count: Some(self.throttle_count + self.error_other_count),
        }
    }

    /// The 500-series counts.
    fn fault_statistics(&self) -> FaultStatistics {
        FaultStatistics {
            other_count: Some(self.fault_count),
            total_count: Some(self.fault_count),
        }
    }

    /// These counts as a node's `SummaryStatistics`.
    fn service_statistics(&self) -> ServiceStatistics {
        ServiceStatistics {
            ok_count: Some(self.ok_count),
            error_statistics: Some(self.error_statistics()),
            fault_statistics: Some(self.fault_statistics()),
            total_count: Some(self.total_count),
            total_response_time: Some(self.total_response_time),
        }
    }

    /// These counts as an edge's `SummaryStatistics`.
    fn edge_statistics(&self) -> EdgeStatistics {
        EdgeStatistics {
            ok_count: Some(self.ok_count),
            error_statistics: Some(self.error_statistics()),
            fault_statistics: Some(self.fault_statistics()),
            total_count: Some(self.total_count),
            total_response_time: Some(self.total_response_time),
        }
    }
}

/// One connection from a node to a downstream node, under construction.
#[derive(Debug, Default)]
struct EdgeAccumulator {
    span: Span,
    statistics: CallStatistics,
    response_times: Vec<f64>,
}

impl EdgeAccumulator {
    /// Count one downstream call on this edge.
    fn record(&mut self, call: Call) {
        self.span.widen(call);
        self.statistics.record(call);
        if let Some(duration) = call.duration() {
            self.response_times.push(duration);
        }
    }

    /// This edge as the modelled `Edge`, naming the downstream node by its
    /// `ReferenceId`.
    fn to_edge(&self, downstream: i32) -> Edge {
        Edge {
            reference_id: Some(downstream),
            start_time: self.span.start_time(),
            end_time: self.span.end_time(),
            summary_statistics: Some(self.statistics.edge_statistics()),
            response_time_histogram: histogram(&self.response_times),
            aliases: None,
            // `link` describes an asynchronous connection, which a stored
            // segment document does not say it is; an edge derived from a
            // parent/child pair is the synchronous default.
            edge_type: None,
            received_event_age_histogram: None,
        }
    }
}

/// What distinguishes one node of a service graph from another.
///
/// A node the stored segments REPORTED is the service of that `name`. Beside it
/// a graph carries the node `Service$Type` documents as `client` — "represents
/// the clients that sent requests to a root service" — which carries the root
/// service's own name, so the name alone does not identify a node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NodeKey {
    /// The canonical service name, which is what `Service$Name` reports.
    name: String,
    /// `Service$Type` for a node no segment reported — `client`, `remote`, or
    /// `AWS::<Service>` — and `None` for one a segment did report, whose type is
    /// that segment's `origin`.
    inferred_type: Option<String>,
}

impl NodeKey {
    /// The node of the service whose segments report `name`.
    fn reported(name: &str) -> Self {
        Self {
            name: name.to_string(),
            inferred_type: None,
        }
    }

    /// The `client` node of the root service called `name`.
    fn client(name: &str) -> Self {
        Self {
            name: name.to_string(),
            inferred_type: Some(SERVICE_TYPE_CLIENT.to_string()),
        }
    }
}

/// The downstream node a SUBSEGMENT names, or `None` when it names none.
///
/// The X-Ray segment-document schema gives a subsegment a `namespace` only when
/// the work left the process: `aws` for an AWS SDK call, `remote` for any other
/// downstream call. `Service$Type` names the matching node types: `remote` is
/// "a downstream service of indeterminate type", and `AWS::<Service>` is what
/// "downstream calls to Amazon Web Services ... that didn't target a specific
/// table" report. A subsegment with neither namespace recorded in-process work
/// and so names no service at all.
fn downstream_node(name: &str, namespace: Option<&str>) -> Option<NodeKey> {
    let inferred_type = match namespace? {
        SERVICE_TYPE_REMOTE => SERVICE_TYPE_REMOTE.to_string(),
        "aws" => format!("AWS::{name}"),
        _ => return None,
    };
    Some(NodeKey {
        name: name.to_string(),
        inferred_type: Some(inferred_type),
    })
}

/// One node of a service graph, under construction.
#[derive(Debug, Default)]
struct NodeAccumulator {
    account_id: Option<String>,
    service_type: Option<String>,
    root: bool,
    complete: bool,
    span: Span,
    statistics: CallStatistics,
    durations: Vec<f64>,
    response_times: Vec<f64>,
    /// Downstream node -> the edge that reaches it.
    edges: BTreeMap<NodeKey, EdgeAccumulator>,
}

impl NodeAccumulator {
    /// Fold one request this node's service RECEIVED into it, without claiming
    /// the request's own identity: what an INFERRED node — a `client`, or a
    /// downstream service a subsegment named — knows about a call is its timing
    /// and its outcome, because it reported no segment of its own.
    fn record_call(&mut self, call: Call) {
        self.span.widen(call);
        self.statistics.record(call);
        if let Some(duration) = call.duration() {
            self.durations.push(duration);
            self.response_times.push(duration);
        }
    }

    /// Fold one of this service's segments into the node.
    fn record(&mut self, segment: &SegmentRecord) {
        let call = Call::of_segment(segment);
        self.span.widen(call);
        self.statistics.record(call);
        if segment.parent_id.is_none() {
            // "Indicates that the service was the first service to process a
            // request" — a segment with no parent is the entry point of its
            // trace.
            self.root = true;
        }
        if let Some(duration) = segment.duration() {
            self.complete = true;
            self.durations.push(duration);
            if segment.parent_id.is_none() {
                self.response_times.push(duration);
            }
        }
        if self.service_type.is_none() {
            self.service_type = segment.origin.clone();
        }
        if self.account_id.is_none() {
            self.account_id = segment.account_id.clone();
        }
    }

    /// This node as the modelled `Service`.
    fn to_service(&self, key: &NodeKey, reference_ids: &BTreeMap<NodeKey, i32>) -> Service {
        Service {
            reference_id: reference_ids.get(key).copied(),
            name: Some(key.name.clone()),
            names: Some(vec![key.name.clone()]),
            root: Some(self.root),
            account_id: self.account_id.clone(),
            // An inferred node's type is what makes it that node; a reported
            // one's is the `origin` its segments carried.
            r#type: key
                .inferred_type
                .clone()
                .or_else(|| self.service_type.clone()),
            state: Some(
                // An inferred node reported no segment of its own, so nothing
                // about its state is known; a reported one is active once one of
                // its segments completed.
                if self.complete && key.inferred_type.is_none() {
                    SERVICE_STATE_ACTIVE
                } else {
                    SERVICE_STATE_UNKNOWN
                }
                .to_string(),
            ),
            start_time: self.span.start_time(),
            end_time: self.span.end_time(),
            edges: Some(
                self.edges
                    .iter()
                    .filter_map(|(downstream, edge)| {
                        Some(edge.to_edge(reference_ids.get(downstream).copied()?))
                    })
                    .collect(),
            ),
            summary_statistics: Some(self.statistics.service_statistics()),
            duration_histogram: histogram(&self.durations),
            response_time_histogram: histogram(&self.response_times),
        }
    }
}

/// The spread of `values` as the modelled `Histogram`: one entry per distinct
/// millisecond-rounded value with how many samples landed on it, ascending, at
/// most [`MAX_HISTOGRAM_ENTRIES`] of them. `None` when nothing was sampled.
fn histogram(values: &[f64]) -> Option<Vec<HistogramEntry>> {
    if values.is_empty() {
        return None;
    }
    let mut buckets: BTreeMap<i64, i32> = BTreeMap::new();
    for value in values {
        #[allow(clippy::cast_possible_truncation)]
        let millis = (value * 1_000.0).round() as i64;
        *buckets.entry(millis).or_default() += 1;
    }
    Some(
        buckets
            .into_iter()
            .take(MAX_HISTOGRAM_ENTRIES)
            .map(|(millis, count)| {
                #[allow(clippy::cast_precision_loss)]
                let value = millis as f64 / 1_000.0;
                HistogramEntry { value, count }
            })
            .collect(),
    )
}

/// Derive the `Service` list a service graph reports from stored traces.
///
/// This is the one derivation behind both graph readers — `GetServiceGraph` over
/// a time window and `GetTraceGraph` over named traces — so the two can never
/// describe the same stored segments differently.
///
/// A node is one distinct segment `name` (the canonical name of the service that
/// handled the request), plus the `client` node `Service$Type` documents for
/// every root service — "represents the clients that sent requests to a root
/// service" — which carries that service's own name and one edge into it, plus
/// one node per SUBSEGMENT whose `namespace` says the call left the process
/// (`remote`, "a downstream service of indeterminate type", or `aws` for an AWS
/// service). A subsegment reports no service of its own otherwise: one with no
/// such namespace records in-process work, so it adds neither a node nor an
/// edge, and the work it did is already counted on the service that recorded it.
///
/// An edge is a stored `parent_id` link between two segments of DIFFERENT
/// services, counted on the upstream node and naming the downstream one by
/// `ReferenceId`; a `parent_id` that names a subsegment resolves to the service
/// that reported it. Every count, time and histogram comes from the stored
/// segment documents; nothing is echoed from the request.
///
/// `window` selects which segments take part: `Some((start, end))` keeps the
/// segments whose `start_time` falls in the half-open interval [start, end) —
/// the same selection `GetTraceSummaries` makes — and `None` keeps every segment
/// of every trace handed in, which is what a per-trace graph is.
pub fn service_graph<'a>(
    traces: impl IntoIterator<Item = &'a TraceRecord>,
    window: Option<(f64, f64)>,
) -> Vec<Service> {
    let selects = |segment: &SegmentRecord| match window {
        Some((start, end)) => segment.start_time >= start && segment.start_time < end,
        None => true,
    };

    let mut nodes: BTreeMap<NodeKey, NodeAccumulator> = BTreeMap::new();
    // Inline subsegments are walked under one budget across the whole
    // derivation, so a graph over many traces of many segments, each carrying
    // many subsegments, cannot multiply into unbounded work: past the budget a
    // graph reports the services its SEGMENTS named.
    let mut subsegment_budget = MAX_GRAPH_SUBSEGMENT_VISITS;
    for trace in traces {
        for segment in &trace.segments {
            // An inferred segment is not one a service REPORTED: it is this
            // service's own projection of a remote subsegment
            // ([`inferred_segments`]), and the downstream node that subsegment
            // names is derived from the subsegment itself below — so counting
            // the projection too would report one downstream call as two nodes.
            if !selects(segment) || segment.is_subsegment() || segment.inferred {
                continue;
            }
            let call = Call::of_segment(segment);
            nodes
                .entry(NodeKey::reported(&segment.name))
                .or_default()
                .record(segment);
            if segment.parent_id.is_none() {
                // The entry point of a trace was reached by a client the stored
                // data names nowhere, which is exactly the node the model
                // documents as `client`: it carries the root service's name,
                // reports the requests it sent, and reaches that service by one
                // edge.
                let client = nodes.entry(NodeKey::client(&segment.name)).or_default();
                client.record_call(call);
                client
                    .edges
                    .entry(NodeKey::reported(&segment.name))
                    .or_default()
                    .record(call);
            }
            // The downstream services this segment's subsegments named: each is
            // one inferred node, reached by one edge from this service.
            let mut pending: Vec<&SubsegmentRecord> = segment.subsegments.iter().rev().collect();
            while let Some(subsegment) = pending.pop() {
                if subsegment_budget == 0 {
                    break;
                }
                subsegment_budget -= 1;
                pending.extend(subsegment.subsegments.iter().rev());
                let Some(downstream) = downstream_node(&subsegment.name, subsegment.namespace.as_deref())
                else {
                    continue;
                };
                if downstream == NodeKey::reported(&segment.name) {
                    continue;
                }
                let subsegment_call = Call::of_subsegment(subsegment, segment);
                nodes
                    .entry(NodeKey::reported(&segment.name))
                    .or_default()
                    .edges
                    .entry(downstream.clone())
                    .or_default()
                    .record(subsegment_call);
                nodes
                    .entry(downstream)
                    .or_default()
                    .record_call(subsegment_call);
            }
        }
        // Edges need both ends, so they are walked once this trace's nodes are
        // known: a segment whose `parent_id` names nothing stored, or whose
        // parent is outside the window, is on no edge. Which service reported
        // each stored id is resolved ONCE for the whole trace, so this pass is
        // linear in the trace's segments however long a chain of subsegment
        // documents is.
        let reporting = reporting_segments(trace);
        for segment in &trace.segments {
            if !selects(segment) || segment.inferred {
                continue;
            }
            // The service that recorded this segment's parent: a `parent_id`
            // naming a subsegment belongs to the service that reported it.
            let Some(parent) = segment
                .parent_id
                .as_deref()
                .and_then(|parent_id| reporting.get(parent_id).copied())
            else {
                continue;
            };
            if !selects(parent) {
                continue;
            }
            let Some(upstream) = nodes.get_mut(&NodeKey::reported(&parent.name)) else {
                continue;
            };
            // A subsegment names a downstream service only through its
            // `namespace`; a segment names the service that handled it.
            let downstream = if segment.is_subsegment() {
                match downstream_node(&segment.name, segment.namespace.as_deref()) {
                    Some(key) => key,
                    None => continue,
                }
            } else {
                NodeKey::reported(&segment.name)
            };
            if downstream == NodeKey::reported(&parent.name) {
                continue;
            }
            let call = Call::of_segment(segment);
            upstream.edges.entry(downstream.clone()).or_default().record(call);
            // A downstream service a subsegment named reported nothing itself,
            // so the call is what the node knows about it.
            if segment.is_subsegment() {
                nodes.entry(downstream).or_default().record_call(call);
            }
        }
    }

    // "Identifier for the service. Unique within the service map": assigned over
    // the nodes in key order — name, then the inferred type that distinguishes a
    // `client` node from the service it calls — so one stored graph always
    // answers with the same ids, across pages of one walk and across calls.
    let reference_ids: BTreeMap<NodeKey, i32> = nodes
        .keys()
        .enumerate()
        .map(|(at, key)| (key.clone(), i32::try_from(at).unwrap_or(i32::MAX)))
        .collect();

    nodes
        .iter()
        .map(|(key, node)| node.to_service(key, &reference_ids))
        .collect()
}

/// Every stored id of one trace mapped to the segment that REPORTED the service
/// it belongs to.
///
/// A `type: "subsegment"` document records work inside its parent's service, so
/// it belongs to whichever segment its `parent_id` chain reaches. Resolving that
/// by walking the chain per segment would be quadratic in the trace's segments,
/// so the map is built ONCE: the segments a service reported seed it, and the
/// subsegment documents are attached by walking OUT from the ids already
/// resolved, which visits each document at most twice and ends by itself on a
/// chain that names itself (each parent's children are consumed once).
fn reporting_segments(trace: &TraceRecord) -> BTreeMap<&str, &SegmentRecord> {
    let mut reporting: BTreeMap<&str, &SegmentRecord> = BTreeMap::new();
    let mut attached: BTreeMap<&str, Vec<&SegmentRecord>> = BTreeMap::new();
    for segment in &trace.segments {
        if segment.is_subsegment() {
            if let Some(parent) = segment.parent_id.as_deref() {
                attached.entry(parent).or_default().push(segment);
            }
        } else {
            reporting.insert(segment.id.as_str(), segment);
        }
    }
    let mut frontier: Vec<&str> = reporting.keys().copied().collect();
    while let Some(id) = frontier.pop() {
        let Some(children) = attached.remove(id) else {
            continue;
        };
        let Some(owner) = reporting.get(id).copied() else {
            continue;
        };
        for child in children {
            reporting.insert(child.id.as_str(), owner);
            frontier.push(child.id.as_str());
        }
    }
    reporting
}

/// One page of a service graph: the `Service` values to answer with, and the
/// `NextToken` that resumes after them when the graph was truncated.
///
/// Lives beside [`service_graph`] so the two graph readers cannot drift in their
/// pagination contract.
#[derive(Debug, Clone)]
pub struct ServiceGraphPage {
    /// The page's services, at most `page_size` of them.
    pub services: Vec<Service>,
    /// The token that resumes after this page, or `None` when it is the last.
    pub next_token: Option<String>,
}

/// Cut one page out of a WHOLE derived graph, resuming at `resume`.
///
/// The page is cut after the graph is derived, so every page of one walk reports
/// the same `ReferenceId` for the same service and an edge's `ReferenceId` names
/// a node whichever page that node lands on. The token is the `ReferenceId` the
/// next page starts at, which is what identifies a node: a `client` node carries
/// the same `Name` as the root service it calls, so a name would not. `Err` is
/// the one refusal this makes possible: a token that is not a `ReferenceId` of
/// this graph, which is a token this service never issued.
pub fn page_service_graph(
    services: &[Service],
    resume: Option<&str>,
    page_size: usize,
) -> Result<ServiceGraphPage, String> {
    let unissued = |token: &str| format!("NextToken `{token}` was not issued by this service");
    let from = match resume {
        Some(token) => {
            let reference: i32 = token.parse().map_err(|_| unissued(token))?;
            services
                .iter()
                .position(|service| service.reference_id.is_some_and(|id| id >= reference))
                .ok_or_else(|| unissued(token))?
        }
        None => 0,
    };
    let remaining = &services[from..];
    Ok(ServiceGraphPage {
        services: remaining.iter().take(page_size).cloned().collect(),
        next_token: remaining
            .get(page_size)
            .and_then(|service| service.reference_id)
            .map(|reference| reference.to_string()),
    })
}

/// The region this request's SigV4 credential scope names, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
///
/// `None` means the service was mounted on an engine that holds no state, which
/// is a misconfiguration of the emulator rather than anything about the request.
/// X-Ray models no server-fault shape — every operation here declares only
/// `InvalidRequestException` (400) and `ThrottledException` (429) — so the
/// handlers answer `ThrottledException` with a message naming the
/// misconfiguration: of the two codes this service can speak, it is the one that
/// does not blame the caller's request. The production router always mounts this
/// service on the shared `InMemoryEngine`, so the branch is unreachable there.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsXrayAwsxrayState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsXrayAwsxrayState>)
}
