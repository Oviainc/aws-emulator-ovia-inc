// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS, and which it must leave to the four peers that
//! share its SigV4 signing name.
//!
//! The sibling suite `route_claims.rs` pins the same rule over one route per peer; this
//! one widens the table to EVERY first path segment the four peer models bind and to the
//! other things the rule generalises over (segment case, later segments, a query string,
//! the routing signal the key is resolved from), and then dispatches through an assembled
//! router so a declined route is seen to reach no service at all.
//!
//! The rule under test: a service whose signing name is shared claims a request only when
//! the request's resolved routing key names that signing name AND the request's FIRST URI
//! path segment is one its own model binds, compared as whole segments. Five emulated
//! Kinesis Video services declare `endpointPrefix`/`sigv4Name` `kinesisvideo`
//! (`smithy_query(query="service_info")` on each model), so claiming on the signing name
//! alone makes all five claim every `kinesisvideo`-signed request and the router answers
//! `500 AmbiguousService` for all of it. This service's model binds exactly one route,
//! `POST /getMedia` (`smithy_query(query="digest")`), so `getMedia` is the only first
//! segment it may claim.
//!
//! The table's rows differ along what the rule generalises over: the first path segment
//! (every segment the other four models bind, taken from their own digests), its CASE, the
//! presence of later segments and of a query string, and the routing signal the key is
//! resolved from (credential scope, then endpoint host). `/getMediaForFragmentList` is the
//! row that pins WHOLE-segment matching: this service's only segment is a PREFIX of it.

use std::sync::Arc;

use bytes::Bytes;
use service_com_amazonaws_kinesisvideomedia_awsacuityinletservice::EmulatedService;
use smithy_server_runtime::{MultiServiceRouter, Service};

/// How a request under test is addressed: what names the service, and at which route.
struct Addressed {
    /// The SigV4 credential scope's service field, when the request carries one.
    signing_name: Option<&'static str>,
    /// The `Host` header, read only when no credential scope resolves a key.
    host: &'static str,
    /// Path (and query) of the request.
    path: &'static str,
}

/// The request `addressed` describes, as the parts `Service::can_handle` is handed.
fn parts(addressed: &Addressed) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{}{}", addressed.host, addressed.path))
        .header("host", addressed.host);
    if let Some(signing_name) = addressed.signing_name {
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    }
    builder
        .body(Bytes::new())
        .expect("the request is well formed")
        .into_parts()
        .0
}

/// A `kinesisvideo`-signed request at `path`, addressed at the emulator's own endpoint.
fn signed(path: &'static str) -> Addressed {
    Addressed {
        signing_name: Some("kinesisvideo"),
        host: "emulator.local",
        path,
    }
}

