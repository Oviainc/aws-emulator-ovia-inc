// SPDX-License-Identifier: CC0-1.0

//! Which requests this service claims when its SigV4 signing name is not its alone.
//!
//! The Lambda control plane signs as `lambda` — a signing name it SHARES with three
//! other emulated services generated from Lambda-family models (Lambda Core, Lambda
//! MicroVMs and Lambda Web; `smithy_query(query="service_info")` gives all four
//! `sigv4Name: lambda`). The router resolves a request's routing key from the SigV4
//! credential scope first, so all four see the key `lambda`: a service that claims on the
//! signing name alone claims its peers' traffic too, every one of them is a claimant for
//! every request, and the router — which refuses to pick one — answers
//! `500 AmbiguousService` to all of it, leaving every Lambda operation unreachable.
//!
//! The rule under test, from this service's side: a request signed for a shared signing
//! name is claimed only when this service's OWN model binds the request's URI path, and
//! — on the `/2017-03-31/tags/{Resource}` path space a peer model binds identically —
//! only when the ARN in that path does not name one of that peer's resources.
//!
//! The cross-service half of the rule (that exactly ONE of the four claims every such
//! request, and that the owner's own answer comes back) is pinned in
//! `core/router/tests/all/lambda_signing_name_routing.rs`, the only place all four
//! services can be mounted together.

use std::sync::Arc;

use smithy_server_runtime::Service;

use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;

/// A SigV4 `Authorization` header whose credential scope names the shared signing name.
/// The signature is never checked here: `can_handle` reads the scope, not the signature.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The request parts the router hands to [`Service::can_handle`].
fn parts(method: &str, uri: &str) -> http::request::Parts {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::new())
        .expect("the test request is well formed");
    request.into_parts().0
}

/// An ARN as a client puts it in the non-greedy `{Resource}` path label: its `:` and `/`
/// percent-encoded.
fn label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

