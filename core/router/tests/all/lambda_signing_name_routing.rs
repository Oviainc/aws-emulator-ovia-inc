// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `lambda` SigV4 signing name, which FOUR emulated
//! services share.
//!
//! The Lambda control plane (`AWSGirApiService`), Lambda Core, Lambda MicroVMs and Lambda
//! Web are four emulated services whose models all declare `sigv4Name: lambda` and
//! `arnNamespace: lambda` (`smithy_query(query="service_info")` on each). The router
//! resolves a request's routing key from the SigV4 credential scope first, so all four see
//! the key `lambda`: a service that claims on the signing name alone claims its peers'
//! traffic too, the router finds four claimants and answers `500 AmbiguousService` — an
//! error code that exists in no AWS model, and a retryable 5xx that burns a client's whole
//! retry budget — for every Lambda request, leaving CreateFunction and every other Lambda
//! operation unreachable.
//!
//! The rule these suites pin: a request signed for a signing name several emulated
//! services share is claimed by exactly ONE of them — the one whose own model binds the
//! request's URI path, and, on a path space two models bind identically (the
//! `/2017-03-31/tags/{Resource}` tagging URIs), the one that owns the resource the ARN in
//! that path names — and the answer that comes back is that service's own.
//!
//! Rows therefore span the dimensions the rule generalises over: which of the four
//! endpoints owns the operation, every API-date path segment the four models bind, both
//! wire forms of a path-label ARN (percent-encoded and raw), an ARN no `lambda` model
//! knows, and a `Host` that agrees with, disagrees with or says nothing about the target —
//! because on a signed request the credential scope, not the host, resolves the key.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// A SigV4 `Authorization` header whose credential scope names the signing name all four
/// services share. The signature is never checked: the routers here run with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTLAMBDA/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The region the credential scope above names, and so the region of every ARN the
/// services below mint.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// An execution role ARN satisfying the control plane's modelled `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A `Code.ZipFile` payload: restJson1 carries a blob base64-encoded in the JSON body.
/// This is the 22-byte empty-archive zip, base64-encoded.
const ZIP_FILE: &str = "UEsFBgAAAAAAAAAAAAAAAAAAAAAAAA==";

/// How a row names the service that must claim it. `MANY` and `NONE` are the two routing
/// failures: `MANY` is the `500 AmbiguousService` this issue is about, `NONE` the router's
/// `404 UnknownService`.
const OWNER_CONTROL_PLANE: &str = "lambda (the control plane)";
const OWNER_CORE: &str = "lambda core";
const OWNER_MICROVMS: &str = "lambda microvms";
const OWNER_WEB: &str = "lambda web";
const OWNER_MANY: &str = "several (the router answers 500 AmbiguousService)";
const OWNER_NONE: &str = "none (the router answers 404 UnknownService)";

type ControlPlane =
    service_com_amazonaws_lambda_awsgirapiservice::EmulatedService<engine::InMemoryEngine>;
type Core =
    service_com_amazonaws_lambdacore_lambdacoreapiservice::EmulatedService<engine::InMemoryEngine>;
type Microvms =
    service_com_amazonaws_lambdamicrovms_lambdamicrovms::EmulatedService<engine::InMemoryEngine>;
type Web = service_com_amazonaws_lambdaweb_lambdaweb::EmulatedService<engine::InMemoryEngine>;

/// The four services that share the `lambda` signing name, over one shared engine,
/// exactly as `core/router/src/lib.rs` mounts them.
fn all_four() -> (ControlPlane, Core, Microvms, Web) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (
        ControlPlane::new(Arc::clone(&engine)),
        Core::new(Arc::clone(&engine)),
        Microvms::new(Arc::clone(&engine)),
        Web::new(engine),
    )
}

/// A router with all four mounted, so a request that two of them claim really is refused
/// as `AmbiguousService`.
fn router_with_all_four() -> MultiServiceRouter {
    let (control_plane, core, microvms, web) = all_four();
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(control_plane);
    router.register_service(core);
    router.register_service(microvms);
    router.register_service(web);
    router
}

/// The request parts the router hands to `Service::can_handle`.
fn parts(method: &str, host: &str, uri: &str) -> http::request::Parts {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::new())
        .expect("the test request is well formed");
    request.into_parts().0
}

