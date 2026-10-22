// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS, under a SigV4 signing name it does not own alone.
//!
//! `AWSMarketplaceDiscovery` signs as `aws-marketplace` — the signing name EVERY service
//! in the AWS Marketplace family carries (`aws.auth#sigv4 name` on Marketplace Agreement,
//! Catalog, Deployment, Discovery, Entitlement Service, Metering and Reporting) — while
//! each of them serves its own endpoint and its own wire surface. The router resolves a
//! request's routing key from the SigV4 credential scope first, so each of those services
//! sees the key `aws-marketplace`; a service that claims on the signing name alone claims
//! the whole family's traffic, and the router refuses every such request as
//! `AmbiguousService` instead of serving it.
//!
//! So the claim is narrowed to this service's own surface: the request's routing key must
//! name the shared signing name AND the request must address a first URI path segment this
//! service's model declares (`POST /2026-02-05/<operation>`, `OPERATIONS.md`).
//!
//! This suite pins that rule over the whole class of requests the family produces, by
//! asking the `Service::can_handle` seam directly — a single-service router would claim
//! everything it is handed, so dispatch cannot see the rule. The cross-service proof that
//! the family routes apart with all services mounted lives in
//! `core/router/tests/all/aws_marketplace_signing_name_routing.rs`.

use std::sync::Arc;

use smithy_server_runtime::Service;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::EmulatedService;

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. Nothing
/// verifies the signature here: `can_handle` reads the scope's service field only.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260205/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// Whether this service claims the described request.
fn claims(
    service: &EmulatedService<engine::InMemoryEngine>,
    method: &str,
    uri: &str,
    target: Option<&str>,
    signing_name: &str,
) -> bool {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        // A loopback host names no service, so the SigV4 credential scope is what the
        // routing key comes from — exactly as it does for an SDK pointed at the emulator.
        .header("host", "localhost:9297")
        .header("authorization", authorization(signing_name))
        .header("content-type", "application/json");
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    let (parts, ()) = builder
        .body(())
        .expect("build the request")
        .into_parts();
    service.can_handle(&parts)
}

/// One row of the claim table: the case name, the request's method, URI, `X-Amz-Target`
/// and credential-scope service, and whether this service must claim it.
type Row = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
    bool,
);

#[tokio::test]
async fn the_service_claims_its_own_path_space_and_no_marketplace_siblings() {
    let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));

    // (case, method, uri, X-Amz-Target, credential-scope service, must be claimed)
    //
    // The claimed rows are this service's nine modeled routes (`OPERATIONS.md`: `POST
    // /2026-02-05/getListing`, `getOffer`, `getOfferSet`, `getOfferTerms`, `getProduct`,
    // `listFulfillmentOptions`, `listPurchaseOptions`, `searchFacets`, `searchListings`)
    // plus the dated prefix carrying a name no operation has: the path SPACE is this
    // service's, so it must reach this service and be answered by its own protocol rather
    // than fall through to a sibling or to the router's refusal.
    //
    // The declined rows are the other six surfaces of the `aws-marketplace` family — the
    // REST siblings' path spaces (Reporting `POST /getBuyerDashboard`, Catalog's
    // per-operation segments, Deployment's `/tags` and `/catalogs`), the RPC siblings'
    // `POST /` with their own `X-Amz-Target`, and an `rpcv2Cbor`
    // `/service/{Service}/operation/{Operation}` path — and finally a request for this
    // service's own path signed for a DIFFERENT service, which this service must not
    // claim either.
    let rows: [Row; 20] = [
        (
            "own route getListing",
            "POST",
            "/2026-02-05/getListing",
            None,
            "aws-marketplace",
            true,
        ),
        (
            "own route getOffer",
            "POST",
            "/2026-02-05/getOffer",
            None,
            "aws-marketplace",
            true,
        ),
        (
            "own route getOfferSet",
            "POST",
            "/2026-02-05/getOfferSet",
            None,
            "aws-marketplace",
            true,
        ),
        (
            "own route getOfferTerms",
            "POST",
            "/2026-02-05/getOfferTerms",
            None,
            "aws-marketplace",
            true,
        ),
        (
            "own route getProduct",
            "POST",
            "/2026-02-05/getProduct",
            None,
            "aws-marketplace",
            true,
        ),
        (
            "own route listFulfillmentOptions",
            "POST",
            "/2026-02-05/listFulfillmentOptions",
            None,
            "aws-marketplace",
            true,
        ),
        (
            "own route listPurchaseOptions",
            "POST",
            "/2026-02-05/listPurchaseOptions",
            None,
            "aws-marketplace",
            true,
        ),
        (
            "own route searchFacets",
            "POST",
            "/2026-02-05/searchFacets",
            None,
            "aws-marketplace",
            true,
        ),
        (
            "own route searchListings",
            "POST",
            "/2026-02-05/searchListings",
            None,
            "aws-marketplace",
            true,
        ),
        (
            "own route searchListings with a query string",
            "POST",
            "/2026-02-05/searchListings?x=1",
            None,
            "aws-marketplace",
            true,
        ),
        (
            "own path space, no such operation",
            "POST",
            "/2026-02-05/notAnOperation",
            None,
            "aws-marketplace",
            true,
        ),
        (
            "sibling Marketplace Reporting",
            "POST",
            "/getBuyerDashboard",
            None,
            "aws-marketplace",
            false,
        ),
        (
            "sibling Marketplace Catalog list",
            "POST",
            "/ListEntities",
            None,
            "aws-marketplace",
            false,
        ),
        (
            "sibling Marketplace Catalog read",
            "GET",
            "/DescribeEntity",
            None,
            "aws-marketplace",
            false,
        ),
        (
            "sibling Marketplace Deployment tags",
            "GET",
            "/tags/arn:aws:aws-marketplace:us-east-1:000000000000:deployment-parameter/p",
            None,
            "aws-marketplace",
            false,
        ),
        (
            "sibling Marketplace Deployment parameters",
            "POST",
            "/catalogs/AWSMarketplace/products/prod-1/deployment-parameters",
            None,
            "aws-marketplace",
            false,
        ),
        (
            "sibling Marketplace Metering over awsJson",
            "POST",
            "/",
            Some("AWSMPMeteringService.MeterUsage"),
            "aws-marketplace",
            false,
        ),
        (
            "sibling Marketplace Entitlement Service over awsJson",
            "POST",
            "/",
            Some("AWSMPEntitlementService.GetEntitlements"),
            "aws-marketplace",
            false,
        ),
        (
            "sibling Marketplace Entitlement Service over rpcv2Cbor",
            "POST",
            "/service/AWSMPEntitlementService/operation/GetEntitlements",
            None,
            "aws-marketplace",
            false,
        ),
        (
            "own path signed for another service",
            "POST",
            "/2026-02-05/searchListings",
            None,
            "dynamodb",
            false,
        ),
    ];

    for (case, method, uri, target, signing_name, expected) in rows {
        let claimed = claims(&service, method, uri, target, signing_name);
        assert_eq!(
            claimed, expected,
            "case {case:?} ({method} {uri}, target {target:?}, signed for {signing_name}): \
             expected can_handle = {expected}, got {claimed}"
        );
    }
}
