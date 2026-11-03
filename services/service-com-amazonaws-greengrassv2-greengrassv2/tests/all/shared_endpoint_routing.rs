// SPDX-License-Identifier: CC0-1.0

//! Which requests the 2020-11-30 Greengrass API CLAIMS, and which it leaves to the
//! 2017-06-07 one.
//!
//! AWS serves both API versions from ONE endpoint: both models declare the endpoint
//! prefix, the ARN namespace and the SigV4 name `greengrass`, so the signing name alone
//! cannot tell them apart — a router that matched on it would see two claimants for every
//! Greengrass request and refuse all of them with a code no AWS model declares. What
//! separates them is the `restJson1` route each request addresses, which is what AWS
//! itself dispatches on, and for the few routes both models bind identically a single
//! declared claimant: the account- and thing-level facts both versions publish, and a
//! tagging request placed by the resource ARN in its path.
//!
//! This suite pins THIS half of the split, over a router holding only this service, so
//! "claimed" and "not claimed" are observable as the service's own answer versus
//! `404 UnknownService`. The other half — the 2017-06-07 service claiming its own routes
//! and the tagging requests whose ARN names one of its resources, and leaving this
//! version's alone — is pinned by the same suite in that crate
//! (`tests/all/shared_endpoint_routing.rs`) over the same rows. Together they say that no
//! row has two claimants and none has zero.

use bytes::Bytes;
use service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A SigV4 header whose credential scope names the signing name BOTH Greengrass models
/// declare, which is the routing key the router resolves first.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/greengrass/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The host a client configured with one local endpoint sends — it names no AWS endpoint
/// prefix, so nothing but the route places the request.
const HOST: &str = "127.0.0.1:9297";

/// A router serving ONLY this service.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One claim row: the case name, the method, the URI, and whether THIS service must claim
/// the request.
type ClaimCase = (&'static str, &'static str, String, bool);

