// SPDX-License-Identifier: CC0-1.0

//! Which requests this service claims when its SigV4 signing name is not its alone.
//!
//! Amazon IVS RealTime signs as `ivs` — a signing name it SHARES with the low-latency
//! Amazon IVS service, whose endpoint is `ivs`. The router resolves a request's routing
//! key from its SigV4 credential scope first, so both services see the key `ivs`; a
//! service that claims on the signing name alone claims its sibling's traffic too and the
//! router refuses every `ivs`-signed request as `500 AmbiguousService`.
//!
//! The rule under test, from this service's side of that pair: a request signed for a
//! shared signing name is claimed only when this service's OWN model binds the request's
//! URI path, and — on the `/tags/{resourceArn}` path space both models bind — only when
//! the ARN in that path names one of THIS service's resources.
//!
//! The cross-service half of the rule (that exactly ONE of the two claims every such
//! request) is pinned in `core/router/tests/all/ivs_signing_name_routing.rs`, which is
//! the only place both services can be mounted together.

use std::sync::Arc;

use smithy_server_runtime::Service;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// A SigV4 `Authorization` header whose credential scope names the shared signing name.
/// The signature is never checked here: `can_handle` reads the scope, not the signature.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The request parts the router hands to [`Service::can_handle`].
fn parts(method: &str, uri: &str) -> http::request::Parts {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost:9297")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::new())
        .expect("the test request is well formed");
    request.into_parts().0
}

/// An ARN as a client puts it in the non-greedy `{resourceArn}` path label: its `:` and
/// `/` percent-encoded.
fn label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

