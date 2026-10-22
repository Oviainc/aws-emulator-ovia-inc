// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-xray-awsxray service.
//!
//! X-Ray's ingest -> query -> read loop is entirely state-derived:
//! `PutTraceSegments` stores each accepted segment document VERBATIM under its
//! trace, and every reader (`BatchGetTraces`, `GetTraceSummaries`) recomputes
//! its answer from those stored documents — nothing is echoed from the request
//! that stored them. Groups are a keyed map plus an ARN index, so `GetGroup`
//! resolves a group by either selector.
//!
//! The segment-document schema this module parses is the one `PutTraceSegments`
//! documents (`smithy_query(query="doc",shape="PutTraceSegments")`): `name`,
//! `id`, `trace_id` and `start_time` are required, `end_time`/`in_progress` say
//! whether the segment is complete, and `error`/`fault`/`throttle`, `http`,
//! `parent_id` and `annotations` are optional.

use std::collections::BTreeMap;

use crate::ops::types::{Group, GroupSummary, InsightsConfiguration};

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
    /// The tags the group was created with, in submission order.
    pub tags: Vec<(String, String)>,
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
    /// The requested tags.
    pub tags: Vec<(String, String)>,
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
