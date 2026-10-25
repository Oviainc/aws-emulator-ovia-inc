// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `aws-marketplace` SigV4 signing name.
//!
//! EVERY AWS Marketplace-family service signs as `aws-marketplace` — Catalog,
//! Agreement, Deployment, Discovery, Entitlement, Metering and Reporting — while each
//! serves its own endpoint (`catalog.marketplace`, `agreement-marketplace`,
//! `deployment-marketplace`, `discovery-marketplace`, `entitlement.marketplace`,
//! `metering.marketplace`, `reporting-marketplace`). The router resolves a request's
//! routing key from the SigV4 credential scope first, so all seven see the key
//! `aws-marketplace`: a service that claims on the signing name alone claims the other
//! six's traffic too, and the router — which refuses to serve a request two services
//! claim — answers `500 AmbiguousService` to every one of them.
//!
//! The rule each of them applies instead: claim a request only when the signing name
//! matches AND the request addresses that service's own modelled wire surface — the
//! URI path space its operations bind for a REST protocol, or (for an RPC one, which
//! posts every operation to one path and names its target out of band) that protocol's
//! path plus the service NAMED in `X-Amz-Target` / the `rpcv2Cbor` URI. Path segments
//! alone cannot separate the three RPC peers (Agreement, Entitlement, Metering): all of
//! them post every operation to the same root path, so only the service a request names
//! distinguishes them.
//!
//! This suite pins that over the whole family, with every service of the emulator
//! mounted: rows across all seven, each reaching the service that owns its surface. A
//! row's expected answer is deliberately one only that service produces — its own
//! modelled error, or a 200 — so it can pass only when exactly one service claimed the
//! request. The router's own refusals (`404 UnknownService` when nobody claims,
//! `500 AmbiguousService` when several do) are excluded by construction.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the signing name all
/// seven services share. The signature is never checked: the router here runs with
/// request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The one catalog every AWS Marketplace Catalog operation documents.
const CATALOG: &str = "AWSMarketplace";

/// A resource ARN of the catalog's own namespace that names no resource of this
/// account's catalog — which its tagging and resource-policy surface answers its own
/// `AccessDeniedException` for.
const FOREIGN_RESOURCE_ARN: &str = "arn:aws:aws-marketplace:us-east-1:000000000000:other/thing";

/// Stack for assembling the router, as `router/tests/all/boot_serve.rs` documents:
/// `build_router` costs more stack than a cargo test thread's default, so it is
/// assembled on a thread sized for it and handed back (a `MultiServiceRouter` is
/// `Send`).
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Every service of this emulator, over one shared engine — the production router —
/// beside that engine, so a row needing a resource created out of band of its
/// service's API can be given one.
fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    std::thread::Builder::new()
        .name("marketplace-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let mut router = aws_emulator_router::build_router(engine.clone());
            router.set_request_authentication(false);
            (router, engine)
        })
        .expect("spawn the router-assembly thread")
        .join()
        .expect("the production router must assemble")
}

/// What the emulator answered: the HTTP status, the wire error code (`__type`, or
/// `code`/`Code` for a model that names it so), and the message.
struct Answer {
    status: u16,
    code: String,
    message: String,
}

async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    target: Option<&str>,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost")
        .header("authorization", AUTHORIZATION);
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let code = ["/__type", "/code", "/Code"]
        .iter()
        .filter_map(|pointer| parsed.pointer(pointer).and_then(Value::as_str))
        .map(|raw| raw.rsplit('#').next().unwrap_or(raw).to_string())
        .next()
        .unwrap_or_default();
    let message = ["/message", "/Message"]
        .iter()
        .filter_map(|pointer| parsed.pointer(pointer).and_then(Value::as_str))
        .map(str::to_string)
        .next()
        .unwrap_or_default();
    Answer {
        status,
        code,
        message,
    }
}

