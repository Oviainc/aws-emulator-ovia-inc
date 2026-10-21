// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader service.
//!
//! This service is a READ plane over a fragment archive: it has no create
//! operation of its own (streams and fragments are ingested through the Kinesis
//! Video Streams control/data plane), so everything the handlers answer is
//! computed from the archive held here. The archive is populated through
//! [`ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState::put_stream`] /
//! [`ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState::append_fragment`],
//! which an ingest path (or a seed/test harness) calls on the one shared engine
//! store.
//!
//! The query helpers (stream resolution, timestamp-range filtering, pagination
//! cursors) live here beside the data they read, so every operation resolves a
//! stream and filters fragments by exactly the same rule.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch for `t`, negative for an instant before it.
///
/// Every timestamp comparison in this service happens in this integer space, so
/// a range bound and a fragment timestamp are compared exactly rather than
/// through a lossy float.
#[must_use]
pub fn epoch_millis(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis())
            .map_or(i64::MIN, |millis| -millis),
    }
}

/// Milliseconds since the Unix epoch for a wire timestamp.
///
/// Exact: the `DateTime` seconds and sub-second nanos are combined as integers,
/// so a request's range bound lands in the same integer space the archive's own
/// timestamps are compared in.
#[must_use]
pub fn date_time_millis(value: &::aws_smithy_types::DateTime) -> i64 {
    value
        .secs()
        .saturating_mul(1000)
        .saturating_add(i64::from(value.subsec_nanos() / 1_000_000))
}

/// The instant `millis` milliseconds from the Unix epoch names.
#[must_use]
pub fn from_epoch_millis(millis: i64) -> SystemTime {
    if millis >= 0 {
        UNIX_EPOCH + Duration::from_millis(millis.unsigned_abs())
    } else {
        UNIX_EPOCH - Duration::from_millis(millis.unsigned_abs())
    }
}

/// `t` as the wire timestamp a response member carries, at the millisecond
/// precision the archive records.
#[must_use]
pub fn to_date_time(t: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(epoch_millis(t))
}

/// Which of a fragment's two timestamps a selector reads.
///
/// The wire values are the `FragmentSelectorType` / `ClipFragmentSelectorType` /
/// `HLSFragmentSelectorType` enum value set (`PRODUCER_TIMESTAMP`,
/// `SERVER_TIMESTAMP`) — one Rust type because all three enums carry the same
/// two values and mean the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampOrigin {
    /// The timestamp the producer stamped on the fragment.
    Producer,
    /// The timestamp the service stamped when it received the fragment.
    Server,
}

impl TimestampOrigin {
    /// Parse a wire enum value, or `None` when it is outside the value set.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "PRODUCER_TIMESTAMP" => Some(Self::Producer),
            "SERVER_TIMESTAMP" => Some(Self::Server),
            _ => None,
        }
    }

    /// The wire enum value for this origin.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Producer => "PRODUCER_TIMESTAMP",
            Self::Server => "SERVER_TIMESTAMP",
        }
    }

    /// The timestamp of `fragment` this origin selects.
    #[must_use]
    pub fn of(self, fragment: &StoredFragment) -> SystemTime {
        match self {
            Self::Producer => fragment.producer_timestamp,
            Self::Server => fragment.server_timestamp,
        }
    }
}

/// One archived fragment: its identity, its two timestamps, its duration and
/// the media bytes the read operations concatenate.
#[derive(Debug, Clone)]
pub struct StoredFragment {
    /// The fragment number, a decimal string that increases with ingestion
    /// order (`^[0-9]+$`).
    pub fragment_number: String,
    /// The timestamp the producer stamped on the fragment.
    pub producer_timestamp: SystemTime,
    /// The timestamp the service stamped when it received the fragment.
    pub server_timestamp: SystemTime,
    /// The playback duration of the fragment.
    pub duration_millis: u64,
    /// The archived media bytes of this fragment.
    pub media: Vec<u8>,
}

