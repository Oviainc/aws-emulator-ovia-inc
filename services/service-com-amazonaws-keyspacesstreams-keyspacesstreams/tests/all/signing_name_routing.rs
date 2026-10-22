// SPDX-License-Identifier: CC0-1.0

//! The routing split between the two services that sign as `cassandra`.
//!
//! Amazon Keyspaces (`KeyspacesService.<Op>`, endpoint `cassandra.<region>.…`) and
//! KeyspacesStreams (`KeyspacesStreams.<Op>`, endpoint `cassandra-streams.<region>.…`)
//! share one SigV4 signing name, so the signing name alone cannot say which service a
//! signed request is for — and a request that TWO services claim is answered by
//! neither: the router rejects an ambiguous claim with a 500. This suite mounts both
//! services on one router and pins that every request reaches exactly one of them.
//!
//! It mounts just the two services involved rather than the whole production router,
//! which is what makes it cheap enough to live beside this crate's other suites. The
//! signed requests it builds are also what lets it pin the region a request is scoped
//! to, which only a SigV4 credential scope carries.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_keyspaces_keyspacesservice::EmulatedService as Keyspaces;
use service_com_amazonaws_keyspacesstreams_keyspacesstreams::state::{service_state, ChangeRecord};
use service_com_amazonaws_keyspacesstreams_keyspacesstreams::EmulatedService as KeyspacesStreams;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names `cassandra` — what an
/// AWS SDK sends for BOTH of these services, and the signal that makes the signing name
/// ambiguous. Signature verification is off here, so only the credential scope matters.
const CASSANDRA_SIGV4: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/cassandra/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The same header scoped to a THIRD service. A request signed this way belongs to that
/// service however it is targeted, so neither cassandra service may claim it.
const THIRD_SERVICE_SIGV4: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// Both cassandra-signing services on one router, over one shared engine, with that
/// engine handed back so a test can seed what the services read.
fn both_services() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(KeyspacesStreams::new(Arc::clone(&engine)));
    router.register_service(Keyspaces::new(Arc::clone(&engine)));
    (engine, router)
}

/// A SigV4 `Authorization` header signed for `cassandra` in `region`.
fn cassandra_sigv4_in(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/cassandra/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=00000000000000000000000000000000"
    )
}

