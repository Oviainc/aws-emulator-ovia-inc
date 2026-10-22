// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-xray-awsxray service.
//!
//! X-Ray's ingest -> query -> read loop is entirely state-derived:
//! `PutTraceSegments` stores each accepted segment document VERBATIM under its
//! trace, and every reader (`BatchGetTraces`, `GetTraceSummaries`) recomputes
//! its answer from those stored documents — nothing is echoed from the request
//! that stored them. Groups are a keyed map plus an ARN index, so `GetGroup`
//! resolves a group by either selector. Indexing rules are a fixed, seeded
//! keyed map: the account's `Default` rule exists before the first call, and
//! `UpdateIndexingRule` rewrites its configuration in place for
//! `GetIndexingRules` to report.
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

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{
    EncryptionConfig, Group, GroupSummary, IndexingRule, IndexingRuleValue, InsightsConfiguration,
    ProbabilisticRuleValue, TelemetryRecord,
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

/// Whether `candidate` is the full ARN of a KMS key or alias
/// (`arn:<partition>:kms:<region>:<account>:key/<key id>` or `…:alias/<name>`),
/// the third form `PutEncryptionConfig$KeyId` documents.
fn is_kms_arn(candidate: &str) -> bool {
    let mut fields = candidate.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some("kms"),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return false;
    };
    !partition.is_empty()
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

/// Per-service state for com-amazonaws-xray-awsxray. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsXrayAwsxrayState {
    /// Ingested traces, keyed by `trace_id`, at most [`MAX_TRACES`] of them.
    pub traces: BTreeMap<String, TraceRecord>,
    /// Groups, keyed by their case-sensitive `GroupName`, at most [`MAX_GROUPS`].
    pub groups: BTreeMap<String, GroupRecord>,
    /// `GroupARN` -> `GroupName`, so a reader can resolve a group by either selector.
    pub group_arn_to_name: BTreeMap<String, String>,
    /// Feeds the unique suffix of a minted group ARN; never reset, so two groups
    /// of the same name created in sequence still get distinct ARNs.
    pub group_seq: u64,
    /// Ingest counter, stamped on a trace so the oldest can be evicted.
    pub ingest_seq: u64,
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
pub fn now_epoch_secs() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
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

/// Why a `(GroupName, GroupARN)` pair names no stored group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupLookupError {
    /// Neither selector was supplied.
    NoSelector,
    /// A selector was supplied but no group matches it.
    NotFound,
}

