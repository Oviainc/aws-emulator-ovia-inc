// SPDX-License-Identifier: CC0-1.0

//! The data plane `GetDataEndpoint` DISCOVERS is one this emulator serves, and the media
//! reads of that plane are bound on it.
//!
//! The rule under test: **an endpoint an operation hands back for a client to dial is an
//! authority this emulator answers on, with that plane's routes bound there**.
//! `GetDataEndpoint` is the only documented way to find a stream's media planes ("You must
//! first call the GetDataEndpoint API to get an endpoint. Then send the ListFragments
//! requests to this endpoint using the --endpoint-url parameter", `ListFragments`), and the
//! client dials exactly the host it is handed, so an announced address this process does not
//! answer on makes the whole producer/consumer path unreachable however well-formed the
//! value looks.
//!
//! Three crates have to agree for that to hold — the Kinesis Video Streams control plane
//! announces the address, Kinesis Video Media serves `/getMedia` on it and Kinesis Video
//! Archived Media serves `/listFragments` — so the suite lives here. That the announced
//! address is a socket of THIS process, and that what answers there is the emulator's own
//! front door, is pinned in the control plane's own suite
//! (`tests/all/stream_data_endpoint.rs` of the kinesisvideo crate); what this suite adds is
//! the other half: the media and archived-media operations of the plane are served when a
//! call is addressed to the announced authority, exactly as when it is addressed to the
//! endpoint the control call arrived on.
//!
//! The rows differ along what the rule generalises over: which `APIName` was asked for (so
//! which plane's address is announced), which media operation of that plane is then called,
//! and whether the call is addressed to the announced authority or to the control-plane one.

use std::net::{Ipv4Addr, Ipv6Addr, TcpStream};
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call here is signed for.
const REGION: &str = "us-west-2";

/// The control-plane address the calls that discover the data plane arrive on — a bare
/// loopback listener, the form a client on this machine reaches the emulator at.
const CONTROL_AUTHORITY: &str = "127.0.0.1:9297";

/// The stream the suite discovers the planes of.
const STREAM: &str = "discovered-plane-probe";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as the router mounts them, with request authentication off so each call carries a
/// credential scope (which is what names the Region and the service) without a computed
/// signature.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_kinesisvideomedia_awsacuityinletservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// What one call answered: its status and its parsed JSON body.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The error code the envelope names, or `""` for a body that names none.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// POST one restJson1 call to `authority`, signed for `kinesisvideo` in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    authority: &str,
    path: &str,
    body: &Value,
) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{authority}{path}"))
        .header("host", authority)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesisvideo/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// The `DataEndpoint` the control plane announces for `api_name`, asserting the 200.
async fn data_endpoint(router: &MultiServiceRouter, api_name: &str) -> String {
    let answer = call(
        router,
        CONTROL_AUTHORITY,
        "/getDataEndpoint",
        &json!({ "StreamName": STREAM, "APIName": api_name }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {api_name:?}: GetDataEndpoint expected HTTP 200, got {answer:?}"
    );
    answer
        .body
        .get("DataEndpoint")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {api_name:?}: no DataEndpoint in {answer:?}"))
        .to_string()
}

/// The authority of an announced `https://host:port` origin, failing the case when the
/// value carries a path: "The returned endpoint does not have the API name appended. The
/// client needs to add the API name to the returned endpoint."
fn announced_authority(case: &str, endpoint: &str) -> String {
    let authority = endpoint
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("case {case:?}: expected an https origin, got {endpoint:?}"));
    assert!(
        !authority.is_empty()
            && !authority.contains('/')
            && !authority.contains('?')
            && !authority.contains('#'),
        "case {case:?}: expected scheme and host only with no path, got {endpoint:?}"
    );
    authority.to_string()
}

/// A connection to an announced endpoint: what the value exists for, so an address this
/// emulator does not answer on fails here.
///
/// The announced host has to be one of this machine's own loopback addresses — the
/// interface the emulator's core binds its listeners on — and the dial is to that address
/// and no other.
fn assert_dialable(case: &str, authority: &str) {
    let (host, port) = authority.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: expected the announced endpoint to carry its port, got {authority:?}")
    });
    let port = port
        .parse::<u16>()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a port, got {authority:?}"));
    let dialed = match host.trim_start_matches('[').trim_end_matches(']') {
        "127.0.0.1" => TcpStream::connect((Ipv4Addr::LOCALHOST, port)),
        "::1" => TcpStream::connect((Ipv6Addr::LOCALHOST, port)),
        "localhost" => TcpStream::connect(("localhost", port)),
        other => panic!(
            "case {case:?}: the announced endpoint must be an address of this machine, got \
             {other:?} in {authority:?}"
        ),
    };
    assert!(
        dialed.is_ok(),
        "case {case:?}: the announced endpoint {authority:?} must accept a connection, got {:?}",
        dialed.err()
    );
}