impl StoredFragment {
    /// An archived fragment carrying `media`.
    #[must_use]
    pub fn new(
        fragment_number: impl Into<String>,
        producer_timestamp: SystemTime,
        server_timestamp: SystemTime,
        duration_millis: u64,
        media: Vec<u8>,
    ) -> Self {
        Self {
            fragment_number: fragment_number.into(),
            producer_timestamp,
            server_timestamp,
            duration_millis,
            media,
        }
    }

    /// The size the `Fragment$FragmentSizeInBytes` member reports.
    ///
    /// DERIVED from the stored media rather than held as a second field: a
    /// stored size could disagree with the bytes `GetMediaForFragmentList`
    /// returns for the same fragment, and then one of the two is a lie.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.media.len() as u64
    }
}

/// One stream's archive.
#[derive(Debug, Clone)]
pub struct ArchivedStream {
    /// The stream name (`StreamName`).
    pub name: String,
    /// The stream ARN (`StreamARN`).
    pub arn: String,
    /// How long the stream retains data. `0` means no retention is configured,
    /// which is what makes the archived-media operations answer
    /// `NoDataRetentionException`.
    pub data_retention_in_hours: u32,
    /// The stream's `MediaType` (e.g. `video/h264`).
    pub media_type: String,
    /// The content type of the archived container, reported by
    /// `GetMediaForFragmentList$ContentType`. Kinesis Video Streams archives
    /// fragments as Matroska (MKV), served as `video/webm`.
    pub archive_content_type: String,
    /// The stream's codec private data, when the producer supplied it.
    pub codec_private_data: Option<Vec<u8>>,
    /// The archive, kept in ingestion order (ascending fragment number).
    pub fragments: Vec<StoredFragment>,
}

/// The content type Kinesis Video Streams archives fragments as: the archive is
/// Matroska, served as `video/webm`.
pub const ARCHIVE_CONTENT_TYPE: &str = "video/webm";

/// The ceiling on the bytes ONE archived-media response may assemble.
///
/// The service publishes this ceiling for the clip it builds itself ("The
/// output will contain the first 100 MB or the first 200 fragments from the
/// specified start timestamp"), so the same number bounds every payload this
/// service assembles. It is a bound on the PRODUCT, not on the request: the
/// modelled list and page limits cap how many fragments a request may name,
/// but a caller may name the largest archived fragment repeatedly, so the
/// bytes must be bounded separately from the count. The limit is published as
/// a round "100 MB" with no unit base, so one is chosen here and named.
pub const MAX_PAYLOAD_BYTES: usize = 100 * 1024 * 1024;

