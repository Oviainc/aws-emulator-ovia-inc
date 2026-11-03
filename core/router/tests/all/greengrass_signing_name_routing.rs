// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `greengrass` SigV4 signing name.
//!
//! TWO emulated services share it: IoT Greengrass (2017-06-07) and IoT Greengrass V2
//! (2020-11-30). Both models declare `endpointPrefix`, `arnNamespace` AND `sigv4Name`
//! `greengrass`, both are `restJson1`, and both bind their operations under the SAME
//! first URI path segment (`greengrass`, plus the `tags` space). The router resolves a
//! request's routing key from the SigV4 credential scope first, so a service that claims
//! on the signing name — or on that first segment — claims the other version's traffic
//! too: every Greengrass request has two claimants and the router, which refuses rather
//! than serving a request by an arbitrary one, answers `500 AmbiguousService`, a code no
//! AWS model declares.
//!
//! The rule these rows pin, which `service_common::greengrass` states once for the pair:
//! a request signed for the signing name both API versions share is claimed by exactly
//! ONE of them — the version whose own model binds a route for the request's method and
//! URI, and, for a route BOTH models bind, the single claimant that module's table names:
//! the account- and thing-level facts both versions publish go to its declared owner, and
//! a tagging request to the version that owns the resource the ARN in its path names.
//!
//! Rows span what that rule generalises over: each version's own path space, every route
//! both models bind, both ARN conventions on the tagging space, an unsigned request
//! addressed by endpoint host, a request signed for another service, and a method or path
//! no model binds at all. The second test then pins the consequence: the resolved claimant
//! SERVES the request from its own state, so a published component version round-trips and
//! an absent one answers the operation's modelled 404.
//!
//! Each crate's own `tests/all/shared_endpoint_routing.rs` pins one HALF of the split over
//! a router holding only that service. What this suite adds is the two mounted TOGETHER,
//! which is the configuration `core/router` ships and the only one in which a row with two
//! claimants is observable at all.

use std::sync::Arc;

use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// A SigV4 `Authorization` header whose credential scope names the signing name both
/// services share — the case the ambiguity came from. The signature is never checked:
/// the routers here run with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTGREENGRASS/20260101/us-east-1/greengrass/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The same header for a request signed for a service outside this family.
const SQS_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTSQS/20260101/us-east-1/sqs/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// How a row names the version that must claim it, by API version date. `NOBODY` is the
/// router's `404 UnknownService`; two claimants render as a list, which is the
/// `500 AmbiguousService` this suite exists to exclude.
const V1: &str = "2017-06-07";
const V2: &str = "2020-11-30";
const NOBODY: &str = "<nobody>";

/// The host a client configured with the emulator's own endpoint sends, which names no
/// AWS endpoint at all — the form every recorded request arrives in.
const EMULATOR_HOST: &str = "localhost:9297";

/// The AWS endpoint host both API versions are served on.
const AWS_HOST: &str = "greengrass.us-east-1.amazonaws.com";

/// The account every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The router's own refusals: either one means the request reached NOBODY
/// (`404 UnknownService`) or BOTH claimants (`500 AmbiguousService`) instead of the one
/// version that owns the route.
const ROUTER_REFUSALS: [&str; 2] = ["AmbiguousService", "UnknownService"];

type Greengrass =
    service_com_amazonaws_greengrass_greengrass::EmulatedService<engine::InMemoryEngine>;
type GreengrassV2 =
    service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService<engine::InMemoryEngine>;

/// Both services that share the `greengrass` signing name, over ONE shared engine
/// exactly as `router/src/lib.rs` mounts them.
struct Family {
    v1: Greengrass,
    v2: GreengrassV2,
}

impl Family {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        Self {
            v1: Greengrass::new(Arc::clone(&engine)),
            v2: GreengrassV2::new(engine),
        }
    }

    /// Who claims a request: the one API version that does, [`NOBODY`], or — the failure
    /// this suite excludes — both, comma-separated.
    fn claimant(&self, parts: &http::request::Parts) -> String {
        let claimants: Vec<&str> = [(V1, self.v1.can_handle(parts)), (V2, self.v2.can_handle(parts))]
            .into_iter()
            .filter_map(|(name, claims)| claims.then_some(name))
            .collect();
        match claimants.as_slice() {
            [] => NOBODY.to_string(),
            [one] => (*one).to_string(),
            many => many.join(", "),
        }
    }
}

/// A router with both of them mounted, so a request both versions claim really is
/// refused as `AmbiguousService` rather than quietly served by the first.
fn router_with_the_family() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(Greengrass::new(Arc::clone(&engine)));
    router.register_service(GreengrassV2::new(engine));
    router
}

