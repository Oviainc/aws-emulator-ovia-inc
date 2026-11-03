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
//!   ([`inferred_segments`]), which is the service X-Ray attributes the call to,
//!   and which the call's own namespace TYPES ([`inferred_service_type`])
//!   because a service nothing reported for carries no `origin`;
//! * every entity (a segment, or a subsegment inside one) records an
//!   [`Outcome`], read from the `fault`/`error`/`throttle` fields its document
//!   DECLARED: the one [`Classification`] a COUNTER splits it
//!   into (a fault outranks a throttle, which outranks an error), and whether it
//!   recorded a 400-series error at all. A recorded `http.response.status` is
//!   data the document carries, reported as `Http$HttpStatus` and selectable
//!   through the `http.status` keyword, and it never stands in for a flag the
//!   document did not set. The trace's
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
    ProbabilisticRuleValue, SamplingRateBoost, SamplingRule, SamplingRuleRecord,
    SamplingStatisticSummary, Service, ServiceStatistics, TelemetryRecord,
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

/// What X-Ray answers a `PutEncryptionConfig` that selects KMS encryption
/// without naming a key with.
///
/// FIXED text: the refusal states the condition that was broken — a key is
/// needed for this type of encryption — and interpolates nothing from the
/// request, so it is the same sentence whatever else the request carried.
pub const ENCRYPTION_KEY_ID_REQUIRED_MESSAGE: &str = "Key ID is required for type KMS";

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

/// The ARN of the KMS key a submitted `KeyId` selector names, as a caller in
/// `region` reads it.
///
/// The configuration a reader reports IDENTIFIES the key that protects the
/// account's trace data, and an identifier that names a key only inside one
/// account and region does not identify it: a bare key id or alias is the same
/// text in every account. So the stored configuration holds the FULLY QUALIFIED
/// form — the partition, the region the configuration was put in and the account
/// this service serves — whatever form the request used, and a selector that
/// already carries those fields is kept exactly as submitted, including the
/// partition, region and account it names.
pub fn kms_key_arn(key_id: &str, region: &str) -> String {
    if is_kms_arn(key_id) {
        return key_id.to_string();
    }
    // The two unqualified forms `PutEncryptionConfig$KeyId` documents: an alias
    // path, which an ARN carries as its resource unchanged, and a key id, whose
    // resource is `key/<id>`.
    let resource = if is_kms_alias(key_id) {
        key_id.to_string()
    } else {
        format!("key/{key_id}")
    };
    ::emulator_server::emulator_arn("kms", region, &resource)
}