impl ArchivedStream {
    /// A stream with an empty archive, with the ARN shape the model's
    /// `ResourceARN` pattern requires
    /// (`arn:aws:kinesisvideo:<region>:<account>:stream/<name>/<creation epoch seconds>`).
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        region: &str,
        created_at: SystemTime,
        data_retention_in_hours: u32,
    ) -> Self {
        let name = name.into();
        let created_secs = epoch_millis(created_at).max(0) / 1000;
        let arn = ::engine::emulator_arn(
            "kinesisvideo",
            region,
            &format!("stream/{name}/{created_secs}"),
        );
        Self {
            name,
            arn,
            data_retention_in_hours,
            media_type: "video/h264".to_string(),
            archive_content_type: ARCHIVE_CONTENT_TYPE.to_string(),
            codec_private_data: None,
            fragments: Vec::new(),
        }
    }

    /// Whether this stream retains data at all. A stream with no retention
    /// holds no archive, which the archived-media operations report as
    /// `NoDataRetentionException`.
    #[must_use]
    pub fn retains_data(&self) -> bool {
        self.data_retention_in_hours > 0
    }

    /// The fragment with this fragment number, if the archive holds it.
    #[must_use]
    pub fn fragment(&self, fragment_number: &str) -> Option<&StoredFragment> {
        self.fragments
            .iter()
            .find(|f| f.fragment_number == fragment_number)
    }

    /// The archived fragments whose `origin` timestamp falls in the inclusive
    /// range `[start_millis, end_millis]`, in ingestion order.
    #[must_use]
    pub fn fragments_in_range(
        &self,
        origin: TimestampOrigin,
        start_millis: i64,
        end_millis: i64,
    ) -> Vec<&StoredFragment> {
        self.fragments
            .iter()
            .filter(|f| {
                let at = epoch_millis(origin.of(f));
                at >= start_millis && at <= end_millis
            })
            .collect()
    }

    /// The total size of `fragments`, saturating rather than overflowing.
    #[must_use]
    pub fn total_bytes(fragments: &[&StoredFragment]) -> usize {
        fragments
            .iter()
            .fold(0usize, |sum, fragment| {
                sum.saturating_add(fragment.media.len())
            })
    }

    /// The media bytes of `fragments`, concatenated in the order given, or
    /// `Err(total)` when those bytes would exceed [`MAX_PAYLOAD_BYTES`].
    ///
    /// The ceiling is checked BEFORE anything is allocated, so no caller can
    /// materialise an unbounded response: a request names a bounded COUNT of
    /// fragments (the model's list and page limits), but a count says nothing
    /// about bytes, and a list may name one large fragment many times.
    pub fn concatenate(fragments: &[&StoredFragment]) -> Result<Vec<u8>, usize> {
        let total = Self::total_bytes(fragments);
        if total > MAX_PAYLOAD_BYTES {
            return Err(total);
        }
        let mut payload = Vec::with_capacity(total);
        for fragment in fragments {
            payload.extend_from_slice(&fragment.media);
        }
        Ok(payload)
    }
}

/// Which playback protocol a streaming session serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionProtocol {
    /// HTTP Live Streaming, from `GetHLSStreamingSessionURL`.
    Hls,
    /// MPEG-DASH, from `GetDASHStreamingSessionURL`.
    Dash,
}

impl SessionProtocol {
    /// The path segment the session's playlist URL uses.
    #[must_use]
    pub fn url_prefix(self) -> &'static str {
        match self {
            Self::Hls => "hls",
            Self::Dash => "dash",
        }
    }
}

/// A recorded streaming session: what a session URL resolves back to.
#[derive(Debug, Clone)]
pub struct StreamingSession {
    /// The opaque session token carried in the session URL.
    pub token: String,
    /// The name of the stream the session plays.
    pub stream_name: String,
    /// The protocol the session serves.
    pub protocol: SessionProtocol,
    /// The session's `PlaybackMode`.
    pub playback_mode: String,
    /// The session's `ContainerFormat`.
    pub container_format: String,
    /// The session's `DiscontinuityMode`.
    pub discontinuity_mode: String,
    /// The session's `DisplayFragmentTimestamp`.
    pub display_fragment_timestamp: String,
    /// The playlist snapshot: the fragment numbers the session serves, capped
    /// by the request's max-results.
    pub fragment_numbers: Vec<String>,
    /// When the session was created.
    pub created_at: SystemTime,
    /// How long after `created_at` the session URL stays valid.
    pub expires_in_secs: u32,
}

impl StreamingSession {
    /// When this session's URL stops being valid.
    #[must_use]
    pub fn expires_at(&self) -> SystemTime {
        self.created_at + Duration::from_secs(u64::from(self.expires_in_secs))
    }
}

/// Why a `StreamName`/`StreamARN` pair did not name one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamLookupError {
    /// The request did not name exactly one stream: a client error the
    /// archived-media operations report as `InvalidArgumentException` (400).
    InvalidArgument(String),
    /// Exactly one of the two was given, and no stream answers to it: reported
    /// as `ResourceNotFoundException` (404).
    NotFound(String),
}

