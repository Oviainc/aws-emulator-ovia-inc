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
//! region without being created and cannot be created or deleted. Indexing
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
//! `parent_id` and `annotations` are optional.
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
//! Every instant a segment document carries is checked where the document is
//! read ([`MAX_SEGMENT_EPOCH_SECONDS`]), so no stored `start_time` / `end_time`
//! can be a value a reader cannot convert to the modelled `Timestamp`; the
//! conversion itself goes through [`timestamp_of`].

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{
    Edge, EdgeStatistics, EncryptionConfig, ErrorStatistics, FaultStatistics, Group, GroupSummary,
    HistogramEntry, IndexingRule, IndexingRuleValue, Insight, InsightsConfiguration,
    ProbabilisticRuleValue, SamplingRule, SamplingRuleRecord, Service, ServiceStatistics,
    TelemetryRecord,
};

/// The region an ARN carries when the request's SigV4 scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, from the model's `awsService.arnNamespace`
/// (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "xray";

/// `UnprocessedTraceSegment$ErrorCode` for a document that is not parseable as a
/// JSON segment object.
///
/// The model documents `ErrorCode` only as "The error that caused processing to
/// fail" and publishes no value set, so these three are this emulator's stable
/// codes; the `Message` beside them is built from the request and names the
/// field at fault.
pub const SEGMENT_ERROR_PARSE: &str = "ParseError";

/// `UnprocessedTraceSegment$ErrorCode` for a document missing a field the
/// `PutTraceSegments` documentation lists as required.
pub const SEGMENT_ERROR_MISSING_FIELD: &str = "MissingRequiredField";

/// `UnprocessedTraceSegment$ErrorCode` for a document whose field is present but
/// of the wrong JSON type.
pub const SEGMENT_ERROR_INVALID_FIELD: &str = "InvalidField";

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
/// `TraceIdList` carries no modelled length bound, and every requested id that
/// is not stored costs one `UnprocessedTraceIds` entry, so the request is
/// bounded here rather than letting it amplify into an unbounded response.
pub const MAX_TRACE_IDS_PER_CALL: usize = 500;

/// The most segment documents one `BatchGetTraces` response carries. A trace
/// that does not fit in what is left of the budget is deferred to the next page
/// rather than truncated, so a page always makes progress and a whole trace is
/// always readable ([`MAX_SEGMENTS_PER_TRACE`] segments fit the budget exactly).
pub const MAX_SEGMENTS_PER_BATCH_RESPONSE: usize = MAX_SEGMENTS_PER_TRACE;

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
    /// One of [`SEGMENT_ERROR_PARSE`], [`SEGMENT_ERROR_MISSING_FIELD`],
    /// [`SEGMENT_ERROR_INVALID_FIELD`].
    pub error_code: &'static str,
    /// What was wrong, naming the field at fault.
    pub message: String,
}

impl SegmentRejection {
    /// A document that could not be read as a JSON segment object.
    fn unparseable(message: impl Into<String>) -> Self {
        Self {
            id: None,
            error_code: SEGMENT_ERROR_PARSE,
            message: message.into(),
        }
    }

    /// A document missing a documented required field.
    fn missing(id: Option<&str>, member: &str) -> Self {
        Self {
            id: id.map(str::to_string),
            error_code: SEGMENT_ERROR_MISSING_FIELD,
            message: format!("segment document is missing the required field `{member}`"),
        }
    }

    /// A document whose field is present but of the wrong JSON type.
    fn invalid(id: Option<&str>, member: &str, expected: &str, found: &serde_json::Value) -> Self {
        Self {
            id: id.map(str::to_string),
            error_code: SEGMENT_ERROR_INVALID_FIELD,
            message: format!(
                "segment document field `{member}` must be {expected}, not {}",
                json_kind(found)
            ),
        }
    }

    /// A document whose instant is not a time a trace could have been recorded
    /// at. Its `ErrorCode` is the "present but unusable" one, and the message is
    /// built from the request: it names the member and the value at fault.
    fn not_an_instant(id: Option<&str>, member: &str, seconds: f64) -> Self {
        Self {
            id: id.map(str::to_string),
            error_code: SEGMENT_ERROR_INVALID_FIELD,
            message: format!(
                "segment document field `{member}` ({seconds}) must be a finite time within {MAX_SEGMENT_EPOCH_SECONDS} seconds of the epoch, in seconds of epoch time"
            ),
        }
    }

