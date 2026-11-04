// SPDX-License-Identifier: CC0-1.0

//! A stream's DATA-plane endpoints: what `GetDataEndpoint` resolves an `APIName` to.
//!
//! `GetDataEndpoint` exists because a stream's media APIs are NOT served at the
//! control-plane endpoint the call itself is made to — "Gets an endpoint for a
//! specified stream for either reading or writing … The returned endpoint does not
//! have the API name appended" — and because there is more than one such endpoint per
//! stream. Kinesis Video Media models the proof: `InvalidEndpointException` is "Status
//! Code: 400, Caller used wrong endpoint to write data to a stream. On receiving such
//! an exception, the user must call `GetDataEndpoint` with `AccessMode` set to "READ"
//! and use the endpoint Kinesis Video returns in the next `GetMedia` call." An endpoint
//! therefore belongs to ONE access mode: the mode the requested API needs is what
//! selects it, and an endpoint of the other mode does not serve that API.
//!
//! So the value reported for a stream is a bare `https://<host>` base URL — scheme and
//! host only, no path, because the client appends the API name itself — whose single
//! leading label carries the access mode the requested `APIName` needs and a generated
//! id of that mode's host for that stream, under this service's regional DNS suffix.
//! Every API of one access mode resolves to one host, the two modes resolve to
//! different hosts, and neither is the control-plane host the call arrived on.
//!
//! The host, its mode marker and the generated id are per-environment, and nothing
//! here is derived from the request's own address: what decides the value is the
//! `APIName` asked for, the stream named, and the region of the call.
//!
//! # Addressing
//!
//! The reported host is the form Kinesis Video hands out, which is a name the machine
//! this emulator runs on resolves no more than it resolves the service's control-plane
//! host — a client reaches either by directing that name at the emulator, which is what
//! it already does to reach this service at all. Nothing more is needed for the media
//! calls to be served: the emulator's front door resolves a request's target service
//! from its SigV4 signing name and its modelled path, not from the host, so a
//! `POST /putMedia`, `/getMedia`, `/listFragments` … sent to a data host this operation
//! reported reaches the ingest and archived-media services this emulator registers for
//! that signing name, exactly as the same call sent to the control-plane address does.
//! What this value carries is therefore the one thing a client cannot otherwise know:
//! which of a stream's planes its API belongs to, as a host of its own.

use ::sha2::{Digest, Sha256};

/// The access mode a stream's media API needs, and so the data endpoint that serves it.
///
/// The two modes Kinesis Video Media names ("the user must call `GetDataEndpoint` with
/// `AccessMode` set to \"READ\"", said of a write addressed to the wrong endpoint):
/// media is WRITTEN to a stream through one endpoint and READ back through another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataPlane {
    /// The plane media is written to.
    Write,
    /// The plane a stream's media is read from.
    Read,
}

impl DataPlane {
    /// The marker the plane's host carries ahead of its generated id — the form Kinesis
    /// Video hands out, in which the two planes' hosts are told apart by that leading
    /// label. The letters themselves are per-environment; that the two planes differ is
    /// the part that is not.
    pub fn host_marker(self) -> &'static str {
        match self {
            DataPlane::Write => "s",
            DataPlane::Read => "b",
        }
    }
}

/// Which plane serves one `APIName`, or `None` for a value the model does not publish.
///
/// The catalog below is the `APIName` enum
/// (`smithy_query(query="shape", shape="APIName")`, mirrored in
/// [`crate::ops::types::A_P_I_NAME_VALUES`]), each value paired with the access mode the
/// API it names needs: `PUT_MEDIA` writes media onto the stream (Kinesis Video Media
/// `PutMedia`), and every other published value reads the stream — its media
/// (`GetMedia`) or its archive (`ListFragments`, `GetMediaForFragmentList`, `GetClip`,
/// `GetHLSStreamingSessionURL`, `GetDASHStreamingSessionURL`, `GetImages`, all of
/// Kinesis Video Archived Media).
///
/// `None` is what refuses an `APIName` outside that published set, so a value this
/// service cannot place on a plane is answered as a bad argument rather than handed an
/// endpoint chosen for it.
pub fn data_plane_of(api_name: &str) -> Option<DataPlane> {
    const PLANES: &[(&str, DataPlane)] = &[
        ("PUT_MEDIA", DataPlane::Write),
        ("GET_MEDIA", DataPlane::Read),
        ("GET_MEDIA_FOR_FRAGMENT_LIST", DataPlane::Read),
        ("LIST_FRAGMENTS", DataPlane::Read),
        ("GET_CLIP", DataPlane::Read),
        ("GET_HLS_STREAMING_SESSION_URL", DataPlane::Read),
        ("GET_DASH_STREAMING_SESSION_URL", DataPlane::Read),
        ("GET_IMAGES", DataPlane::Read),
    ];
    PLANES
        .iter()
        .find(|(name, _)| *name == api_name)
        .map(|(_, plane)| *plane)
}

/// The base URL of `plane` for the stream `stream_arn` names, in `region`.
///
/// Scheme and host only: "The returned endpoint does not have the API name appended.
/// The client needs to add the API name to the returned endpoint", so a path here would
/// be appended to rather than replaced.
///
/// The host is the plane's marker and the generated id of that plane's host for this
/// stream, one label under [`service_host`] — so the value is the plane's, the stream's
/// and the region's, and is not the authority the request arrived on. It is DERIVED,
/// not drawn: the same plane of the same stream resolves to the same endpoint for as
/// long as the stream exists, so reading it twice reports one address and a client can
/// key a session on it.
pub fn data_endpoint(plane: DataPlane, region: &str, stream_arn: &str) -> String {
    format!(
        "https://{}-{}.{}",
        plane.host_marker(),
        host_label(plane, stream_arn),
        service_host(region)
    )
}

/// This service's regional host: its endpoint prefix, the request's region and the AWS
/// DNS suffix. Every host this service reports is this one or a label under it.
pub fn service_host(region: &str) -> String {
    format!("kinesisvideo.{region}.amazonaws.com")
}

/// How many hexadecimal symbols a generated host id carries.
const HOST_LABEL_LENGTH: usize = 8;

/// The generated id one plane's host carries for one stream.
///
/// A digest of the plane and the stream's ARN — which already holds the account, the
/// region, the stream's name and its creation instant — so it is STABLE for the life of
/// the stream, distinct between two streams and between the two planes of one stream,
/// while revealing no stream name to whoever reads the endpoint. Nothing about the
/// request that asked for it, or about when it was asked, goes in.
fn host_label(plane: DataPlane, stream_arn: &str) -> String {
    let digest = Sha256::digest(format!("{}:{stream_arn}", plane.host_marker()).as_bytes());
    ::hex::encode(digest)
        .chars()
        .take(HOST_LABEL_LENGTH)
        .collect()
}