/// A pagination cursor over one stream's archive, carried in `NextToken`.
///
/// Self-describing, so a replayed token continues where the previous page
/// stopped without the service holding per-token server state: it carries the
/// stream it was issued for, the selector the first page used (the model makes
/// `FragmentSelector` optional precisely because a continuation re-uses the
/// selector the token remembers) and the offset the next page starts at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentCursor {
    /// The name of the stream the token was issued for.
    pub stream_name: String,
    /// The timestamp field the first page selected on.
    pub origin: TimestampOrigin,
    /// The inclusive start of the selected range.
    pub start_millis: i64,
    /// The inclusive end of the selected range.
    pub end_millis: i64,
    /// How many matching fragments earlier pages already returned.
    pub offset: usize,
}

impl FragmentCursor {
    /// Encode this cursor as a `NextToken`.
    ///
    /// Base64 of the cursor's JSON, which satisfies the model's
    /// `^[a-zA-Z0-9+/]+={0,2}$` token pattern (the standard base64 alphabet
    /// plus at most two `=` of padding).
    #[must_use]
    pub fn encode(&self) -> String {
        let json = ::serde_json::json!({
            "s": self.stream_name,
            "o": self.origin.as_str(),
            "f": self.start_millis,
            "t": self.end_millis,
            "i": self.offset,
        });
        ::aws_smithy_types::base64::encode(json.to_string())
    }

    /// Decode a `NextToken` produced by [`Self::encode`], or `None` when it is
    /// not one.
    #[must_use]
    pub fn decode(token: &str) -> Option<Self> {
        let raw = ::aws_smithy_types::base64::decode(token).ok()?;
        let json: ::serde_json::Value = ::serde_json::from_slice(&raw).ok()?;
        Some(Self {
            stream_name: json.get("s")?.as_str()?.to_string(),
            origin: TimestampOrigin::parse(json.get("o")?.as_str()?)?,
            start_millis: json.get("f")?.as_i64()?,
            end_millis: json.get("t")?.as_i64()?,
            offset: usize::try_from(json.get("i")?.as_u64()?).ok()?,
        })
    }
}

/// The most streaming sessions this service keeps at once.
///
/// A session is minted per request, so the store that holds them needs a
/// ceiling of its own: expiry alone bounds nothing inside one `Expires` window
/// (up to 12 hours). The number is this emulator's own choice — AWS publishes
/// no per-account session cap — and it is well above any playback a caller can
/// have in flight, so reaching it means sessions are being created and
/// abandoned, which is exactly when the oldest is the right one to drop.
pub const MAX_STREAMING_SESSIONS: usize = 1024;

/// Per-service state for com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState {
    /// Every stream's archive, keyed by stream name.
    pub streams: HashMap<String, ArchivedStream>,
    /// `StreamARN` -> stream name, so an ARN and a name resolve to the same
    /// [`ArchivedStream`] rather than to two copies of one stream.
    pub arn_index: HashMap<String, String>,
    /// Every minted streaming session, keyed by its session token.
    pub sessions: HashMap<String, StreamingSession>,
    /// Monotonic counter behind [`Self::mint_session_token`], so two sessions
    /// minted in the same instant still get distinct tokens.
    session_ordinal: u64,
}

impl ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState {
    /// Add (or replace) a stream's archive, keeping [`Self::arn_index`] in step.
    ///
    /// Replacing a stream drops the ARN the previous entry was indexed under:
    /// a re-created stream has a new ARN (its creation instant is part of it),
    /// and a stale ARN that still resolved to the live stream would answer a
    /// request that named a stream which no longer exists.
    pub fn put_stream(&mut self, stream: ArchivedStream) {
        if let Some(replaced) = self.streams.get(&stream.name) {
            if replaced.arn != stream.arn {
                self.arn_index.remove(&replaced.arn);
            }
        }
        self.arn_index
            .insert(stream.arn.clone(), stream.name.clone());
        self.streams.insert(stream.name.clone(), stream);
    }

