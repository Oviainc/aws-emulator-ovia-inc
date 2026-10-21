// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudwatchlogs-logs-20140328 service.
//!
//! The log-group / log-stream / log-event core loop is stored here, together with the
//! small helpers every handler of that loop shares: ARN construction, the wall clock,
//! log-group identifier resolution and the opaque pagination tokens
//! `GetLogEvents` / `DescribeLogStreams` / `DescribeLogGroups` hand back.

use std::collections::BTreeMap;

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Generated emulators default to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The log group class `CreateLogGroup` applies when the request omits
/// `logGroupClass` ("If you omit this parameter, the default of STANDARD is used").
pub const DEFAULT_LOG_GROUP_CLASS: &str = "STANDARD";

/// Per-event storage overhead CloudWatch Logs charges a log group's `storedBytes`.
/// From the PutLogEvents quota: a batch's size "is calculated as the sum of all event
/// messages in UTF-8, plus 26 bytes for each log event".
pub const EVENT_OVERHEAD_BYTES: i64 = 26;

/// The number of items `DescribeLogGroups` / `DescribeLogStreams` return when the
/// request omits `limit` ("If you don't specify a value, the default is up to 50 items").
pub const DEFAULT_DESCRIBE_LIMIT: i32 = 50;

/// The number of events `GetLogEvents` returns when the request omits `limit`: the
/// documented ceiling of "up to 10,000 log events" per page.
pub const DEFAULT_EVENTS_LIMIT: i32 = 10_000;

/// AWS's documented log group quota, from `CreateLogGroup`: "You can create up to
/// 1,000,000 log groups per Region per account."
pub const MAX_LOG_GROUPS: usize = 1_000_000;

/// THE EMULATOR'S OWN CEILING on the log streams one group holds, not an AWS quota:
/// CloudWatch Logs documents no limit on log streams, but this emulator keeps every
/// stream in memory, so it refuses to create more than this many in one group rather
/// than growing without bound on request.
pub const MAX_STREAMS_PER_GROUP: usize = 10_000;

/// THE EMULATOR'S OWN STORAGE CEILING for one log stream, not an AWS quota: the events
/// a stream holds live in memory, so it keeps at most this many of them, and at most
/// [`MAX_STREAM_BYTES`] of message bytes, discarding the OLDEST first when a write would
/// exceed either. That is the effect a caller already observes from the real service
/// when events pass a log group's retention period and are deleted.
pub const MAX_STREAM_EVENTS: usize = 20_000;

/// The byte half of the per-stream storage ceiling. See [`MAX_STREAM_EVENTS`].
pub const MAX_STREAM_BYTES: i64 = 8 * 1024 * 1024;

/// Per-service state for com-amazonaws-cloudwatchlogs-logs-20140328. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudwatchlogsLogs20140328State {
    /// Every created log group, keyed by `logGroupName`. A `BTreeMap` so
    /// `DescribeLogGroups` enumerates in log-group-name order without re-sorting.
    pub log_groups: BTreeMap<String, LogGroupRecord>,
    /// Monotonic ingestion counter. It totally orders events that share a timestamp
    /// and makes the pagination cursors and sequence tokens stable.
    pub next_ingestion_seq: u64,
}

impl ComAmazonawsCloudwatchlogsLogs20140328State {
    /// Take the next ingestion sequence number, advancing the counter.
    pub fn take_ingestion_seq(&mut self) -> u64 {
        let seq = self.next_ingestion_seq;
        self.next_ingestion_seq = self.next_ingestion_seq.saturating_add(1);
        seq
    }
}

/// One created log group and everything stored under it.
#[derive(Debug, Clone)]
pub struct LogGroupRecord {
    pub name: String,
    /// The ARN version that carries a trailing `:*` (the `LogGroup$arn` member).
    pub arn: String,
    /// The ARN version without the trailing `:*` (the `LogGroup$logGroupArn` member).
    pub log_group_arn: String,
    pub creation_time_ms: i64,
    pub retention_in_days: Option<i32>,
    pub kms_key_id: Option<String>,
    pub log_group_class: String,
    pub deletion_protection_enabled: bool,
    pub stored_bytes: i64,
    pub metric_filter_count: i32,
    pub tags: BTreeMap<String, String>,
    /// The group's log streams, keyed by `logStreamName`.
    pub streams: BTreeMap<String, LogStreamRecord>,
}

