// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisvideomedia-awsacuityinletservice service.
//!
//! The service's whole surface is one operation, `GetMedia`, which READS the chunks a
//! producer previously wrote to a Kinesis video stream. So the durable thing this
//! service owns is the per-stream fragment log, plus the continuation cursors a
//! `GetMedia` response hands back so the next call resumes exactly where the last one
//! stopped.
//!
//! The producer side of the same data (`PutMedia`) belongs to the sibling
//! `kinesisvideo` service, whose handlers are all still scaffolded, so today the only
//! writer of this log is [`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::put_fragment`]
//! — the one entry point a producer path (or a test seeding a stream) calls. `GetMedia`
//! itself never appends a fragment and never creates a stream.

use std::collections::HashMap;
use std::time::SystemTime;

/// The content type a Kinesis video stream reports when nothing else is known about
/// its media: Kinesis Video Streams stores producer fragments as Matroska, and
/// `GetMedia` answers `Content-Type: video/webm` for such a stream.
pub const DEFAULT_CONTENT_TYPE: &str = "video/webm";

/// One persisted fragment — what the service documentation calls a "chunk": the media
/// bytes the producer wrote plus the metadata Kinesis Video Streams records alongside
/// them.
#[derive(Debug, Clone)]
pub struct Fragment {
    /// The stream-scoped fragment number, a decimal string (the modelled
    /// `FragmentNumberString` is `^[0-9]+$`, 1..128), increasing with write order.
    pub fragment_number: String,
    /// When the producer says the fragment was captured.
    pub producer_timestamp: SystemTime,
    /// When this service accepted the fragment.
    pub server_timestamp: SystemTime,
    /// The opaque media bytes of the chunk, exactly as the producer wrote them.
    pub chunk: Vec<u8>,
}

/// A stream's persisted media log.
#[derive(Debug, Clone)]
pub struct MediaStream {
    /// The stream name a request may name directly.
    pub name: String,
    /// The stream ARN that resolves to this same stream.
    pub arn: String,
    /// The content type `GetMedia` reports for this stream's media.
    pub content_type: String,
    /// Every fragment ever written, oldest first. `GetMedia` emits a contiguous run of
    /// these, so the index of a fragment in this vector IS the stream cursor.
    pub fragments: Vec<Fragment>,
    /// The number the next fragment written to this stream gets.
    pub next_fragment_number: u64,
    /// A short, stable id used to build this stream's continuation tokens. The stream
    /// NAME cannot be used for that: a name is up to 256 characters while a
    /// `ContinuationToken` is at most 128.
    pub id: u64,
}

/// Where a continuation token resumes: the stream it was minted for, and the index of
/// the first fragment the next call must emit.
#[derive(Debug, Clone)]
pub struct Continuation {
    /// The stream the token was minted for. A token presented against another stream
    /// does not resolve.
    pub stream_name: String,
    /// Index into [`MediaStream::fragments`] of the first fragment still to deliver.
    pub next_fragment_index: usize,
}

/// Per-service state for com-amazonaws-kinesisvideomedia-awsacuityinletservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisvideomediaAwsacuityinletserviceState {
    /// Streams by name.
    pub streams: HashMap<String, MediaStream>,
    /// Stream ARN -> stream name, so either identifier a request may carry resolves to
    /// the same stream.
    pub arn_index: HashMap<String, String>,
    /// Continuation token -> cursor, so a token this service minted is resolved from
    /// persisted state rather than parsed back out of the token text.
    pub continuations: HashMap<String, Continuation>,
    /// The id the next stream created here gets.
    next_stream_id: u64,
}

impl ComAmazonawsKinesisvideomediaAwsacuityinletserviceState {
    /// The stream called `stream_name`, created empty (with its ARN registered) when
    /// this is the first time it is named.
    ///
    /// A stream exists before any fragment is written to it — that is what makes an
    /// empty stream a `GetMedia` with an empty payload rather than a 404. `created_at`
    /// is used only on creation, and `region` only to build the ARN there, but both
    /// identifiers are checked on every call so a name or region the model cannot
    /// express never reaches the log.
    ///
    /// `None` when `stream_name` is not a name the service accepts (see
    /// [`is_valid_stream_name`]) or `region` is not a region an ARN can carry (see
    /// [`is_valid_region`]): a producer naming a stream the model cannot express is
    /// refused, so no stream here can carry an ARN the modelled `ResourceARN` pattern
    /// would reject.
    pub fn ensure_stream(
        &mut self,
        stream_name: &str,
        region: &str,
        created_at: SystemTime,
    ) -> Option<&mut MediaStream> {
        if !is_valid_stream_name(stream_name) || !is_valid_region(region) {
            return None;
        }
        if !self.streams.contains_key(stream_name) {
            let id = self.next_stream_id;
            self.next_stream_id += 1;
            // The modelled `ResourceARN` is
            // `^arn:aws:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`:
            // the trailing segment is the stream's creation time, which is what makes
            // two streams that reuse one name distinguishable.
            let created_millis = epoch_millis(created_at).max(0);
            let arn = ::engine::emulator_arn(
                "kinesisvideo",
                region,
                &format!("stream/{stream_name}/{created_millis}"),
            );
            self.arn_index.insert(arn.clone(), stream_name.to_string());
            self.streams.insert(
                stream_name.to_string(),
                MediaStream {
                    name: stream_name.to_string(),
                    arn,
                    content_type: DEFAULT_CONTENT_TYPE.to_string(),
                    fragments: Vec::new(),
                    next_fragment_number: 1,
                    id,
                },
            );
        }
        self.streams.get_mut(stream_name)
    }