/// The request parts the router hands to `Service::can_handle`.
fn parts(method: &str, host: &str, uri: &str, authorization: Option<&str>) -> http::request::Parts {
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json");
    if let Some(authorization) = authorization {
        request = request.header("authorization", authorization);
    }
    request
        .body(Bytes::new())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

/// An ARN as a client puts it in a non-greedy `@httpLabel`: its `:` and `/`
/// percent-encoded.
fn label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// The name-level component ARN the 2020-11-30 API mints.
fn component_arn(name: &str) -> String {
    format!("arn:aws:greengrass:us-east-1:{ACCOUNT}:components:{name}")
}

/// The component-version ARN the 2020-11-30 API mints.
fn component_version_arn(name: &str, version: &str) -> String {
    format!("{}:versions:{version}", component_arn(name))
}

/// One answer from the router, reduced to what the rows assert on.
struct Answer {
    status: u16,
    /// The wire error-shape name: `x-amzn-errortype` (which `restJson1` resolves the
    /// shape from), falling back to the body's `__type` so a router refusal — which uses
    /// the generic `awsJson` envelope — is still recognised.
    error_code: String,
    content_type: String,
    body: Value,
}

impl Answer {
    fn str_at(&self, key: &str) -> String {
        self.body
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Whether the router itself refused the request instead of a service answering it.
    fn refused_by_the_router(&self) -> bool {
        ROUTER_REFUSALS.contains(&self.error_code.as_str())
    }
}

/// Dispatch one `greengrass`-signed request through both versions.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("a JSON value serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{AWS_HOST}{uri}"))
        .header("host", AWS_HOST)
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let error_type = header("x-amzn-errortype");
    let raw = response.into_body();
    let body: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    let error_code = if error_type.is_empty() {
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        error_type
    };
    Answer {
        status,
        error_code,
        content_type,
        body,
    }
}

#[test]
fn exactly_one_greengrass_api_version_claims_each_request() {
    let family = Family::new();

    // Each row is (case, method, host, uri, authorization, the API version that must
    // claim it). The tuple's type is left to inference: written out it is six fields
    // wide, which `clippy::type_complexity` rejects.
    //
    // Every operation path comes from the two models' `smithy.api#http` bindings
    // (`smithy_query(query="digest")` on each).
    let rows = vec![
        // The 2020-11-30 API's own path space: everything it binds lives under
        // `/greengrass/v2/…`.
        (
            "V2 CreateComponentVersion",
            "POST",
            EMULATOR_HOST,
            "/greengrass/v2/createComponentVersion".to_string(),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "V2 ListComponents",
            "GET",
            EMULATOR_HOST,
            "/greengrass/v2/components".to_string(),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "V2 ListComponentVersions",
            "GET",
            EMULATOR_HOST,
            format!(
                "/greengrass/v2/components/{}/versions",
                label(&component_arn("com.example.App"))
            ),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "V2 DescribeComponent",
            "GET",
            EMULATOR_HOST,
            format!(
                "/greengrass/v2/components/{}/metadata",
                label(&component_version_arn("com.example.App", "1.0.0"))
            ),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "V2 DeleteComponent",
            "DELETE",
            EMULATOR_HOST,
            format!(
                "/greengrass/v2/components/{}",
                label(&component_version_arn("com.example.App", "1.0.0"))
            ),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "V2 ResolveComponentCandidates",
            "POST",
            EMULATOR_HOST,
            "/greengrass/v2/resolveComponentCandidates".to_string(),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "V2 ListInstalledComponents",
            "GET",
            EMULATOR_HOST,
            "/greengrass/v2/coreDevices/probe/installedComponents".to_string(),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "V2 CreateDeployment",
            "POST",
            EMULATOR_HOST,
            "/greengrass/v2/deployments".to_string(),
            Some(AUTHORIZATION),
            V2,
        ),
        // The 2017-06-07 API's own path space: groups, definitions, bulk deployments,
        // software updates and a thing's runtime configuration.
        (
            "V1 CreateGroup",
            "POST",
            EMULATOR_HOST,
            "/greengrass/groups".to_string(),
            Some(AUTHORIZATION),
            V1,
        ),
        (
            "V1 GetGroupVersion",
            "GET",
            EMULATOR_HOST,
            "/greengrass/groups/g1/versions/v1".to_string(),
            Some(AUTHORIZATION),
            V1,
        ),
        (
            "V1 ListCoreDefinitions",
            "GET",
            EMULATOR_HOST,
            "/greengrass/definition/cores".to_string(),
            Some(AUTHORIZATION),
            V1,
        ),
        (
            "V1 StartBulkDeployment",
            "POST",
            EMULATOR_HOST,
            "/greengrass/bulk/deployments".to_string(),
            Some(AUTHORIZATION),
            V1,
        ),
        (
            "V1 CreateSoftwareUpdateJob",
            "POST",
            EMULATOR_HOST,
            "/greengrass/updates".to_string(),
            Some(AUTHORIZATION),
            V1,
        ),
        (
            "V1 GetThingRuntimeConfiguration",
            "GET",
            EMULATOR_HOST,
            "/greengrass/things/probe/runtimeconfig".to_string(),
            Some(AUTHORIZATION),
            V1,
        ),
        (
            "V1 ResetDeployments, whose URI carries a literal '$'",
            "POST",
            EMULATOR_HOST,
            "/greengrass/groups/g1/deployments/$reset".to_string(),
            Some(AUTHORIZATION),
            V1,
        ),
        // The routes BOTH models bind: the account service role (three methods each)
        // and a core device's connectivity information (two each). One account-level and
        // one thing-level fact, published by both versions, so each goes to the single
        // claimant `service_common::greengrass` declares for it.
        (
            "the service role, read",
            "GET",
            EMULATOR_HOST,
            "/greengrass/servicerole".to_string(),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "the service role, associated",
            "PUT",
            EMULATOR_HOST,
            "/greengrass/servicerole".to_string(),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "the service role, disassociated",
            "DELETE",
            EMULATOR_HOST,
            "/greengrass/servicerole".to_string(),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "connectivity information, read",
            "GET",
            EMULATOR_HOST,
            "/greengrass/things/probe/connectivityInfo".to_string(),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "connectivity information, updated",
            "PUT",
            EMULATOR_HOST,
            "/greengrass/things/probe/connectivityInfo".to_string(),
            Some(AUTHORIZATION),
            V2,
        ),
        // The tagging routes both models bind, split by the resource the ARN in the path
        // names: a 2017-06-07 ARN carries that API's own URI path as its resource, a
        // 2020-11-30 one a `<type>:<id>` resource.
        (
            "tags of a component version",
            "GET",
            EMULATOR_HOST,
            format!(
                "/tags/{}",
                label(&component_version_arn("com.example.App", "1.0.0"))
            ),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "tags of a core device, raw separators",
            "POST",
            EMULATOR_HOST,
            format!("/tags/arn:aws:greengrass:us-east-1:{ACCOUNT}:coreDevices:probe"),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "tags of a deployment",
            "DELETE",
            EMULATOR_HOST,
            format!(
                "/tags/{}",
                label(&format!(
                    "arn:aws:greengrass:us-east-1:{ACCOUNT}:deployments:d-1"
                ))
            ),
            Some(AUTHORIZATION),
            V2,
        ),
        (
            "tags of a V1 group, whose ARN resource is a path",
            "GET",
            EMULATOR_HOST,
            format!(
                "/tags/{}",
                label(&format!(
                    "arn:aws:greengrass:us-east-1:{ACCOUNT}:/greengrass/groups/g1"
                ))
            ),
            Some(AUTHORIZATION),
            V1,
        ),
        (
            "tags of a label that is no ARN, which names no 2017-06-07 resource",
            "GET",
            EMULATOR_HOST,
            "/tags/not-an-arn".to_string(),
            Some(AUTHORIZATION),
            V2,
        ),
        // An UNSIGNED request addressed by the AWS endpoint host: the routing key comes
        // from the host label, which is the prefix both versions publish, so the route
        // is again what places it.
        (
            "unsigned, host-addressed V2 listing",
            "GET",
            AWS_HOST,
            "/greengrass/v2/coreDevices".to_string(),
            None,
            V2,
        ),
        (
            "unsigned, host-addressed V1 listing",
            "GET",
            AWS_HOST,
            "/greengrass/groups".to_string(),
            None,
            V1,
        ),
        // Requests neither version serves.
        (
            "signed for another service entirely",
            "GET",
            EMULATOR_HOST,
            "/greengrass/v2/components".to_string(),
            Some(SQS_AUTHORIZATION),
            NOBODY,
        ),
        (
            "a path no Greengrass model binds",
            "GET",
            EMULATOR_HOST,
            "/greengrass/v2/widgets".to_string(),
            Some(AUTHORIZATION),
            NOBODY,
        ),
        (
            "a method no model binds on a V2 route",
            "PATCH",
            EMULATOR_HOST,
            "/greengrass/v2/components".to_string(),
            Some(AUTHORIZATION),
            NOBODY,
        ),
        // A request on a route both models bind is matched on its METHOD too, so a
        // method neither of them publishes there reaches nobody, exactly as it does on a
        // route only one of them binds.
        (
            "a method no model binds on a route BOTH bind",
            "PATCH",
            EMULATOR_HOST,
            "/greengrass/servicerole".to_string(),
            Some(AUTHORIZATION),
            NOBODY,
        ),
        (
            "the tagging space with no ARN label at all",
            "GET",
            EMULATOR_HOST,
            "/tags".to_string(),
            Some(AUTHORIZATION),
            NOBODY,
        ),
        (
            "the root path",
            "GET",
            EMULATOR_HOST,
            "/".to_string(),
            Some(AUTHORIZATION),
            NOBODY,
        ),
    ];

    for (case, method, host, uri, authorization, expected) in rows {
        let actual = family.claimant(&parts(method, host, &uri, authorization));
        assert_eq!(
            actual, expected,
            "case {case:?} ({method} {uri} as {host:?}): expected claimant {expected:?}, \
             got {actual:?}"
        );
    }
}

#[tokio::test]
async fn the_claiming_version_serves_the_request_from_its_own_state() {
    let router = router_with_the_family();
    let recipe = "RecipeFormatVersion: '2020-01-25'\n\
                  ComponentName: com.example.Router\n\
                  ComponentVersion: 1.2.3\n";

    // The 2020-11-30 API publishes a component version through the shared endpoint.
    let created = call(
        &router,
        "POST",
        "/greengrass/v2/createComponentVersion",
        json!({ "inlineRecipe": base64::encode(recipe.as_bytes()) }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "CreateComponentVersion: expected 201, got {} ({:?})",
        created.status, created.body
    );
    assert_eq!(
        created.content_type, "application/json",
        "CreateComponentVersion: expected content-type application/json, got {:?}",
        created.content_type
    );
    assert_eq!(
        created.str_at("componentName"),
        "com.example.Router",
        "CreateComponentVersion: expected the recipe's componentName, got {:?}",
        created.body
    );

    // …and the same endpoint reads it back, which only the version that stored it can
    // answer.
    let listed = call(
        &router,
        "GET",
        &format!(
            "/greengrass/v2/components/{}/versions",
            label(&created.str_at("arn").replace(":versions:1.2.3", ""))
        ),
        Value::Null,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "ListComponentVersions: expected 200, got {} ({:?})",
        listed.status, listed.body
    );
    let versions = listed
        .body
        .get("componentVersions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        versions.len(),
        1,
        "ListComponentVersions: expected the one published version, got {:?}",
        listed.body
    );

    // An absent component version answers the operation's own modelled 404, not a
    // router refusal.
    let missing = call(
        &router,
        "GET",
        &format!(
            "/greengrass/v2/components/{}/metadata",
            label(&component_version_arn("com.example.Absent", "9.9.9"))
        ),
        Value::Null,
    )
    .await;
    assert_eq!(
        missing.status, 404,
        "DescribeComponent on an absent version: expected 404, got {} ({:?})",
        missing.status, missing.body
    );
    assert!(
        missing.error_code.contains("ResourceNotFoundException"),
        "DescribeComponent on an absent version: expected ResourceNotFoundException, got \
         {:?}",
        missing.error_code
    );

    // The 2017-06-07 API is reached through the SAME endpoint and answers from its own
    // store.
    let group = call(
        &router,
        "POST",
        "/greengrass/groups",
        json!({ "Name": "router-group" }),
    )
    .await;
    assert!(
        !group.refused_by_the_router(),
        "CreateGroup: expected the 2017-06-07 service to answer, got {:?} ({:?})",
        group.error_code,
        group.body
    );
    assert_eq!(
        group.str_at("Name"),
        "router-group",
        "CreateGroup: expected the submitted group name, got {:?}",
        group.body
    );

    // A URI shape both models bind reaches its declared owner, so it is answered by a
    // service rather than refused as ambiguous. Those operations are not implemented in
    // either version yet, so what is asserted here is only that the router no longer
    // refuses them.
    for (case, method, uri) in [
        ("the service role", "GET", "/greengrass/servicerole"),
        (
            "connectivity information",
            "GET",
            "/greengrass/things/probe/connectivityInfo",
        ),
    ] {
        let answer = call(&router, method, uri, Value::Null).await;
        assert!(
            !answer.refused_by_the_router(),
            "case {case:?}: expected one service to answer, got {:?} ({:?})",
            answer.error_code,
            answer.body
        );
    }
}
