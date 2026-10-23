// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisvideo-kinesisvideo-20170930 service.
//!
//! One store behind one lock, shaped per resource so the signaling-channel,
//! edge-configuration and tagging slices can be layered in beside the stream
//! index without a second copy of any record.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::StreamInfo;

/// The region used when a request carries no SigV4 credential scope to read one
/// from (an anonymous or host-routed call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `Status` values, from `smithy_query(query="shape", shape="Status")` via
/// [`crate::ops::types::STATUS_VALUES`].
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// How long a freshly created stream reports `CREATING` before the clock owes it
/// `ACTIVE`.
///
/// The model documents `CreateStream` as "an asynchronous operation", so the
/// stream is returned in a non-terminal status and the move to `ACTIVE` is owed
/// by the clock rather than performed by whoever reads it next. AWS publishes no
/// duration for it, so this span is CHOSEN — deliberately non-zero, so a caller
/// that describes the stream immediately still observes `CREATING`.
pub const STREAM_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// The most streams this emulator holds at once, per account and region.
///
/// `streams` grows by one record per `CreateStream` request — each holding
/// caller-supplied strings and a tag map — and shrinks only on an explicit
/// `DeleteStream`, so without a ceiling its entry COUNT, and with it the
/// per-request work every stream operation does under the lock
/// ([`ComAmazonawsKinesisvideoKinesisvideo20170930State::settle_clock`] walks the
/// index), is set by how many requests one caller makes. AWS bounds the same thing
/// with a per-account stream quota and refuses a create past it; that quota's value
/// is adjustable and is not in the model, so this number is the EMULATOR's own bound
/// rather than a claim about AWS's, while the refusal it produces is the error the
/// model declares for exactly this case: `AccountStreamLimitExceededException`
/// (HTTP 400, "The number of streams created for the account is too high.").
pub const MAX_STREAMS: usize = 2_500;

/// One Kinesis video stream, as this service persists it.
#[derive(Debug, Clone)]
pub struct Stream {
    pub stream_name: String,
    pub stream_arn: String,
    pub device_name: Option<String>,
    pub media_type: Option<String>,
    /// Never `None`: `CreateStream` defaults it to the Kinesis Video-managed key.
    pub kms_key_id: String,
    /// Opaque alphanumeric token, re-minted on every mutation of the record.
    pub version: String,
    pub status: String,
    /// `CreationTime`, as Unix epoch milliseconds.
    pub creation_time_millis: i64,
    pub data_retention_in_hours: i32,
    pub tags: BTreeMap<String, String>,
    /// The `DefaultStorageTier` the create call asked for, kept so the
    /// storage-configuration operations (their own gap issues) read the value the
    /// client sent rather than a fresh default.
    pub default_storage_tier: Option<String>,
    /// When the clock owes this stream the move out of `CREATING`; `None` once
    /// the status is terminal.
    pub activates_at: Option<Instant>,
}

impl Stream {
    /// This record as the modeled `StreamInfo` every reader returns.
    pub fn to_stream_info(&self) -> StreamInfo {
        StreamInfo {
            device_name: self.device_name.clone(),
            stream_name: Some(self.stream_name.clone()),
            stream_arn: Some(self.stream_arn.clone()),
            media_type: self.media_type.clone(),
            kms_key_id: Some(self.kms_key_id.clone()),
            version: Some(self.version.clone()),
            status: Some(self.status.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from_millis(
                self.creation_time_millis,
            )),
            data_retention_in_hours: Some(self.data_retention_in_hours),
        }
    }
}

/// The most tags one stream carries at once.
///
/// `ResourceTags`, the map every tagging operation reads and writes, is modeled
/// `@length(min: 1, max: 50)` (`smithy_query(query="shape", shape="ResourceTags")`),
/// and `TagsPerResourceExceededLimitException` states the same bound in prose: "You
/// have exceeded the limit of tags that you can associate with the resource. A
/// Kinesis video stream can support up to 50 tags." One request cannot carry more
/// than this (the boundary validator refuses it), so the ceiling bites when a
/// request's tags are MERGED into the ones a stream already holds.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `TagKey`'s modeled minimum length, from
/// `smithy_query(query="shape", shape="TagKey")` (`@length(min: 1, max: 128)`).
pub const MIN_TAG_KEY_LENGTH: usize = 1;