    /// Append `chunk` to `stream_name`, creating the stream on first write, and answer
    /// the fragment number it was stored under.
    ///
    /// This is the producer-side entry point: the writer of the fragment log. `region`
    /// is the caller's signing region, used only when the stream is created, because
    /// the stream ARN carries it. `None` when the stream name is one the service does
    /// not accept (see [`ensure_stream`](Self::ensure_stream)).
    pub fn put_fragment(
        &mut self,
        stream_name: &str,
        region: &str,
        chunk: Vec<u8>,
        producer_timestamp: SystemTime,
        server_timestamp: SystemTime,
    ) -> Option<String> {
        let stream = self.ensure_stream(stream_name, region, server_timestamp)?;
        let fragment_number = stream.next_fragment_number.to_string();
        stream.next_fragment_number += 1;
        stream.fragments.push(Fragment {
            fragment_number: fragment_number.clone(),
            producer_timestamp,
            server_timestamp,
            chunk,
        });
        Some(fragment_number)
    }

    /// The stream a request named by `StreamName`, or `None` when there is no such
    /// stream — which `GetMedia` answers as `ResourceNotFoundException`.
    pub fn stream_by_name(&self, name: &str) -> Option<&MediaStream> {
        self.streams.get(name)
    }

    /// The stream a request named by `StreamARN`. The ARN index is what makes the two
    /// identifiers resolve to one stream.
    pub fn stream_by_arn(&self, arn: &str) -> Option<&MediaStream> {
        self.streams.get(self.arn_index.get(arn)?)
    }

    /// The token that resumes `stream_name` at `next_fragment_index`, recording the
    /// cursor so the next request resolves it from state.
    ///
    /// Derived from the cursor (and the stream's own [`MediaStream::id`], which the
    /// caller has because it resolved the stream) rather than drawn at random, so
    /// re-reading the same fragments answers the same token instead of growing the map
    /// on every call. The text satisfies the modelled `ContinuationToken`
    /// (`^[a-zA-Z0-9_\.\-]+$`, 1..128).
    pub fn record_continuation(
        &mut self,
        stream_name: &str,
        stream_id: u64,
        next_fragment_index: usize,
    ) -> String {
        // A cursor past the end of the log names no chunk, so never record one: the
        // caller resolved `next_fragment_index` against this same store, and the clamp
        // keeps a future caller from minting a token that resumes nowhere. A stream
        // this store does not hold cannot be clamped against, and a cursor recorded
        // for one is caught instead when the token is resolved (see
        // [`continuation`](Self::continuation)).
        let next_fragment_index = self
            .streams
            .get(stream_name)
            .map_or(next_fragment_index, |stream| {
                next_fragment_index.min(stream.fragments.len())
            });
        let token = format!("kvm-{stream_id}-{next_fragment_index}");
        self.continuations
            .entry(token.clone())
            .or_insert_with(|| Continuation {
                stream_name: stream_name.to_string(),
                next_fragment_index,
            });
        token
    }

    /// The cursor a previously minted token resumes at ON `stream_name`.
    ///
    /// `None` when the token was never minted here, was minted for another stream, or
    /// names a position outside that stream's log: a cursor is an index into one
    /// stream's fragment log, so both bindings are enforced here rather than left to
    /// each caller. A cursor exactly at the end of the log is live — it is where the
    /// next fragment will land, so it resumes with an empty payload.
    pub fn continuation(&self, token: &str, stream_name: &str) -> Option<&Continuation> {
        let continuation = self.continuations.get(token)?;
        if continuation.stream_name != stream_name {
            return None;
        }
        self.streams
            .get(stream_name)
            .is_some_and(|stream| continuation.next_fragment_index <= stream.fragments.len())
            .then_some(continuation)
    }
}

/// Whether `name` is a stream name this service accepts: the modelled `StreamName`
/// (len 1..256, `^[a-zA-Z0-9_.-]+$`). The same charset is what keeps the stream ARN
/// built from it inside the modelled `ResourceARN` pattern.
pub fn is_valid_stream_name(name: &str) -> bool {
    (1..=256).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Whether `region` is one a stream ARN can carry: the region segment of the modelled
/// `ResourceARN` pattern (`[a-z0-9-]+`). A signing region outside it would produce an
/// ARN the model rejects, so a stream is not created for it.
pub fn is_valid_region(region: &str) -> bool {
    !region.is_empty()
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `t` as milliseconds since the Unix epoch (negative before it), the unit Kinesis
/// Video Streams reports its `AWS_KINESISVIDEO_*_TIMESTAMP` MKV tags in.
pub fn epoch_millis(t: SystemTime) -> i64 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisvideomediaAwsacuityinletserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisvideomediaAwsacuityinletserviceState>)
}