/// Rows across every service sharing the `aws-marketplace` signing name: the request
/// that addresses a service's own wire surface reaches it, and no two services claim
/// one request.
#[tokio::test]
async fn each_service_sharing_the_aws_marketplace_signing_name_serves_its_own_surface() {
    let (router, engine) = router();

    // The deployment row below writes a deployment parameter, which is targeted at
    // an agreement of the account for the product its URI names. An agreement is
    // created out of band of that API (a buyer accepts a seller's offer), so this
    // suite provisions the one its row writes against — otherwise the write is that
    // service's own `ResourceNotFoundException` rather than the 200 the row pins.
    // `seed_agreements` answers how many agreements that store holds AFTERWARDS,
    // which includes the targets it boots with, so what is asserted below is that
    // the shared engine really is where that store lives — not a count.
    let held = service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::seed_agreements(
        &*engine,
        [
            service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::ProvisionedAgreement::new(
                CATALOG, "agmt-1",
            )
            .with_product("prod-1"),
        ],
    );
    assert!(
        held.is_some_and(|held| held >= 1),
        "the shared engine must hold the deployment service's store, got {held:?}"
    );

    // (case, method, uri, x-amz-target, body, expected status, expected error code)
    //
    // The REST four are addressed by their own path space — several rows each, spanning
    // the shapes those path spaces take (a literal per operation, a `/tags/{arn}`
    // label, a date-stamped prefix) — and the RPC three post to one path and name
    // themselves in `X-Amz-Target`. An expected `""` code is a success.
    //
    // Every row asserts PATH OWNERSHIP, not the owning service's own rules: what it
    // excludes is the router's two routing failures (404 `UnknownService` when nobody
    // claims a request, 500 `AmbiguousService` when several do), so any answer only the
    // owning service can produce — a success, a modelled refusal, a 501 — serves.
    #[allow(clippy::type_complexity)]
    let rows: [(&str, &str, &str, Option<&str>, Option<Value>, u16, &str); 17] = [
        (
            "catalog: an empty list of change sets",
            "POST",
            "/ListChangeSets",
            None,
            Some(json!({"Catalog": CATALOG})),
            200,
            "",
        ),
        (
            "catalog: a change set that does not exist",
            "PATCH",
            "/CancelChangeSet?catalog=AWSMarketplace&changeSetId=cs-does-not-exist",
            None,
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "catalog: an entity it does not hold",
            "GET",
            "/DescribeEntity?catalog=AWSMarketplace&entityId=prod-nosuchentity01",
            None,
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "catalog: a batch read reporting a missing entity in its own Errors map",
            "POST",
            "/BatchDescribeEntities",
            None,
            Some(json!({
                "EntityRequestList": [{"Catalog": CATALOG, "EntityId": "prod-nosuchentity01"}],
            })),
            200,
            "",
        ),
        (
            "catalog: an assessment that does not exist",
            "POST",
            "/DescribeAssessment",
            None,
            Some(json!({"Catalog": CATALOG, "AssessmentIdentifier": "assessment-1a2b3c"})),
            404,
            "ResourceNotFoundException",
        ),
        (
            "catalog: an empty list of entities",
            "POST",
            "/ListEntities",
            None,
            Some(json!({"Catalog": CATALOG, "EntityType": "AmiProduct"})),
            200,
            "",
        ),
        (
            "catalog: tags of a resource outside this catalog",
            "POST",
            "/ListTagsForResource",
            None,
            Some(json!({"ResourceArn": FOREIGN_RESOURCE_ARN})),
            403,
            "AccessDeniedException",
        ),
        (
            "catalog: the policy of a resource outside this catalog",
            "GET",
            "/GetResourcePolicy?resourceArn=arn%3Aaws%3Aaws-marketplace%3Aus-east-1%3A000000000000%3Aother%2Fthing",
            None,
            None,
            403,
            "AccessDeniedException",
        ),
        (
            // A request addressing a REST peer's path while naming an RPC peer in
            // `X-Amz-Target` contradicts itself. The path's owner serves it (a REST
            // claim is its path, and that header is nothing to it) and the RPC peer it
            // names does NOT also claim it — which would be two claimants, the 500 this
            // issue is about.
            "catalog: its own path, carrying an RPC peer's target",
            "POST",
            "/ListChangeSets",
            Some("AWSMPMeteringService.ResolveCustomer"),
            Some(json!({"Catalog": CATALOG})),
            200,
            "",
        ),
        (
            // `GetBuyerDashboard` is "available only to Amazon Web Services Organization
            // management accounts or delegated administrators registered for the
            // procurement insights … feature", a grant AWS Organizations holds and this
            // router's fresh engine has no record of — so Reporting answers its own
            // modelled `UnauthorizedException`, which it decides before reading the
            // identifier. Neither of the router's routing failures could produce it, so
            // the row still pins ownership of this path.
            "reporting: a caller with no procurement-insights grant on record",
            "POST",
            "/getBuyerDashboard",
            None,
            Some(json!({
                "dashboardIdentifier": "arn:aws:aws-marketplace",
                "embeddingDomains": ["https://a.example.com"],
            })),
            401,
            "UnauthorizedException",
        ),
        (
            "deployment: a parameter write",
            "POST",
            "/catalogs/AWSMarketplace/products/prod-1/deployment-parameters",
            None,
            Some(json!({
                "agreementId": "agmt-1",
                "deploymentParameter": {"name": "param", "secretString": "secret"},
            })),
            200,
            "",
        ),
        (
            "deployment: tags of a parameter that does not exist",
            "GET",
            "/tags/arn%3Aaws%3Aaws-marketplace%3Aus-east-1%3A000000000000%3Adeployment-parameter%2Fd-1",
            None,
            None,
            404,
            // Marketplace Deployment answers an ARN it holds no record for with its
            // own recorded code `NotFoundException`, not the modelled
            // `ResourceNotFoundException` shape name — so this code, too, is
            // answered by exactly one service of the family.
            "NotFoundException",
        ),
        (
            "discovery: a listing that does not exist",
            "POST",
            "/2026-02-05/getListing",
            None,
            Some(json!({"listingId": "no-such-listing"})),
            404,
            "ResourceNotFoundException",
        ),
        (
            "discovery: a listing search",
            "POST",
            "/2026-02-05/searchListings",
            None,
            Some(json!({})),
            200,
            "",
        ),
        (
            "entitlement: entitlements for a product with none",
            "POST",
            "/",
            Some("AWSMPEntitlementService.GetEntitlements"),
            Some(json!({"ProductCode": "no-such-product"})),
            200,
            "",
        ),
        (
            "agreement: an agreement that does not exist",
            "POST",
            "/",
            Some("AWSMPCommerceService_v20200301.DescribeAgreement"),
            Some(json!({"agreementId": "no-such-agreement"})),
            // The Agreement service answers a lookup that found nothing with the
            // modelled `ResourceNotFoundException` body at HTTP 400 — the status
            // its recorded responses carry with that body, which its model's
            // `@httpError(404)` understates (see that crate's
            // `state::NOT_FOUND_STATUS`). The point of this row is unchanged:
            // only the Agreement service models this operation, so only it can
            // answer here.
            400,
            "ResourceNotFoundException",
        ),
        (
            "metering: a registration token it rejects",
            "POST",
            "/",
            Some("AWSMPMeteringService.ResolveCustomer"),
            Some(json!({"RegistrationToken": "token"})),
            400,
            // `InvalidTokenException` is modelled by Marketplace Metering alone, so
            // only Metering can answer it.
            "InvalidTokenException",
        ),
    ];

    for (case, method, uri, target, body, expected_status, expected_code) in rows {
        let answer = dispatch(&router, method, uri, target, body).await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?} from the one \
             service that owns {uri}, got {} {:?} ({:?})",
            answer.status,
            answer.code,
            answer.message
        );
    }
}