/// An ARN as a client puts it in the non-greedy `{Resource}` path label: its `:` and `/`
/// percent-encoded.
fn label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// The service that claims a request, as one of the six `OWNER_*` outcomes.
fn claimant(
    services: &(ControlPlane, Core, Microvms, Web),
    parts: &http::request::Parts,
) -> &'static str {
    let (control_plane, core, microvms, web) = services;
    let claims = [
        (control_plane.can_handle(parts), OWNER_CONTROL_PLANE),
        (core.can_handle(parts), OWNER_CORE),
        (microvms.can_handle(parts), OWNER_MICROVMS),
        (web.can_handle(parts), OWNER_WEB),
    ];
    let claimants: Vec<&'static str> = claims
        .iter()
        .filter_map(|(claimed, owner)| claimed.then_some(*owner))
        .collect();
    match claimants.as_slice() {
        [one] => one,
        [] => OWNER_NONE,
        _ => OWNER_MANY,
    }
}

/// One answer from the router: the HTTP status, the restJson1 `__type` error code (the
/// empty string on a success, which carries no `__type`) and the parsed body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Value,
) -> (u16, String, Value) {
    let payload = if body.is_null() {
        String::new()
    } else {
        body.to_string()
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost:9297")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, parsed)
}

#[test]
fn exactly_one_service_sharing_the_lambda_signing_name_claims_each_request() {
    let services = all_four();

    // (case, method, host, uri, the service that must claim it)
    //
    // The operation paths come from each model's `smithy.api#http` bindings
    // (`smithy_query(query="digest", …)` on the four models): the control plane binds
    // `2014-11-13` … `2026-07-09`, Lambda Core `2026-04-04`, Lambda MicroVMs `2025-09-09`
    // plus the shared `2017-03-31`, and Lambda Web `2025-03-07`. The tagging ARNs are
    // built from the resource segments the two `TaggableResource` patterns pin.
    let rows: Vec<(String, &str, &str, String, &str)> = vec![
        // The control plane's own operations, on hosts that name the Lambda endpoint and
        // on a bare emulator endpoint.
        (
            "control plane CreateFunction".to_string(),
            "POST",
            "lambda.us-east-1.amazonaws.com",
            "/2015-03-31/functions".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "control plane CreateFunction on a bare emulator endpoint".to_string(),
            "POST",
            "localhost:9297",
            "/2015-03-31/functions".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "control plane GetFunction".to_string(),
            "GET",
            "localhost:9297",
            "/2015-03-31/functions/probe".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "control plane InvokeAsync, its oldest API date".to_string(),
            "POST",
            "localhost:9297",
            "/2014-11-13/functions/probe/invoke-async".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "control plane GetAccountSettings".to_string(),
            "GET",
            "localhost:9297",
            "/2016-08-19/account-settings".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "control plane ListLayers".to_string(),
            "GET",
            "localhost:9297",
            "/2018-10-31/layers".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "control plane ListCodeSigningConfigs".to_string(),
            "GET",
            "localhost:9297",
            "/2020-04-22/code-signing-configs".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "control plane GetFunctionUrlConfig".to_string(),
            "GET",
            "localhost:9297",
            "/2021-10-31/functions/probe/url".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "control plane ListCapacityProviders".to_string(),
            "GET",
            "localhost:9297",
            "/2025-11-30/capacity-providers".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "control plane GetDurableExecution".to_string(),
            "GET",
            "localhost:9297",
            "/2025-12-01/durable-executions/exec-1".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "control plane GetResourcePolicy, its newest API date".to_string(),
            "GET",
            "localhost:9297",
            "/2026-07-09/resource-policy/probe".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        // Lambda Core.
        (
            "core ListNetworkConnectors".to_string(),
            "GET",
            "localhost:9297",
            "/2026-04-04/network-connectors".to_string(),
            OWNER_CORE,
        ),
        (
            "core CreateNetworkConnector on the Lambda endpoint host".to_string(),
            "POST",
            "lambda.us-east-1.amazonaws.com",
            "/2026-04-04/network-connectors".to_string(),
            OWNER_CORE,
        ),
        (
            "core GetNetworkConnector".to_string(),
            "GET",
            "localhost:9297",
            "/2026-04-04/network-connectors/nc-1".to_string(),
            OWNER_CORE,
        ),
        // Lambda MicroVMs.
        (
            "microvms ListMicrovms".to_string(),
            "GET",
            "localhost:9297",
            "/2025-09-09/microvms".to_string(),
            OWNER_MICROVMS,
        ),
        (
            "microvms CreateMicrovmImage".to_string(),
            "POST",
            "localhost:9297",
            "/2025-09-09/microvm-images".to_string(),
            OWNER_MICROVMS,
        ),
        (
            "microvms ListManagedMicrovmImages".to_string(),
            "GET",
            "localhost:9297",
            "/2025-09-09/managed-microvm-images".to_string(),
            OWNER_MICROVMS,
        ),
        // Lambda Web, whose own tagging operations live under its own API date.
        (
            "web ListWebFunctions".to_string(),
            "POST",
            "localhost:9297",
            "/2025-03-07/web-functions".to_string(),
            OWNER_WEB,
        ),
        (
            "web GetWebAccountSettings".to_string(),
            "GET",
            "localhost:9297",
            "/2025-03-07/web-account-settings".to_string(),
            OWNER_WEB,
        ),
        (
            "web ListTags, on its own tagging path".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/2025-03-07/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:function:web-probe")
            ),
            OWNER_WEB,
        ),
        // The shared `/2017-03-31/tags/{Resource}` space: the resource type decides, over
        // all three tagging methods, several regions and accounts, and both wire forms of
        // the label.
        (
            "tags: list on a function".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:function:probe")
            ),
            OWNER_CONTROL_PLANE,
        ),
        (
            "tags: tag a function version".to_string(),
            "POST",
            "localhost:9297",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:eu-west-1:123456789012:function:probe:7")
            ),
            OWNER_CONTROL_PLANE,
        ),
        (
            "tags: untag a code signing config".to_string(),
            "DELETE",
            "localhost:9297",
            format!(
                "/2017-03-31/tags/{}",
                label(
                    "arn:aws:lambda:us-east-1:000000000000:code-signing-config:\
                     csc-0123456789abcdefg"
                )
            ),
            OWNER_CONTROL_PLANE,
        ),
        (
            "tags: list on an event source mapping".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/2017-03-31/tags/{}",
                label(
                    "arn:aws:lambda:us-east-2:210987654321:event-source-mapping:\
                     12345678-1234-1234-1234-123456789012"
                )
            ),
            OWNER_CONTROL_PLANE,
        ),
        (
            "tags: list on a capacity provider".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:ap-northeast-1:000000000000:capacity-provider:cp-1")
            ),
            OWNER_CONTROL_PLANE,
        ),
        (
            "tags: list on a network connector".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:network-connector:nc-1")
            ),
            OWNER_CONTROL_PLANE,
        ),
        (
            "tags: a function ARN with its separators sent raw".to_string(),
            "GET",
            "localhost:9297",
            "/2017-03-31/tags/arn:aws:lambda:us-east-1:000000000000:function:probe".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "tags: tag a MicroVM image".to_string(),
            "POST",
            "localhost:9297",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:microvm-image:img-1")
            ),
            OWNER_MICROVMS,
        ),
        (
            "tags: a MicroVM image ARN sent raw".to_string(),
            "GET",
            "localhost:9297",
            "/2017-03-31/tags/arn:aws:lambda:eu-west-1:123456789012:microvm-image:img-2"
                .to_string(),
            OWNER_MICROVMS,
        ),
        (
            "tags: a MicroVM image whose resource type is not canonically cased".to_string(),
            "DELETE",
            "localhost:9297",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:Microvm-Image:img-3")
            ),
            OWNER_MICROVMS,
        ),
        (
            // The MicroVM image ARN pattern allows an empty region and account
            // (`^arn:[a-z0-9-]+:lambda:[a-z0-9-]*:[a-z0-9]*:microvm-image:.+$`), and such
            // an ARN names the same resource, so it must reach the same endpoint.
            "tags: a MicroVM image ARN with no region and no account".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:::microvm-image:img-4")
            ),
            OWNER_MICROVMS,
        ),
        // Labels no `lambda` model recognises stay with the endpoint whose own identity IS
        // the shared signing name, so exactly one service still claims them.
        (
            "tags: another service's ARN whose sixth field looks like a resource type"
                .to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:sqs:us-east-1:000000000000:microvm-image:img-1")
            ),
            OWNER_CONTROL_PLANE,
        ),
        (
            "tags: not an ARN at all".to_string(),
            "GET",
            "localhost:9297",
            "/2017-03-31/tags/not-an-arn".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            "tags: no resource at all".to_string(),
            "GET",
            "localhost:9297",
            "/2017-03-31/tags".to_string(),
            OWNER_CONTROL_PLANE,
        ),
        (
            // The framework matches a path segment by exact equality, so an encoded
            // segment is a path NO model binds as an operation URI — but it is still
            // under an API date the control plane binds, so it is that service's request
            // to refuse rather than an ambiguity or an `UnknownService`.
            "tags: the `tags` path segment itself percent-encoded".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/2017-03-31/%74ags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:function:probe")
            ),
            OWNER_CONTROL_PLANE,
        ),
        (
            "a path no model binds".to_string(),
            "POST",
            "localhost:9297",
            "/NoSuchOperation".to_string(),
            OWNER_NONE,
        ),
    ];

    for (case, method, host, uri, expected) in rows {
        let actual = claimant(&services, &parts(method, host, &uri));
        assert_eq!(
            actual, expected,
            "case {case:?} ({method} {uri} to host {host}): expected it to be claimed by \
             {expected}, got {actual}"
        );
    }
}