#[test]
fn this_service_claims_its_own_request_space_and_no_more() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    // (case, method, uri, whether this service claims it)
    //
    // The operation paths are the `smithy.api#http` bindings of the four models
    // (`smithy_query(query="digest", …)` on each): this model's API-date segments
    // (`2014-11-13` … `2026-07-09`), Lambda Core's `2026-04-04`, Lambda MicroVMs'
    // `2025-09-09` and Lambda Web's `2025-03-07`. The tagging ARNs are built from the
    // resource segments the two `TaggableResource` patterns pin
    // (`smithy_query(query="shape", shape="TaggableResource")` on each).
    let rows: Vec<(&str, &str, String, bool)> = vec![
        // This model's own path space, one row per API date it binds.
        ("own CreateFunction", "POST", "/2015-03-31/functions".to_string(), true),
        ("own ListFunctions", "GET", "/2015-03-31/functions".to_string(), true),
        (
            "own GetFunction",
            "GET",
            "/2015-03-31/functions/probe".to_string(),
            true,
        ),
        (
            "own InvokeAsync, the oldest API date",
            "POST",
            "/2014-11-13/functions/probe/invoke-async".to_string(),
            true,
        ),
        (
            "own GetAccountSettings",
            "GET",
            "/2016-08-19/account-settings".to_string(),
            true,
        ),
        (
            "own PutFunctionConcurrency",
            "PUT",
            "/2017-10-31/functions/probe/concurrency".to_string(),
            true,
        ),
        ("own ListLayers", "GET", "/2018-10-31/layers".to_string(), true),
        (
            "own GetFunctionEventInvokeConfig",
            "GET",
            "/2019-09-25/functions/probe/event-invoke-config".to_string(),
            true,
        ),
        (
            "own ListCodeSigningConfigs",
            "GET",
            "/2020-04-22/code-signing-configs".to_string(),
            true,
        ),
        (
            "own GetFunctionUrlConfig",
            "GET",
            "/2021-10-31/functions/probe/url".to_string(),
            true,
        ),
        (
            "own ListCapacityProviders",
            "GET",
            "/2025-11-30/capacity-providers".to_string(),
            true,
        ),
        (
            "own GetDurableExecution",
            "GET",
            "/2025-12-01/durable-executions/exec-1".to_string(),
            true,
        ),
        (
            "own GetResourcePolicy, the newest API date",
            "GET",
            "/2026-07-09/resource-policy/probe".to_string(),
            true,
        ),
        // A path inside this model's own space that names no operation of it is still
        // THIS service's request — it answers its own routing error rather than letting
        // the router refuse it.
        (
            "an unbound path under an API date this model binds",
            "GET",
            "/2015-03-31/no-such-collection".to_string(),
            true,
        ),
        // The peers' path spaces.
        (
            "Lambda Core ListNetworkConnectors",
            "GET",
            "/2026-04-04/network-connectors".to_string(),
            false,
        ),
        (
            "Lambda MicroVMs ListMicrovms",
            "GET",
            "/2025-09-09/microvms".to_string(),
            false,
        ),
        (
            "Lambda MicroVMs CreateMicrovmImage",
            "POST",
            "/2025-09-09/microvm-images".to_string(),
            false,
        ),
        (
            "Lambda Web ListWebFunctions",
            "POST",
            "/2025-03-07/web-functions".to_string(),
            false,
        ),
        (
            "Lambda Web's own tagging path",
            "GET",
            format!(
                "/2025-03-07/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:function:web")
            ),
            false,
        ),
        ("a path no model binds", "POST", "/NoSuchOperation".to_string(), false),
        // The shared `/2017-03-31/tags/{Resource}` space: every resource type this
        // model's own pattern accepts reaches this service, across all three methods.
        (
            "tags: list on a function",
            "GET",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:function:probe")
            ),
            true,
        ),
        (
            "tags: tag a function version",
            "POST",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:eu-west-1:123456789012:function:probe:3")
            ),
            true,
        ),
        (
            "tags: untag a code signing config",
            "DELETE",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-0123456789abcdefg")
            ),
            true,
        ),
        (
            "tags: list on an event source mapping",
            "GET",
            format!(
                "/2017-03-31/tags/{}",
                label(
                    "arn:aws:lambda:us-east-2:210987654321:event-source-mapping:\
                     12345678-1234-1234-1234-123456789012"
                )
            ),
            true,
        ),
        (
            "tags: list on a capacity provider",
            "GET",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:capacity-provider:cp-1")
            ),
            true,
        ),
        (
            "tags: list on a network connector",
            "GET",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:network-connector:nc-1")
            ),
            true,
        ),
        (
            "tags: a function ARN with its separators sent raw",
            "GET",
            "/2017-03-31/tags/arn:aws:lambda:us-east-1:000000000000:function:probe".to_string(),
            true,
        ),
        // An ARN no `lambda` model recognises, and no ARN at all, stay with this
        // endpoint, whose own identity IS the shared signing name.
        (
            "tags: another service's ARN",
            "GET",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:sqs:us-east-1:000000000000:function:probe")
            ),
            true,
        ),
        (
            "tags: not an ARN at all",
            "GET",
            "/2017-03-31/tags/not-an-arn".to_string(),
            true,
        ),
        ("tags: no resource at all", "GET", "/2017-03-31/tags".to_string(), true),
        (
            // The framework matches a path segment by exact equality, so an encoded
            // segment is a path NO model binds — but it is still under an API date this
            // model binds, so it is this service's request to refuse.
            "tags: the `tags` path segment itself percent-encoded",
            "GET",
            format!(
                "/2017-03-31/%74ags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:function:probe")
            ),
            true,
        ),
        // The one resource type on this path that belongs to a peer: a MicroVM image,
        // minted by Lambda MicroVMs' `CreateMicrovmImage`.
        (
            "tags: a MicroVM image, owned by the MicroVMs endpoint",
            "GET",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:microvm-image:img-1")
            ),
            false,
        ),
        (
            "tags: a MicroVM image, raw ARN",
            "POST",
            "/2017-03-31/tags/arn:aws:lambda:us-east-1:000000000000:microvm-image:img-1"
                .to_string(),
            false,
        ),
        (
            "tags: a MicroVM image whose resource type is not canonically cased",
            "DELETE",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:us-east-1:000000000000:Microvm-Image:img-1")
            ),
            false,
        ),
        (
            // The MicroVM image ARN pattern allows an empty region and account
            // (`^arn:[a-z0-9-]+:lambda:[a-z0-9-]*:[a-z0-9]*:microvm-image:.+$`), and such
            // an ARN names the same peer-owned resource.
            "tags: a MicroVM image ARN with no region and no account",
            "GET",
            format!(
                "/2017-03-31/tags/{}",
                label("arn:aws:lambda:::microvm-image:img-2")
            ),
            false,
        ),
    ];

    for (case, method, uri, expected) in rows {
        let actual = service.can_handle(&parts(method, &uri));
        assert_eq!(
            actual, expected,
            "case {case:?} ({method} {uri}): expected can_handle = {expected}, got {actual}"
        );
    }
}

#[test]
fn a_request_signed_for_another_service_is_never_claimed() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    // The path conjunct narrows the claim; it must never WIDEN it. A request whose
    // credential scope names a different signing name is not this service's, however
    // exactly its path matches.
    for signing_name in ["s3", "dynamodb", "lambdacore", ""] {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=not-verified"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/2015-03-31/functions")
            .header("host", "localhost:9297")
            .header("authorization", authorization)
            .body(bytes::Bytes::new())
            .expect("the test request is well formed");
        let (parts, _body) = request.into_parts();
        assert!(
            !service.can_handle(&parts),
            "case {signing_name:?}: a request signed for {signing_name:?} must not be claimed \
             by the Lambda control plane, got can_handle = true"
        );
    }
}
