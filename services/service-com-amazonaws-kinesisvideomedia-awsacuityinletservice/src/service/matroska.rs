// SPDX-License-Identifier: CC0-1.0

//! The Matroska container a producer submits to the ingest plane, read Cluster by
//! Cluster.
//!
//! Kinesis Video Streams ingests media as Matroska and stores it as fragments, where ONE
//! Cluster is ONE fragment: "When you put media data (fragments) on a stream, Kinesis
//! Video Streams stores each incoming fragment and related metadata in what is called a
//! chunk" (`GetMedia`'s own documentation). So reading a submitted container means
//! finding its Clusters, reading the timecode each one declares and the frames it
//! carries, and giving each Cluster back as a STANDALONE Matroska document — the chunk
//! the read planes serve, which carries the submitted header (the EBML header, the
//! segment information and the track declarations) in front of that one Cluster, so a
//! reader handed the chunk can decode it without the rest of the session.
//!
//! The container is parsed by `webm-iterable`, a Matroska/EBML reader: the element tree,
//! its variable-size integers, its unknown-size master elements and the block header one
//! frame carries are that crate's business, not this emulator's. What this module owns is
//! the Kinesis Video Streams reading of that tree.

use std::io::Cursor;

use ::webm_iterable::matroska_spec::{Master, MatroskaSpec};
use ::webm_iterable::{WebmIterator, WebmWriter};

/// How many nanoseconds one Matroska timecode tick is worth when the container does not
/// declare a `TimestampScale`: the Matroska default of one millisecond.
const DEFAULT_TIMECODE_SCALE_NANOS: u64 = 1_000_000;

/// Nanoseconds in one millisecond — the unit Kinesis Video Streams states fragment
/// timecodes and durations in.
const NANOS_PER_MILLI: u64 = 1_000_000;

/// The most Clusters one submitted container is read for.
///
/// A ceiling on the WORK one ingest request can ask for, not an AWS limit: every Cluster
/// becomes a persisted fragment with a document of its own, so a single request must not
/// be able to grow the store without bound. It is far above any producer's session — the
/// Kinesis Video Streams producers send a few fragments per second and reconnect — so no
/// honest ingest reaches it.
///
/// A container that carries more is not a FAULT and is not reported as one: the Clusters
/// past the ceiling are simply not read, so nothing is acknowledged for them and a
/// producer that reads no acknowledgement for a fragment sends it again on its next
/// session — which is what it already does when a session ends mid-stream.
pub const MAX_CLUSTERS_PER_REQUEST: usize = 1024;

/// The most media bytes one submitted container is read for.
///
/// The second half of the same work ceiling, and the one that bounds the PRODUCT rather
/// than the count: each Cluster's chunk carries the submitted header in front of it, so
/// the bytes a session persists are its Clusters times that header and not the size of
/// the request. A count ceiling alone would admit [`MAX_CLUSTERS_PER_REQUEST`] times
/// whatever one header a producer sends.
///
/// The figure is one second of the media rate `GetMedia`'s documentation states for a
/// Kinesis Video Streams session ("up to 25 megabytes per second"), which is the same
/// number that bounds one read session in [`crate::ops::get_media`]. Clusters past it are
/// left unread, exactly as the count ceiling leaves them.
pub const MAX_INGESTED_BYTES_PER_REQUEST: usize = 25_000_000;

/// One submitted Cluster: one fragment.
#[derive(Debug, Clone)]
pub struct SubmittedCluster {
    /// The Cluster's timecode, exactly as the producer wrote it. This is the value an
    /// acknowledgement reports as `FragmentTimecode`, so it is reported as sent and
    /// never rescaled.
    pub timecode: u64,
    /// The Cluster's timecode in milliseconds, through the container's own
    /// `TimestampScale`. What the fragment's producer timestamp is computed from.
    pub timecode_millis: i64,
    /// The span the Cluster's frames cover, in milliseconds: from its first frame to its
    /// last, as their block timecodes space them.
    pub span_millis: u64,
    /// How many frames (`SimpleBlock`s) the Cluster carries.
    pub frames: usize,
    /// This Cluster as a standalone Matroska document: the submitted header elements
    /// followed by this one Cluster.
    pub document: Vec<u8>,
}

/// A submitted container this module could not read as Matroska.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFault {
    /// The bytes are not a readable Matroska document: the EBML tree does not parse, a
    /// Cluster is truncated or carries no timecode, or a frame's block header does not
    /// read.
    Unreadable,
}

