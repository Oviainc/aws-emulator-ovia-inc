// SPDX-License-Identifier: CC0-1.0

//! Reading a Prometheus remote-write request: the wire format a remote_write client
//! sends, and the bounds this emulator reads it under.
//!
//! # The format
//!
//! Remote write 1.0 states the request: the body is a `prometheus.WriteRequest` protobuf
//! message, compressed with Snappy's block format, announced `Content-Encoding: snappy`
//! and `Content-Type: application/x-protobuf`. The four messages below are that format's
//! own definitions, declared with `prost`'s derive (the field numbers ARE the wire
//! contract, so they are written out rather than regenerated from a `.proto` at build
//! time):
//!
//! ```text
//! message WriteRequest { repeated TimeSeries timeseries = 1; }
//! message TimeSeries   { repeated Label labels = 1; repeated Sample samples = 2; }
//! message Label        { string name = 1; string value = 2; }
//! message Sample       { double value = 1; int64 timestamp = 2; }
//! ```
//!
//! The format's later members (`metadata`, exemplars, the 2.0 symbol table) are not
//! declared, which is exactly what protobuf's unknown-field rule asks for: a sender that
//! carries them is read for the series and samples it also carries, and nothing is
//! invented for what is skipped.
//!
//! # The bounds
//!
//! A request is a caller-supplied compressed document, so every step is bounded BEFORE
//! the work proportional to it runs:
//!
//! * the compressed body against [`MAX_COMPRESSED_BYTES`];
//! * the length the snappy frame DECLARES against [`MAX_DECOMPRESSED_BYTES`], read from
//!   the frame's header before a byte is decompressed, so a small body cannot ask for a
//!   large allocation;
//! * the series and samples one request carries against [`MAX_SERIES_PER_REQUEST`] and
//!   [`MAX_SAMPLES_PER_REQUEST`], which is also what bounds the work the write then does
//!   while the store is held.
//!
//! The decompression and the decode need no state, so the plane runs them on the blocking
//! pool before it takes the store's lock.

use std::collections::BTreeMap;

use prost::Message;

use crate::state::{Sample, TimeSeries};

/// The largest remote-write body this plane reads.
///
/// The EMULATOR's own bound: Amazon Managed Service for Prometheus publishes no request
/// size for remote write, and a real agent's batch is far below this. Judged on the body
/// that arrived, which the framework has already materialised, so what it bounds is the
/// decompression and the store rather than the buffer.
pub const MAX_COMPRESSED_BYTES: usize = 8 << 20;

/// The largest decompressed `WriteRequest` this plane reads, judged against the length
/// the snappy frame declares before anything is decompressed — so a body that declares
/// more than this is refused instead of allocated for.
pub const MAX_DECOMPRESSED_BYTES: usize = 64 << 20;

/// How many series one request may carry.
///
/// The EMULATOR's own bound, like [`MAX_COMPRESSED_BYTES`]: it bounds the work one
/// request asks of the store, separately from how many series a workspace HOLDS
/// ([`crate::state::MAX_ACTIVE_SERIES`]).
pub const MAX_SERIES_PER_REQUEST: usize = 10_000;

/// How many samples one request may carry, counted across its series — the same kind of
/// bound as [`MAX_SERIES_PER_REQUEST`], and the one that caps the work a single write
/// does while the store is held. Well above a real agent's batch: Prometheus's own
/// remote-write default sends at most 2,000 samples per request.
pub const MAX_SAMPLES_PER_REQUEST: usize = 20_000;

/// The `Content-Encoding` a remote-write body is compressed with.
pub const SNAPPY_ENCODING: &str = "snappy";

/// The `Content-Type` a remote-write body announces.
pub const PROTOBUF_CONTENT_TYPE: &str = "application/x-protobuf";

/// `prometheus.WriteRequest`: the series a remote-write request carries.
#[derive(Clone, PartialEq, Message)]
pub struct WriteRequest {
    #[prost(message, repeated, tag = "1")]
    pub timeseries: Vec<WireTimeSeries>,
}

