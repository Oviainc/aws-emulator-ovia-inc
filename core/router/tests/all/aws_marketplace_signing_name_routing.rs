// SPDX-License-Identifier: CC0-1.0

//! The AWS Marketplace family is SEVEN services behind ONE SigV4 signing name
//! (`aws-marketplace`): Marketplace Agreement, Catalog, Deployment, Discovery,
//! Entitlement Service, Metering and Reporting all carry `aws.auth#sigv4 name =
//! "aws-marketplace"` while each serves its own endpoint (`agreement-marketplace`,
//! `catalog.marketplace`, `deployment-marketplace`, `discovery-marketplace`,
//! `entitlement.marketplace`, `metering.marketplace`, `reporting-marketplace`). No Smithy
//! trait expresses that split, so with all of them mounted the default signing-name
//! `can_handle` makes all seven claim every Marketplace-signed request and the router
//! answers `500 AmbiguousService` for all of it — the whole family unreachable.
//!
//! Each service's traffic is therefore told apart by what the request itself addresses:
//! the first URI path segment its model declares for the REST members, and the
//! `X-Amz-Target` service prefix (or the rpcv2Cbor `/service/{Service}/…` label) for the
//! RPC members, which all post to `/`.
//!
//! This suite needs the WHOLE router: the ambiguity only exists when the siblings are
//! mounted together, so a single-service test cannot see it.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming `aws-marketplace` — what EVERY one of the seven
/// services' requests is signed with, and therefore the whole problem.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260205/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The error code a response carries, read from the body's `__type` discriminator (or an
/// `Code` field): the status alone cannot tell the router's own `UnknownService` (nobody
/// claimed the request) or `AmbiguousService` (several did) from a service's own modeled
/// 404 or 500.
fn error_code(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|body| {
            body.get("__type")
                .or_else(|| body.get("Code"))
                .and_then(serde_json::Value::as_str)
                .map(|code| {
                    // An awsJson `__type` may be a shape id (`namespace#Name`).
                    code.rsplit('#').next().unwrap_or(code).to_string()
                })
        })
}

/// One row of the routing table: the case name, the request's method, URI,
/// `X-Amz-Target` and body, and the `(status, error code)` the owning service answers
/// when that answer does not depend on an operation still being scaffolded.
type Case = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
    Option<(u16, &'static str)>,
);

