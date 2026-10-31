// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisvideomedia-awsacuityinletservice service.
//!
//! The service's whole surface is one operation, `GetMedia`, which READS the chunks a
//! producer previously wrote to a Kinesis video stream. So the durable thing this
//! service owns is the per-stream fragment log, plus the continuation cursors a
//! `GetMedia` response hands back so the next call resumes exactly where the last one
//! stopped.
//!
//! The STREAM itself is not this service's resource: a Kinesis video stream is created
//! by the Kinesis Video Streams control plane (`CreateStream`), reported by it
//! (`DescribeStream`) and removed by it (`DeleteStream`), and a data plane reached
//! through `GetDataEndpoint` reads THAT stream. So a request's `StreamName` /
//! `StreamARN` is resolved in that service's registry in the shared engine
//! ([`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::resolve_stream`]), whose
//! record decides the stream's identity — which is what makes a just-created stream
//! readable here (with an empty payload until something is produced to it), a removed
//! one a missing resource, and a stream created again under a removed one's name a
//! stream with no media of its own.
//!
//! The producer side of the media itself (`PutMedia`) is not modelled by any crate in
//! this emulator yet, so today the only writer of this log is
//! [`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::put_fragment`] — the one
//! entry point a producer path (or a test seeding a stream) calls. `GetMedia` itself
//! never appends a fragment and never creates a stream.

use std::collections::HashMap;
use std::time::SystemTime;

use ::service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::state as control_plane;

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
    /// The `StreamARN` of the stream whose media this is, and the key the log is held
    /// under: the identity the control plane minted for THIS incarnation of the stream.
    pub arn: String,
    /// The `StreamName` that stream is registered under. Carried for attribution only;
    /// a request is resolved through the registry, never through this field.
    pub name: String,
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
    /// The `StreamARN` of the stream the token was minted for. A token presented
    /// against another stream does not resolve.
    pub stream_arn: String,
    /// Index into [`MediaStream::fragments`] of the first fragment still to deliver.
    pub next_fragment_index: usize,
}

/// Per-service state for com-amazonaws-kinesisvideomedia-awsacuityinletservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisvideomediaAwsacuityinletserviceState {
    /// Per-stream fragment logs, keyed by the `StreamARN` the control plane minted for
    /// the stream. NOT a stream registry: an entry here is the media this service holds
    /// for a stream, and a stream with no entry is one nothing has been produced to —
    /// not a stream that does not exist.
    ///
    /// Keyed by the ARN rather than by the `StreamName`, because a name is reusable
    /// while the ARN carries the stream's creation instant: a stream deleted and
    /// created again under one name is a different stream, and must not inherit the
    /// previous incarnation's media or its cursors.
    pub streams: HashMap<String, MediaStream>,
    /// Continuation token -> cursor, so a token this service minted is resolved from
    /// persisted state rather than parsed back out of the token text.
    pub continuations: HashMap<String, Continuation>,
    /// The last log id handed out; `0` means none has been, so an id is always at least
    /// `1` and never collides with the "no log yet" id a log-less stream reports.
    next_stream_id: u64,
}

impl ComAmazonawsKinesisvideomediaAwsacuityinletserviceState {
    /// The media log of the registered stream `stream_arn` identifies, created empty
    /// when this is the first time it is named.
    ///
    /// Private: a log is not a stream. A stream EXISTS because the control plane
    /// registered it, so nothing outside this store creates a log — the one writer is
    /// [`put_fragment`](Self::put_fragment), which resolves the registered stream
    /// first.
    fn log_mut(&mut self, stream_arn: &str, stream_name: &str) -> &mut MediaStream {
        // Ids are handed out from 1, so the `0` a stream with no log reports can never
        // be a live log's id, and two streams never share a continuation-token prefix.
        let fresh_id = self.next_stream_id.saturating_add(1);
        if !self.streams.contains_key(stream_arn) {
            self.next_stream_id = fresh_id;
        }
        self.streams
            .entry(stream_arn.to_string())
            .or_insert_with(|| MediaStream {
                arn: stream_arn.to_string(),
                name: stream_name.to_string(),
                content_type: DEFAULT_CONTENT_TYPE.to_string(),
                fragments: Vec::new(),
                next_fragment_number: 1,
                id: fresh_id,
            })
    }