/// A request signed for `aws-marketplace` that addresses NO service's surface is the
/// router's `404 UnknownService` — never the `500 AmbiguousService` several claimants
/// produced, and never one service's answer to another's traffic.
#[tokio::test]
async fn a_request_on_no_marketplace_surface_is_unclaimed() {
    let (router, _engine) = router();

    // (case, method, uri, x-amz-target)
    let rows: [(&str, &str, &str, Option<&str>); 6] = [
        ("a path no model binds", "POST", "/NoSuchOperation", None),
        ("another family member's style of path", "GET", "/v1/things/t-1", None),
        ("a path no family member declares", "POST", "/NotAnOperation", None),
        ("a root post naming no service", "POST", "/", None),
        (
            "a target naming no service of this family",
            "POST",
            "/",
            Some("NotAMarketplaceService.DoThing"),
        ),
        (
            "an rpcv2Cbor uri naming no service of this family",
            "POST",
            "/service/NotAMarketplaceService/operation/DoThing",
            None,
        ),
    ];

    for (case, method, uri, target) in rows {
        let answer = dispatch(&router, method, uri, target, None).await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (404, "UnknownService"),
            "case {case:?}: expected the router's 404 UnknownService, got {} {:?} ({:?})",
            answer.status,
            answer.code,
            answer.message
        );
    }
}