    /// A document refused because its trace is full.
    fn trace_full(id: &str, trace_id: &str) -> Self {
        Self {
            id: Some(id.to_string()),
            error_code: SEGMENT_ERROR_LIMIT_EXCEEDED,
            message: format!(
                "trace `{trace_id}` already holds the maximum of {MAX_SEGMENTS_PER_TRACE} segments"
            ),
        }
    }
}

/// The JSON type name of `value`, for a rejection message.
fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
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
        Some(other) => Err(SegmentRejection::invalid(id, member, "a string", other)),
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
            let seconds = number.as_f64().ok_or_else(|| {
                SegmentRejection::invalid(
                    id,
                    member,
                    "a number",
                    &serde_json::Value::Number(number.clone()),
                )
            })?;
            epoch_instant(id, member, seconds)
        }
        Some(other) => Err(SegmentRejection::invalid(id, member, "a number", other)),
    }
}

/// One instant a segment document carries, refused when it is not a time a
/// trace could have been recorded at.
///
/// Checked HERE, where the document is read, so no unusable instant is ever
/// stored: every reader converts a stored instant to the modelled `Timestamp`,
/// and a value that is not finite, or is further from the epoch than
/// [`MAX_SEGMENT_EPOCH_SECONDS`], has no `Timestamp` to convert to.
fn epoch_instant(id: Option<&str>, member: &str, seconds: f64) -> Result<f64, SegmentRejection> {
    if !seconds.is_finite() || seconds.abs() > MAX_SEGMENT_EPOCH_SECONDS {
        return Err(SegmentRejection::not_an_instant(id, member, seconds));
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
    let value: serde_json::Value = serde_json::from_str(document).map_err(|err| {
        SegmentRejection::unparseable(format!("segment document is not valid JSON: {err}"))
    })?;
    let object = value.as_object().ok_or_else(|| {
        SegmentRejection::unparseable(format!(
            "segment document must be a JSON object, not {}",
            json_kind(&value)
        ))
    })?;

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

    // "Specify either an end_time or in_progress" — a document with neither is
    // treated as in progress rather than refused, so a partial trace is still
    // readable.
    let end_time = match object.get("end_time") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(number)) => match number.as_f64() {
            Some(seconds) => Some(epoch_instant(id_ref, "end_time", seconds)?),
            None => None,
        },
        Some(other) => {
            return Err(SegmentRejection::invalid(
                id_ref,
                "end_time",
                "a number",
                other,
            ))
        }
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
        annotations: object
            .get("annotations")
            .and_then(serde_json::Value::as_object)
            .map(|annotations| {
                annotations
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        origin: optional_string(object, "origin"),
        account_id: optional_string(object, "account_id"),
    };
    Ok((trace_id, segment))
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

impl SegmentRecord {
    /// The `http.response.status` this segment recorded, if any.
    pub fn http_status(&self) -> Option<i32> {
        self.http.as_ref().and_then(|http| http.response_status)
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

    /// Whether this segment recorded a 400-series error — its `error` flag, or
    /// an `http.response.status` in the 400 series (`TraceSummary$HasError`).
    pub fn is_error(&self) -> bool {
        self.error || self.http_status().is_some_and(|status| (400..500).contains(&status))
    }

    /// Whether this segment recorded a 500-series fault — its `fault` flag, or
    /// an `http.response.status` of 500 or above (`TraceSummary$HasFault`).
    pub fn is_fault(&self) -> bool {
        self.fault || self.http_status().is_some_and(|status| status >= 500)
    }

    /// Whether this segment recorded a throttling error — its `throttle` flag,
    /// or an `http.response.status` of 429 (`TraceSummary$HasThrottle`).
    pub fn is_throttle(&self) -> bool {
        self.throttle || self.http_status() == Some(429)
    }
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

    /// Whether any segment recorded a 400-series error.
    pub fn has_error(&self) -> bool {
        self.segments.iter().any(SegmentRecord::is_error)
    }

    /// Whether any segment recorded a 500-series fault.
    pub fn has_fault(&self) -> bool {
        self.segments.iter().any(SegmentRecord::is_fault)
    }

    /// Whether any segment recorded a throttling error.
    pub fn has_throttle(&self) -> bool {
        self.segments.iter().any(SegmentRecord::is_throttle)
    }

    /// Whether any segment is still in progress (`TraceSummary$IsPartial`).
    pub fn is_partial(&self) -> bool {
        self.segments.iter().any(|segment| segment.in_progress)
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

    /// Store one parsed segment under its trace, replacing a segment already
    /// held under the same id — which is how an in-progress segment is
    /// overwritten by the completed one.
    ///
    /// Bounded in both directions: a trace holds at most
    /// [`MAX_SEGMENTS_PER_TRACE`] segments (a further new one is refused, and
    /// marks the trace `limit_exceeded`), and the store holds at most
    /// [`MAX_TRACES`] traces (a new trace past that evicts the least recently
    /// ingested one).
    pub fn ingest_segment(
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
                    return Err(SegmentRejection::trace_full(&segment.id, trace_id));
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
    /// X-Ray tags attach to a group or a sampling rule; a group's ARN is the
    /// only one that resolves here, so a sampling rule's tags — which
    /// `CreateSamplingRule` validates and stores on the rule — are NOT reachable
    /// through the tagging operations yet, and this lookup is the single place
    /// their branch goes when they are.
    ///
    /// No region argument: a group ARN carries the region it was minted in and a
    /// suffix unique to the group, so it names at most one record in the whole
    /// store — the ARN IS the key, and resolving one cannot reach a resource in
    /// another region by accident.
    ///
    /// The reserved [`DEFAULT_GROUP_NAME`] group resolves here too, by the ARN it
    /// is read with, so every tagging operation accepts exactly the ARNs the
    /// group operations do. Until something tags it there is no record to read
    /// from, and the group carries no tags, so the answer is the empty map.
    pub fn tags_for_resource(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        /// The tags a resource that holds none reports. `static`, so the read
        /// path borrows it rather than materialising a record.
        static NO_TAGS: BTreeMap<String, String> = BTreeMap::new();

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

/// X-Ray's filter-expression language, as `CreateGroup` and `UpdateGroup` see
/// it: an expression is parsed before anything is written, so an unparseable one
/// never reaches the store.
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
    const WORD_COMPARISONS: &[&str] = &["contains", "beginswith", "startswith", "endswith"];

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

    /// One lexical item. The six comparison operators collapse into one token:
    /// which comparison it is does not change whether the expression parses.
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
        /// `STARTSWITH`, `ENDSWITH`): which comparison it is does not change
        /// whether the expression parses.
        Comparison,
        OpenParen,
        CloseParen,
        OpenBrace,
        CloseBrace,
        Comma,
        Colon,
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
            Token::Comparison => "a comparison operator".to_string(),
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
                    tokens.push(Token::Comparison);
                    at += 1;
                }
                '<' | '>' => {
                    at += usize::from(characters.get(at + 1) == Some(&'=')) + 1;
                    tokens.push(Token::Comparison);
                }
                '!' => {
                    if characters.get(at + 1) != Some(&'=') {
                        return Err(Error::Syntax(
                            "`!` is only valid as part of `!=`".to_string(),
                        ));
                    }
                    tokens.push(Token::Comparison);
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
                    } else if WORD_COMPARISONS
                        .iter()
                        .any(|operator| word.eq_ignore_ascii_case(operator))
                    {
                        Token::Comparison
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

        fn expression(&mut self, depth: usize) -> Result<(), Error> {
            self.term(depth)?;
            while matches!(self.peek(), Some(Token::And | Token::Or)) {
                self.at += 1;
                self.term(depth)?;
            }
            Ok(())
        }

        fn term(&mut self, depth: usize) -> Result<(), Error> {
            if depth > MAX_DEPTH {
                return Err(Error::Syntax(format!(
                    "nested more than {MAX_DEPTH} levels deep"
                )));
            }
            match self.take() {
                Some(Token::Not) => self.term(depth + 1),
                Some(Token::OpenParen) => {
                    self.expression(depth + 1)?;
                    self.expect(&Token::CloseParen, "a closing `)`")
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

        fn predicate(&mut self, path: String, depth: usize) -> Result<(), Error> {
            if !is_indexed_field(&path) {
                return Err(Error::UnknownField(path));
            }
            if self.eat(&Token::OpenParen) {
                self.arguments()?;
            }
            if self.eat(&Token::OpenBrace) {
                self.expression(depth + 1)?;
                return self.expect(&Token::CloseBrace, "a closing `}`");
            }
            if self.eat(&Token::Comparison) {
                return self.operand();
            }
            Ok(())
        }

        fn arguments(&mut self) -> Result<(), Error> {
            loop {
                // A labelled argument, as in `id(name: "svc", type: "AWS::EC2")`.
                // The label is a member of the keyword, not a field of its own,
                // so it is not looked up in the catalog.
                if matches!(self.peek(), Some(Token::Field(_)))
                    && self.tokens.get(self.at + 1) == Some(&Token::Colon)
                {
                    self.at += 2;
                }
                self.operand()?;
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect(&Token::CloseParen, "a closing `)`")
        }

        fn operand(&mut self) -> Result<(), Error> {
            match self.take() {
                Some(Token::Text(_) | Token::Number(_) | Token::Boolean(_)) => Ok(()),
                Some(Token::Field(path)) => {
                    if is_indexed_field(&path) {
                        return Ok(());
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

    /// Parse `expression`, answering `Ok(())` only for one X-Ray would accept.
    ///
    /// An ABSENT expression is the same refusal as an empty one — X-Ray treats
    /// the two identically — so a handler passes `unwrap_or_default()` here
    /// rather than skipping the check when the member is `None`.
    pub fn validate(expression: &str) -> Result<(), Error> {
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
        parser.expression(0)?;
        if let Some(trailing) = parser.peek() {
            return Err(Error::Syntax(format!(
                "unexpected trailing {}",
                describe(trailing)
            )));
        }
        Ok(())
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
/// the `RetrievalToken` it minted for it.
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
    /// When the retrieval times out, [`TRACE_RETRIEVAL_TIMEOUT_SECONDS`] after
    /// it started.
    pub expires_at: f64,
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
        self.retrievals.insert(
            retrieval_token.clone(),
            TraceRetrievalRecord {
                retrieval_token: retrieval_token.clone(),
                trace_ids,
                start_time,
                end_time,
                started_at: now,
                expires_at: now + TRACE_RETRIEVAL_TIMEOUT_SECONDS,
            },
        );
        &self.retrievals[&retrieval_token]
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
    /// The tags the rule was created with, in submission order.
    pub tags: Vec<(String, String)>,
}

impl StoredSamplingRule {
    /// This record as the modelled `SamplingRuleRecord`.
    pub fn to_record(&self) -> SamplingRuleRecord {
        SamplingRuleRecord {
            sampling_rule: Some(self.rule.clone()),
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
    if rule.resource_arn != ANY_RESOURCE_ARN && !rule.resource_arn.starts_with("arn:") {
        return Err(invalid(format!(
            "SamplingRule.ResourceARN must be an ARN or `{ANY_RESOURCE_ARN}`, not `{}`",
            rule.resource_arn
        )));
    }
    Ok(name)
}

/// The only `SamplingRule$Version` X-Ray accepts: "the version of the sampling
/// rule format (`1`)".
pub const SAMPLING_RULE_FORMAT_VERSION: i32 = 1;

/// The `SamplingRule$ResourceARN` that matches any resource.
pub const ANY_RESOURCE_ARN: &str = "*";

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
        tags: Vec<(String, String)>,
        now: f64,
    ) -> Result<&StoredSamplingRule, SamplingRuleRejection> {
        let name = sampling_rule_name(&rule)?.to_string();
        if self.sampling_rules.contains_key(&name) {
            return Err(SamplingRuleRejection::Invalid(format!(
                "{name} already exists"
            )));
        }
        if self.sampling_rules.len() >= MAX_SAMPLING_RULES {
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
/// progress: nothing is known about how it answered.
pub const SERVICE_STATE_UNKNOWN: &str = "unknown";

/// The time span a node or an edge covers, widened segment by segment.
#[derive(Debug, Default)]
struct Span {
    start: Option<f64>,
    end: Option<f64>,
}

impl Span {
    /// Widen this span to include one segment.
    fn widen(&mut self, segment: &SegmentRecord) {
        self.start = Some(
            self.start
                .map_or(segment.start_time, |start| start.min(segment.start_time)),
        );
        self.end = Some(
            self.end
                .map_or(segment.latest(), |end| end.max(segment.latest())),
        );
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
    /// Count one segment. A fault outranks an error (a 500-series answer is not
    /// also a 400-series one), a throttled error is counted as a throttle rather
    /// than as "other", and a segment that recorded neither is OK — the same
    /// classification `TraceSummary$HasFault` / `HasError` / `HasThrottle` use.
    fn record(&mut self, segment: &SegmentRecord) {
        self.total_count += 1;
        if segment.is_fault() {
            self.fault_count += 1;
        } else if segment.is_throttle() {
            self.throttle_count += 1;
        } else if segment.is_error() {
            self.error_other_count += 1;
        } else {
            self.ok_count += 1;
        }
        self.total_response_time += segment.duration().unwrap_or(0.0);
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
    /// Count one downstream segment on this edge.
    fn record(&mut self, segment: &SegmentRecord) {
        self.span.widen(segment);
        self.statistics.record(segment);
        if let Some(duration) = segment.duration() {
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
    /// Downstream service name -> the edge that reaches it.
    edges: BTreeMap<String, EdgeAccumulator>,
}

impl NodeAccumulator {
    /// Fold one of this service's segments into the node.
    fn record(&mut self, segment: &SegmentRecord) {
        self.span.widen(segment);
        self.statistics.record(segment);
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
    fn to_service(&self, name: &str, reference_ids: &BTreeMap<String, i32>) -> Service {
        Service {
            reference_id: reference_ids.get(name).copied(),
            name: Some(name.to_string()),
            names: Some(vec![name.to_string()]),
            root: Some(self.root),
            account_id: self.account_id.clone(),
            r#type: self.service_type.clone(),
            state: Some(
                if self.complete {
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
/// handled the request); an edge is a stored `parent_id` link between two
/// segments of DIFFERENT services, counted on the upstream node and naming the
/// downstream one by `ReferenceId`. Every count, time and histogram comes from
/// the stored segment documents; nothing is echoed from the request.
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

    let mut nodes: BTreeMap<String, NodeAccumulator> = BTreeMap::new();
    for trace in traces {
        for segment in &trace.segments {
            if selects(segment) {
                nodes.entry(segment.name.clone()).or_default().record(segment);
            }
        }
        // Edges need both ends, so they are walked once this trace's nodes are
        // known: a segment whose `parent_id` names nothing stored, or whose
        // parent is outside the window, is on no edge.
        for segment in &trace.segments {
            if !selects(segment) {
                continue;
            }
            let Some(parent) = segment
                .parent_id
                .as_deref()
                .and_then(|parent_id| trace.segment(parent_id))
            else {
                continue;
            };
            if parent.name == segment.name || !selects(parent) {
                continue;
            }
            let Some(upstream) = nodes.get_mut(&parent.name) else {
                continue;
            };
            upstream
                .edges
                .entry(segment.name.clone())
                .or_default()
                .record(segment);
        }
    }

    // "Identifier for the service. Unique within the service map": assigned over
    // the nodes in name order, so one stored graph always answers with the same
    // ids — across pages of one walk, and across calls.
    let reference_ids: BTreeMap<String, i32> = nodes
        .keys()
        .enumerate()
        .map(|(at, name)| {
            (
                name.clone(),
                i32::try_from(at).unwrap_or(i32::MAX),
            )
        })
        .collect();

    nodes
        .iter()
        .map(|(name, node)| node.to_service(name, &reference_ids))
        .collect()
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
/// a node whichever page that node lands on. `Err` is the one refusal this makes
/// possible: a `NextToken` that names no service at or after it, which is a
/// token this service never issued.
pub fn page_service_graph(
    services: &[Service],
    resume: Option<&str>,
    page_size: usize,
) -> Result<ServiceGraphPage, String> {
    let from = match resume {
        Some(token) => services
            .iter()
            .position(|service| service.name.as_deref().is_some_and(|name| name >= token))
            .ok_or_else(|| format!("NextToken `{token}` was not issued by this service"))?,
        None => 0,
    };
    let remaining = &services[from..];
    Ok(ServiceGraphPage {
        services: remaining.iter().take(page_size).cloned().collect(),
        next_token: remaining
            .get(page_size)
            .and_then(|service| service.name.clone()),
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
