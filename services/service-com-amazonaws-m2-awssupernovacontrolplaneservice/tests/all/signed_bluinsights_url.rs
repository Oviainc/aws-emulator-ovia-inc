// SPDX-License-Identifier: CC0-1.0

//! `GetSignedBluinsightsUrl`, driven over the wire.
//!
//! The rule: `GET /signed-bi-url` is a MODELLED outcome of this service — 200 carrying
//! the required `signedBiUrl` member
//! (`smithy_query(query="operation",operation="GetSignedBluinsightsUrl")`: input
//! `Unit`, output `GetSignedBluinsightsUrlResponse{signedBiUrl required}`, errors
//! `AccessDeniedException`/`InternalServerException`/`ThrottlingException`) — so it is
//! never answered with the scaffold's 501, which is not among them.
//!
//! What the URL is, is the emulator's own choice; what it must DO is an invariant, and
//! that is what this suite asserts over a table of regions:
//!
//! * the member is present on every call and is an absolute `https` URL;
//! * it names the account and the region the request was signed for, so a caller
//!   signing for another region gets another URL;
//! * it is STABLE: the operation is `@readonly`, so two reads answer the same URL and
//!   a read changes nothing a later caller sees (shown by reading an application's
//!   listing across the calls);
//! * a request whose credential scope names no region the emulator can read falls back
//!   to the emulator's documented default region rather than to the caller's value.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_m2_awssupernovacontrolplaneservice::state::{
    CREATION_SETTLES_AFTER, DEFAULT_REGION,
};
use service_com_amazonaws_m2_awssupernovacontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account id every minted value carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The URI the model binds this operation to.
const PATH: &str = "/signed-bi-url";

/// The endpoint the requests below arrive on — a loopback authority, the way a client
/// addresses this emulator — held constant so the region is the only thing varying in
/// the region table.
const ENDPOINT: &str = "127.0.0.1:9297";

/// A router serving ONE m2 service over ONE shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names m2 in `region`. The
/// signature bytes are filler: authentication is off for this router, and the scope is
/// what routes the request and names the region.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTM2EXAMPLE/20260101/{region}/m2/aws4_request, \
         SignedHeaders=host;x-amz-date, \
         Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Dispatch one GET signed for `region`, arriving on `host`, and return its status and
/// parsed body.
async fn get_on(
    router: &MultiServiceRouter,
    host: &str,
    region: &str,
    path: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization(region));
    let response = router
        .dispatch(request.body(Bytes::new()).expect("a valid test request"))
        .await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Dispatch one GET signed for `region` on the shared [`ENDPOINT`].
async fn get(router: &MultiServiceRouter, region: &str, path: &str) -> (u16, Value) {
    get_on(router, ENDPOINT, region, path).await
}

/// The `signedBiUrl` member of a 200 body, or a marker when it is absent.
fn signed_url(body: &Value) -> &str {
    body.get("signedBiUrl")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
}