#[test]
fn only_this_services_own_route_is_claimed_under_the_shared_signing_name() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    // (case, addressed request, must this service claim it).
    //
    // The non-claimed path segments are every first segment the four peer models bind, as
    // their own `smithy_query(query="digest")` lists them: the control plane
    // `createStream`/`describeStream`/`getDataEndpoint`/`listStreams`/`TagResource`/…,
    // Archived Media `getClip`/`getDASHStreamingSessionURL`/`getHLSStreamingSessionURL`/
    // `getImages`/`getMediaForFragmentList`/`listFragments`, WebRTC Storage
    // `joinStorageSession`/`joinStorageSessionAsViewer`, and Signaling `v1` (both of its
    // routes are `/v1/…`).
    let rows: Vec<(&str, Addressed, bool)> = vec![
        ("its own route", signed("/getMedia"), true),
        ("its own route with a query string", signed("/getMedia?x=1"), true),
        (
            "its own route with a trailing slash",
            signed("/getMedia/"),
            true,
        ),
        (
            "its own route with a later segment",
            signed("/getMedia/extra"),
            true,
        ),
        (
            // `resolve_route_key` normalizes the credential scope's service field, so the
            // key matches however the client cased it.
            "its own route signed with an upper-case scope",
            Addressed {
                signing_name: Some("KINESISVIDEO"),
                host: "emulator.local",
                path: "/getMedia",
            },
            true,
        ),
        (
            // No credential scope: the key falls back to the endpoint host's first label,
            // which still names this family.
            "its own route addressed by endpoint host alone",
            Addressed {
                signing_name: None,
                host: "kinesisvideo.us-east-1.amazonaws.com",
                path: "/getMedia",
            },
            true,
        ),
        (
            // THE PREFIX TRAP: `getMedia` is a prefix of `getMediaForFragmentList`, which
            // Archived Media owns. Matching is whole-segment, so this is not claimed.
            "Archived Media's GetMediaForFragmentList",
            signed("/getMediaForFragmentList"),
            false,
        ),
        (
            "Archived Media's ListFragments",
            signed("/listFragments"),
            false,
        ),
        ("Archived Media's GetClip", signed("/getClip"), false),
        (
            "Archived Media's GetHLSStreamingSessionURL",
            signed("/getHLSStreamingSessionURL"),
            false,
        ),
        (
            "Archived Media's GetDASHStreamingSessionURL",
            signed("/getDASHStreamingSessionURL"),
            false,
        ),
        ("Archived Media's GetImages", signed("/getImages"), false),
        (
            "the control plane's CreateStream",
            signed("/createStream"),
            false,
        ),
        (
            "the control plane's DescribeStream",
            signed("/describeStream"),
            false,
        ),
        (
            "the control plane's GetDataEndpoint",
            signed("/getDataEndpoint"),
            false,
        ),
        (
            "the control plane's ListStreams",
            signed("/listStreams"),
            false,
        ),
        (
            // A control-plane route the model spells with a capital.
            "the control plane's TagResource",
            signed("/TagResource"),
            false,
        ),
        (
            "WebRTC Storage's JoinStorageSession",
            signed("/joinStorageSession"),
            false,
        ),
        (
            "WebRTC Storage's JoinStorageSessionAsViewer",
            signed("/joinStorageSessionAsViewer"),
            false,
        ),
        (
            "Signaling's GetIceServerConfig",
            signed("/v1/get-ice-server-config"),
            false,
        ),
        (
            "Signaling's SendAlexaOfferToMaster",
            signed("/v1/send-alexa-offer-to-master"),
            false,
        ),
        (
            // A path segment is compared case-sensitively, as HTTP paths are.
            "its own route in the wrong case",
            signed("/getmedia"),
            false,
        ),
        (
            "its own route capitalised",
            signed("/GetMedia"),
            false,
        ),
        ("the root path", signed("/"), false),
        (
            // The path conjunct never replaces the signing name.
            "its own route signed for another service",
            Addressed {
                signing_name: Some("kinesis"),
                host: "emulator.local",
                path: "/getMedia",
            },
            false,
        ),
        (
            "its own route with no routing signal at all",
            Addressed {
                signing_name: None,
                host: "emulator.local",
                path: "/getMedia",
            },
            false,
        ),
    ];

    for (case, addressed, claimed) in &rows {
        let actual = service.can_handle(&parts(addressed));
        assert_eq!(
            actual, *claimed,
            "case {case:?} ({}): expected this service to {} the request, got claimed={actual}",
            addressed.path,
            if *claimed { "claim" } else { "decline" }
        );
    }
}

/// What a declined request looks like end to end when this service is the only one
/// mounted: the router finds no claimant and answers `404 UnknownService`, never this
/// service's own answer — so a peer's route is free for the peer to serve.
#[tokio::test]
async fn a_peers_route_is_not_served_by_this_service() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));

    // (case, path, is this service's own route).
    let rows: Vec<(&str, &'static str, bool)> = vec![
        ("its own route", "/getMedia", true),
        ("the control plane's CreateStream", "/createStream", false),
        (
            "Archived Media's GetMediaForFragmentList",
            "/getMediaForFragmentList",
            false,
        ),
        ("WebRTC Storage's JoinStorageSession", "/joinStorageSession", false),
        ("Signaling's GetIceServerConfig", "/v1/get-ice-server-config", false),
    ];

    for (case, path, own) in &rows {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://emulator.local{path}"))
            .header("host", "emulator.local")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/kinesisvideo/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("content-type", "application/json")
            .body(Bytes::from_static(
                br#"{"StreamName":"my-stream","StartSelector":{"StartSelectorType":"EARLIEST"}}"#,
            ))
            .expect("the request is well formed");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body()).to_string();

        if *own {
            assert!(
                !body.contains("UnknownService"),
                "case {case:?}: this service must claim its own route, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {case:?}: a peer's route must reach no service here, got {status} {body}"
            );
            assert!(
                body.contains("UnknownService"),
                "case {case:?}: expected 404 UnknownService, got {status} {body}"
            );
        }
    }
}