/// `TagKey`'s modeled maximum length. See [`MIN_TAG_KEY_LENGTH`].
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// `TagValue`'s modeled maximum length, from
/// `smithy_query(query="shape", shape="TagValue")` (`@length(min: 0, max: 256)`);
/// its minimum is 0, so an empty value is valid.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// Why `key` is not a `TagKey`, or `None` when it is one.
///
/// A MAP KEY's own `@length` is not among the constraints the request walk's validator
/// checks — an empty key reaches the handler — so the operations that accept tag keys
/// enforce it themselves. The message is built from the request, naming the member and
/// the constraint it failed, in the form the framework's own validation envelope uses.
///
/// `TagKey`'s `@pattern` (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, a regular expression over
/// Unicode general categories) is NOT evaluated here: `@pattern` checking belongs to
/// the framework's validating walk, which owns it for every member it does reach, and
/// a key the pattern rejects is therefore stored rather than refused. Closing that gap
/// belongs in that walk rather than in one service's handlers.
pub fn tag_key_violation(member: &str, key: &str) -> Option<String> {
    let length = key.chars().count();
    if !(MIN_TAG_KEY_LENGTH..=MAX_TAG_KEY_LENGTH).contains(&length) {
        return Some(format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             Member must have length greater than or equal to {MIN_TAG_KEY_LENGTH} and less \
             than or equal to {MAX_TAG_KEY_LENGTH}"
        ));
    }
    None
}

/// Why `value` is not a `TagValue`, or `None` when it is one. The companion of
/// [`tag_key_violation`]: a map VALUE's `@length` is checked by the request walk for a
/// list or structure member, but not for the value side of a map, so the tagging
/// operations check it here.
pub fn tag_value_violation(member: &str, value: &str) -> Option<String> {
    if value.chars().count() > MAX_TAG_VALUE_LENGTH {
        return Some(format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             Member must have length less than or equal to {MAX_TAG_VALUE_LENGTH}"
        ));
    }
    None
}

/// Why a `StreamName`/`StreamARN` pair did not name a stored stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamLookupError {
    /// Neither identifier was supplied; the operation requires one of them.
    NoIdentifier,
    /// Both were supplied and they name different streams.
    Mismatch,
    /// No stream of that name or ARN exists in this account and region.
    NotFound,
}

/// Per-service state for com-amazonaws-kinesisvideo-kinesisvideo-20170930. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisvideoKinesisvideo20170930State {
    /// The primary stream index. `BTreeMap`, not `HashMap`: `ListStreams`
    /// paginates over the sorted key set, and a token resolves against that set
    /// rather than against a snapshot copy.
    pub streams: BTreeMap<String, Stream>,
    /// `StreamARN` -> `StreamName`, so every operation that accepts an ARN
    /// resolves to the same record the name form does.
    pub stream_arn_index: BTreeMap<String, String>,
    /// Bumped on every mutation, so a re-minted `Version` never repeats.
    pub version_seq: u64,
}

impl ComAmazonawsKinesisvideoKinesisvideo20170930State {
    /// Materialise every status transition the clock already owes.
    ///
    /// Called at the top of each stream operation, including the reporting ones:
    /// the move out of `CREATING` was due at a fixed instant whether or not
    /// anybody called, so applying it here changes nothing a later caller would
    /// not have seen anyway.
    pub fn settle_clock(&mut self) {
        let now = Instant::now();
        for stream in self.streams.values_mut() {
            if stream.activates_at.is_some_and(|due| now >= due) {
                stream.activates_at = None;
                stream.status = STATUS_ACTIVE.to_string();
            }
        }
    }

