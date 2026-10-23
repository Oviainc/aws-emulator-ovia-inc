// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS, and which it leaves to its peer.
//!
//! This configuration service and the CloudSearch DOMAIN data plane share one
//! SigV4 signing name (`cloudsearch`), so the signing name alone cannot tell them
//! apart: a router that matched on it would see two claimants and answer
//! `500 AmbiguousService` — a code no AWS model declares — for every request to
//! either one. What separates them is the endpoint, so this suite pins THIS half of
//! the split: this model is `awsQuery`, so its whole wire surface is `POST /` with
//! the operation in the `Action` field, and a request addressing the domain
//! endpoint's dated data-plane paths (`/2013-01-01/…`) is NOT claimed here, which a
//! router holding only this service answers `404 UnknownService`.
//!
//! The other half — the domain service claiming the dated paths and leaving the
//! root alone — is pinned by the same suite in that crate
//! (`tests/all/endpoint_claim.rs`). Together they say that no path has two
//! claimants and none has zero.

use bytes::Bytes;
use service_com_amazonaws_cloudsearch_a9searchcloudconfigservice2013 as cloudsearch;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A SigV4 header whose credential scope names the signing name BOTH CloudSearch
/// services sign with, which is the routing key the router resolves first.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/cloudsearch/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// A router serving ONLY this service, so "claimed" and "not claimed" are
/// observable as the service's own answer versus `404 UnknownService`.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cloudsearch::EmulatedService::new(engine));
    router
}

/// One claim row: case name, method, URI, host, content type, body, and whether
/// this service must claim the request.
type ClaimCase<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    Option<&'a str>,
    &'a str,
    bool,
);

#[tokio::test]
async fn only_the_query_protocol_root_is_claimed() {
    let router = router();
    // The awsQuery root is claimed whichever `Action` the body names and whichever
    // host it arrives on; the domain endpoint's dated paths are not.
    let cases: &[ClaimCase<'_>] = &[
        (
            "CreateDomain on the configuration root",
            "POST",
            "/",
            "cloudsearch.us-east-1.amazonaws.com",
            Some("application/x-www-form-urlencoded"),
            "Action=CreateDomain&Version=2013-01-01&DomainName=movies",
            true,
        ),
        (
            "DescribeDomains on the configuration root",
            "POST",
            "/",
            "cloudsearch.us-east-1.amazonaws.com",
            Some("application/x-www-form-urlencoded"),
            "Action=DescribeDomains&Version=2013-01-01",
            true,
        ),
        (
            "the configuration root reached on a domain host",
            "POST",
            "/",
            "doc-movies-1a2b3c.us-east-1.cloudsearchdomain.amazonaws.com",
            Some("application/x-www-form-urlencoded"),
            "Action=DescribeDomains&Version=2013-01-01",
            true,
        ),
        (
            "UploadDocuments on the document endpoint",
            "POST",
            "/2013-01-01/documents/batch?format=sdk",
            "doc-movies-1a2b3c.us-east-1.cloudsearchdomain.amazonaws.com",
            Some("application/json"),
            r#"[{"type":"add","id":"doc1","fields":{"title":"Hamlet"}}]"#,
            false,
        ),
        (
            "Search on the search endpoint",
            "GET",
            "/2013-01-01/search?format=sdk&pretty=true&q=hamlet",
            "search-movies-1a2b3c.us-east-1.cloudsearchdomain.amazonaws.com",
            None,
            "",
            false,
        ),
        (
            "Suggest on the search endpoint",
            "GET",
            "/2013-01-01/suggest?format=sdk&pretty=true&q=ham&suggester=title",
            "search-movies-1a2b3c.us-east-1.cloudsearchdomain.amazonaws.com",
            None,
            "",
            false,
        ),
    ];

    for (case, method, uri, host, content_type, body, claimed) in cases {
        let mut request = http::Request::builder()
            .method(*method)
            .uri(*uri)
            .header("host", *host)
            .header("authorization", AUTHORIZATION);
        if let Some(content_type) = content_type {
            request = request.header("content-type", *content_type);
        }
        let request = request
            .body(Bytes::from((*body).to_string()))
            .unwrap_or_else(|error| panic!("case {case:?}: building the request: {error}"));
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let rendered = String::from_utf8_lossy(response.body()).to_string();
        // The unclaimed envelope is the router's own awsJson one, so it is told
        // apart from this service's awsQuery XML by its `__type`.
        let unknown_service = serde_json::from_str::<serde_json::Value>(&rendered)
            .ok()
            .and_then(|body| {
                body.get("__type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .is_some_and(|code| code.ends_with("UnknownService"));
        if *claimed {
            assert!(
                !unknown_service,
                "case {case:?}: this service must claim the request, got {status} {rendered}"
            );
            assert_eq!(
                status, 200,
                "case {case:?}: expected the service's own 200, got {status} {rendered}"
            );
        } else {
            assert!(
                unknown_service,
                "case {case:?}: this service must NOT claim the request, so a router \
                 holding only it answers 404 UnknownService, got {status} {rendered}"
            );
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for an unclaimed request, got {status} {rendered}"
            );
        }
    }
}