/// The operation answers 200 with a `signedBiUrl` that names the account and the
/// region the caller signed for — never a 501, and never the same URL for two regions.
#[tokio::test]
async fn the_signed_url_names_the_account_and_the_signing_region() {
    // (case, the region the request is signed for, the region the URL must name).
    // The rows span the region forms the modelled `Arn` pattern's region segment
    // admits — a plain commercial region, a GovCloud one, a China one, an ISO one —
    // plus two the emulator cannot read as a region, which fall back to its default.
    let cases: &[(&str, &str, &str)] = &[
        ("a commercial region", "eu-west-1", "eu-west-1"),
        ("another commercial region", "us-west-2", "us-west-2"),
        ("a GovCloud region", "us-gov-west-1", "us-gov-west-1"),
        ("a China region", "cn-north-1", "cn-north-1"),
        ("an ISO region", "us-isob-east-1", "us-isob-east-1"),
        ("a scope naming no region shape", "not_a_region", DEFAULT_REGION),
        ("a scope naming a region in upper case", "US-EAST-1", DEFAULT_REGION),
    ];

    let router = router();
    let mut urls: BTreeSet<String> = BTreeSet::new();
    for (case, signed_for, expected_region) in cases {
        let (status, body) = get(&router, signed_for, PATH).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body:?}"
        );
        let url = signed_url(&body).to_string();
        assert!(
            url.starts_with(&format!("https://{ENDPOINT}/")),
            "case {case:?}: expected the URL to be built on the endpoint the request \
             arrived on (https://{ENDPOINT}/), got {url:?}"
        );
        assert!(
            url.contains(&format!("account={ACCOUNT}")),
            "case {case:?}: expected the URL to name account {ACCOUNT}, got {url:?}"
        );
        assert!(
            url.contains(&format!("region={expected_region}")),
            "case {case:?}: expected the URL to name region {expected_region}, got {url:?}"
        );
        assert!(
            url.contains("token="),
            "case {case:?}: expected a federation token in the URL, got {url:?}"
        );
        urls.insert(url);
    }

    // Two of the rows resolve to the same (default) region, so the distinct URLs are
    // one per distinct region named above.
    let distinct_regions: BTreeSet<&str> = cases.iter().map(|(_, _, region)| *region).collect();
    assert_eq!(
        urls.len(),
        distinct_regions.len(),
        "a URL is a property of the region: expected {} distinct URLs, got {urls:?}",
        distinct_regions.len()
    );

    // The URL addresses the EMULATOR, not AWS: it follows the endpoint the request
    // arrived on, so a caller that follows it comes back here.
    for host in ["127.0.0.1:9297", "localhost:8080", "emulator.test:443"] {
        let (status, body) = get_on(&router, host, "eu-west-1", PATH).await;
        assert_eq!(
            status, 200,
            "case {host:?}: expected HTTP 200, got {status} {body:?}"
        );
        let url = signed_url(&body);
        assert!(
            url.starts_with(&format!("https://{host}/")),
            "case {host:?}: expected the URL to name the endpoint it arrived on, got {url:?}"
        );
        assert!(
            !url.contains("amazonaws.com") && !url.contains("bluinsights.aws"),
            "case {host:?}: the URL must not address a host outside the emulator, got {url:?}"
        );
    }
}

/// Reading the URL changes nothing: two reads in one region answer the same URL, and
/// an application created before them is reported identically after.
#[tokio::test]
async fn reading_the_signed_url_changes_nothing() {
    let router = router();
    let region = "eu-west-1";

    // A resource to read back across the calls, so "changes nothing" is checked
    // against the store rather than only against the URL itself.
    let create = http::Request::builder()
        .method("POST")
        .uri(format!("https://{ENDPOINT}/applications"))
        .header("host", ENDPOINT)
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(Bytes::from(
            br#"{"name":"BluInsightsApp","engineType":"bluage","definition":{"content":"{}"}}"#
                .to_vec(),
        ))
        .expect("a valid test request");
    let created = router.dispatch(create).await;
    assert_eq!(
        created.status().as_u16(),
        200,
        "the application the reads are checked against must be created: {:?}",
        String::from_utf8_lossy(created.body())
    );

    let (first_status, first) = get(&router, region, PATH).await;
    // Past the span the clock owes the new application its settled status, so the
    // two listings below differ only if a read wrote something.
    tokio::time::sleep(CREATION_SETTLES_AFTER + Duration::from_millis(50)).await;
    let (_, before) = get(&router, region, "/applications").await;
    let (second_status, second) = get(&router, region, PATH).await;
    let (_, after) = get(&router, region, "/applications").await;

    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "both reads answer 200, got {first_status} and {second_status}"
    );
    assert_eq!(
        signed_url(&first),
        signed_url(&second),
        "case {region:?}: a @readonly read must answer the same URL, got {:?} then {:?}",
        signed_url(&first),
        signed_url(&second)
    );
    assert_eq!(
        before, after,
        "reading the signed URL must not change what a later caller sees: {before:?} then {after:?}"
    );
}