impl LogGroupRecord {
    /// A brand-new log group: the class default, no retention, no stored bytes.
    pub fn new(region: &str, name: &str, creation_time_ms: i64) -> Self {
        Self {
            name: name.to_string(),
            arn: log_group_arn(region, name, true),
            log_group_arn: log_group_arn(region, name, false),
            creation_time_ms,
            retention_in_days: None,
            kms_key_id: None,
            log_group_class: DEFAULT_LOG_GROUP_CLASS.to_string(),
            deletion_protection_enabled: false,
            stored_bytes: 0,
            metric_filter_count: 0,
            tags: BTreeMap::new(),
            streams: BTreeMap::new(),
        }
    }
}

/// One created log stream and its events.
#[derive(Debug, Clone)]
pub struct LogStreamRecord {
    pub name: String,
    pub arn: String,
    pub creation_time_ms: i64,
    pub first_event_timestamp: Option<i64>,
    pub last_event_timestamp: Option<i64>,
    pub last_ingestion_time: Option<i64>,
    /// `None` until the first `PutLogEvents`: AWS reports no `uploadSequenceToken`
    /// for a stream nothing has been written to.
    pub upload_sequence_token: Option<String>,
    /// Events kept sorted by `(timestamp, ingestion_seq)`.
    pub events: Vec<StoredEvent>,
}

impl LogStreamRecord {
    /// A brand-new, empty log stream.
    pub fn new(region: &str, group_name: &str, name: &str, creation_time_ms: i64) -> Self {
        Self {
            name: name.to_string(),
            arn: log_stream_arn(region, group_name, name),
            creation_time_ms,
            first_event_timestamp: None,
            last_event_timestamp: None,
            last_ingestion_time: None,
            upload_sequence_token: None,
            events: Vec::new(),
        }
    }

    /// Re-derive the event-time summary members from the stored events.
    pub fn refresh_event_times(&mut self) {
        self.first_event_timestamp = self.events.first().map(|e| e.timestamp);
        self.last_event_timestamp = self.events.last().map(|e| e.timestamp);
    }

    /// The bytes the stream's events occupy, counted the way a log group's
    /// `storedBytes` counts them.
    pub fn stored_bytes(&self) -> i64 {
        self.events
            .iter()
            .map(StoredEvent::stored_bytes)
            .fold(0i64, i64::saturating_add)
    }

    /// Hold the stream inside the emulator's storage ceiling ([`MAX_STREAM_EVENTS`] /
    /// [`MAX_STREAM_BYTES`]) by discarding the oldest events, and answer how many bytes
    /// went, so the owning log group's `storedBytes` can be reduced by the same amount.
    ///
    /// Events are stored oldest-first, so the discarded run is a prefix. A single event
    /// can never exceed the byte ceiling (a log event is capped at 1 MB on the way in),
    /// so the newest event always survives.
    pub fn enforce_storage_ceiling(&mut self) -> i64 {
        let mut kept_bytes = self.stored_bytes();
        let mut discard = 0usize;
        while discard < self.events.len()
            && (self.events.len() - discard > MAX_STREAM_EVENTS || kept_bytes > MAX_STREAM_BYTES)
        {
            kept_bytes = kept_bytes.saturating_sub(self.events[discard].stored_bytes());
            discard += 1;
        }
        let discarded_bytes = self.events[..discard]
            .iter()
            .map(StoredEvent::stored_bytes)
            .fold(0i64, i64::saturating_add);
        self.events.drain(..discard);
        discarded_bytes
    }
}

/// One ingested log event.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub timestamp: i64,
    pub message: String,
    pub ingestion_time: i64,
    pub ingestion_seq: u64,
}

impl StoredEvent {
    /// The total order events are stored and paginated in.
    pub fn key(&self) -> EventKey {
        EventKey {
            timestamp: self.timestamp,
            seq: self.ingestion_seq,
        }
    }