/// A resource ARN as a client percent-encodes a non-greedy `@httpLabel`.
fn encoded(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// Every row both halves of the split are checked over, with the claim as the 2020-11-30
/// service must answer it.
///
/// The rows span what the rule generalises over: a route only this model binds, a route
/// only its peer binds, a route BOTH bind (where the single declared claimant decides),
/// the tagging routes both bind (where the ARN in the path decides, under both ARN
/// conventions), and a method or path neither model binds at all.
///
/// Source for every route: `smithy_query(query="digest")` on the two models.
fn rows() -> Vec<ClaimCase> {
    vec![
        // Routes only the 2020-11-30 model binds: its whole `/greengrass/v2/…` space.
        (
            "a component version created",
            "POST",
            "/greengrass/v2/createComponentVersion".to_string(),
            true,
        ),
        (
            "the component list",
            "GET",
            "/greengrass/v2/components".to_string(),
            true,
        ),
        (
            "one component's versions",
            "GET",
            format!(
                "/greengrass/v2/components/{}/versions",
                encoded("arn:aws:greengrass:us-east-1:000000000000:components:probe")
            ),
            true,
        ),
        (
            "a core device read",
            "GET",
            "/greengrass/v2/coreDevices/laser-1".to_string(),
            true,
        ),
        (
            "a deployment created",
            "POST",
            "/greengrass/v2/deployments".to_string(),
            true,
        ),
        (
            "a deployment deleted",
            "DELETE",
            "/greengrass/v2/deployments/d-1".to_string(),
            true,
        ),
        (
            "component candidates resolved",
            "POST",
            "/greengrass/v2/resolveComponentCandidates".to_string(),
            true,
        ),
        // Routes only the 2017-06-07 model binds: groups, the definition resources, bulk
        // deployments, software updates and a thing's runtime configuration.
        (
            "a group created",
            "POST",
            "/greengrass/groups".to_string(),
            false,
        ),
        (
            "the group list",
            "GET",
            "/greengrass/groups".to_string(),
            false,
        ),
        (
            "the core definition list",
            "GET",
            "/greengrass/definition/cores".to_string(),
            false,
        ),
        (
            "a function definition created",
            "POST",
            "/greengrass/definition/functions".to_string(),
            false,
        ),
        (
            "a thing's runtime configuration",
            "GET",
            "/greengrass/things/laser-1/runtimeconfig".to_string(),
            false,
        ),
        (
            "the bulk deployment list",
            "GET",
            "/greengrass/bulk/deployments".to_string(),
            false,
        ),
        (
            "a group deployment created",
            "POST",
            "/greengrass/groups/g-1/deployments".to_string(),
            false,
        ),
        // Routes BOTH models bind, about one account-level and one thing-level fact each
        // version publishes: the single declared claimant answers them.
        (
            "the account's service role read",
            "GET",
            "/greengrass/servicerole".to_string(),
            true,
        ),
        (
            "the account's service role associated",
            "PUT",
            "/greengrass/servicerole".to_string(),
            true,
        ),
        (
            "the account's service role disassociated",
            "DELETE",
            "/greengrass/servicerole".to_string(),
            true,
        ),
        (
            "a thing's connectivity info read",
            "GET",
            "/greengrass/things/laser-1/connectivityInfo".to_string(),
            true,
        ),
        (
            "a thing's connectivity info written",
            "PUT",
            "/greengrass/things/laser-1/connectivityInfo".to_string(),
            true,
        ),
        // The tagging routes both models bind, placed by the ARN in the path: the
        // 2020-11-30 resources carry a `<type>:<id>` resource field, the 2017-06-07 ones
        // carry that API's own URI path.
        (
            "a component version's tags listed",
            "GET",
            format!(
                "/tags/{}",
                encoded(
                    "arn:aws:greengrass:us-east-1:000000000000:components:probe:versions:1.0.0"
                )
            ),
            true,
        ),
        (
            "a deployment tagged",
            "POST",
            format!(
                "/tags/{}",
                encoded("arn:aws:greengrass:us-east-1:000000000000:deployments:d-1")
            ),
            true,
        ),
        (
            "a group untagged",
            "DELETE",
            format!(
                "/tags/{}",
                encoded("arn:aws:greengrass:us-east-1:000000000000:/greengrass/groups/g-1")
            ),
            false,
        ),
        (
            "a core definition version's tags listed",
            "GET",
            format!(
                "/tags/{}",
                encoded(
                    "arn:aws:greengrass:us-west-2:000000000000:/greengrass/definition/cores/c-1/versions/v-1"
                )
            ),
            false,
        ),
        (
            "a label that is no ARN at all",
            "POST",
            "/tags/not-an-arn".to_string(),
            true,
        ),
        (
            "another service's ARN",
            "GET",
            format!(
                "/tags/{}",
                encoded("arn:aws:iot:us-east-1:000000000000:thing/laser-1")
            ),
            true,
        ),
        // Neither model binds these, so neither version claims them and the router
        // answers them as the unknown endpoint they address.
        (
            "a method neither model binds on a shared path",
            "POST",
            "/greengrass/servicerole".to_string(),
            false,
        ),
        (
            "a path inside the 2020-11-30 space that no route binds",
            "GET",
            "/greengrass/v2/nothing-here".to_string(),
            false,
        ),
        (
            "the root path",
            "POST",
            "/".to_string(),
            false,
        ),
    ]
}

#[tokio::test]
async fn the_modelled_route_decides_which_api_version_claims_a_request() {
    let router = router();
    for (case, method, uri, claimed) in rows() {
        let request = http::Request::builder()
            .method(method)
            .uri(&uri)
            .header("host", HOST)
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(Bytes::from_static(b"{}"))
            .unwrap_or_else(|error| panic!("case {case:?}: building the request: {error}"));
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let rendered = String::from_utf8_lossy(response.body()).to_string();
        let code = error_code(&rendered);
        assert!(
            code.as_deref() != Some("AmbiguousService"),
            "case {case:?} ({method} {uri}): a request must never be refused for having \
             several claimants, got {status} with {rendered}"
        );
        let unclaimed = code.as_deref() == Some("UnknownService");
        assert_eq!(
            !unclaimed,
            claimed,
            "case {case:?} ({method} {uri}): expected claimed={claimed}, got \
             claimed={} from {status} with {rendered}",
            !unclaimed
        );
    }
}

/// The error code a router refusal carries, or `None` for an answer that is not one: the
/// `__type` of the JSON body, with any namespace prefix dropped (`UnknownService` and
/// `AmbiguousService` are the router's own two).
fn error_code(body: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    let rendered = parsed.get("__type")?.as_str()?;
    Some(
        rendered
            .rsplit(['#', '.'])
            .next()
            .unwrap_or(rendered)
            .to_string(),
    )
}