    /// Append one ingested fragment to a stream's archive, keeping it in
    /// ascending fragment-number order. Answers `false` when no such stream
    /// exists.
    ///
    /// The archive is already ordered, so the insertion point is found by
    /// binary search rather than by re-sorting the whole archive per ingest.
    pub fn append_fragment(&mut self, stream_name: &str, fragment: StoredFragment) -> bool {
        let Some(stream) = self.streams.get_mut(stream_name) else {
            return false;
        };
        let at = stream
            .fragments
            .binary_search_by(|held| {
                numeric_order(&held.fragment_number, &fragment.fragment_number)
            })
            .unwrap_or_else(|insertion_point| insertion_point);
        stream.fragments.insert(at, fragment);
        true
    }

    /// Resolve the one stream a request names.
    ///
    /// The model documents `StreamName` and `StreamARN` as "Specify either this
    /// parameter or the StreamName parameter", so exactly one must be present:
    /// neither and both are client errors, and a name or ARN that answers to no
    /// stream is a missing resource.
    pub fn resolve_stream(
        &self,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
    ) -> Result<&ArchivedStream, StreamLookupError> {
        match (stream_name, stream_arn) {
            (Some(_), Some(_)) => Err(StreamLookupError::InvalidArgument(
                "StreamName and StreamARN are mutually exclusive: specify exactly one".to_string(),
            )),
            (None, None) => Err(StreamLookupError::InvalidArgument(
                "Either StreamName or StreamARN is required".to_string(),
            )),
            (Some(name), None) => self.streams.get(name).ok_or_else(|| {
                StreamLookupError::NotFound(format!("No such stream: StreamName {name}"))
            }),
            (None, Some(arn)) => self
                .arn_index
                .get(arn)
                .and_then(|name| self.streams.get(name))
                .ok_or_else(|| {
                    StreamLookupError::NotFound(format!("No such stream: StreamARN {arn}"))
                }),
        }
    }

    /// Mint a fresh, unique streaming-session token.
    ///
    /// Opaque and URL-safe: a monotonic ordinal and the creation instant, so
    /// two sessions for the same stream and the same parameters never share a
    /// token.
    pub fn mint_session_token(&mut self, created_at: SystemTime) -> String {
        self.session_ordinal = self.session_ordinal.wrapping_add(1);
        let nanos = created_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{:016x}{:032x}", self.session_ordinal, nanos)
    }

    /// Record a minted session, so the URL it is reported in resolves back to
    /// the session's own parameters and playlist snapshot.
    ///
    /// The store is BOUNDED here, at the one site that grows it, because a
    /// session is minted per request:
    ///
    /// * every session whose `Expires` window the clock has already closed is
    ///   dropped — the session record carries its own TTL
    ///   ([`StreamingSession::expires_at`]), and an expired playback session
    ///   can no longer be used ("When a session expires, no new calls to
    ///   GetHLSMasterPlaylist ... can be made for that session"), so removing
    ///   it changes nothing a caller could still observe;
    /// * and if live sessions alone reach [`MAX_STREAMING_SESSIONS`], the
    ///   oldest is evicted to make room, so the map can never hold more than
    ///   that many entries however many requests arrive.
    ///
    /// Every operation that mints a session — HLS now, DASH when it is
    /// implemented — records it through this one method, so the bound covers
    /// them all.
    pub fn record_session(&mut self, session: StreamingSession) {
        let now = session.created_at;
        self.sessions.retain(|_, held| held.expires_at() > now);
        while self.sessions.len() >= MAX_STREAMING_SESSIONS {
            let oldest = self
                .sessions
                .values()
                .min_by_key(|held| held.created_at)
                .map(|held| held.token.clone());
            match oldest {
                Some(token) => {
                    self.sessions.remove(&token);
                }
                None => break,
            }
        }
        self.sessions.insert(session.token.clone(), session);
    }
}

/// Order two fragment numbers the way ingestion order does: numerically when
/// both are decimal (the model's `^[0-9]+$`), lexicographically otherwise, so a
/// non-decimal number still sorts deterministically instead of panicking.
fn numeric_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<u128>(), b.parse::<u128>()) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState>)
}