/// `prometheus.TimeSeries`: one series' labels and the samples written to it.
#[derive(Clone, PartialEq, Message)]
pub struct WireTimeSeries {
    #[prost(message, repeated, tag = "1")]
    pub labels: Vec<WireLabel>,
    #[prost(message, repeated, tag = "2")]
    pub samples: Vec<WireSample>,
}

/// `prometheus.Label`: one label of a series.
#[derive(Clone, PartialEq, Message)]
pub struct WireLabel {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

/// `prometheus.Sample`: one value at one millisecond timestamp.
#[derive(Clone, PartialEq, Message)]
pub struct WireSample {
    #[prost(double, tag = "1")]
    pub value: f64,
    #[prost(int64, tag = "2")]
    pub timestamp: i64,
}

/// Why a remote-write body could not be read at all. Every arm is a `400`: the sender
/// has to fix the request, and the message carries nothing from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The body is larger than [`MAX_COMPRESSED_BYTES`], or declares a decompressed
    /// length larger than [`MAX_DECOMPRESSED_BYTES`].
    TooLarge,
    /// The body is not a snappy block.
    NotSnappy,
    /// The decompressed bytes are not a `WriteRequest`.
    NotProtobuf,
    /// The request carries more series or samples than one request may
    /// ([`MAX_SERIES_PER_REQUEST`], [`MAX_SAMPLES_PER_REQUEST`]).
    TooManySamples,
}

impl DecodeError {
    /// What the refusal says, worded as Prometheus words the same failures.
    pub fn message(&self) -> &'static str {
        match self {
            Self::TooLarge => "the remote write request is larger than this emulator reads",
            Self::NotSnappy => "snappy decode error",
            Self::NotProtobuf => "unable to decode the remote write request",
            Self::TooManySamples => {
                "the remote write request carries more samples than this emulator reads"
            }
        }
    }
}

/// Read a remote-write body into the series it carries.
///
/// `compressed` says whether the request announced snappy; an uncompressed body is read
/// as the protobuf message itself, which is what the format's `Content-Encoding` member
/// distinguishes.
///
/// A series' labels become a sorted label set, so two writes of the same labels in
/// different orders land on one series. A series carrying no sample is dropped: there is
/// nothing to write for it, and it must not create a series by itself.
pub fn read_write_request(
    body: &[u8],
    compressed: bool,
) -> Result<Vec<TimeSeries>, DecodeError> {
    if body.len() > MAX_COMPRESSED_BYTES {
        return Err(DecodeError::TooLarge);
    }
    let decoded = if compressed {
        // The frame's declared length is read first: `decompress_vec` allocates it, so a
        // body that declares more than this plane reads is refused before anything is
        // allocated for it.
        let declared = snap::raw::decompress_len(body).map_err(|_| DecodeError::NotSnappy)?;
        if declared > MAX_DECOMPRESSED_BYTES {
            return Err(DecodeError::TooLarge);
        }
        snap::raw::Decoder::new()
            .decompress_vec(body)
            .map_err(|_| DecodeError::NotSnappy)?
    } else {
        if body.len() > MAX_DECOMPRESSED_BYTES {
            return Err(DecodeError::TooLarge);
        }
        body.to_vec()
    };
    let request = WriteRequest::decode(&decoded[..]).map_err(|_| DecodeError::NotProtobuf)?;
    if request.timeseries.len() > MAX_SERIES_PER_REQUEST {
        return Err(DecodeError::TooManySamples);
    }
    let samples: usize = request
        .timeseries
        .iter()
        .map(|series| series.samples.len())
        .sum();
    if samples > MAX_SAMPLES_PER_REQUEST {
        return Err(DecodeError::TooManySamples);
    }
    Ok(request
        .timeseries
        .into_iter()
        .filter(|series| !series.samples.is_empty())
        .map(|series| TimeSeries {
            labels: series
                .labels
                .into_iter()
                .map(|label| (label.name, label.value))
                .collect::<BTreeMap<String, String>>(),
            samples: series
                .samples
                .into_iter()
                .map(|sample| Sample {
                    timestamp_ms: sample.timestamp,
                    value: sample.value,
                })
                .collect(),
        })
        .collect())
}