/// Every Cluster of the submitted container, in the order the producer wrote them.
///
/// `Err` when the bytes are not a readable Matroska document: the whole container is
/// then refused, because a reader that cannot find the Clusters cannot say which
/// fragments were submitted.
pub fn read_clusters(body: &[u8]) -> Result<Vec<SubmittedCluster>, ContainerFault> {
    // No bytes at all carry no Matroska document, so there is nothing to read them as.
    // A container that DOES carry a document but no Cluster yet — a producer that has
    // sent only its header — is not a fault: it submitted no fragment, and answers an
    // acknowledgement stream with nothing in it.
    if body.is_empty() {
        return Err(ContainerFault::Unreadable);
    }
    // Elements seen before the first Cluster: the EBML header, the segment information
    // and the track declarations. Each chunk carries them in front of its own Cluster,
    // which is what makes the chunk a document of its own.
    let mut header: Vec<MatroskaSpec> = Vec::new();
    // The Cluster being read, with its own elements, or `None` between Clusters.
    let mut open: Option<Vec<MatroskaSpec>> = None;
    let mut clusters: Vec<SubmittedCluster> = Vec::new();
    let mut scale_nanos = DEFAULT_TIMECODE_SCALE_NANOS;
    // What the Clusters read so far will persist, against the byte half of the work
    // ceiling.
    let mut document_bytes = 0usize;

    for read in WebmIterator::new(Cursor::new(body), &[]) {
        let tag = read.map_err(|_| ContainerFault::Unreadable)?;
        match tag {
            MatroskaSpec::Cluster(Master::Start) => {
                // A Cluster inside a Cluster is not a Matroska document.
                if open.is_some() {
                    return Err(ContainerFault::Unreadable);
                }
                open = Some(vec![MatroskaSpec::Cluster(Master::Start)]);
            }
            MatroskaSpec::Cluster(Master::End) => {
                let mut elements = open.take().ok_or(ContainerFault::Unreadable)?;
                elements.push(MatroskaSpec::Cluster(Master::End));
                let cluster = cluster_of(&header, &elements, scale_nanos)
                    .ok_or(ContainerFault::Unreadable)?;
                document_bytes = document_bytes.saturating_add(cluster.document.len());
                clusters.push(cluster);
                if clusters.len() >= MAX_CLUSTERS_PER_REQUEST
                    || document_bytes >= MAX_INGESTED_BYTES_PER_REQUEST
                {
                    return Ok(clusters);
                }
            }
            other => {
                if let MatroskaSpec::TimestampScale(nanos) = &other {
                    // A zero scale would make every fragment land at the same instant.
                    scale_nanos = (*nanos).max(1);
                }
                match &mut open {
                    Some(elements) => elements.push(other),
                    None => header.push(other),
                }
            }
        }
    }
    // A Cluster whose end never arrived: the session was cut mid-fragment.
    if open.is_some() {
        return Err(ContainerFault::Unreadable);
    }
    Ok(clusters)
}

/// One finished Cluster: what it declares, and the standalone document that carries it.
///
/// `None` when the Cluster declares no timecode (it then names no instant, so nothing
/// can say when its fragment was produced), when a frame's block header does not read,
/// or when the document cannot be written back.
fn cluster_of(
    header: &[MatroskaSpec],
    elements: &[MatroskaSpec],
    scale_nanos: u64,
) -> Option<SubmittedCluster> {
    let mut timecode: Option<u64> = None;
    let mut earliest: Option<i64> = None;
    let mut latest: Option<i64> = None;
    let mut frames = 0usize;
    for element in elements {
        match element {
            MatroskaSpec::Timestamp(ticks) => timecode = Some(*ticks),
            MatroskaSpec::SimpleBlock(_) => {
                let block = ::webm_iterable::matroska_spec::SimpleBlock::try_from(element).ok()?;
                let at = i64::from(block.timestamp);
                frames += 1;
                earliest = Some(earliest.map_or(at, |held: i64| held.min(at)));
                latest = Some(latest.map_or(at, |held: i64| held.max(at)));
            }
            _ => {}
        }
    }
    let timecode = timecode?;
    let span_ticks = latest.unwrap_or(0).saturating_sub(earliest.unwrap_or(0)).max(0);
    Some(SubmittedCluster {
        timecode,
        timecode_millis: ticks_as_millis(timecode, scale_nanos),
        span_millis: u64::try_from(ticks_as_millis(span_ticks.unsigned_abs(), scale_nanos))
            .unwrap_or_default(),
        frames,
        document: standalone_document(header, elements)?,
    })
}

/// `ticks` of the container's timecode scale, in milliseconds.
///
/// `TimestampScale` is "the number of nanoseconds per tick" (Matroska), so a tick is
/// worth `scale_nanos` nanoseconds and the conversion is exact at the millisecond
/// precision Kinesis Video Streams reports.
fn ticks_as_millis(ticks: u64, scale_nanos: u64) -> i64 {
    let nanos = u128::from(ticks).saturating_mul(u128::from(scale_nanos));
    i64::try_from(nanos / u128::from(NANOS_PER_MILLI)).unwrap_or(i64::MAX)
}

/// `elements` as a Matroska document of its own: the submitted `header` elements, then
/// this Cluster, then the end of the segment they sit in.
///
/// The segment end is written only when the header OPENED one, so a container whose
/// Clusters sit at the top level is written back the same way. A producer's segment is
/// usually of unknown size — the session has no end while it is being written — and a
/// chunk does have one, so the element is closed here.
fn standalone_document(header: &[MatroskaSpec], elements: &[MatroskaSpec]) -> Option<Vec<u8>> {
    let open_segment = header
        .iter()
        .any(|element| matches!(element, MatroskaSpec::Segment(Master::Start)));
    let mut writer = WebmWriter::new(Vec::new());
    for element in header.iter().chain(elements.iter()) {
        writer.write(element).ok()?;
    }
    if open_segment {
        writer.write(&MatroskaSpec::Segment(Master::End)).ok()?;
    }
    writer.into_inner().ok()
}
