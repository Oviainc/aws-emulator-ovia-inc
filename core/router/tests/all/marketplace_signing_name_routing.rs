// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `aws-marketplace` SigV4 signing name.
//!
//! EVERY AWS Marketplace-family service signs as `aws-marketplace` — Marketplace
//! Catalog, Agreement, Deployment, Discovery, Entitlement, Metering and Reporting — while
//! each serves its own endpoint (`catalog.marketplace`, `agreement-marketplace`,
//! `deployment-marketplace`, `discovery-marketplace`, `entitlement.marketplace`,
//! `metering.marketplace`, `reporting-marketplace`). The router resolves a request's
//! routing key from the SigV4 credential scope first, so all seven see the key
//! `aws-marketplace`: a service that claims on the signing name alone claims the other
//! six's traffic too, and the router answers `500 AmbiguousService` for ALL of them. Each
//! one therefore also requires the request to address its own wire path space — the first
//! URI path segments its model declares, or the root path for a service whose protocol
//! posts every operation to `/` — and, for the three RPC members that all post to that
//! one root path, the operation the request names out of band (`x-amz-target`, or the
//! `/operation/<Op>` segment of an `rpcv2Cbor` path).
//!
//! This suite pins that: with all seven mounted, each one's own path space reaches it,
//! each RPC member's own operation reaches it, and none of them answers the router's
//! refusals.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the signing name all seven
/// services share. The signature is never checked: the router here runs with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The router's OWN refusals, which no request below may answer: nobody claimed it, or
/// more than one service did.
const ROUTER_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_marketplaceagreement_awsmpcommerceservice_v20200301::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_marketplaceentitlementservice_awsmpentitlementservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_marketplacemetering_awsmpmeteringservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_marketplacereporting_awsmarketplacereporting::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// Dispatch one request and report its status and wire error code. `target` is the
/// `x-amz-target` header an RPC request names its operation in, absent for a REST one.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    target: Option<&str>,
    body: &'static str,
) -> (u16, Option<String>) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "catalog.marketplace.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION);
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    let request = builder
        .body(Bytes::from_static(body.as_bytes()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    // The error code, read from the body's `__type` discriminator: a status alone cannot
    // tell the router's own `UnknownService` (nobody claimed the request) or
    // `AmbiguousService` (several did) from a service's own modeled error.
    let error_code = serde_json::from_slice::<serde_json::Value>(response.body())
        .ok()
        .and_then(|body| {
            body.get("__type")
                .or_else(|| body.get("Code"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .map(|code| code.rsplit('#').next().unwrap_or(&code).to_string());
    (status, error_code)
}

#[tokio::test]
async fn each_service_sharing_the_aws_marketplace_signing_name_serves_its_own_path_space() {
    let router = build_router();

    // (case, method, uri, body, expected status, expected error code)
    //
    // Asserting the ERROR CODE as well as the status is what excludes the two routing
    // failures: the router answers `UnknownService` (404) when nobody claims a request
    // and `AmbiguousService` (500) when several do, so a row naming a service's OWN
    // answer can only pass when exactly one service claimed it.
    //
    // Marketplace Catalog owns a literal path per operation; its tagging and
    // resource-policy operations are implemented, so an ARN naming no resource of its
    // catalog is its own `AccessDeniedException`, its entity reads answer their own
    // modeled result for an entity the catalog does not hold, and an operation of its own
    // that is still scaffolded answers the scaffold's `NotImplemented` on its own path.
    // The other family members answer on their own paths too — Reporting
    // `/getBuyerDashboard`, Deployment `/tags/{resourceArn}`, Discovery `/2026-02-05/...`
    // — whether they are implemented (a 200) or still scaffolded (a 501). The RPC members
    // (Agreement, Entitlement, Metering) all claim the root path their protocol posts to;
    // the operation each request names is what separates those three, which the suite
    // below asserts.
    let rows: [(&str, &str, &str, &'static str, u16, &str); 9] = [
        (
            "marketplace catalog list tags",
            "POST",
            "/ListTagsForResource",
            r#"{"ResourceArn":"arn:aws:aws-marketplace:us-east-1:000000000000:other/thing"}"#,
            403,
            "AccessDeniedException",
        ),
        (
            "marketplace catalog get resource policy",
            "GET",
            "/GetResourcePolicy?resourceArn=arn%3Aaws%3Aaws-marketplace%3Aus-east-1%3A000000000000%3Aother%2Fthing",
            "",
            403,
            "AccessDeniedException",
        ),
        (
            "marketplace catalog list entities",
            "POST",
            "/ListEntities",
            r#"{"Catalog":"AWSMarketplace","EntityType":"AmiProduct"}"#,
            200,
            // A success carries no `__type`; the row is asserted on its status alone
            // below, which a routing refusal (404 / 500) could not produce.
            "",
        ),
        (
            "marketplace catalog describe entity, for an entity it does not hold",
            "GET",
            "/DescribeEntity?catalog=AWSMarketplace&entityId=prod-nosuchentity01",
            "",
            404,
            // The catalog's OWN modeled 404, which the router's own 404
            // (`UnknownService`) could not produce.
            "ResourceNotFoundException",
        ),
        (
            "marketplace catalog batch describe entities",
            "POST",
            "/BatchDescribeEntities",
            r#"{"EntityRequestList":[{"Catalog":"AWSMarketplace","EntityId":"prod-nosuchentity01"}]}"#,
            200,
            // A batch read reports a missing entity inside its `Errors` map, so the call
            // itself succeeds and carries no `__type`.
            "",
        ),
        (
            "marketplace catalog describe assessment, still scaffolded",
            "POST",
            "/DescribeAssessment",
            r#"{"Catalog":"AWSMarketplace","AssessmentIdentifier":"assessment-1a2b3c"}"#,
            501,
            "NotImplemented",
        ),
        (
            "marketplace reporting buyer dashboard",
            "POST",
            "/getBuyerDashboard",
            r#"{"dashboardIdentifier":"arn:aws:aws-marketplace::000000000000:AWSMarketplace/ReportingData/Agreement_V1/Dashboard/AgreementSummary_V1","embeddingDomains":["https://example.com"]}"#,
            200,
            // Implemented, so it answers its own success — which carries no `__type`.
            "",
        ),
        (
            "marketplace deployment tags",
            "GET",
            "/tags/arn%3Aaws%3Aaws-marketplace%3Aus-east-1%3A000000000000%3Adeployment-parameter%2Fd-1",
            "",
            404,
            // Implemented, so an unknown deployment parameter is its OWN modeled
            // `ResourceNotFoundException` — which a routing failure could not produce: the
            // router's own 404 is `UnknownService`, asserted by the suite below.
            "ResourceNotFoundException",
        ),
        (
            "marketplace discovery search listings",
            "POST",
            "/2026-02-05/searchListings",
            r#"{}"#,
            200,
            // Implemented, so it answers its own success — which carries no `__type`.
            "",
        ),
    ];

    for (case, method, uri, body, expected_status, expected_code) in rows {
        let (status, error_code) = dispatch(&router, method, uri, None, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from the one service that owns \
             {uri}, got {status} {error_code:?}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code.as_deref(),
                Some(expected_code),
                "case {case:?}: expected {expected_code} from the one service that owns \
                 {uri}, got {status} {error_code:?}"
            );
        }
    }
}

/// The three RPC members of the family (Agreement, Entitlement Service, Metering) all
/// serve the one path their protocol posts to — `POST /`, plus `POST
/// /service/{Service}/operation/{Op}` for the two that also speak `rpcv2Cbor` — so the
/// path cannot separate them. The operation the request names can, and each service's own
/// generated routing table is what answers whether it is that service's.
///
/// The payload here is not a real one for each operation, so a row's own service answers
/// however it answers that request; what is asserted is that a SERVICE answered, never a
/// router refusal.
#[tokio::test]
async fn an_rpc_request_reaches_the_service_whose_model_names_its_operation() {
    let router = build_router();

    // (case, uri, x-amz-target)
    let rows: [(&str, &str, Option<&str>); 5] = [
        (
            "Agreement over awsJson: DescribeAgreement",
            "/",
            Some("AWSMPCommerceService_v20200301.DescribeAgreement"),
        ),
        (
            "Agreement over rpcv2Cbor: SearchAgreements",
            "/service/AWSMPCommerceService_v20200301/operation/SearchAgreements",
            None,
        ),
        (
            "Entitlement Service over rpcv2Cbor: GetEntitlements",
            "/service/AWSMPEntitlementService/operation/GetEntitlements",
            None,
        ),
        (
            "Metering over awsJson: MeterUsage",
            "/",
            Some("AWSMPMeteringService.MeterUsage"),
        ),
        (
            "Metering over awsJson: ResolveCustomer",
            "/",
            Some("AWSMPMeteringService.ResolveCustomer"),
        ),
    ];

    for (case, uri, target) in rows {
        let (status, error_code) = dispatch(&router, "POST", uri, target, "{}").await;
        let code = error_code.clone().unwrap_or_default();
        assert!(
            !ROUTER_REFUSALS.contains(&code.as_str()),
            "case {case:?}: exactly one service must claim this request, got {status} \
             {error_code:?}"
        );
    }
}

#[tokio::test]
async fn no_marketplace_service_claims_another_family_members_path() {
    let router = build_router();

    // A path NO service in the family declares reaches none of them: the router answers
    // its own `UnknownService`, which is the proof that each claim is narrowed to its own
    // path space rather than left open (an open claim would answer `AmbiguousService`,
    // and a single over-broad claim would answer that service's own error).
    for (case, method, uri) in [
        ("a path no family member declares", "POST", "/NotAnOperation"),
        ("another service's style of path", "GET", "/v1/things/t-1"),
    ] {
        let (status, error_code) = dispatch(&router, method, uri, None, "").await;
        assert_eq!(
            (status, error_code.as_deref()),
            (404, Some("UnknownService")),
            "case {case:?}: {uri} must reach no service at all, got {status} {error_code:?}"
        );
    }
}