/// A fragment selector covering the whole archive, which `ListFragments` requires on a first
/// page ("This is only required when the NextToken isn't passed in the API").
fn whole_archive() -> Value {
    json!({
        "FragmentSelectorType": "SERVER_TIMESTAMP",
        "TimestampRange": { "StartTimestamp": 0, "EndTimestamp": 4_000_000_000_i64 },
    })
}

/// The discovered data plane is reachable, and the media reads of that plane are served when
/// a call is addressed to the authority the control plane announced.
#[tokio::test(flavor = "multi_thread")]
async fn the_announced_data_endpoint_serves_the_media_reads_of_its_plane() {
    let router = emulator();
    let created = call(
        &router,
        CONTROL_AUTHORITY,
        "/createStream",
        &json!({ "StreamName": STREAM, "MediaType": "video/h264" }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateStream expected HTTP 200, got {created:?}"
    );

    // One row per plane the published `APIName` set is partitioned into: the plane media is
    // written to, and the plane it is read back from.
    let write = data_endpoint(&router, "PUT_MEDIA").await;
    let read = data_endpoint(&router, "GET_MEDIA").await;
    let write_authority = announced_authority("PUT_MEDIA", &write);
    let read_authority = announced_authority("GET_MEDIA", &read);
    for (case, authority) in [("PUT_MEDIA", &write_authority), ("GET_MEDIA", &read_authority)] {
        assert_ne!(
            authority, CONTROL_AUTHORITY,
            "case {case:?}: the data authority must not be the control-plane one, got \
             {authority:?}"
        );
        assert_dialable(case, authority);
    }
    assert_ne!(
        write_authority, read_authority,
        "the plane that writes media must not be announced at the address that reads it, got \
         {write_authority:?} and {read_authority:?}"
    );
    // Every API that reads the stream shares the read plane's one address.
    for api_name in ["LIST_FRAGMENTS", "GET_MEDIA_FOR_FRAGMENT_LIST", "GET_CLIP"] {
        let endpoint = data_endpoint(&router, api_name).await;
        assert_eq!(
            endpoint, read,
            "case {api_name:?}: every API that reads the stream resolves to one endpoint, got \
             {endpoint:?}"
        );
    }

    // The media and archived-media operations of the read plane, addressed to the authority
    // the control plane announced for it: each is served by the service that models it, and
    // the same call addressed to the control-plane authority is served the same way — the
    // announced address is an address of this emulator, not a different emulator.
    let reads: &[(&str, &str, Value)] = &[
        (
            "GetMedia on the announced authority",
            "/getMedia",
            json!({ "StreamName": STREAM, "StartSelector": { "StartSelectorType": "EARLIEST" } }),
        ),
        (
            "ListFragments on the announced authority",
            "/listFragments",
            json!({ "StreamName": STREAM, "FragmentSelector": whole_archive() }),
        ),
    ];
    for (case, path, body) in reads {
        for authority in [read_authority.as_str(), CONTROL_AUTHORITY] {
            let answer = call(&router, authority, path, body).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} at {authority:?}: expected the plane's own service to serve it, \
                 got {answer:?}"
            );
            assert_eq!(
                answer.error_code(),
                "",
                "case {case:?} at {authority:?}: expected no refusal, got {answer:?}"
            );
        }
    }

    // And the stream the reads are of is the one the control plane holds: the planes are
    // addresses of one emulator over one store, so the control plane still answers for it.
    let described = call(
        &router,
        CONTROL_AUTHORITY,
        "/describeStream",
        &json!({ "StreamName": STREAM }),
    )
    .await;
    assert_eq!(
        described.status, 200,
        "DescribeStream expected HTTP 200, got {described:?}"
    );
    assert_eq!(
        described.body["StreamInfo"]["StreamName"].as_str(),
        Some(STREAM),
        "DescribeStream must report the stream the planes were discovered for, got {described:?}"
    );
}