#[test]
fn this_service_claims_its_own_request_space_and_no_more() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    // (case, method, uri, whether this service claims it)
    //
    // The operation paths are this model's `smithy.api#http` bindings and the sibling
    // model's (`smithy_query(query="digest", …)` on each). The tagging ARNs are built
    // from the resource segment each model's ARN shapes pin: `stage`,
    // `encoder-configuration`, `storage-configuration`, `composition`, `public-key` and
    // `ingest-configuration` here, `channel`, `stream-key`, `playback-key`,
    // `playback-restriction-policy`, `recording-configuration` and `ad-configuration` on
    // the sibling.
    let rows: Vec<(&str, &str, String, bool)> = vec![
        ("own operation: CreateStage", "POST", "/CreateStage".to_string(), true),
        ("own operation: GetStage", "POST", "/GetStage".to_string(), true),
        ("own operation: ListPublicKeys", "POST", "/ListPublicKeys".to_string(), true),
        (
            "own operation: StartComposition",
            "POST",
            "/StartComposition".to_string(),
            true,
        ),
        (
            "own operation: ListIngestConfigurations",
            "POST",
            "/ListIngestConfigurations".to_string(),
            true,
        ),
        ("sibling operation: CreateChannel", "POST", "/CreateChannel".to_string(), false),
        ("sibling operation: GetChannel", "POST", "/GetChannel".to_string(), false),
        (
            "sibling operation: ListRecordingConfigurations",
            "POST",
            "/ListRecordingConfigurations".to_string(),
            false,
        ),
        ("a path neither model binds", "POST", "/NoSuchOperation".to_string(), false),
        // The shared tagging path space, one row per resource type on either side.
        (
            "tags on a stage",
            "GET",
            format!("/tags/{}", label("arn:aws:ivs:us-east-1:000000000000:stage/s1")),
            true,
        ),
        (
            "tags on an encoder configuration",
            "POST",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-2:210987654321:encoder-configuration/e1")
            ),
            true,
        ),
        (
            "tags on a storage configuration",
            "DELETE",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:storage-configuration/sc1")
            ),
            true,
        ),
        (
            "tags on a composition",
            "GET",
            format!("/tags/{}", label("arn:aws:ivs:us-east-1:000000000000:composition/cp1")),
            true,
        ),
        (
            "tags on a public key",
            "GET",
            format!("/tags/{}", label("arn:aws:ivs:eu-west-1:123456789012:public-key/pk1")),
            true,
        ),
        (
            "tags on an ingest configuration",
            "GET",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:ingest-configuration/i1")
            ),
            true,
        ),
        (
            "tags on a sibling channel",
            "GET",
            format!("/tags/{}", label("arn:aws:ivs:us-east-1:000000000000:channel/c1")),
            false,
        ),
        (
            "tags on a sibling stream key",
            "POST",
            format!("/tags/{}", label("arn:aws:ivs:us-east-1:000000000000:stream-key/k1")),
            false,
        ),
        (
            "tags on a sibling playback key pair",
            "DELETE",
            format!("/tags/{}", label("arn:aws:ivs:us-east-1:000000000000:playback-key/p1")),
            false,
        ),
        (
            "tags on a sibling playback restriction policy",
            "GET",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:playback-restriction-policy/r1")
            ),
            false,
        ),
        (
            "tags on a sibling recording configuration",
            "GET",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:recording-configuration/rc1")
            ),
            false,
        ),
        (
            "tags on a sibling ad configuration",
            "GET",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:ad-configuration/a1")
            ),
            false,
        ),
        // The ARN's separators sent RAW rather than percent-encoded: the resource type
        // decides either way.
        (
            "tags on a stage, raw ARN",
            "GET",
            "/tags/arn:aws:ivs:us-east-1:000000000000:stage/s1".to_string(),
            true,
        ),
        (
            "tags on a sibling channel, raw ARN",
            "GET",
            "/tags/arn:aws:ivs:us-east-1:000000000000:channel/c1".to_string(),
            false,
        ),
        // An ARN neither model recognises is left to the sibling endpoint, whose
        // `endpointPrefix` IS the shared signing name, so exactly one service claims it.
        (
            "tags on an unknown resource type",
            "GET",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:unknown-thing/x1")
            ),
            false,
        ),
        (
            "tags on a stage whose ARN resource type is not canonically cased",
            "GET",
            format!("/tags/{}", label("arn:aws:ivs:us-east-1:000000000000:Stage/s1")),
            true,
        ),
        (
            "tags on another service's ARN whose sixth field looks like a resource type",
            "GET",
            format!("/tags/{}", label("arn:aws:ivschat:us-east-1:000000000000:stage/s1")),
            false,
        ),
        (
            "tags with the path segment itself percent-encoded, which no model binds",
            "GET",
            format!("/%74ags/{}", label("arn:aws:ivs:us-east-1:000000000000:stage/s1")),
            false,
        ),
        ("tags on something that is not an ARN", "GET", "/tags/not-an-arn".to_string(), false),
        ("tags with no resource at all", "GET", "/tags".to_string(), false),
    ];

    for (case, method, uri, expected) in rows {
        let actual = service.can_handle(&parts(method, &uri));
        assert_eq!(
            actual, expected,
            "case {case:?} ({method} {uri}): expected can_handle = {expected}, got {actual}"
        );
    }
}

#[test]
fn a_request_signed_for_another_service_is_never_claimed() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    // The path conjunct narrows the claim; it must never WIDEN it. A request whose
    // credential scope names a different signing name is not this service's, however
    // exactly its path matches — `ivschat` is the neighbour that makes the point: it is
    // also a restJson1 IVS-family service, and it signs as `ivschat`.
    for signing_name in ["ivschat", "s3", ""] {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/us-east-1/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/CreateStage")
            .header("host", "localhost:9297")
            .header("authorization", authorization)
            .body(bytes::Bytes::new())
            .expect("the test request is well formed");
        let (parts, _body) = request.into_parts();
        assert!(
            !service.can_handle(&parts),
            "case {signing_name:?}: a request signed for {signing_name:?} must not be claimed \
             by the IVS RealTime service, got can_handle = true"
        );
    }
}