    /// Append `chunk` to the media log of the stream `stream_name`/`stream_arn` names,
    /// and answer the fragment number it was stored under.
    ///
    /// This is the producer-side entry point: the writer of the fragment log. The
    /// stream it addresses is resolved in the control-plane registry exactly as a read
    /// resolves it, so media is only ever written to a stream that EXISTS, under the
    /// identity that stream is registered with. `None` when the registry holds no such
    /// stream — the producer side of `ResourceNotFoundException`.
    pub fn put_fragment<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
        chunk: Vec<u8>,
        producer_timestamp: SystemTime,
        server_timestamp: SystemTime,
    ) -> Option<String> {
        let (name, arn) = registered_stream(engine, stream_name, stream_arn)?;
        let stream = self.log_mut(&arn, &name);
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

    /// The media log held for the stream `arn` identifies, or `None` when nothing has
    /// been produced to that stream.
    pub fn log_by_arn(&self, arn: &str) -> Option<&MediaStream> {
        self.streams.get(arn)
    }

    /// The one stream a `GetMedia` request addresses.
    ///
    /// A stream EXISTS because the Kinesis Video Streams control plane created it, so
    /// the identifier is resolved in that service's registry in the shared engine, and
    /// its record is what the stream's name and ARN are read from — so both planes
    /// answer to the one identity, a stream created a moment ago is readable here, and
    /// a stream the control plane no longer holds is not. The FRAGMENTS are this
    /// service's own log for that stream, empty for a registered stream nothing has
    /// been produced to, and that emptiness is what an empty payload is computed from.
    ///
    /// `None` when the registry holds no stream for this identifier — the one condition
    /// `ResourceNotFoundException` documents ("The stream with the given name does not
    /// exist").
    ///
    /// The control-plane store is locked while this service's own store is already
    /// locked (the caller holds that guard); the control plane never reads this
    /// service's state, so there is no cycle between the two.
    pub fn resolve_stream<Eng: Send + Sync + 'static>(
        &self,
        engine: &Eng,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
    ) -> Option<ResolvedStream<'_>> {
        let (name, arn) = registered_stream(engine, stream_name, stream_arn)?;
        let log = self.streams.get(&arn);
        Some(ResolvedStream {
            name,
            arn,
            content_type: log.map_or_else(
                || DEFAULT_CONTENT_TYPE.to_string(),
                |held| held.content_type.clone(),
            ),
            id: log.map_or(0, |held| held.id),
            fragments: log.map_or(&[][..], |held| held.fragments.as_slice()),
        })
    }

    /// The token that resumes the stream `stream_arn` identifies at
    /// `next_fragment_index`, recording the cursor so the next request resolves it from
    /// state.
    ///
    /// Derived from the cursor (and the stream's own [`MediaStream::id`], which the
    /// caller has because it resolved the stream) rather than drawn at random, so
    /// re-reading the same fragments answers the same token instead of growing the map
    /// on every call. The text satisfies the modelled `ContinuationToken`
    /// (`^[a-zA-Z0-9_\.\-]+$`, 1..128).
    pub fn record_continuation(
        &mut self,
        stream_arn: &str,
        stream_id: u64,
        next_fragment_index: usize,
    ) -> String {
        // A cursor past the end of the log names no chunk, so never record one: the
        // caller resolved `next_fragment_index` against this same store, and the clamp
        // keeps a future caller from minting a token that resumes nowhere. A stream
        // this store holds no log for cannot be clamped against, and a cursor recorded
        // for one is caught instead when the token is resolved (see
        // [`continuation`](Self::continuation)).
        let next_fragment_index = self
            .streams
            .get(stream_arn)
            .map_or(next_fragment_index, |stream| {
                next_fragment_index.min(stream.fragments.len())
            });
        let token = format!("kvm-{stream_id}-{next_fragment_index}");
        self.continuations
            .entry(token.clone())
            .or_insert_with(|| Continuation {
                stream_arn: stream_arn.to_string(),
                next_fragment_index,
            });
        token
    }

    /// The cursor a previously minted token resumes at ON the stream `stream_arn`
    /// identifies, whose log holds `fragment_count` fragments.
    ///
    /// `None` when the token was never minted here, was minted for another stream, or
    /// names a position outside that stream's log: a cursor is an index into one
    /// stream's fragment log, so both bindings are enforced here rather than left to
    /// each caller. A cursor exactly at the end of the log is live — it is where the
    /// next fragment will land, so it resumes with an empty payload.
    pub fn continuation(
        &self,
        token: &str,
        stream_arn: &str,
        fragment_count: usize,
    ) -> Option<&Continuation> {
        let continuation = self.continuations.get(token)?;
        if continuation.stream_arn != stream_arn {
            return None;
        }
        (continuation.next_fragment_index <= fragment_count).then_some(continuation)
    }
}

/// The one stream a `GetMedia` request addresses: its IDENTITY as the Kinesis Video
/// Streams control plane holds it, with the fragments this service owns for it.
///
/// Read through
/// [`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::resolve_stream`] on every
/// call, so a stream the control plane created or removed since the last read is
/// observed by the next one rather than by a copy taken earlier.
#[derive(Debug)]
pub struct ResolvedStream<'a> {
    /// The stream's `StreamName`.
    pub name: String,
    /// The stream's `StreamARN`, which is also the key its media log is held under.
    pub arn: String,
    /// The content type `GetMedia` reports for this stream's media.
    pub content_type: String,
    /// The short, stable id this stream's continuation tokens are built from; `0` for a
    /// stream with no log yet, which carries no fragment and so mints no token (a live
    /// log's id is always at least `1`).
    pub id: u64,
    /// This stream's fragment log, oldest first; empty for a registered stream nothing
    /// has been produced to.
    pub fragments: &'a [Fragment],
}

/// The `StreamName` and `StreamARN` the Kinesis Video Streams control plane registry
/// holds for this identifier pair, or `None` when it holds no such stream.
///
/// The registry is that service's state in the SHARED engine, so a stream a client
/// created through `CreateStream` is the very record read here — no copy of it is kept
/// on this side.
///
/// The registry's clock is settled first, so a stream whose removal the control plane
/// already owed (`DeleteStream` fixes that instant) is gone here too: that removal was
/// due at the same instant whether or not anybody called, so it changes nothing a later
/// caller would not have seen anyway.
///
/// The record's `Status` is not read: a stream is resolved whatever status the control
/// plane holds it in, so whether a read of a stream that is still creating (or already
/// marked for deletion) should be refused belongs with the stream-state work on the
/// control plane, not here.
fn registered_stream<Eng: Send + Sync + 'static>(
    engine: &Eng,
    stream_name: Option<&str>,
    stream_arn: Option<&str>,
) -> Option<(String, String)> {
    let registry = control_plane::service_state(engine)?;
    let mut registry = registry.lock();
    registry.settle_clock();
    let stream = registry.resolve(stream_name, stream_arn).ok()?;
    Some((stream.stream_name.clone(), stream.stream_arn.clone()))
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