    /// What this event adds to its log group's `storedBytes`: the UTF-8 message plus
    /// the documented 26-byte per-event overhead.
    pub fn stored_bytes(&self) -> i64 {
        i64::try_from(self.message.len()).unwrap_or(i64::MAX) + EVENT_OVERHEAD_BYTES
    }
}

/// A position in a stream's event order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventKey {
    pub timestamp: i64,
    pub seq: u64,
}

impl EventKey {
    /// The position before every event, so a forward scan from it starts at the head.
    pub const BEFORE_ALL: Self = Self {
        timestamp: i64::MIN,
        seq: 0,
    };
    /// The position after every event, so a backward scan from it starts at the tail.
    pub const AFTER_ALL: Self = Self {
        timestamp: i64::MAX,
        seq: u64::MAX,
    };
}

/// The region to stamp into the ARNs this request's resources get: the SigV4
/// credential scope's region, or [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Wall-clock milliseconds since the Unix epoch — what every CloudWatch Logs
/// timestamp member is expressed in.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// A log group's ARN, with the trailing `:*` when `wildcard_suffix` is set (the two
/// versions the model's `arn` and `logGroupArn` members document).
pub fn log_group_arn(region: &str, name: &str, wildcard_suffix: bool) -> String {
    let resource = if wildcard_suffix {
        format!("log-group:{name}:*")
    } else {
        format!("log-group:{name}")
    };
    ::engine::emulator_arn("logs", region, &resource)
}

/// A log stream's ARN.
pub fn log_stream_arn(region: &str, group_name: &str, stream_name: &str) -> String {
    ::engine::emulator_arn(
        "logs",
        region,
        &format!("log-group:{group_name}:log-stream:{stream_name}"),
    )
}

/// The log group name a `logGroupIdentifier` names: the identifier itself when it is a
/// name, or the name segment of a log-group ARN (either ARN version).
pub fn log_group_name_from_identifier(identifier: &str) -> &str {
    const MARKER: &str = ":log-group:";
    match identifier.find(MARKER) {
        Some(at) => {
            let rest = &identifier[at + MARKER.len()..];
            let rest = rest.strip_suffix(":*").unwrap_or(rest);
            // A log-stream ARN carries the group name before `:log-stream:`.
            match rest.find(":log-stream:") {
                Some(end) => &rest[..end],
                None => rest,
            }
        }
        None => identifier,
    }
}

/// Resolve the log group an operation's `logGroupName` / `logGroupIdentifier` pair
/// names. The model documents them as mutually exclusive with one of them required, so
/// neither-or-both is a client error; the `Err` carries the message to refuse with.
pub fn resolve_log_group_target(
    log_group_name: Option<&str>,
    log_group_identifier: Option<&str>,
) -> Result<String, String> {
    match (log_group_name, log_group_identifier) {
        (Some(name), None) => Ok(name.to_string()),
        (None, Some(identifier)) => Ok(log_group_name_from_identifier(identifier).to_string()),
        (Some(_), Some(_)) => Err(
            "logGroupName and logGroupIdentifier are mutually exclusive; specify only one"
                .to_string(),
        ),
        (None, None) => {
            Err("either logGroupName or logGroupIdentifier must be specified".to_string())
        }
    }
}

/// The sequence token a `PutLogEvents` call hands back, minted from the monotonic
/// ingestion counter so it is unique and derived from stored state. Rendered the way
/// CloudWatch Logs renders one: a fixed-width decimal string.
pub fn mint_sequence_token(seq: u64) -> String {
    format!("{seq:056}")
}

/// Encode a pagination cursor as an opaque token.
///
/// The payload is this service's own structure; hex-encoding it keeps the token
/// opaque to a client, which must only round-trip it back to the next call.
pub fn encode_token(payload: &serde_json::Value) -> String {
    hex::encode(serde_json::to_vec(payload).unwrap_or_default())
}

/// Decode a token [`encode_token`] produced. `None` for anything else, which a
/// handler refuses as an invalid `nextToken`.
pub fn decode_token(token: &str) -> Option<serde_json::Value> {
    let bytes = hex::decode(token).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudwatchlogsLogs20140328State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudwatchlogsLogs20140328State>)
}