#[test]
fn every_service_sharing_the_aws_marketplace_signing_name_serves_its_own_surface() {
    std::thread::Builder::new()
        .name("aws-marketplace-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Each row: one service's own wire surface — method, URI, `X-Amz-Target`,
            // body — and the (status, error code) the OWNING service answers when that
            // is fixed by this change. What this suite is about is that NO row answers
            // `AmbiguousService` (several services claimed it) or `UnknownService`
            // (none did), so a row whose answer depends on an operation still being
            // scaffolded carries `None`: pinning its 501 would fail the day that
            // operation is implemented, for a reason that has nothing to do with
            // routing.
            //
            // The routes come from each service's `OPERATIONS.md` "Route" / "Request
            // routing" lines.
            let cases: [Case; 9] = [
                (
                    "Marketplace Discovery search",
                    "POST",
                    "http://localhost/2026-02-05/searchListings",
                    None,
                    r#"{"maxResults":1}"#,
                    None,
                ),
                (
                    "Marketplace Discovery read of an absent listing",
                    "POST",
                    "http://localhost/2026-02-05/getListing",
                    None,
                    r#"{"listingId":"no-such-listing"}"#,
                    Some((404, "ResourceNotFoundException")),
                ),
                (
                    "Marketplace Discovery refusing an over-long listingId",
                    "POST",
                    "http://localhost/2026-02-05/getListing",
                    None,
                    // `GetListingInput.listingId` is `@length(min: 1, max: 255)`, so a
                    // 256-character id is refused at the modeled boundary.
                    r#"{"listingId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
                    Some((400, "ValidationException")),
                ),
                (
                    "Marketplace Reporting dashboard embed",
                    "POST",
                    "http://localhost/getBuyerDashboard",
                    None,
                    r#"{"dashboardIdentifier":"arn:aws:aws-marketplace::000000000000:AWSMarketplace/ReportingData/Agreement_V1/Dashboard/AgreementV1"}"#,
                    None,
                ),
                (
                    "Marketplace Catalog entity list",
                    "POST",
                    "http://localhost/ListEntities",
                    None,
                    r#"{"Catalog":"AWSMarketplace","EntityType":"AmiProduct"}"#,
                    None,
                ),
                (
                    "Marketplace Deployment parameter tags",
                    "GET",
                    "http://localhost/tags/arn:aws:aws-marketplace:us-east-1:000000000000:deployment-parameter/p",
                    None,
                    "",
                    None,
                ),
                (
                    "Marketplace Metering over awsJson",
                    "POST",
                    "http://localhost/",
                    Some("AWSMPMeteringService.ResolveCustomer"),
                    r#"{"RegistrationToken":"token"}"#,
                    None,
                ),
                (
                    "Marketplace Entitlement Service over awsJson",
                    "POST",
                    "http://localhost/",
                    Some("AWSMPEntitlementService.GetEntitlements"),
                    r#"{"ProductCode":"product"}"#,
                    None,
                ),
                (
                    "Marketplace Agreement over awsJson",
                    "POST",
                    "http://localhost/",
                    Some("AWSMPCommerceService_v20200301.ListAgreementCharges"),
                    r#"{"agreementId":"agmt-1"}"#,
                    None,
                ),
            ];

            for (case, method, uri, target, body, expected) in cases {
                let mut builder = http::Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("authorization", AUTHORIZATION)
                    .header("content-type", "application/json");
                if let Some(target) = target {
                    builder = builder
                        .header("x-amz-target", target)
                        .header("content-type", "application/x-amz-json-1.1");
                }
                let request = builder
                    .body(bytes::Bytes::from(body.to_string()))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let code = error_code(response.body());
                let rendered = String::from_utf8_lossy(response.body()).to_string();

                assert_ne!(
                    code.as_deref(),
                    Some("AmbiguousService"),
                    "case {case:?} ({method} {uri}): the request was claimed by SEVERAL \
                     services sharing the aws-marketplace signing name, got {status} with \
                     {rendered}"
                );
                assert_ne!(
                    code.as_deref(),
                    Some("UnknownService"),
                    "case {case:?} ({method} {uri}): the request was claimed by NO service, \
                     got {status} with {rendered}"
                );
                if let Some((expected_status, expected_code)) = expected {
                    assert_eq!(
                        (status, code.as_deref()),
                        (expected_status, Some(expected_code)),
                        "case {case:?} ({method} {uri}): the owning service must answer HTTP \
                         {expected_status} {expected_code}, got {status} with {rendered}"
                    );
                }
            }

            // The claim is NARROWED, not widened: a Marketplace-signed request whose path
            // no member of the family declares must be claimed by nobody, which the router
            // reports as `UnknownService` rather than serving it from an arbitrary member.
            let request = http::Request::builder()
                .method("POST")
                .uri("http://localhost/not-a-marketplace-path")
                .header("authorization", AUTHORIZATION)
                .header("content-type", "application/json")
                .body(bytes::Bytes::from_static(b"{}"))
                .expect("a well-formed request");
            let response = runtime.block_on(router.dispatch(request));
            let status = response.status().as_u16();
            let code = error_code(response.body());
            let rendered = String::from_utf8_lossy(response.body()).to_string();
            assert_eq!(
                (status, code.as_deref()),
                (404, Some("UnknownService")),
                "case \"a path no family member declares\": expected 404 UnknownService, got \
                 {status} with {rendered}"
            );
        })
        .expect("spawn the routing thread")
        .join()
        .expect("the aws-marketplace family must route apart");
}