    /// A fresh opaque `Version` token: alphanumeric, 1..64 characters, per
    /// `smithy_query(query="shape", shape="Version")`.
    pub fn mint_version(&mut self) -> String {
        self.version_seq = self.version_seq.wrapping_add(1);
        format!(
            "{}{}",
            base36(u128::from(self.version_seq)),
            base36(now_nanos())
        )
    }

    /// The stored stream a `StreamName`/`StreamARN` pair names.
    pub fn resolve(
        &self,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
    ) -> Result<&Stream, StreamLookupError> {
        let name = match (stream_name, stream_arn) {
            (None, None) => return Err(StreamLookupError::NoIdentifier),
            (Some(name), None) => name.to_string(),
            (None, Some(arn)) => self
                .stream_arn_index
                .get(arn)
                .cloned()
                .ok_or(StreamLookupError::NotFound)?,
            (Some(name), Some(arn)) => {
                let from_arn = self
                    .stream_arn_index
                    .get(arn)
                    .cloned()
                    .ok_or(StreamLookupError::NotFound)?;
                if from_arn != name {
                    return Err(StreamLookupError::Mismatch);
                }
                from_arn
            }
        };
        self.streams.get(&name).ok_or(StreamLookupError::NotFound)
    }

    /// [`Self::resolve`] for an operation that MUTATES the stream it names, so the
    /// two forms of identifier reach the one stored record through the same rule.
    pub fn resolve_mut(
        &mut self,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
    ) -> Result<&mut Stream, StreamLookupError> {
        // The immutable resolution decides WHICH record this is; the name it yields
        // is then re-borrowed mutably, so the lookup rule lives in one place.
        let name = self.resolve(stream_name, stream_arn)?.stream_name.clone();
        self.streams
            .get_mut(&name)
            .ok_or(StreamLookupError::NotFound)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisvideoKinesisvideo20170930State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisvideoKinesisvideo20170930State>)
}

/// Unix epoch milliseconds now.
pub fn now_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or_default(),
    )
    .unwrap_or(i64::MAX)
}

/// Unix epoch nanoseconds now.
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default()
}

/// Base-36 (`0-9a-z`) rendering of `value`, so a token built from it satisfies an
/// alphanumeric-only `@pattern`.
fn base36(mut value: u128) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(DIGITS[usize::try_from(value % 36).unwrap_or(0)]);
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_else(|_| "0".to_string())
}

/// The `StreamARN` of a stream, in the one form the modeled `@pattern`
/// `^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`
/// accepts. The trailing numeric segment is the record's creation instant, so the
/// ARN is stable for the record's whole lifetime.
pub fn stream_arn(region: &str, stream_name: &str, creation_time_millis: i64) -> String {
    ::engine::emulator_arn(
        "kinesisvideo",
        region,
        &format!("stream/{stream_name}/{creation_time_millis}"),
    )
}

/// The Kinesis Video-managed key a stream is encrypted with when the create call
/// names none: "If no key ID is specified, the default, Kinesis Video-managed key
/// (aws/kinesisvideo) is used" (`KmsKeyId` docs), reported as that key's ARN.
pub fn default_kms_key_id(region: &str) -> String {
    ::engine::emulator_arn("kms", region, "alias/aws/kinesisvideo")
}

/// The region this request is scoped to, from its SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The base URL a data-plane client should send this stream's media calls to.
///
/// It is the emulator's OWN endpoint — the authority this request arrived on —
/// because the endpoint has to be reachable: the model notes "The returned
/// endpoint does not have the API name appended. The client needs to add the API
/// name to the returned endpoint", so a client appends `/putMedia` and must land
/// back on this emulator.
pub fn request_endpoint(parts: &::http::request::Parts, region: &str) -> String {
    let authority = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|host| !host.is_empty())
        .map(ToString::to_string)
        .or_else(|| parts.uri.authority().map(ToString::to_string));
    match authority {
        Some(authority) => format!("https://{authority}"),
        None => format!("https://kinesisvideo.{region}.amazonaws.com"),
    }
}