/// The encryption configuration this service reports: what `PutEncryptionConfig`
/// last stored, and when the clock owes it `ACTIVE`.
#[derive(Debug, Clone)]
pub struct EncryptionConfigRecord {
    /// `EncryptionConfig$Type`: [`ENCRYPTION_TYPE_KMS`] or
    /// [`ENCRYPTION_TYPE_NONE`].
    pub encryption_type: String,
    /// `EncryptionConfig$KeyId`: the ARN of the key the caller selected, which
    /// is what IDENTIFIES it ([`kms_key_arn`]); `None` under
    /// [`ENCRYPTION_TYPE_NONE`].
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
///
/// The form itself is read through the ONE shared reading of it
/// (`service_common::xray_trace`), which a service that merely CARRIES a trace
/// header asks as well, so no two services judge one id differently.
pub fn is_trace_id(value: &str) -> bool {
    ::service_common::xray_trace::is_trace_id(value)
}

/// What a READ asked for a trace id that is not of [`is_trace_id`]'s form is
/// refused with: X-Ray's template, naming the value the request carried and the
/// rule it broke, and nothing else.
///
/// Written once because every reader that takes a `TraceId` refuses the same
/// malformed id the same way, whichever member carried it.
pub fn malformed_trace_id_message(trace_id: &str) -> String {
    format!("Invalid trace id {trace_id}: incorrect id format")
}

/// What a read naming more trace ids than [`MAX_TRACE_IDS_PER_CALL`] is refused
/// with: X-Ray's fixed phrase, which states the limit itself — the model
/// publishes none, so the refusal is the only place a client can read it.
pub fn exceeding_maximum_query_size_message() -> String {
    format!("Exceeding maximum query size: {MAX_TRACE_IDS_PER_CALL}")
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

/// The most frames one response-time root cause's chain descends through
/// ([`TraceRecord::root_causes`] for `RootCauseKind::ResponseTime`).
///
/// The descent takes one step per frame, and a stored trace can grow in two
/// dimensions at once — [`MAX_SEGMENTS_PER_TRACE`] segments, each
/// [`MAX_SUBSEGMENT_DEPTH`] levels deep — so this caps their product rather than
/// either factor. Past it the chain reports the frames it reached.
pub const MAX_CRITICAL_PATH_FRAMES: usize = MAX_SEGMENTS_PER_TRACE + MAX_SUBSEGMENT_DEPTH;

/// How much of a trace's response time one component below its root segment must
/// have been running for to be reported as the trace's response-time root cause
/// ([`TraceRecord::root_causes`] for `RootCauseKind::ResponseTime`).
///
/// A response-time root cause names the component the time went to, so there has
/// to be one: a trace whose root segment spent its span on nothing in particular
/// — several short children, or no child at all — has no such component, and
/// reports no cause. X-Ray publishes no threshold, so "most of the response
/// time" is this emulator's reading of it.
pub const RESPONSE_TIME_ROOT_CAUSE_MIN_COVERAGE: f64 = 0.5;

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
    /// The sampling results services reported through `GetSamplingTargets`, the
    /// reservoir shares computed from them, and when the account's sampling-rule
    /// configuration last changed.
    pub sampling: SamplingStatisticsStore,
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
    /// When a segment of this trace was last accepted, in seconds of epoch
    /// time: the trace's UPDATE time, which is the instant
    /// `GetTraceSummaries$TimeRangeType` `Event` windows a trace by. Written at
    /// ingest, so it is the service's own clock rather than anything a document
    /// carried.
    pub last_updated_at: f64,
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
    /// The service type of the node this segment stands for when the service
    /// INFERRED it from a call out of another service
    /// ([`inferred_service_type`]); `None` for a segment a service submitted,
    /// whose type is the `origin` it carried. Read through
    /// [`SegmentRecord::service_type`].
    pub inferred_type: Option<String>,
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

/// Classify one entity from the flags its own document declared.
///
/// The outcome of an entity is the `fault` / `error` / `throttle` booleans its
/// document SET. A recorded `http.response.status` is data the document carries
/// — reported as `Http$HttpStatus` and selectable through the `http.status`
/// keyword — and not one of those flags, so a document that declares none of
/// them recorded none, whatever status it also recorded; the status-to-flag
/// convention belongs to the instrumentation that writes the document, not to
/// the service that reads it. This is where the three become EXCLUSIVE, in the
/// order X-Ray reclassifies them: a fault outranks a throttle, which outranks
/// an error.
fn classify(fault: bool, error: bool, throttle: bool) -> Classification {
    if fault {
        Classification::Fault
    } else if throttle {
        Classification::Throttle
    } else if recorded_error(error, throttle) {
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

/// The `UnprocessedStatistics$Message` for a statistics document reported
/// against a sampling rule the account does not hold.
///
/// A DIFFERENT sentence from [`SAMPLING_RULE_NOT_FOUND_MESSAGE`], and
/// deliberately so: that one is the message of a whole-request refusal
/// (`DeleteSamplingRule`, `UpdateSamplingRule`), while this is the per-document
/// note inside a `GetSamplingTargets` 200 — a report the service declined to
/// process rather than a call it refused. FIXED text: the entry names the rule
/// in its own `RuleName` member, so nothing of the request reaches the sentence.
pub const UNKNOWN_SAMPLING_RULE_MESSAGE: &str = "Unknown rule";

/// What X-Ray answers when a `ResourceARN` handed to one of the tagging
/// operations names no resource this account holds.
///
/// The template states the resource the caller asked about and that it was not
/// found, with the ARN echoed VERBATIM — undecorated, so a client that parses
/// the ARN back out of the message reads the bytes it sent. Single-homed here so
/// `TagResource`, `UntagResource` and `ListTagsForResource` cannot drift into
/// three sentences for one error class.
#[must_use]
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource {resource_arn} not found")
}

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

/// What a read whose `FilterExpression` spent its whole work budget
/// ([`filter::MAX_QUERY_STEPS`]) answers.
///
/// The request is well formed; what it would COST is the problem, so every
/// reader that evaluates an expression lifts this one sentence into its own
/// modelled THROTTLING error rather than into one that blames the input — and it
/// is an error rather than a short answer, because a derivation that stopped
/// halfway is indistinguishable from one that found nothing more.
pub const FILTER_BUDGET_SPENT_MESSAGE: &str = "the FilterExpression could not be evaluated over every trace in [StartTime, EndTime) within the work one request may do; narrow the time range or simplify the expression";

/// The refusal X-Ray answers when a request does not supply EXACTLY ONE group
/// selector — neither of them, or both at once.
///
/// A group is identified by exactly one of `GroupName` or `GroupARN`, so a
/// request carrying both states two identities at once and one carrying neither
/// names no group at all; X-Ray words both halves of that one rule the same way
/// ("Provide either GroupName or GroupARN") rather than servicing one selector
/// or widening the call to every group.
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
            // One sentence for both halves of "exactly one selector": a request
            // naming neither is as unresolvable as one naming two, and X-Ray
            // states them identically (see
            // [`CONFLICTING_GROUP_SELECTORS_MESSAGE`]).
            Self::NoSelector | Self::ConflictingSelectors => CONFLICTING_GROUP_SELECTORS_MESSAGE,
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
        // NOT read from the document: `inferred` and the type that goes with it
        // are service-assigned, and the segment schema `PutTraceSegments`
        // documents has no such member, so a submitted document cannot claim to
        // be a service the trace only called. [`inferred_segment`] is the one
        // place they are set; a submitted segment is typed by its `origin`.
        inferred: false,
        inferred_type: None,
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

/// The `namespace` that makes a subsegment a call out through the AWS SDK, so
/// the work it covers is an AWS service's.
pub const AWS_NAMESPACE: &str = "aws";

/// The service type of a node X-Ray INFERRED from a call out of an instrumented
/// service, which the calling entity's `namespace` names, or `None` when the
/// entity names no downstream service at all.
///
/// `Service$Type` documents the vocabulary: [`SERVICE_TYPE_REMOTE`] is "a
/// downstream service of indeterminate type", which is what a
/// [`REMOTE_NAMESPACE`] entity records, and `AWS::<name>` is what
/// "downstream calls to Amazon Web Services ... that didn't target a specific
/// table" report, which is what an [`AWS_NAMESPACE`] entity records. An entity
/// with neither namespace recorded in-process work and infers no service.
///
/// This is the crate's ONE namespace-to-service-type derivation, so every
/// surface that reports the identity of an inferred service — a service graph's
/// nodes, a trace summary's `ServiceIds`, a root cause's services, a
/// `service(name, type)` filter expression — types that service the same way.
pub fn inferred_service_type(name: &str, namespace: Option<&str>) -> Option<String> {
    match namespace? {
        REMOTE_NAMESPACE => Some(SERVICE_TYPE_REMOTE.to_string()),
        AWS_NAMESPACE => Some(format!("AWS::{name}")),
        _ => None,
    }
}

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
///
/// The tree is walked with an explicit worklist rather than by recursion, so the
/// nesting a submitted document carries costs heap and not call stack; the order
/// is the recursion's, a node then its children left to right.
pub fn inferred_segments(trace_id: &str, segment: &SegmentRecord) -> Vec<SegmentRecord> {
    let mut out = Vec::new();
    if segment.inferred {
        // A synthesised segment never derives further segments from itself. It
        // carries no subsegments either, so this is belt and braces: only
        // `inferred_segment` sets the flag, never a submitted document.
        return out;
    }
    let mut pending: Vec<&SubsegmentRecord> = segment.subsegments.iter().rev().collect();
    while let Some(subsegment) = pending.pop() {
        if let (Some(remote_id), true) = (
            subsegment.id.as_deref(),
            subsegment.namespace.as_deref() == Some(REMOTE_NAMESPACE),
        ) {
            out.push(inferred_segment(
                trace_id,
                remote_id,
                subsegment,
                segment.start_time,
            ));
        }
        pending.extend(subsegment.subsegments.iter().rev());
    }
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
        // The node this segment stands for reported nothing itself, so it has no
        // `origin`: what types it is the call that inferred it.
        inferred_type: inferred_service_type(&remote.name, remote.namespace.as_deref()),
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

/// Whether an entity classified `class` can ANCHOR a root cause of that class:
/// it recorded the outcome, and its own `cause` block named at least one
/// exception to attribute the outcome to.
///
/// A fault or error root cause is the attribution of an outcome to an exception
/// a `cause` block named ([`RootCauseException`](CauseException) is "the
/// exception associated with a root cause"), so an entity that declared a flag
/// and submitted no exception gives the service nothing to attribute and
/// anchors no root cause. This is the ONE place that condition is written, so
/// every reader of a derived root cause — a trace summary's lists and the
/// filter language's `rootcause.*` keywords — agrees on which entities have one.
fn anchors_root_cause(
    class: Classification,
    recorded: Classification,
    exceptions: &[CauseException],
) -> bool {
    recorded == class && !exceptions.is_empty()
}

/// One entity's recorded outcome.
///
/// Two facts, because the model reports two: the EXCLUSIVE class the counters
/// split a call into ([`Classification`]), and whether the entity recorded a
/// 400-series error at all. They disagree exactly where X-Ray does: a throttled
/// request is counted under `ErrorStatistics$ThrottleCount` rather than
/// `$OtherCount`, and a document that declared both a throttle and an error is a
/// throttle alone.
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

    /// This subsegment's outcome, from the flags it declared.
    pub fn classification(&self) -> Classification {
        classify(self.fault, self.error, self.throttle)
    }

    /// Whether this subsegment recorded a 400-series error.
    pub fn is_error(&self) -> bool {
        recorded_error(self.error, self.throttle)
    }

    /// This subsegment's recorded outcome: its exclusive class, and whether it
    /// recorded a 400-series error.
    pub fn outcome(&self) -> Outcome {
        Outcome {
            class: self.classification(),
            error_400: self.is_error(),
        }
    }

    /// Whether this subsegment declared a 500-series fault.
    pub fn is_fault(&self) -> bool {
        self.fault
    }

    /// Whether this subsegment declared a throttling error.
    pub fn is_throttle(&self) -> bool {
        self.throttle
    }

    /// Whether this subsegment is a call OUT of the service it sits in.
    pub fn is_remote(&self) -> bool {
        self.namespace.as_deref() == Some(REMOTE_NAMESPACE)
    }

    /// How long this subsegment ran, in seconds; `None` unless it recorded both
    /// of the instants a duration needs.
    pub fn duration(&self) -> Option<f64> {
        let span = self.end_time? - self.start_time?;
        (span.is_finite() && span >= 0.0).then_some(span)
    }

    /// This subsegment as one frame of a root cause's entity path.
    fn entity(&self) -> RootCauseEntityView {
        RootCauseEntityView {
            name: self.name.clone(),
            exceptions: self.exceptions.clone(),
            remote: self.is_remote(),
            coverage: None,
        }
    }

    /// This subsegment as one frame of a root cause's entity path, covering
    /// `coverage` of the trace's response time.
    fn entity_covering(&self, coverage: Option<f64>) -> RootCauseEntityView {
        RootCauseEntityView {
            coverage,
            ..self.entity()
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
    /// that ANCHORS a root cause of `class` ([`anchors_root_cause`]), or `None`
    /// when its tree holds none.
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
        anchors_root_cause(class, self.classification(), &self.exceptions)
            .then(|| vec![self.entity()])
    }
}

/// Whether a recorded outcome is a 400-series error: the `error` flag the
/// document set, unless the same document also declared a `throttle`.
///
/// X-Ray reclassifies a declared throttle rather than unioning it, so a document
/// whose flags say `throttle: true, error: true` comes back as a throttle and
/// NOT as an error. Nothing else is consulted: `TraceSummary$HasError` reports
/// what the documents declared, not what their recorded statuses imply.
fn recorded_error(error: bool, throttle: bool) -> bool {
    error && !throttle
}

impl SegmentRecord {
    /// The `http.response.status` this segment recorded, if any.
    pub fn http_status(&self) -> Option<i32> {
        self.http.as_ref().and_then(|http| http.response_status)
    }

    /// This segment's own outcome, from the flags it declared.
    pub fn classification(&self) -> Classification {
        classify(self.fault, self.error, self.throttle)
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

    /// Whether this segment recorded a 400-series error — the `error` flag it
    /// declared, a throttle apart.
    pub fn is_error(&self) -> bool {
        recorded_error(self.error, self.throttle)
    }

    /// This segment's recorded outcome: its exclusive class, and whether it
    /// recorded a 400-series error.
    pub fn outcome(&self) -> Outcome {
        Outcome {
            class: self.classification(),
            error_400: self.is_error(),
        }
    }

    /// Whether this segment declared a 500-series fault — the `fault` flag it
    /// carried.
    pub fn is_fault(&self) -> bool {
        self.fault
    }

    /// Whether this segment declared a throttling error — the `throttle` flag it
    /// carried.
    pub fn is_throttle(&self) -> bool {
        self.throttle
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
            coverage: None,
        }
    }

    /// This segment as one frame of a root cause's entity path, covering
    /// `coverage` of the trace's response time.
    fn entity_covering(&self, coverage: Option<f64>) -> RootCauseEntityView {
        RootCauseEntityView {
            coverage,
            ..self.entity()
        }
    }

    /// The service type this segment's identity reports: the type the call that
    /// INFERRED the service gave it ([`inferred_service_type`]) when no service
    /// reported this segment, and otherwise the `origin` the reporting document
    /// carried.
    ///
    /// Every projection of a service identity reads this, so an inferred service
    /// is typed the same whichever surface reports it.
    pub fn service_type(&self) -> Option<String> {
        self.inferred_type
            .clone()
            .or_else(|| self.origin.clone())
    }

    /// This segment's service identity, as a root cause reports it.
    fn service(&self, entity_path: Vec<RootCauseEntityView>) -> RootCauseServiceView {
        RootCauseServiceView {
            name: self.name.clone(),
            r#type: self.service_type(),
            account_id: self.account_id.clone(),
            inferred: self.inferred,
            entity_path,
        }
    }

    /// The entity path from this segment down to the deepest entity inside it
    /// that ANCHORS a root cause of `class` ([`anchors_root_cause`]), or `None`
    /// when it holds none.
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
        anchors_root_cause(class, self.classification(), &self.exceptions)
            .then(|| vec![self.entity()])
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
    /// Its service type: the `origin` a reporting service carried, or the type
    /// the call that inferred the service gave it
    /// ([`SegmentRecord::service_type`]).
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
    /// How much of the trace's response time this frame ran for, as a fraction
    /// of it — `ResponseTimeRootCauseEntity$Coverage`, "the relative amount of
    /// time that the component was running". Only a response-time root cause
    /// reports it; the fault and error entity shapes carry no `Coverage` member,
    /// so their frames leave it unset.
    pub coverage: Option<f64>,
}

/// Which root causes a reader is asking for — the three lists X-Ray reports on a
/// `TraceSummary`. Two are read off the outcome a submitted document declared;
/// the third is read off the TIMING those documents recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootCauseKind {
    /// `TraceSummary$FaultRootCauses`.
    Fault,
    /// `TraceSummary$ErrorRootCauses`.
    Error,
    /// `TraceSummary$ResponseTimeRootCauses`.
    ResponseTime,
}

impl RootCauseKind {
    /// The entity classification this kind of root cause is built from, for the
    /// two kinds a declared outcome builds; `None` for the response-time kind,
    /// which is built from the recorded timing instead.
    pub fn classification(self) -> Option<Classification> {
        match self {
            Self::Fault => Some(Classification::Fault),
            Self::Error => Some(Classification::Error),
            Self::ResponseTime => None,
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

    /// Whether any stored segment of this trace STARTED inside the half-open
    /// window [`start`, `end`) — the selection every windowed derivation of this
    /// crate makes, segment by segment.
    ///
    /// Asked BEFORE a group's `FilterExpression` is evaluated over a trace, so a
    /// trace that cannot contribute to the answer costs none of the query budget
    /// ([`filter::QueryBudget`]) however large the store grows.
    pub fn touches_window(&self, start: f64, end: f64) -> bool {
        self.segments
            .iter()
            .any(|segment| segment.start_time >= start && segment.start_time < end)
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
    /// class, because the two differ where a document declared an error inside a
    /// segment that is itself counted as something else ([`recorded_error`]).
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
    /// (`TraceSummary$FaultRootCauses` / `$ErrorRootCauses`), or the one the
    /// recorded timing explains ([`response_time_root_causes`](Self::response_time_root_causes))
    /// for `RootCauseKind::ResponseTime`.
    ///
    /// One per segment whose own tree holds an entity that ANCHORS a cause of
    /// that class — one that recorded the outcome AND named the exceptions it is
    /// attributed to ([`anchors_root_cause`]). Its `Services` are the chain of
    /// services from the trace's root down to that segment, each carrying the
    /// frames inside it the outcome ran through — the frame that called out (a
    /// dependency frame, `Remote`) or the work it did itself (a pipeline frame)
    /// — and the deepest frame carries the exceptions its own `cause` block
    /// named. A trace whose documents declared an outcome but submitted no
    /// exception to attribute it to reports NO root cause of that class: the
    /// lists are derived from what the documents carry, and there is nothing
    /// here to diagnose it with.
    ///
    /// Bounded in both dimensions a trace can grow in: at most
    /// [`MAX_ROOT_CAUSES_PER_TRACE`] causes are derived, and each chain walks
    /// each of the trace's segments at most once, resolving every hop through
    /// one entity index built here rather than by re-scanning the segments per
    /// hop. A trace that recorded more outcomes than the cap therefore reports
    /// the first of them rather than making one read do super-linear work.
    pub fn root_causes(&self, kind: RootCauseKind) -> Vec<RootCauseView> {
        let Some(class) = kind.classification() else {
            return self.response_time_root_causes();
        };
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

    /// The response-time root cause of this trace
    /// (`TraceSummary$ResponseTimeRootCauses`), derived from the TIMING the
    /// stored documents recorded rather than from an outcome any of them
    /// declared.
    ///
    /// A trace's response time is its root segment's own span, and what explains
    /// that span is the chain of components the time was spent in: starting at
    /// the root segment, each step descends into the child entity that ran
    /// LONGEST — a subsegment of the entity in hand, or the segment another
    /// service reported under it — until no child recorded a span. That chain is
    /// the cause's `Services`, root first, each carrying the frames inside it the
    /// time ran through: the frame that called out of its service (`Remote`, a
    /// dependency frame) or the work it did itself (a pipeline frame), with each
    /// frame's `Coverage` the part of the response time it ran for.
    ///
    /// A cause is reported only when the chain reaches a component BELOW the
    /// root segment that ran for at least
    /// [`RESPONSE_TIME_ROOT_CAUSE_MIN_COVERAGE`] of the response time: a trace
    /// whose time is not attributable to one component inside it has no
    /// response-time root cause, which is why most traces report none. An
    /// in-progress root segment has no response time yet, so it has none either.
    ///
    /// Bounded: the index of child segments is built once, the descent takes at
    /// most [`MAX_CRITICAL_PATH_FRAMES`] steps, and each step looks only at the
    /// children of the entity in hand.
    fn response_time_root_causes(&self) -> Vec<RootCauseView> {
        let Some(root_at) = self
            .segments
            .iter()
            .position(|segment| segment.parent_id.is_none() && !segment.inferred)
        else {
            return Vec::new();
        };
        let Some(response_time) = self.segments[root_at]
            .duration()
            .filter(|span| span.is_finite() && *span > 0.0)
        else {
            return Vec::new();
        };
        let covers = |span: Option<f64>| span.map(|span| (span / response_time).clamp(0.0, 1.0));

        // Which segments hang off which entity id, so a descent resolves the
        // next service by lookup instead of re-scanning the trace per step.
        let mut children_of: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (at, segment) in self.segments.iter().enumerate() {
            if let Some(parent_id) = segment.parent_id.as_deref() {
                children_of.entry(parent_id).or_default().push(at);
            }
        }

        // The chain, root first: which segment each frame belongs to, so the
        // frames can be grouped into one service per segment below.
        let mut frames: Vec<(usize, RootCauseEntityView)> = vec![(
            root_at,
            self.segments[root_at].entity_covering(covers(Some(response_time))),
        )];
        let mut at = root_at;
        let mut standing_on: Option<&SubsegmentRecord> = None;
        let mut leaf_coverage = 0.0_f64;
        for _ in 0..MAX_CRITICAL_PATH_FRAMES {
            let (entity_id, subsegments) = match standing_on {
                None => (
                    Some(self.segments[at].id.as_str()),
                    &self.segments[at].subsegments,
                ),
                Some(subsegment) => (subsegment.id.as_deref(), &subsegment.subsegments),
            };
            let longest_subsegment = subsegments
                .iter()
                .filter_map(|subsegment| Some((subsegment, subsegment.duration()?)))
                .max_by(|left, right| left.1.total_cmp(&right.1));
            let longest_segment = entity_id
                .and_then(|entity_id| children_of.get(entity_id))
                .into_iter()
                .flatten()
                .filter_map(|child| Some((*child, self.segments[*child].duration()?)))
                .max_by(|left, right| left.1.total_cmp(&right.1));
            let subsegment_span = longest_subsegment.map_or(f64::NEG_INFINITY, |(_, span)| span);
            let segment_span = longest_segment.map_or(f64::NEG_INFINITY, |(_, span)| span);
            if subsegment_span >= segment_span {
                let Some((subsegment, span)) = longest_subsegment else {
                    break;
                };
                leaf_coverage = covers(Some(span)).unwrap_or_default();
                frames.push((at, subsegment.entity_covering(covers(Some(span)))));
                standing_on = Some(subsegment);
            } else {
                let Some((child, span)) = longest_segment else {
                    break;
                };
                leaf_coverage = covers(Some(span)).unwrap_or_default();
                frames.push((child, self.segments[child].entity_covering(covers(Some(span)))));
                at = child;
                standing_on = None;
            }
        }

        if frames.len() < 2 || leaf_coverage < RESPONSE_TIME_ROOT_CAUSE_MIN_COVERAGE {
            return Vec::new();
        }

        // One service per segment the chain passed through, in the order it
        // passed through them, carrying that segment's frames.
        let mut services: Vec<RootCauseServiceView> = Vec::new();
        let mut current: Option<(usize, Vec<RootCauseEntityView>)> = None;
        for (segment_at, frame) in frames {
            match &mut current {
                Some((holder, path)) if *holder == segment_at => path.push(frame),
                _ => {
                    if let Some((holder, path)) = current.take() {
                        services.push(self.segments[holder].service(path));
                    }
                    current = Some((segment_at, vec![frame]));
                }
            }
        }
        if let Some((holder, path)) = current {
            services.push(self.segments[holder].service(path));
        }

        vec![RootCauseView {
            services,
            // The chain explains the ROOT segment's own span, which is the time
            // the trace's client waited for, so the client saw it.
            client_impacting: true,
        }]
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
            last_updated_at: self.last_updated_at,
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
        // The trace's update time: when THIS service accepted a document for
        // it, which is what the `Event` time range windows a trace by.
        trace.last_updated_at = now_epoch_seconds();
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

    /// The compiled `FilterExpression` a `(GroupName, GroupARN)` pair narrows a
    /// derived read by — the ONE resolution every read over a group makes, so
    /// `GetServiceGraph` and `GetTimeSeriesServiceStatistics` cannot drift into
    /// two readings of one rule.
    ///
    /// `Ok(None)` is "nothing narrows this read": the request named no group, or
    /// the group it named carries no expression (the reserved
    /// [`DEFAULT_GROUP_NAME`] group is the one such group, since `CreateGroup`
    /// requires an expression). `Ok(Some(expression))` selects a SUBSET of the
    /// stored traces, which is an answer over less data and never a refusal.
    ///
    /// `Err` carries the refusal message, which each caller lifts into its own
    /// modelled client-input error: a selector that resolves no single group
    /// ([`GroupLookupError::message`]), or an expression the evaluator cannot
    /// answer — refused naming it, because answering a filtered request with the
    /// UNFILTERED derivation would be a wrong answer rather than a missing one.
    pub fn group_filter(
        &self,
        region: &str,
        group_name: Option<&str>,
        group_arn: Option<&str>,
    ) -> Result<Option<filter::FilterExpression>, String> {
        if group_name.is_none() && group_arn.is_none() {
            return Ok(None);
        }
        let group = self
            .find_group(region, group_name, group_arn)
            .map_err(|failure| failure.message().to_string())?;
        let Some(source) = group
            .filter_expression
            .as_deref()
            .map(str::trim)
            .filter(|expression| !expression.is_empty())
        else {
            return Ok(None);
        };
        filter::parse(source).map(Some).map_err(|error| {
            format!(
                "the FilterExpression of group `{}` could not be evaluated: {}",
                group.group_name, error.message
            )
        })
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
        /// The expression holds input this language does not admit, at the place
        /// the parse gave up: the line and the one-based column of that place,
        /// and the input read there.
        InvalidSymbol {
            /// The one-based line the parse gave up on.
            line: usize,
            /// The one-based column within that line.
            position: usize,
            /// The input read at that place and not admitted.
            symbol: String,
        },
        /// The expression broke a bound this emulator declares on how much
        /// parsing one request may drive ([`MAX_LENGTH`], [`MAX_DEPTH`]) —
        /// which is this emulator's limit rather than a refusal of the
        /// language, so it states the limit instead of a location in the input.
        Bound(String),
    }

    impl Error {
        /// What to tell the caller.
        ///
        /// A parse failure carries X-Ray's own template: the location the parse
        /// gave up at and the input symbol it could not read there, which is
        /// the actionable part of the refusal — it points at the characters that
        /// broke the expression. The empty case carries X-Ray's fixed wording,
        /// and a bound this emulator adds states that bound.
        #[must_use]
        pub fn message(&self) -> String {
            match self {
                Self::Empty => "Filter Expression is empty".to_string(),
                Self::InvalidSymbol {
                    line,
                    position,
                    symbol,
                } => format!(
                    "Error Location: line {line}, position {position} : Invalid input symbol: '{symbol}'"
                ),
                Self::Bound(detail) => format!("Invalid filter expression: {detail}"),
            }
        }
    }

    /// The refusal for input at character offset `at` of `expression` that this
    /// language does not admit: where that offset falls, and the `symbol` read
    /// there.
    fn invalid_symbol(expression: &[char], at: usize, symbol: String) -> Error {
        let before = expression.get(..at.min(expression.len())).unwrap_or_default();
        Error::InvalidSymbol {
            line: 1 + before.iter().filter(|character| **character == '\n').count(),
            position: before
                .iter()
                .rev()
                .take_while(|character| **character != '\n')
                .count()
                + 1,
            symbol,
        }
    }

    /// The punctuation a token may begin with, beside the two string quotes —
    /// the lexemes [`could_begin_a_token`] judges a run of input against.
    const PUNCTUATION: &[&str] = &[
        "\"", "'", "(", ")", "{", "}", ",", ":", "=", "!=", "<", "<=", ">", ">=",
    ];

    /// The words that are not keyword paths: the boolean operators, the boolean
    /// literals, and the word comparisons [`WORD_COMPARISONS`] names.
    const WORD_TOKENS: &[&str] = &[
        "and", "or", "not", "true", "false", "contains", "beginswith", "startswith", "endswith",
    ];

    /// Whether `text` could still grow into a token of this language: some
    /// keyword, word token or punctuation mark begins with it, or it is a
    /// keyword root followed by a caller-chosen suffix (`annotation.<key>`).
    ///
    /// This is what decides how much input a refusal reports: a tokenizer reads
    /// as far as a token could still reach and names what it had read when the
    /// next character could no longer belong to one.
    fn could_begin_a_token(text: &str) -> bool {
        if text.is_empty() {
            return true;
        }
        let lowered = text.to_ascii_lowercase();
        KEYWORDS.iter().any(|keyword| keyword.starts_with(&lowered))
            || WORD_TOKENS.iter().any(|word| word.starts_with(&lowered))
            || KEYWORD_PREFIXES
                .iter()
                .any(|prefix| prefix.starts_with(&lowered) || lowered.starts_with(prefix))
            || PUNCTUATION.iter().any(|mark| mark.starts_with(text))
    }

    /// The input a tokenizer reports when it cannot read a token at `rest`: the
    /// longest run there that could still have become one
    /// ([`could_begin_a_token`]), plus the character that could not belong to
    /// it.
    fn unreadable_symbol(rest: &[char]) -> String {
        let mut read = 0;
        for take in 1..=rest.len() {
            let candidate: String = rest[..take].iter().collect();
            if !could_begin_a_token(&candidate) {
                break;
            }
            read = take;
        }
        rest[..(read + 1).min(rest.len())].iter().collect()
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

    /// One lexical item and the span of the expression it occupies, in
    /// characters — which is what lets a refusal name the place the parse gave
    /// up and the input it had read there.
    #[derive(Debug, Clone, PartialEq)]
    struct Spanned {
        /// The item.
        token: Token,
        /// The character offset the item starts at.
        start: usize,
        /// The character offset just past the item.
        end: usize,
    }

    /// Split `expression` into tokens, refusing anything that is not one at the
    /// place it was read.
    fn tokenize(characters: &[char]) -> Result<Vec<Spanned>, Error> {
        let mut tokens: Vec<Spanned> = Vec::new();
        let mut at = 0;
        // Push one token, remembering the span it occupies. Called once the
        // walk has stepped past the item, so `at` is its end.
        macro_rules! push {
            ($start:expr, $token:expr) => {
                tokens.push(Spanned {
                    token: $token,
                    start: $start,
                    end: at,
                })
            };
        }
        while at < characters.len() {
            let character = characters[at];
            let start = at;
            match character {
                whitespace if whitespace.is_whitespace() => at += 1,
                '"' | '\'' => {
                    at += 1;
                    let mut text = String::new();
                    loop {
                        match characters.get(at) {
                            // A string the expression never closes runs to the
                            // end of the input, so that whole run is the input
                            // read and not admitted.
                            None => {
                                return Err(invalid_symbol(
                                    characters,
                                    start,
                                    characters[start..].iter().collect(),
                                ))
                            }
                            Some('\\') => {
                                let Some(escaped) = characters.get(at + 1) else {
                                    return Err(invalid_symbol(
                                        characters,
                                        start,
                                        characters[start..].iter().collect(),
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
                    push!(start, Token::Text(text));
                }
                '(' => {
                    at += 1;
                    push!(start, Token::OpenParen);
                }
                ')' => {
                    at += 1;
                    push!(start, Token::CloseParen);
                }
                '{' => {
                    at += 1;
                    push!(start, Token::OpenBrace);
                }
                '}' => {
                    at += 1;
                    push!(start, Token::CloseBrace);
                }
                ',' => {
                    at += 1;
                    push!(start, Token::Comma);
                }
                ':' => {
                    at += 1;
                    push!(start, Token::Colon);
                }
                '=' => {
                    at += 1;
                    push!(start, Token::Comparison(Operator::Equal));
                }
                '<' | '>' => {
                    let inclusive = characters.get(at + 1) == Some(&'=');
                    at += usize::from(inclusive) + 1;
                    push!(
                        start,
                        Token::Comparison(match (character, inclusive) {
                            ('<', false) => Operator::Less,
                            ('<', true) => Operator::LessOrEqual,
                            (_, false) => Operator::Greater,
                            (_, true) => Operator::GreaterOrEqual,
                        })
                    );
                }
                '!' => {
                    if characters.get(at + 1) != Some(&'=') {
                        return Err(invalid_symbol(
                            characters,
                            start,
                            unreadable_symbol(&characters[start..]),
                        ));
                    }
                    at += 2;
                    push!(start, Token::Comparison(Operator::NotEqual));
                }
                digit
                    if digit.is_ascii_digit()
                        || (digit == '-'
                            && characters.get(at + 1).is_some_and(char::is_ascii_digit)) =>
                {
                    at += 1;
                    while characters
                        .get(at)
                        .is_some_and(|next| next.is_ascii_digit() || *next == '.')
                    {
                        at += 1;
                    }
                    let literal: String = characters[start..at].iter().collect();
                    let Ok(number) = literal.parse::<f64>() else {
                        return Err(invalid_symbol(characters, start, literal));
                    };
                    push!(start, Token::Number(number));
                }
                word if word.is_alphabetic() || word == '_' => {
                    while characters
                        .get(at)
                        .is_some_and(|next| next.is_alphanumeric() || "_.-".contains(*next))
                    {
                        at += 1;
                    }
                    let word: String = characters[start..at].iter().collect();
                    let token = if word.eq_ignore_ascii_case("and") {
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
                    };
                    push!(start, token);
                }
                _ => {
                    return Err(invalid_symbol(
                        characters,
                        start,
                        unreadable_symbol(&characters[start..]),
                    ));
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
    struct Parser<'a> {
        /// The expression's characters, so a refusal can name where in the
        /// input the walk gave up and what it was reading there.
        source: &'a [char],
        tokens: Vec<Spanned>,
        at: usize,
    }

    impl Parser<'_> {
        fn peek(&self) -> Option<&Token> {
            self.tokens.get(self.at).map(|spanned| &spanned.token)
        }

        /// Where the item the walk is looking at begins — the end of the input
        /// when there is none left.
        fn peek_start(&self) -> usize {
            self.tokens
                .get(self.at)
                .map_or(self.source.len(), |spanned| spanned.start)
        }

        fn eat(&mut self, token: &Token) -> bool {
            if self.peek() == Some(token) {
                self.at += 1;
                return true;
            }
            false
        }

        /// The refusal for the item the walk cannot read: located where that
        /// item begins, and naming the item itself — the input symbol that
        /// could not stand there. When the expression ran OUT before the grammar
        /// did there is no symbol to name, so the refusal is located at the end
        /// of the input and names the whole expression.
        fn invalid(&self) -> Error {
            match self.tokens.get(self.at) {
                Some(spanned) => invalid_symbol(
                    self.source,
                    spanned.start,
                    self.source
                        .get(spanned.start..spanned.end)
                        .unwrap_or_default()
                        .iter()
                        .collect(),
                ),
                None => invalid_symbol(
                    self.source,
                    self.source.len(),
                    self.source.iter().collect(),
                ),
            }
        }

        /// The refusal for input at `start` that is not a token of this
        /// language at all, as the tokenizer states one: the run it could read
        /// there and the character that broke it.
        fn unreadable_at(&self, start: usize) -> Error {
            invalid_symbol(
                self.source,
                start,
                unreadable_symbol(self.source.get(start..).unwrap_or_default()),
            )
        }

        fn expect(&mut self, token: &Token) -> Result<(), Error> {
            if self.eat(token) {
                return Ok(());
            }
            Err(self.invalid())
        }

        /// ```text
        /// selector := "service" "(" id ")" | "edge" "(" id "," id ")" | id
        /// ```
        fn entity_selector(&mut self) -> Result<EntitySelector, Error> {
            let start = self.peek_start();
            let Some(Token::Field(function)) = self.peek().cloned() else {
                return Err(self.invalid());
            };
            match function.as_str() {
                "id" => Ok(EntitySelector::Service(self.entity_id()?)),
                "service" => {
                    self.at += 1;
                    self.expect(&Token::OpenParen)?;
                    let entity = self.entity_id()?;
                    self.expect(&Token::CloseParen)?;
                    Ok(EntitySelector::Service(entity))
                }
                "edge" => {
                    self.at += 1;
                    self.expect(&Token::OpenParen)?;
                    let source = self.entity_id()?;
                    self.expect(&Token::Comma)?;
                    let destination = self.entity_id()?;
                    self.expect(&Token::CloseParen)?;
                    Ok(EntitySelector::Edge {
                        source,
                        destination,
                    })
                }
                _ => Err(self.unreadable_at(start)),
            }
        }

        /// ```text
        /// id       := "id" "(" argument ("," argument)* ")"
        /// argument := label ":" text
        /// ```
        fn entity_id(&mut self) -> Result<EntityId, Error> {
            let start = self.peek_start();
            match self.peek() {
                Some(Token::Field(function)) if function == "id" => self.at += 1,
                Some(_) => return Err(self.unreadable_at(start)),
                None => return Err(self.invalid()),
            }
            self.expect(&Token::OpenParen)?;
            let mut entity = EntityId::default();
            loop {
                let label_at = self.peek_start();
                let Some(Token::Field(label)) = self.peek().cloned() else {
                    return Err(self.invalid());
                };
                self.at += 1;
                self.expect(&Token::Colon)?;
                let value_at = self.peek_start();
                let Operand::Text(value) = self.operand()? else {
                    return Err(self.unreadable_at(value_at));
                };
                match label.as_str() {
                    "name" => entity.name = Some(value),
                    "type" => entity.r#type = Some(value),
                    "account.id" => entity.account_id = Some(value),
                    _ => return Err(self.unreadable_at(label_at)),
                }
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect(&Token::CloseParen)?;
            Ok(entity)
        }

        fn expression(&mut self, depth: usize) -> Result<Node, Error> {
            // The terms and the operators between them, folded below so `AND`
            // binds tighter than `OR` while the WALK stays exactly what it was:
            // which expressions parse does not depend on the fold.
            let mut terms = vec![self.term(depth)?];
            let mut operators: Vec<Token> = Vec::new();
            while let Some(operator @ (Token::And | Token::Or)) = self.peek().cloned() {
                self.at += 1;
                operators.push(operator);
                terms.push(self.term(depth)?);
            }
            Ok(fold(terms, &operators))
        }

        fn term(&mut self, depth: usize) -> Result<Node, Error> {
            if depth > MAX_DEPTH {
                return Err(Error::Bound(format!(
                    "nested more than {MAX_DEPTH} levels deep"
                )));
            }
            let start = self.peek_start();
            match self.peek().cloned() {
                Some(Token::Not) => {
                    self.at += 1;
                    Ok(Node::Not(Box::new(self.term(depth + 1)?)))
                }
                Some(Token::OpenParen) => {
                    self.at += 1;
                    let inner = self.expression(depth + 1)?;
                    self.expect(&Token::CloseParen)?;
                    Ok(inner)
                }
                Some(Token::Field(path)) => {
                    self.at += 1;
                    self.predicate(path, start, depth)
                }
                _ => Err(self.invalid()),
            }
        }

        fn predicate(&mut self, path: String, start: usize, depth: usize) -> Result<Node, Error> {
            // A word that names no field of the language is not a keyword this
            // grammar has a token for at all, so it is reported where it was
            // read rather than as a field that merely is not indexed.
            if !is_indexed_field(&path) {
                return Err(self.unreadable_at(start));
            }
            let arguments = if self.eat(&Token::OpenParen) {
                self.arguments()?
            } else {
                Vec::new()
            };
            if self.eat(&Token::OpenBrace) {
                let block = self.expression(depth + 1)?;
                self.expect(&Token::CloseBrace)?;
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
                if let (Some(Token::Field(name)), Some(Token::Colon)) = (
                    self.peek(),
                    self.tokens.get(self.at + 1).map(|spanned| &spanned.token),
                ) {
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
            self.expect(&Token::CloseParen)?;
            Ok(arguments)
        }

        fn operand(&mut self) -> Result<Operand, Error> {
            let start = self.peek_start();
            let operand = match self.peek().cloned() {
                Some(Token::Text(text)) => Operand::Text(text),
                Some(Token::Number(number)) => Operand::Number(number),
                Some(Token::Boolean(value)) => Operand::Boolean(value),
                Some(Token::Field(path)) => {
                    if !is_indexed_field(&path) {
                        return Err(self.unreadable_at(start));
                    }
                    Operand::Field(path)
                }
                _ => return Err(self.invalid()),
            };
            self.at += 1;
            Ok(operand)
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
            return Err(Error::Bound(format!(
                "longer than the {MAX_LENGTH} characters this emulator parses"
            )));
        }
        let characters: Vec<char> = expression.chars().collect();
        let mut parser = Parser {
            source: &characters,
            tokens: tokenize(&characters)?,
            at: 0,
        };
        let node = parser.expression(0)?;
        if parser.peek().is_some() {
            return Err(parser.invalid());
        }
        Ok(node)
    }

    /// Whether `expression` is one X-Ray would accept, for a surface that only
    /// STORES it: [`parse`] with the tree thrown away.
    pub fn validate(expression: &str) -> Result<(), Error> {
        parse(expression).map(drop)
    }

    /// Which entities of the recorded traffic an aggregation covers.
    ///
    /// `GetTimeSeriesServiceStatistics$EntitySelectorExpression` is "a filter
    /// expression defining entities that will be aggregated for statistics.
    /// Supports ID, service, and edge functions" — a production of its own over
    /// the same lexis as the boolean expression above, so it is read here, by the
    /// same tokenizer, and refused in the same words.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum EntitySelector {
        /// `id(…)` on its own, or wrapped in `service(…)`: the segments of the
        /// service it names.
        Service(EntityId),
        /// `edge(id(…), id(…))`: the calls that reached the second entity from
        /// the first.
        Edge {
            /// The entity the calls came from.
            source: EntityId,
            /// The entity they reached, whose segments the aggregate counts.
            destination: EntityId,
        },
    }

    /// What one `id(…)` function names: each member it labels is one the entity
    /// has to match, and a member it omits narrows nothing.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct EntityId {
        /// `id(name: "…")`: the service's name.
        pub name: Option<String>,
        /// `id(type: "…")`: the service's type.
        pub r#type: Option<String>,
        /// `id(account.id: "…")`: the account the service runs in.
        pub account_id: Option<String>,
    }

    /// Parse an `EntitySelectorExpression` into the entities it names, answering
    /// `Ok` only for a selector X-Ray would accept.
    pub fn parse_entity_selector(expression: &str) -> Result<EntitySelector, Error> {
        if expression.trim().is_empty() {
            return Err(Error::Empty);
        }
        if expression.chars().count() > MAX_LENGTH {
            return Err(Error::Bound(format!(
                "longer than the {MAX_LENGTH} characters this emulator parses"
            )));
        }
        let characters: Vec<char> = expression.chars().collect();
        let mut parser = Parser {
            source: &characters,
            tokens: tokenize(&characters)?,
            at: 0,
        };
        let selector = parser.entity_selector()?;
        if parser.peek().is_some() {
            return Err(parser.invalid());
        }
        Ok(selector)
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

/// What X-Ray tells a caller whose `EndTime` is before its `StartTime`.
///
/// A FIXED service text, not a sentence computed from the request: the refusal
/// names neither bound, so the same words answer the same mistake on every
/// operation that takes a window, and nothing in it varies from run to run.
pub const INVERTED_WINDOW_MESSAGE: &str = "End time cannot be earlier than start time";

/// The preamble a refused request PARAMETER is stated under by the operations
/// that word it that way (`StartTraceRetrieval`), ahead of the sentence naming
/// what was wrong with it.
pub const INVALID_INPUT_PARAMETERS_PREAMBLE: &str = "Input parameters invalid for request.";

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
    /// What to tell the caller: the service's own fixed text for an inverted
    /// window, and — for the bound this emulator adds — the members at fault
    /// and the values that broke the rule.
    pub fn message(self, start: f64, end: f64) -> String {
        match self {
            Self::Inverted => INVERTED_WINDOW_MESSAGE.to_string(),
            Self::Future => format!(
                "StartTime ({start}) and EndTime ({end}) must be within {MAX_WINDOW_FUTURE_SKEW_SECONDS} seconds of the present, in seconds of epoch time"
            ),
        }
    }
}

/// Refuse a `[StartTime, EndTime]` window that runs BACKWARDS: an `EndTime`
/// before its `StartTime` names no interval at all, whatever instants the two
/// bounds are.
///
/// Split out of [`validate_window`] so a reader that must serve any instant its
/// store holds — one whose stored timestamps may lie arbitrarily far out — still
/// refuses an inverted window with the one judgement and the one wording every
/// other window-taking operation uses.
pub fn validate_window_order(start: f64, end: f64) -> Result<(), WindowError> {
    if end < start {
        return Err(WindowError::Inverted);
    }
    Ok(())
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
    validate_window_order(start, end)?;
    if start > now + MAX_WINDOW_FUTURE_SKEW_SECONDS || end > now + MAX_WINDOW_FUTURE_SKEW_SECONDS {
        return Err(WindowError::Future);
    }
    Ok(())
}

/// When X-Ray became generally available, in seconds of epoch time
/// (2017-04-19T00:00:00Z).
///
/// The FLOOR a retrieval's window is judged against: no trace can have been
/// recorded before the service that records them existed, so a window ending
/// below it names no retrievable traffic at all.
pub const XRAY_GENERAL_AVAILABILITY_EPOCH_SECONDS: f64 = 1_492_560_000.0;

/// What X-Ray tells a caller who asks to retrieve a window that lies outside
/// the interval a retrieval can cover — below the floor the service's own
/// availability sets, or past a time any recorded trace could carry, which is
/// what a millisecond-scale epoch value read as seconds is.
///
/// A FIXED service text, its two sentences run together exactly as the service
/// states them: it names neither bound and no limit, so nothing in it varies
/// from run to run.
pub const RETRIEVAL_WINDOW_OUT_OF_RANGE_MESSAGE: &str = "Invalid input parameters for starting trace retrieval.End time should not be before the service was generally available.";

/// Why a retrieval's `[StartTime, EndTime]` window names no interval a
/// retrieval could cover.
///
/// Separate from [`WindowError`] because the refusals a window-READER states
/// are not the ones the retrieval STARTER states: each class here carries the
/// service's own fixed sentence for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalWindowError {
    /// `EndTime` is before `StartTime`, so the window contains no instant.
    Inverted,
    /// A bound lies outside the interval a retrieval can cover: below
    /// [`XRAY_GENERAL_AVAILABILITY_EPOCH_SECONDS`], or further than
    /// [`MAX_WINDOW_FUTURE_SKEW_SECONDS`] past now.
    OutOfRange,
}

impl RetrievalWindowError {
    /// What to tell the caller: the service's fixed text for this class,
    /// interpolating nothing from the request.
    pub fn message(self) -> String {
        match self {
            Self::Inverted => format!("{INVALID_INPUT_PARAMETERS_PREAMBLE} {INVERTED_WINDOW_MESSAGE}"),
            Self::OutOfRange => RETRIEVAL_WINDOW_OUT_OF_RANGE_MESSAGE.to_string(),
        }
    }
}

/// Refuse a window a retrieval could not cover: one running backwards, and one
/// whose bounds are not instants retrievable traffic could carry — below the
/// floor the service's own general availability sets, or past the present by
/// more than any clock skew.
pub fn validate_retrieval_window(
    start: f64,
    end: f64,
    now: f64,
) -> Result<(), RetrievalWindowError> {
    if !start.is_finite() || !end.is_finite() {
        return Err(RetrievalWindowError::OutOfRange);
    }
    if validate_window_order(start, end).is_err() {
        return Err(RetrievalWindowError::Inverted);
    }
    if end < XRAY_GENERAL_AVAILABILITY_EPOCH_SECONDS
        || start > now + MAX_WINDOW_FUTURE_SKEW_SECONDS
        || end > now + MAX_WINDOW_FUTURE_SKEW_SECONDS
    {
        return Err(RetrievalWindowError::OutOfRange);
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

    /// Stop the retrieval a token names, so every later read of that token
    /// reports `RETRIEVAL_STATUS_CANCELLED`.
    ///
    /// IDEMPOTENT in the statuses a retrieval can no longer leave: a retrieval
    /// that already reached a terminal status is left exactly as it stands —
    /// cancelling asks for a state it is already in or past, so there is nothing
    /// to do and nothing to refuse. `Some` is the stored retrieval as it stands
    /// after the call; `None` is a token that names no retrieval at all, which is
    /// the one thing a caller is refused for.
    pub fn cancel_trace_retrieval(
        &mut self,
        retrieval_token: &str,
        now: f64,
    ) -> Option<&TraceRetrievalRecord> {
        self.expire_trace_retrievals(now);
        let retrieval = self.retrievals.get_mut(retrieval_token)?;
        if retrieval.is_ongoing(now) {
            retrieval.cancelled_at = Some(now);
        }
        Some(retrieval)
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

    /// Mint an opaque `RetrievalToken`: [`RETRIEVAL_TOKEN_HEX_DIGITS`]
    /// hexadecimal digits, of which the first half identifies the retrieval and
    /// the second half is a CHECK over that first half
    /// ([`retrieval_token_check`]), so a token that was damaged in transit can be
    /// told from one this service never issued.
    ///
    /// The identifying half mixes the never-reset
    /// [`retrieval_seq`](Self::retrieval_seq) with what the retrieval asked for,
    /// so no two retrievals share a token even when they ask for the same traces
    /// and no clock enters the value; a mixing that happened to land on a token
    /// already held is re-mixed rather than returned, so the token a caller gets
    /// names its own retrieval and no other.
    fn mint_retrieval_token(&self, trace_ids: &[String], start_time: f64, end_time: f64) -> String {
        let mut token = String::new();
        for attempt in 0..=MAX_TRACE_RETRIEVALS {
            let identity = ::service_common::stable_hex_id(&[
                &self.retrieval_seq.to_string(),
                &attempt.to_string(),
                &trace_ids.join(","),
                &start_time.to_bits().to_string(),
                &end_time.to_bits().to_string(),
            ]);
            token = format!("{identity}{}", retrieval_token_check(&identity));
            if !self.retrievals.contains_key(&token) {
                break;
            }
        }
        token
    }
}

/// The CHECK half of a `RetrievalToken` whose identifying half is `identity`:
/// the same deterministic mixing every derived id of this crate uses, so the
/// check can be recomputed from the token alone
/// ([`retrieval_token_defect`]).
///
/// This makes the token integrity-checked, not unguessable: it tells a damaged
/// token from an intact one, and nothing authenticates on it.
pub fn retrieval_token_check(identity: &str) -> String {
    ::service_common::stable_hex_id(&["retrieval-token", identity])
}

/// How many hexadecimal digits a `RetrievalToken` this service mints carries
/// ([`ComAmazonawsXrayAwsxrayState::mint_retrieval_token`]).
pub const RETRIEVAL_TOKEN_HEX_DIGITS: usize = 32;

/// Why a submitted `RetrievalToken` is not a value this service ISSUED.
///
/// Both classes are a malformed parameter — `InvalidRequestException` at HTTP
/// 400, decided BEFORE any lookup — and they differ only in what the
/// execution-id decoder the value was handed can say about it
/// ([`malformed_retrieval_token_message`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalTokenDefect {
    /// The value is not of the form a `RetrievalToken` has at all: a width other
    /// than [`RETRIEVAL_TOKEN_HEX_DIGITS`], or a digit outside the hexadecimal
    /// alphabet. The decoder was handed something that is not an execution id,
    /// so it can say only that the execution id was invalid.
    NotTokenForm,
    /// The value has the minted width and the minted alphabet, but its check
    /// half does not derive from its identifying half
    /// ([`retrieval_token_check`]) — a token damaged after it was issued, by one
    /// character or by many, the case of a digit included. The decoder read an
    /// execution id of the right shape and could not decode it, so it names the
    /// value it could not decode.
    UndecodableExecutionId,
}

/// How `token` misses the `RetrievalToken` values this service mints, or `None`
/// when it is one this service could have ISSUED: the right number of
/// hexadecimal digits, and a check half that matches its identifying half
/// ([`retrieval_token_check`]).
///
/// The only retrieval tokens that exist are the ones `StartTraceRetrieval`
/// minted, and every one of them carries that check, so a value whose check does
/// not match could never have named a retrieval however much it looks like one:
/// it is a malformed parameter rather than a retrieval that is gone — which is
/// what a token damaged in transit, by one character or by many, is. That
/// distinction is what lets the retrieval readers answer such a token with
/// `InvalidRequestException` (HTTP 400) BEFORE any existence lookup, and reserve
/// `ResourceNotFoundException` (HTTP 404) for an intact token whose retrieval was
/// never started, has been evicted, or has timed out.
///
/// The check is recomputed over the EXACT characters the request submitted, and
/// nothing about them is folded: a retrieval token is an opaque string whose
/// characters are each significant, so a value that differs from an issued one in
/// the CASE of a digit is a value this service never issued — damaged in transit
/// exactly like one whose digit changed to another value — and it is refused as a
/// malformed parameter rather than resolved. Folding case here would also let a
/// value pass this gate and then miss the store that resolves it, which is keyed
/// by the exact string the mint emitted, so the caller would be told its
/// retrieval is gone when what is wrong is the value it submitted.
pub fn retrieval_token_defect(token: &str) -> Option<RetrievalTokenDefect> {
    if token.len() != RETRIEVAL_TOKEN_HEX_DIGITS
        || !token.chars().all(|digit| digit.is_ascii_hexdigit())
    {
        return Some(RetrievalTokenDefect::NotTokenForm);
    }
    let (identity, check) = token.split_at(RETRIEVAL_TOKEN_HEX_DIGITS / 2);
    if retrieval_token_check(identity) == check {
        None
    } else {
        Some(RetrievalTokenDefect::UndecodableExecutionId)
    }
}

/// What a `RetrievalToken` carrying `defect` ([`retrieval_token_defect`]) is
/// refused with, before any lookup: the name the operation gives the member, the
/// value the request submitted, and what the execution-id decoder made of it.
///
/// X-Ray words this refusal as TWO clauses: an outer one naming the parameter and
/// the value the request submitted — "Invalid <member> <value> passed: " — and
/// then the clause of the execution-id decoder the value was handed to. The
/// request's value is the sentence's ONLY variable; how often it appears is what
/// the decoder's clause decides:
///
/// * a value that is not of the token form at all got no further than the
///   decoder's own entry check, which states that the execution id was invalid
///   and names nothing ([`MALFORMED_RETRIEVAL_TOKEN_CAUSE`]) — so the value
///   appears once, in the outer clause; and
/// * a value of the token form whose check does not verify was DECODED and
///   rejected, and the decoder names the value it could not decode
///   ([`UNDECODABLE_EXECUTION_ID_CAUSE`]) — so the value appears twice, once per
///   clause.
///
/// The sentence is built here once and each reader passes the member name it uses
/// (`retrieval token`, `execution id`), so the two readers of one class never
/// disagree about its wording.
pub fn malformed_retrieval_token_message(
    member: &str,
    token: &str,
    defect: RetrievalTokenDefect,
) -> String {
    match defect {
        RetrievalTokenDefect::NotTokenForm => {
            format!("Invalid {member} {token} passed: {MALFORMED_RETRIEVAL_TOKEN_CAUSE}")
        }
        RetrievalTokenDefect::UndecodableExecutionId => {
            format!(
                "Invalid {member} {token} passed: {UNDECODABLE_EXECUTION_ID_CAUSE}{token}"
            )
        }
    }
}

/// The decoder clause a value that is not of the token form at all is refused
/// with ([`malformed_retrieval_token_message`]): fixed text, capitalised exactly
/// this way, naming no value, whatever the operation and whatever the value.
pub const MALFORMED_RETRIEVAL_TOKEN_CAUSE: &str = "Invalid Execution Id";

/// The decoder clause a value of the token form whose check does not verify is
/// refused with ([`malformed_retrieval_token_message`]): fixed text, sentence
/// cased, followed by the value the decoder could not decode.
pub const UNDECODABLE_EXECUTION_ID_CAUSE: &str = "Invalid execution id provided: ";

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
/// (`smithy_query(query="shape",shape="SamplingRule")`) — so what is judged is
/// ARN SYNTAX, and the one syntax rule a value can break here is the prefix
/// every ARN starts with. [`ANY_RESOURCE_ARN`] matches any resource, and a value
/// that names no resource at all (the empty string) constrains nothing, so
/// neither is an ARN that has to parse. Written once, because a rule may not
/// reach a value through `UpdateSamplingRule` that `CreateSamplingRule` refuses.
pub fn validate_resource_arn(resource_arn: &str) -> Result<(), SamplingRuleRejection> {
    if resource_arn.is_empty() || resource_arn == ANY_RESOURCE_ARN {
        return Ok(());
    }
    if !resource_arn.starts_with(ARN_PREFIX) {
        return Err(SamplingRuleRejection::Invalid(format!(
            "Invalid resource ARN: {resource_arn}. Malformed ARN - doesn't start with '{ARN_PREFIX}'"
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

/// What every Amazon Resource Name begins with — the one piece of ARN syntax a
/// submitted `ResourceARN` is judged against ([`validate_resource_arn`]).
pub const ARN_PREFIX: &str = "arn:";

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
        // The account's sampling-rule configuration just changed, which is what
        // `GetSamplingTargets` reports as `LastRuleModification` so a service
        // knows to re-read the rules.
        self.sampling.rules_modified_at = now;
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
        // The write is scoped, so the record can be answered by a fresh lookup
        // once the account's configuration instant has been stamped below.
        {
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
        }
        // The configuration a service reads changed, so every later
        // `GetSamplingTargets` tells it to re-read the rules.
        self.sampling.rules_modified_at = now;
        self.sampling_rules.get(rule_name)
    }

    /// Remove one sampling rule, answering the record that was removed.
    ///
    /// "Deletes a sampling rule" is the whole of the operation: the rule is gone
    /// from the store, so no later listing carries it and no later selector
    /// resolves it. The service-managed [`DEFAULT_SAMPLING_RULE_NAME`] rule is
    /// not removable — `DeleteSamplingRule` refuses it before reaching here —
    /// and removing its materialised record would only put the derived rule back
    /// in the listing.
    /// `now` is the instant the removal happened, supplied by the caller the
    /// same way the create and the update supply theirs, so every write to a
    /// rule stamps the account's configuration instant from ONE reading of the
    /// clock per request.
    pub fn remove_sampling_rule(
        &mut self,
        rule_name: &str,
        now: f64,
    ) -> Option<StoredSamplingRule> {
        let removed = self.sampling_rules.remove(rule_name)?;
        // A rule leaving the account is a change to the configuration a service
        // reads, exactly as creating or updating one is.
        self.sampling.rules_modified_at = now;
        Some(removed)
    }

    /// The rule `rule_name` names, as a READER in `region` sees it: the stored
    /// record, or the account's service-managed catch-all rule, which is derived
    /// rather than stored ([`default_sampling_rule`]).
    ///
    /// `None` means the account holds no rule under that name, which is what
    /// `GetSamplingTargets` reports as unprocessed statistics rather than
    /// serving an allocation for a rule nobody configured.
    pub fn visible_sampling_rule(
        &self,
        region: &str,
        rule_name: &str,
    ) -> Option<StoredSamplingRule> {
        if rule_name == DEFAULT_SAMPLING_RULE_NAME {
            return Some(self.default_sampling_rule_record(region));
        }
        self.sampling_rules.get(rule_name).cloned()
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
// Sampling statistics, reservoir shares and boosts
// ---------------------------------------------------------------------------

/// How long one sampling reporting window is, in seconds.
///
/// The model documents the span twice: a `SamplingStatisticsDocument` carries
/// results "for the last 10 seconds unless the service has been assigned a
/// longer reporting interval after a previous call to GetSamplingTargets", and a
/// `SamplingStatisticSummary` is "aggregated request sampling data for a
/// sampling rule across all services for a 10-second window"
/// (`smithy_query(query="shape",shape="SamplingStatisticsDocument")`). Reported
/// results are therefore accumulated per rule into windows of this length, and
/// a summary reports one of them.
pub const SAMPLING_WINDOW_SECONDS: i64 = 10;

/// `SamplingTargetDocument$Interval`: "the number of seconds for the service to
/// wait before getting sampling targets again".
///
/// Served only on a document that GRANTS a share of a rule's reservoir, which
/// is what the member is for: the service is told to come back for its next
/// share when the one it was just granted expires
/// ([`SAMPLING_ALLOCATION_TTL_SECONDS`]), one reporting window
/// ([`SAMPLING_WINDOW_SECONDS`]) later. A document that grants no share assigns
/// no cadence either, so it carries no interval at all and the reporting
/// service keeps the one it came with — the case a `SamplingStatisticsDocument`
/// describes as its default, "the last 10 seconds unless the service has been
/// assigned a longer reporting interval after a previous call".
pub const SAMPLING_TARGET_INTERVAL_SECONDS: i32 = 10;

/// How long an allocated reservoir quota and a granted sampling boost stay
/// live, in seconds.
///
/// CHOSEN, not documented: the allocation is granted for the interval the same
/// document assigns the service ([`SAMPLING_TARGET_INTERVAL_SECONDS`]), which is
/// the reporting window it reports over ([`SAMPLING_WINDOW_SECONDS`]), so a
/// service that comes back when it was told to is answered the share the cycle
/// it just took part in computed, and one that goes quiet falls back to the
/// rule's `FixedRate` instead of holding a share of the pool forever.
pub const SAMPLING_ALLOCATION_TTL_SECONDS: f64 = 10.0;

/// How long a computed allocation stays live after the cycle that computed it,
/// in seconds.
///
/// CHOSEN: an allocation outlives the cycle that produced it, because it is
/// what a service is told at its NEXT report — including when the rule itself
/// has left the account in between, which does not cancel the share the service
/// was already granted. Once it has aged past this, a service reporting for a
/// rule the account no longer holds is answered as unprocessed.
pub const SAMPLING_ALLOCATION_CACHE_SECONDS: f64 = 60.0;

/// How long a service's report keeps it in the pool sharing a rule's reservoir,
/// in seconds.
///
/// CHOSEN: several missed reporting intervals, after which a service is no
/// longer counted when the reservoir is shared out, so a service that stops
/// calling stops diluting the share of the ones still reporting.
pub const SAMPLING_CLIENT_REPORT_TTL_SECONDS: f64 = 60.0;

/// The most reporting windows one rule keeps — about half an hour of them.
/// Reaching it drops the oldest window, so a service reporting forever cannot
/// grow the store without bound.
pub const MAX_SAMPLING_WINDOWS_PER_RULE: usize = 180;

/// The most services one rule's reservoir is shared between. Reaching it drops
/// the least recently reporting service.
pub const MAX_SAMPLING_CLIENTS_PER_RULE: usize = 100;

/// The most services one rule records a boost grant for. Reaching it drops the
/// least recently granted service, so a caller naming a fresh `ServiceName` on
/// every call cannot grow the store without bound.
pub const MAX_SAMPLING_BOOST_SERVICES_PER_RULE: usize = 100;

/// The most rules the statistics store tracks: the rules an account may hold
/// ([`MAX_SAMPLING_RULES`]) plus the service-managed catch-all. Reaching it
/// drops the rule whose last report is the oldest, so a create/delete cycle
/// through many names cannot grow the store without bound.
pub const MAX_SAMPLING_STATISTIC_RULES: usize = MAX_SAMPLING_RULES + 1;

/// The start of the [`SAMPLING_WINDOW_SECONDS`] window the instant `seconds`
/// falls in, in whole seconds of epoch time.
///
/// Windows are aligned to the epoch rather than to whichever instant reported
/// first, so two services reporting the same ten seconds accumulate into ONE
/// window and the summary of it is one row. Bounded exactly as
/// [`timestamp_of`] is, so no reported instant can produce a window start a
/// reader cannot answer.
pub fn sampling_window_start(seconds: f64) -> i64 {
    let bounded = if seconds.is_nan() {
        0.0
    } else {
        seconds.clamp(-MAX_SEGMENT_EPOCH_SECONDS, MAX_SEGMENT_EPOCH_SECONDS)
    };
    #[allow(clippy::cast_possible_truncation)]
    let whole = bounded.floor() as i64;
    whole - whole.rem_euclid(SAMPLING_WINDOW_SECONDS)
}

/// A summed counter as the modelled `Integer` member of a
/// `SamplingStatisticSummary` carries it.
///
/// Counters are summed as `i64` so no sequence of reports overflows a total,
/// and the wire member is a 32-bit `Integer`: a total past what it can carry is
/// reported at its maximum rather than wrapping to a count nobody reported.
fn wire_count(total: i64) -> i32 {
    i32::try_from(total).unwrap_or(i32::MAX)
}

/// One service's reported results for one rule: what a
/// `SamplingStatisticsDocument` carries, in this module's units.
#[derive(Debug, Clone, Copy)]
pub struct ReportedSamplingCounts {
    /// The document's `Timestamp` ("the current time"), in seconds of epoch
    /// time — which window the results are accumulated into.
    pub timestamp: f64,
    /// `RequestCount`: "the number of requests that matched the rule".
    pub request_count: i64,
    /// `SampledCount`: "the number of requests recorded".
    pub sampled_count: i64,
    /// `BorrowCount`: "the number of requests recorded with borrowed reservoir
    /// quota".
    pub borrow_count: i64,
}

/// A set of sampling counters for one rule and one reporting window: either
/// what ONE service states for that window, or — once every reporting service's
/// is added up — what a `SamplingStatisticSummary` is ("aggregated request
/// sampling data for a sampling rule across all services for a 10-second
/// window").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SamplingStatisticsWindow {
    /// The `RequestCount` of the window.
    pub request_count: i64,
    /// The `SampledCount` of the window.
    pub sampled_count: i64,
    /// The `BorrowCount` of the window.
    pub borrow_count: i64,
}

impl SamplingStatisticsWindow {
    /// The counters one reported `SamplingStatisticsDocument` states.
    fn of(report: ReportedSamplingCounts) -> Self {
        Self {
            request_count: report.request_count,
            sampled_count: report.sampled_count,
            borrow_count: report.borrow_count,
        }
    }

    /// Add another service's counters for the same window, saturatingly, so no
    /// number of reporting services can overflow a total.
    fn add(&mut self, other: Self) {
        self.request_count = self.request_count.saturating_add(other.request_count);
        self.sampled_count = self.sampled_count.saturating_add(other.sampled_count);
        self.borrow_count = self.borrow_count.saturating_add(other.borrow_count);
    }
}

/// One row of the account's reported sampling results: which rule and which
/// window, and the counters every service reporting that window stated.
///
/// `GetSamplingStatisticSummaries` reports these, so the rows a reader walks
/// are recomputed from what services reported rather than echoed from any one
/// request.
#[derive(Debug, Clone)]
pub struct SamplingStatisticsEntry {
    /// The rule the results were reported for.
    pub rule_name: String,
    /// The start of the window, in whole seconds of epoch time — the summary's
    /// `Timestamp`, "the start time of the reporting window".
    pub window_start: i64,
    /// The counters of the window, added up across its reporting services.
    pub counts: SamplingStatisticsWindow,
}

impl SamplingStatisticsEntry {
    /// This row as the modelled `SamplingStatisticSummary`.
    pub fn to_summary(&self) -> SamplingStatisticSummary {
        #[allow(clippy::cast_precision_loss)]
        let window_start = self.window_start as f64;
        SamplingStatisticSummary {
            rule_name: Some(self.rule_name.clone()),
            timestamp: Some(timestamp_of(window_start)),
            request_count: wire_count(self.counts.request_count),
            borrow_count: wire_count(self.counts.borrow_count),
            sampled_count: wire_count(self.counts.sampled_count),
        }
    }

    /// The pagination token that resumes a listing AT this row.
    ///
    /// It NAMES the row — its rule and its window — so a token for a row the
    /// store no longer holds is refused rather than answered with an arbitrary
    /// page, exactly as `GetSamplingRules`' token is.
    pub fn token(&self) -> String {
        format!("{}|{}", self.rule_name, self.window_start)
    }
}

/// What one service last reported for one rule, and the allocation the service
/// HOLDS for it.
#[derive(Debug, Clone, Copy)]
pub struct SamplingClientReport {
    /// When the report was accepted, in seconds of epoch time — what
    /// [`SAMPLING_CLIENT_REPORT_TTL_SECONDS`] is measured from.
    pub reported_at: f64,
    /// The `RequestCount` it reported: this service's share of the traffic the
    /// rule matched.
    pub request_count: i64,
    /// The allocation computed for this service at the END of a reporting cycle
    /// it took part in, which is what its NEXT report is answered with. `None`
    /// until a cycle has run for it.
    pub allocation: Option<SamplingAllocation>,
}

/// The allocation one reporting cycle computed for one service and one rule.
///
/// A service is told the allocation of the cycle it already took part in, never
/// one computed out of the report it is making right now: before its first
/// report there is no cycle it was part of, so it holds no reservoir share at
/// all and samples at the rule's fixed rate until the next cycle has run.
#[derive(Debug, Clone, Copy)]
pub struct SamplingAllocation {
    /// The share of the rule's reservoir the cycle gave this service.
    pub reservoir_quota: i32,
    /// When the cycle ran, in seconds of epoch time.
    pub allocated_at: f64,
}

/// The pool one allocation cycle divided, as that cycle found the rule it is
/// for configured.
///
/// Held per rule NAME, because that is what a `SamplingTargetDocument`
/// identifies what it allocates by: the target computation answers a report out
/// of the division it last made for that name, so it keeps the rate and the
/// pool size that division was made under rather than re-reading the rule
/// store. A name no cycle has divided a pool for has none of this, which is the
/// only case a reported document is answered as unprocessed.
#[derive(Debug, Clone, Copy)]
pub struct SamplingPool {
    /// The rule's `FixedRate` as the cycle found it.
    pub fixed_rate: f64,
    /// The rule's `ReservoirSize` as the cycle found it — the size of the pool
    /// the cycle divided.
    pub reservoir_size: i32,
    /// When the cycle divided it, in seconds of epoch time.
    pub divided_at: f64,
}

/// The account's sampling results and the allocations computed from them.
///
/// `GetSamplingTargets` writes here — recording what each service reported and
/// reading back the share of each rule's reservoir that report entitles it to —
/// and `GetSamplingStatisticSummaries` reports the accumulated windows. Nothing
/// in a response is echoed from the request that stored it: a target document's
/// `FixedRate` comes from the stored rule and its `ReservoirQuota` from the
/// reports of every service using that rule.
#[derive(Debug)]
pub struct SamplingStatisticsStore {
    /// When the account's sampling-rule configuration last changed, in seconds
    /// of epoch time — `GetSamplingTargetsResult$LastRuleModification`, "the
    /// last time a user changed the sampling rule configuration", which a
    /// service compares against to decide whether to re-read the rules.
    ///
    /// Stamped by every create, update and delete of a rule. Until one happens
    /// it is when this account's configuration came into being, which is a
    /// FIXED instant rather than a reading of the clock that would move on
    /// every read.
    pub rules_modified_at: f64,
    /// Reported results per rule, per [`SAMPLING_WINDOW_SECONDS`] window keyed
    /// by the window's start, and within a window per reporting service's
    /// `ClientID` — at most [`MAX_SAMPLING_WINDOWS_PER_RULE`] windows per rule
    /// and [`MAX_SAMPLING_CLIENTS_PER_RULE`] services per window.
    ///
    /// Per SERVICE rather than one running total, because a summary aggregates
    /// "across all services" for a window: what a service states for a window
    /// is that service's whole contribution to it, so a service restating the
    /// window replaces its own figures instead of being counted twice.
    windows: BTreeMap<String, BTreeMap<i64, BTreeMap<String, SamplingStatisticsWindow>>>,
    /// The latest report each service made for a rule, keyed by rule and then
    /// by the reporting service's `ClientID`, at most
    /// [`MAX_SAMPLING_CLIENTS_PER_RULE`] of them per rule.
    clients: BTreeMap<String, BTreeMap<String, SamplingClientReport>>,
    /// The pool the latest allocation cycle divided for a rule, keyed by rule
    /// NAME ([`SamplingPool`]) — the state the target computation answers a
    /// report out of, which is why it survives the rule leaving the store.
    pools: BTreeMap<String, SamplingPool>,
    /// When a rule's sampling boost was last granted to a service, keyed by
    /// rule and then by the reporting `ServiceName` — what the configured
    /// cooldown window is measured from.
    boosts: BTreeMap<String, BTreeMap<String, f64>>,
}

impl Default for SamplingStatisticsStore {
    fn default() -> Self {
        Self {
            rules_modified_at: now_epoch_seconds(),
            windows: BTreeMap::new(),
            clients: BTreeMap::new(),
            pools: BTreeMap::new(),
            boosts: BTreeMap::new(),
        }
    }
}

impl SamplingStatisticsStore {
    /// Record one reported `SamplingStatisticsDocument`: store its counters as
    /// the reporting service's figures for the window its `Timestamp` falls in,
    /// and remember the report as this service's claim on the rule's reservoir.
    ///
    /// A document states the results of a reporting window, so a service that
    /// reports the same window again — a client polling for its reservoir
    /// share restates the window it is in — RESTATES its own figures rather
    /// than adding to them; a summary of the window adds up what each service
    /// stated for it.
    ///
    /// The share the reporting service is answered is the one it already HOLDS
    /// ([`Self::held_allocation`]), computed at the end of a cycle it took part
    /// in, so recording a report never hands out a share of its own: the first
    /// report a service makes precedes the first allocation cycle for it.
    pub fn record(
        &mut self,
        rule_name: &str,
        client_id: &str,
        report: ReportedSamplingCounts,
        now: f64,
    ) {
        let windows = self.windows.entry(rule_name.to_string()).or_default();
        let window = windows
            .entry(sampling_window_start(report.timestamp))
            .or_default();
        window.insert(
            client_id.to_string(),
            SamplingStatisticsWindow::of(report),
        );
        while window.len() > MAX_SAMPLING_CLIENTS_PER_RULE {
            // Which service's figures make room is arbitrary — the window holds
            // no order over them — so the first by `ClientID` goes, and the
            // window stays bounded.
            let Some(crowded) = window.keys().next().cloned() else {
                break;
            };
            window.remove(&crowded);
        }
        while windows.len() > MAX_SAMPLING_WINDOWS_PER_RULE {
            let Some(oldest) = windows.keys().next().copied() else {
                break;
            };
            windows.remove(&oldest);
        }

        let clients = self.clients.entry(rule_name.to_string()).or_default();
        let held = clients
            .get(client_id)
            .and_then(|report| report.allocation)
            .filter(|allocation| {
                now - allocation.allocated_at <= SAMPLING_ALLOCATION_CACHE_SECONDS
            });
        clients.insert(
            client_id.to_string(),
            SamplingClientReport {
                reported_at: now,
                request_count: report.request_count,
                allocation: held,
            },
        );
        // A service that has gone quiet for longer than a report stays live no
        // longer shares the pool.
        clients.retain(|_, report| now - report.reported_at <= SAMPLING_CLIENT_REPORT_TTL_SECONDS);
        while clients.len() > MAX_SAMPLING_CLIENTS_PER_RULE {
            let Some(stale) = clients
                .iter()
                .min_by(|left, right| left.1.reported_at.total_cmp(&right.1.reported_at))
                .map(|(client, _)| client.clone())
            else {
                break;
            };
            clients.remove(&stale);
        }
        self.clients.retain(|_, clients| !clients.is_empty());
        self.prune_rules();
    }

    /// The pool the latest allocation cycle divided for `rule_name`, while that
    /// division is still live — the state the target computation answers a
    /// report for that rule NAME out of.
    ///
    /// `None` means no cycle has divided a pool for the name, or the last one
    /// has aged past [`SAMPLING_ALLOCATION_CACHE_SECONDS`]: there is no division
    /// to tell a reporting service its share of. Whether the account still
    /// holds a rule under the name does not enter into it — the division stands
    /// on its own, exactly as the share granted out of it does.
    pub fn divided_pool(&self, rule_name: &str, now: f64) -> Option<SamplingPool> {
        self.pools
            .get(rule_name)
            .copied()
            .filter(|pool| now - pool.divided_at <= SAMPLING_ALLOCATION_CACHE_SECONDS)
    }

    /// What is left UNCLAIMED of a divided pool of `reservoir_size`: the size
    /// of the pool less every share the services still live hold out of it.
    ///
    /// This is what a service that was not in the division is told, because the
    /// pool "applies to all services using the rule collectively" and is
    /// zero-sum: a newcomer may have what the division did not give away, which
    /// is nothing once the pool is fully claimed. Never negative, and never more
    /// than the pool.
    pub fn unclaimed(&self, rule_name: &str, reservoir_size: i32, now: f64) -> i32 {
        if reservoir_size <= 0 {
            return 0;
        }
        let claimed = self.clients.get(rule_name).map_or(0_i64, |clients| {
            clients
                .values()
                .filter(|report| now - report.reported_at <= SAMPLING_CLIENT_REPORT_TTL_SECONDS)
                .filter_map(|report| report.allocation)
                .filter(|allocation| {
                    now - allocation.allocated_at <= SAMPLING_ALLOCATION_CACHE_SECONDS
                })
                .map(|allocation| i64::from(allocation.reservoir_quota))
                .fold(0_i64, i64::saturating_add)
        });
        i32::try_from(i64::from(reservoir_size).saturating_sub(claimed))
            .unwrap_or(0)
            .clamp(0, reservoir_size)
    }

    /// The allocation the service `client_id` still HOLDS for `rule_name`,
    /// without reporting anything new.
    ///
    /// This is the share a cycle already granted it, which stands until it ages
    /// past [`SAMPLING_ALLOCATION_CACHE_SECONDS`] — including when the rule has
    /// left the account in between, because deleting a rule does not reach into
    /// an allocation the service was already told about.
    pub fn held_allocation(
        &self,
        rule_name: &str,
        client_id: &str,
        now: f64,
    ) -> Option<SamplingAllocation> {
        self.clients
            .get(rule_name)?
            .get(client_id)?
            .allocation
            .filter(|allocation| {
                now - allocation.allocated_at <= SAMPLING_ALLOCATION_CACHE_SECONDS
            })
    }

    /// Run the allocation cycle for one service and one rule: compute the share
    /// of the pool every live report entitles it to and store it as the
    /// allocation that service HOLDS, which its next report is answered with.
    pub fn allocate(
        &mut self,
        rule_name: &str,
        client_id: &str,
        reservoir_size: i32,
        fixed_rate: f64,
        now: f64,
    ) {
        // The cycle divides the pool as it finds the rule configured NOW, and
        // records what it divided under the rule's name, so the target
        // computation keeps answering out of that division without re-reading
        // the rule store.
        self.pools.insert(
            rule_name.to_string(),
            SamplingPool {
                fixed_rate,
                reservoir_size,
                divided_at: now,
            },
        );
        // A division that has aged past the span a share of it stays live can
        // no longer be answered out of, so it is dropped rather than kept.
        self.pools
            .retain(|_, pool| now - pool.divided_at <= SAMPLING_ALLOCATION_CACHE_SECONDS);
        let allocation = SamplingAllocation {
            reservoir_quota: self.reservoir_quota(rule_name, client_id, reservoir_size, now),
            allocated_at: now,
        };
        if let Some(report) = self
            .clients
            .get_mut(rule_name)
            .and_then(|clients| clients.get_mut(client_id))
        {
            report.allocation = Some(allocation);
        }
    }

    /// Keep the store to [`MAX_SAMPLING_STATISTIC_RULES`] rules, dropping the
    /// rule whose most recent window is the oldest.
    fn prune_rules(&mut self) {
        while self.windows.len() > MAX_SAMPLING_STATISTIC_RULES {
            let Some(stale) = self
                .windows
                .iter()
                .min_by_key(|(_, windows)| windows.keys().next_back().copied().unwrap_or(i64::MIN))
                .map(|(rule_name, _)| rule_name.clone())
            else {
                break;
            };
            self.windows.remove(&stale);
            self.clients.remove(&stale);
            self.pools.remove(&stale);
            self.boosts.remove(&stale);
        }
    }

    /// The share of `rule_name`'s reservoir the service `client_id` is
    /// allocated, from what every service using the rule reported.
    ///
    /// A rule's `ReservoirSize` is "a fixed number of matching requests to
    /// instrument per second, prior to applying the fixed rate ... not used
    /// directly by services, but applies to all services using the rule
    /// collectively" (`smithy_query(query="shape",shape="SamplingRule")`), so
    /// the pool is SHARED: each reporting service is allocated the fraction of
    /// it that its own reported `RequestCount` is of everything reported for the
    /// rule by services still live, and services that report no requests at all
    /// split it evenly. The share is floored, so the pool is never
    /// over-allocated and a service whose share rounds to nothing instruments at
    /// the rule's fixed rate instead.
    pub fn reservoir_quota(
        &self,
        rule_name: &str,
        client_id: &str,
        reservoir_size: i32,
        now: f64,
    ) -> i32 {
        if reservoir_size <= 0 {
            return 0;
        }
        let Some(clients) = self.clients.get(rule_name) else {
            return 0;
        };
        let live = || {
            clients
                .values()
                .filter(|report| now - report.reported_at <= SAMPLING_CLIENT_REPORT_TTL_SECONDS)
        };
        let Some(mine) = clients
            .get(client_id)
            .filter(|report| now - report.reported_at <= SAMPLING_CLIENT_REPORT_TTL_SECONDS)
        else {
            return 0;
        };
        let reported: i64 = live()
            .map(|report| report.request_count)
            .fold(0_i64, i64::saturating_add);
        let sharing = live().count();
        #[allow(clippy::cast_precision_loss)]
        let share = if reported > 0 {
            mine.request_count as f64 / reported as f64
        } else if sharing > 0 {
            1.0 / sharing as f64
        } else {
            return 0;
        };
        let pool = f64::from(reservoir_size);
        #[allow(clippy::cast_possible_truncation)]
        {
            (pool * share).floor().clamp(0.0, pool) as i32
        }
    }

    /// The boost rate to grant the service `service_name` for `rule_name`, or
    /// `None` when no boost is due.
    ///
    /// A rule carries a boost configuration only when a caller asked for one:
    /// `MaxRate` "defines max temporary sampling rate to apply when a boost is
    /// triggered. Calculated boost rate by X-Ray will be less than or equal to
    /// this max rate", and `CooldownWindowMinutes` "sets the time window in
    /// which only one sampling rate boost can be triggered. After a boost
    /// occurs, no further boosts are allowed until the next window"
    /// (`smithy_query(query="shape",shape="SamplingRateBoost")`). So a boost is
    /// triggered by the ANOMALIES a service reported — at the rate it reported
    /// them, capped by the configured maximum — and only once per cooldown
    /// window. A service that reported no anomaly is not boosted at all.
    pub fn grant_boost(
        &mut self,
        rule_name: &str,
        service_name: &str,
        anomaly_rate: f64,
        configured: &SamplingRateBoost,
        now: f64,
    ) -> Option<f64> {
        if !anomaly_rate.is_finite() || anomaly_rate <= 0.0 || configured.max_rate <= 0.0 {
            return None;
        }
        let cooldown = f64::from(configured.cooldown_window_minutes) * 60.0;
        let granted = self
            .boosts
            .get(rule_name)
            .and_then(|services| services.get(service_name).copied());
        if granted.is_some_and(|at| now - at < cooldown) {
            return None;
        }
        self.boosts
            .entry(rule_name.to_string())
            .or_default()
            .insert(service_name.to_string(), now);
        self.prune_boosts(rule_name, cooldown, now);
        Some(anomaly_rate.min(configured.max_rate))
    }

    /// Keep one rule's boost grants to what they are for: a grant older than the
    /// cooldown window can no longer suppress a boost, so it carries no
    /// information and is dropped, and the rule records at most
    /// [`MAX_SAMPLING_BOOST_SERVICES_PER_RULE`] services, the least recently
    /// granted one going first.
    fn prune_boosts(&mut self, rule_name: &str, cooldown: f64, now: f64) {
        if let Some(services) = self.boosts.get_mut(rule_name) {
            // A grant that has aged past the cooldown window can no longer
            // suppress a boost, so it is dropped rather than kept; a rule with
            // no cooldown window suppresses nothing and so records nothing.
            services.retain(|_, granted| now - *granted < cooldown);
            while services.len() > MAX_SAMPLING_BOOST_SERVICES_PER_RULE {
                let Some(stale) = services
                    .iter()
                    .min_by(|left, right| left.1.total_cmp(right.1))
                    .map(|(service, _)| service.clone())
                else {
                    break;
                };
                services.remove(&stale);
            }
        }
        self.boosts.retain(|_, services| !services.is_empty());
    }

    /// Every reported window the account holds, in rule-name order and then in
    /// window order — what `GetSamplingStatisticSummaries` pages over.
    ///
    /// Each row adds up what every service reporting that window stated for it,
    /// which is what the summary aggregates "across all services".
    ///
    /// Bounded by [`MAX_SAMPLING_STATISTIC_RULES`] *
    /// [`MAX_SAMPLING_WINDOWS_PER_RULE`].
    pub fn entries(&self) -> Vec<SamplingStatisticsEntry> {
        self.windows
            .iter()
            .flat_map(|(rule_name, windows)| {
                windows.iter().map(move |(window_start, reporters)| {
                    let mut counts = SamplingStatisticsWindow::default();
                    for stated in reporters.values() {
                        counts.add(*stated);
                    }
                    SamplingStatisticsEntry {
                        rule_name: rule_name.clone(),
                        window_start: *window_start,
                        counts,
                    }
                })
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Service graphs
// ---------------------------------------------------------------------------

/// `Service$State` for a node that has at least one completed segment in the
/// graph's window: the service answered.
pub const SERVICE_STATE_ACTIVE: &str = "active";

/// `Service$State` for a node whose every segment in the graph's window is still
/// in progress: the service reported, but nothing is known yet about how it
/// answered.
pub const SERVICE_STATE_UNKNOWN: &str = "unknown";

/// `Service$State` for a node the graph knows only through another service's
/// downstream subsegment — a service that submitted no segment of its own, so
/// X-Ray knows it passively, by being called.
pub const SERVICE_STATE_PASSIVE: &str = "passive";

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
        // The call's flags are the ones its document declared
        // ([`Call::of_segment`] / [`Call::of_subsegment`] read them through
        // `is_fault`/`is_throttle`/`is_error`), and a recorded status is not one
        // of them, so nothing here consults it.
        match classify(call.fault, call.error, call.throttle) {
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
    let inferred_type = inferred_service_type(name, namespace)?;
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
                // A node the graph DISCOVERED downstream reported no segment of
                // its own: it is known passively, by having been called, which
                // is what distinguishes it from a service that reports. A node
                // whose own segments are all still in progress has reported
                // nothing to judge yet, and the `client` node is the stand-in
                // for the callers of a root service rather than a service the
                // graph discovered, so neither is passive.
                match key.inferred_type.as_deref() {
                    Some(SERVICE_TYPE_CLIENT) | None if self.complete => SERVICE_STATE_ACTIVE,
                    Some(SERVICE_TYPE_CLIENT) | None => SERVICE_STATE_UNKNOWN,
                    Some(_) => SERVICE_STATE_PASSIVE,
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