/// Dispatch one awsJson request and return its status and body.
async fn dispatch(
    router: &MultiServiceRouter,
    authorization: &str,
    target: &str,
    body: &'static str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://localhost/")
        .header("authorization", authorization)
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", target)
        .body(bytes::Bytes::from_static(body.as_bytes()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// RULE: with both services mounted, a signed request is claimed by exactly the one it
/// names — never by both (500 `AmbiguousService`), and never by the wrong one.
#[tokio::test]
async fn a_signed_request_is_claimed_by_exactly_the_service_it_names() {
    let (_engine, router) = both_services();

    // The streams service answers its own target, and answers it for real: an empty
    // store still lists streams, which no other service's response carries.
    let (status, body) = dispatch(
        &router,
        CASSANDRA_SIGV4,
        "KeyspacesStreams.ListStreams",
        "{}",
    )
    .await;
    assert_eq!(
        status, 200,
        "case KeyspacesStreams.ListStreams: exactly one service must claim it, got {status} {body}"
    );
    assert!(
        body.contains("\"streams\""),
        "case KeyspacesStreams.ListStreams: the streams service must answer it, got {body}"
    );

    // A Keyspaces target reaches Keyspaces: whatever that service answers (it owns its
    // own operations), it must not be refused as an ambiguous claim and must not be
    // answered by the streams service.
    let (status, body) = dispatch(
        &router,
        CASSANDRA_SIGV4,
        "KeyspacesService.ListKeyspaces",
        "{}",
    )
    .await;
    assert_ne!(
        status, 500,
        "case KeyspacesService.ListKeyspaces: exactly one service must claim it, got \
         {status} {body}"
    );
    assert!(
        !body.contains("AmbiguousService") && !body.contains("\"streams\""),
        "case KeyspacesService.ListKeyspaces: Keyspaces must answer it, not the streams \
         service, got {body}"
    );
}

/// RULE: a request whose SigV4 scope names a THIRD service belongs to that service
/// however it is targeted — so with only these two mounted, neither claims it.
#[tokio::test]
async fn a_request_signed_for_another_service_is_claimed_by_neither() {
    let (_engine, router) = both_services();
    let (status, body) = dispatch(
        &router,
        THIRD_SERVICE_SIGV4,
        "KeyspacesStreams.ListStreams",
        "{}",
    )
    .await;
    assert_eq!(
        status, 404,
        "case third-party-signed KeyspacesStreams target: neither cassandra service may \
         claim it, got {status} {body}"
    );
    assert!(
        !body.contains("\"streams\""),
        "case third-party-signed KeyspacesStreams target: the streams service must not \
         answer a request signed for another service, got {body}"
    );
}

/// RULE: the signal that names the service wins in the order
/// [`smithy_server_runtime::resolve_route_key`] reads it — the `X-Amz-Target` prefix
/// before the endpoint host — so the claim follows the target whatever host the
/// request carries, and the host decides only when the target names neither service.
#[tokio::test]
async fn the_target_names_the_service_ahead_of_the_endpoint_host() {
    let (_engine, router) = both_services();

    // (Host header, X-Amz-Target, whether the streams service must answer)
    let cases: &[(&str, &str, bool)] = &[
        (
            "cassandra-streams.us-east-1.amazonaws.com",
            "KeyspacesStreams.ListStreams",
            true,
        ),
        (
            "cassandra.us-east-1.amazonaws.com",
            "KeyspacesStreams.ListStreams",
            true,
        ),
        (
            "cassandra-streams.us-east-1.amazonaws.com",
            "KeyspacesService.ListKeyspaces",
            false,
        ),
    ];
    for &(host, target, streams_answers) in cases {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://localhost/")
            .header("host", host)
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", target)
            .body(bytes::Bytes::from_static(b"{}"))
            .expect("build the request");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body()).to_string();
        assert_ne!(
            status, 500,
            "case host {host} target {target}: exactly one service may claim it, got \
             {status} {body}"
        );
        assert_eq!(
            body.contains("\"streams\""),
            streams_answers,
            "case host {host} target {target}: the streams service must{} answer it, got \
             {status} {body}",
            if streams_answers { "" } else { " not" }
        );
    }
}

/// RULE: a stream belongs to the region of the table it follows, and a request reaches
/// one region's endpoint — so a request signed for another region neither lists nor
/// reads it. A request that names no region (unsigned) is scoped by nothing and sees
/// every stream the store holds.
#[tokio::test]
async fn a_stream_is_visible_only_to_its_own_region() {
    let (engine, router) = both_services();
    // One table with CDC enabled in each of two regions.
    let seeded: Vec<(&str, String)> = ["us-east-1", "eu-west-1"]
        .iter()
        .map(|region| {
            let state = service_state(&*engine).expect("state");
            let mut guard = state.lock();
            let arn = guard
                .enable_stream(region, "shop", "orders", "NEW_IMAGE")
                .expect("NEW_IMAGE is a modelled view type");
            guard.append_change_record(&arn, ChangeRecord::default());
            (*region, arn)
        })
        .collect();

    for (region, arn) in &seeded {
        // Signed for this stream's own region: listed, and readable.
        let (status, body) = dispatch(
            &router,
            &cassandra_sigv4_in(region),
            "KeyspacesStreams.ListStreams",
            "{}",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region} lists its own stream: got {status} {body}"
        );
        assert!(
            body.contains(arn.as_str()),
            "case {region} lists its own stream: {arn} must be listed, got {body}"
        );
        let listed_other = seeded
            .iter()
            .filter(|(other, _)| other != region)
            .all(|(_, other_arn)| !body.contains(other_arn.as_str()));
        assert!(
            listed_other,
            "case {region} lists its own stream: another region's stream must not be \
             listed, got {body}"
        );

        // Signed for another region: the same ARN does not exist there.
        let other_region = if *region == "us-east-1" {
            "eu-west-1"
        } else {
            "us-east-1"
        };
        let request = http::Request::builder()
            .method("POST")
            .uri("https://localhost/")
            .header("authorization", cassandra_sigv4_in(other_region))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", "KeyspacesStreams.GetStream")
            .body(bytes::Bytes::from(json!({ "streamArn": arn }).to_string()))
            .expect("build the request");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 404,
            "case {other_region} reads {region}'s stream: it must not exist there, got \
             {status} {body}"
        );
        assert!(
            body.contains("ResourceNotFoundException"),
            "case {other_region} reads {region}'s stream: the refusal is a \
             ResourceNotFoundException, got {body}"
        );
    }

    // An unsigned request names no region, so nothing is hidden from it.
    let (status, body) = dispatch(&router, "", "KeyspacesStreams.ListStreams", "{}").await;
    assert_eq!(
        status, 200,
        "case unsigned: the request must still be served, got {status} {body}"
    );
    for (region, arn) in &seeded {
        assert!(
            body.contains(arn.as_str()),
            "case unsigned: {region}'s stream must be listed, got {body}"
        );
    }
}