impl GroupLookupError {
    /// What to tell the caller. Lives here, beside
    /// [`ComAmazonawsXrayAwsxrayState::resolve_group_name`], so `GetGroup` and
    /// `DeleteGroup` cannot drift into two wordings of one refusal.
    pub fn message(self, group_name: Option<&str>, group_arn: Option<&str>) -> String {
        match self {
            Self::NoSelector => "either GroupName or GroupARN must be specified".to_string(),
            Self::NotFound => format!(
                "no group matches GroupName {group_name:?} / GroupARN {group_arn:?}"
            ),
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

/// A required epoch-seconds field, distinguishing "absent" from "wrong type".
fn required_seconds(
    object: &JsonObject,
    member: &str,
    id: Option<&str>,
) -> Result<f64, SegmentRejection> {
    match object.get(member) {
        None | Some(serde_json::Value::Null) => Err(SegmentRejection::missing(id, member)),
        Some(serde_json::Value::Number(number)) => number.as_f64().ok_or_else(|| {
            SegmentRejection::invalid(
                id,
                member,
                "a number",
                &serde_json::Value::Number(number.clone()),
            )
        }),
        Some(other) => Err(SegmentRejection::invalid(id, member, "a number", other)),
    }
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
        Some(serde_json::Value::Number(number)) => number.as_f64(),
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

    /// Mint a group ARN of the form
    /// `arn:aws:xray:<region>:<account>:group/<GroupName>/<12 hex digits>`, as
    /// X-Ray's `GroupARN` is "generated based on the GroupName".
    ///
    /// The 12-hex suffix is unique per minted group: it is derived from the name
    /// and the never-reset [`group_seq`](Self::group_seq), so two groups never
    /// share an ARN even when one replaces the other by name, and no clock
    /// enters the value. The mixing function is `std`'s default hasher, whose
    /// algorithm `std` does not promise to keep stable across releases, so the
    /// suffix is a unique opaque id rather than a value to depend on.
    pub fn mint_group_arn(&mut self, region: &str, group_name: &str) -> String {
        use std::hash::{Hash, Hasher};

        self.group_seq += 1;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        group_name.hash(&mut hasher);
        self.group_seq.hash(&mut hasher);
        // 48 bits renders as exactly 12 hexadecimal digits.
        let suffix = hasher.finish() & 0xffff_ffff_ffff;
        ::emulator_server::emulator_arn(
            ARN_NAMESPACE,
            region,
            &format!("group/{group_name}/{suffix:012x}"),
        )
    }

    /// Insert a group and index its ARN, returning the stored record, or `None`
    /// when the store already holds [`MAX_GROUPS`] groups.
    ///
    /// Inserting over a name that already exists REPLACES that record, and drops
    /// its superseded ARN from the index in the same step — so the index never
    /// grows past `groups`, and a superseded ARN resolves to nothing rather than
    /// to whatever group later took the name.
    pub fn insert_group(&mut self, region: &str, record: GroupRecordDraft) -> Option<&GroupRecord> {
        match self.groups.remove(&record.group_name) {
            Some(superseded) => {
                self.group_arn_to_name.remove(&superseded.group_arn);
            }
            None if self.groups.len() >= MAX_GROUPS => return None,
            None => {}
        }
        let group_arn = self.mint_group_arn(region, &record.group_name);
        let group_name = record.group_name.clone();
        self.group_arn_to_name
            .insert(group_arn.clone(), group_name.clone());
        self.groups.insert(
            group_name.clone(),
            GroupRecord {
                group_name: group_name.clone(),
                group_arn,
                filter_expression: record.filter_expression,
                insights_configuration: record.insights_configuration,
                tags: record.tags,
            },
        );
        Some(&self.groups[&group_name])
    }

    /// The stored name a `(GroupName, GroupARN)` pair selects.
    ///
    /// Either selector resolves the same record; `GroupName` is tried first, then
    /// the ARN index.
    pub fn resolve_group_name(
        &self,
        group_name: Option<&str>,
        group_arn: Option<&str>,
    ) -> Result<String, GroupLookupError> {
        if group_name.is_none() && group_arn.is_none() {
            return Err(GroupLookupError::NoSelector);
        }
        if let Some(name) = group_name {
            if self.groups.contains_key(name) {
                return Ok(name.to_string());
            }
        }
        if let Some(arn) = group_arn {
            if let Some(name) = self.group_arn_to_name.get(arn) {
                return Ok(name.clone());
            }
        }
        Err(GroupLookupError::NotFound)
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

    /// Apply an `UpdateGroup` to the stored group `group_name`, returning the
    /// record as it now stands, or `None` when no group is stored under that
    /// name.
    ///
    /// Each member the update SUPPLIES replaces the stored one; each member it
    /// OMITS leaves the stored one as it is. The group's identity — its
    /// case-sensitive name and the ARN that "was generated upon creation"
    /// (`UpdateGroupRequest$GroupARN`) — and its tags are never touched by an
    /// update, so neither the keyed map nor the ARN index needs maintenance
    /// here and every selector that resolved the group before still does.
    pub fn apply_group_update(
        &mut self,
        group_name: &str,
        update: GroupUpdate,
    ) -> Option<&GroupRecord> {
        let record = self.groups.get_mut(group_name)?;
        if let Some(filter_expression) = update.filter_expression {
            record.filter_expression = Some(filter_expression);
        }
        if let Some(insights_configuration) = update.insights_configuration {
            record.insights_configuration = Some(insights_configuration);
        }
        Some(record)
    }

    /// Remove a group and its ARN index entry.
    pub fn remove_group(&mut self, group_name: &str) -> Option<GroupRecord> {
        let record = self.groups.remove(group_name)?;
        self.group_arn_to_name.remove(&record.group_arn);
        Some(record)
    }

    /// The tags held by the resource `resource_arn` names, or `None` when it
    /// names no resource this store holds.
    ///
    /// X-Ray tags attach to a group or a sampling rule; sampling rules are not
    /// implemented in this emulator yet (`CreateSamplingRule` is still a
    /// scaffold), so a stored group's ARN is the only one that resolves today,
    /// and this lookup is the single place their branch goes when they land.
    pub fn tags_for_resource(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        let group_name = self.group_arn_to_name.get(resource_arn)?;
        Some(&self.groups.get(group_name)?.tags)
    }

    /// The same lookup, for a tagging operation that writes.
    pub fn tags_for_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        let group_name = self.group_arn_to_name.get(resource_arn)?.clone();
        Some(&mut self.groups.get_mut(&group_name)?.tags)
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