#[tokio::test]
async fn every_lambda_signed_request_is_dispatched_instead_of_refused_as_ambiguous() {
    let router = router_with_all_four();

    // (case, method, uri, body, expected status, expected error code)
    //
    // Each row names the answer of the ONE service that owns it, which is what excludes
    // both routing failures: `AmbiguousService` (500, several claimants) and
    // `UnknownService` (404, none). An implemented operation answers its own modelled
    // success or error; one that is still scaffolded answers the scaffold's
    // `501 NotImplemented`, which is also an answer from the service that owns the path.
    let rows: Vec<(&str, &str, String, Value, u16, &str)> = vec![
        (
            // The request the issue reports as `500 AmbiguousService`.
            "control plane CreateFunction",
            "POST",
            "/2015-03-31/functions".to_string(),
            json!({
                "FunctionName": "routing-check",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": {"ZipFile": ZIP_FILE},
            }),
            201,
            "",
        ),
        (
            "control plane ListFunctions",
            "GET",
            "/2015-03-31/functions".to_string(),
            Value::Null,
            200,
            "",
        ),
        (
            "control plane GetFunction on a function that does not exist",
            "GET",
            "/2015-03-31/functions/missing".to_string(),
            Value::Null,
            404,
            "ResourceNotFoundException",
        ),
        (
            // This change implements `GetAccountSettings` (see
            // `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/get_account_settings.rs`),
            // so the owner of this path now answers its own 200 rather than the scaffold's
            // 501 — which is still a response only the control plane can produce.
            "control plane GetAccountSettings",
            "GET",
            "/2016-08-19/account-settings".to_string(),
            Value::Null,
            200,
            "",
        ),
        (
            "core ListNetworkConnectors",
            "GET",
            "/2026-04-04/network-connectors".to_string(),
            Value::Null,
            200,
            "",
        ),
        (
            "microvms ListMicrovms",
            "GET",
            "/2025-09-09/microvms".to_string(),
            Value::Null,
            200,
            "",
        ),
        (
            "web ListWebFunctions",
            "POST",
            "/2025-03-07/web-functions".to_string(),
            json!({}),
            200,
            "",
        ),
        (
            // The shared tagging path, control-plane side: its own `TagResource` is still
            // scaffolded, so the owner's answer is the scaffold's 501.
            "tags: TagResource on a function",
            "POST",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:function:routing-check")
            ),
            json!({"Tags": {"team": "routing"}}),
            501,
            "NotImplemented",
        ),
        (
            // The shared tagging path, MicroVMs side. This row is what distinguishes the
            // two: the control plane's own `TaggableResource` pattern does NOT accept a
            // `microvm-image` ARN, so had the request landed there the framework would
            // have refused it before any handler ran — only the MicroVMs service can
            // answer its scaffold's 501 here.
            "tags: TagResource on a MicroVM image",
            "POST",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:microvm-image:img-1")
            ),
            json!({"Tags": {"team": "routing"}}),
            501,
            "NotImplemented",
        ),
    ];

    for (case, method, uri, body, expected_status, expected_code) in rows {
        let (status, code, parsed) = call(&router, method, &uri, body).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?} from the one \
             service that owns {uri}, got {status} {code:?} ({parsed})"
        );
    }
}

#[tokio::test]
async fn a_function_created_through_the_shared_endpoint_reads_back() {
    let router = router_with_all_four();
    let name = "round-trip-through-the-router";

    let (status, code, created) = call(
        &router,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": {"ZipFile": ZIP_FILE},
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (201, ""),
        "case \"CreateFunction\": expected HTTP 201 from the Lambda control plane, got \
         {status} {code:?} ({created})"
    );
    let expected_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}");
    assert_eq!(
        created["FunctionArn"].as_str(),
        Some(expected_arn.as_str()),
        "case \"CreateFunction\": expected the function ARN {expected_arn}, got {created}"
    );

    let (status, code, fetched) = call(
        &router,
        "GET",
        &format!("/2015-03-31/functions/{name}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"GetFunction after CreateFunction\": expected HTTP 200, got {status} {code:?} \
         ({fetched})"
    );
    assert_eq!(
        fetched["Configuration"]["FunctionArn"].as_str(),
        Some(expected_arn.as_str()),
        "case \"GetFunction after CreateFunction\": the function read back must be the \
         function created ({expected_arn}), got {fetched}"
    );
}
