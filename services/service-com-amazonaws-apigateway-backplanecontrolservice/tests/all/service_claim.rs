// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS — the `Service::can_handle` seam.
//!
//! The Amazon API Gateway control plane signs as `apigateway`, and so does Amazon API
//! Gateway V2 (`services/service-com-amazonaws-apigatewayv2-apigatewayv2`): a different
//! service, with the same `restJson1` protocol and the same `apigateway` endpoint prefix
//! and signing name, which AWS separates by URI path space — every V2 operation is bound
//! under `/v2/…`. The router resolves a request's routing key from the SigV4 credential
//! scope, then from `X-Amz-Target`, so both services see the key `apigateway`; claiming on
//! that key alone makes each service claim the other's traffic, the router finds two
//! claimants and answers `500 AmbiguousService` — a code no AWS model carries — for every
//! API Gateway request instead of serving either service.
//!
//! The claim is therefore the routing key AND the request's first URI path segment,
//! matched by exact equality against this service's model-derived `ROUTE_PATH_SEGMENTS`
//! (`OPERATIONS.md`: `/account`, `/apikeys`, `/clientcertificates`,
//! `/domainnameaccessassociations`, `/domainnames`,
//! `/rejectdomainnameaccessassociations`, `/restapis`, `/sdktypes`, `/tags`,
//! `/usageplans`, `/vpclinks`). This suite pins that rule over the class of requests it
//! generalises over: each family of its own first segments, collection and instance
//! paths, a query string, both routing tiers, the peer's `/v2/…` paths, near-miss
//! spellings, and requests that name another service or none.

use std::sync::Arc;

use service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService;
use smithy_server_runtime::Service;

/// A SigV4 credential scope naming `signing_name`; the signature is never inspected by
/// the routing key.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The request parts `can_handle` is handed: the URI, and the headers the routing key is
/// read from.
fn parts(
    method: &str,
    path_and_query: &str,
    signing_name: Option<&str>,
    target: Option<&str>,
) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://127.0.0.1:9297{path_and_query}"))
        .header("host", "127.0.0.1:9297");
    if let Some(signing_name) = signing_name {
        builder = builder.header("authorization", authorization(signing_name));
    }
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    builder
        .body(())
        .expect("the request is well formed")
        .into_parts()
        .0
}

/// One claim case: the request, and whether THIS service must claim it.
struct Case {
    name: &'static str,
    method: &'static str,
    path_and_query: &'static str,
    signing_name: Option<&'static str>,
    target: Option<&'static str>,
    claimed: bool,
}

const CASES: &[Case] = &[
    // One row per family of this service's own modelled first segments, in both the
    // collection and the instance shape where the model binds both.
    Case {
        name: "the issue's own call: POST /restapis",
        method: "POST",
        path_and_query: "/restapis",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "instance path /restapis/{restApiId}",
        method: "GET",
        path_and_query: "/restapis/abc1230000",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "a deep path under /restapis",
        method: "PUT",
        path_and_query: "/restapis/abc1230000/resources/xyz/methods/GET/integration",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "collection path /apikeys",
        method: "POST",
        path_and_query: "/apikeys",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "/apikeys with the import query string",
        method: "POST",
        path_and_query: "/apikeys?mode=import",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "singleton path /account",
        method: "GET",
        path_and_query: "/account",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "/domainnames/{domainName}/basepathmappings",
        method: "GET",
        path_and_query: "/domainnames/api.example.com/basepathmappings",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "/domainnameaccessassociations",
        method: "GET",
        path_and_query: "/domainnameaccessassociations",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "/rejectdomainnameaccessassociations",
        method: "POST",
        path_and_query: "/rejectdomainnameaccessassociations",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "/usageplans/{usagePlanId}/keys",
        method: "GET",
        path_and_query: "/usageplans/up123/keys",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "/vpclinks",
        method: "POST",
        path_and_query: "/vpclinks",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "/clientcertificates",
        method: "POST",
        path_and_query: "/clientcertificates",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "/sdktypes/{id}",
        method: "GET",
        path_and_query: "/sdktypes/java",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        name: "the tagging path /tags/{resourceArn}",
        method: "GET",
        path_and_query: "/tags/arn%3Aaws%3Aapigateway%3Aus-east-1%3A%3A%2Frestapis%2Fabc",
        signing_name: Some("apigateway"),
        target: None,
        claimed: true,
    },
    Case {
        // The signing name is normalised (trimmed, lowercased) before it is compared.
        name: "signed for APIGATEWAY in upper case",
        method: "GET",
        path_and_query: "/restapis",
        signing_name: Some("APIGATEWAY"),
        target: None,
        claimed: true,
    },
    // The peer's path space: this service must claim NONE of it, or the router is
    // ambiguous again.
    Case {
        name: "the peer's /v2/apis",
        method: "POST",
        path_and_query: "/v2/apis",
        signing_name: Some("apigateway"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's /v2/apis/{ApiId}/routes",
        method: "POST",
        path_and_query: "/v2/apis/a1b2c3/routes",
        signing_name: Some("apigateway"),
        target: None,
        claimed: false,
    },
    Case {
        // `domainnames` and `tags` are segments BOTH models bind — under `/v2/` they are
        // the peer's, because only the FIRST segment decides.
        name: "the peer's /v2/domainnames",
        method: "GET",
        path_and_query: "/v2/domainnames",
        signing_name: Some("apigateway"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's /v2/tags/{ResourceArn}",
        method: "GET",
        path_and_query: "/v2/tags/arn%3Aaws%3Aapigateway%3Aus-east-1%3A%3A%2Fapis%2Fa1",
        signing_name: Some("apigateway"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's /v2/portalproducts",
        method: "GET",
        path_and_query: "/v2/portalproducts",
        signing_name: Some("apigateway"),
        target: None,
        claimed: false,
    },
    Case {
        // Matching is exact segment equality, never a prefix test or a case fold.
        name: "near miss /restapisextra",
        method: "GET",
        path_and_query: "/restapisextra",
        signing_name: Some("apigateway"),
        target: None,
        claimed: false,
    },
    Case {
        name: "near miss /RestApis in the wrong case",
        method: "GET",
        path_and_query: "/RestApis",
        signing_name: Some("apigateway"),
        target: None,
        claimed: false,
    },
    Case {
        // This model binds every operation to a URI, so it claims no root post.
        name: "the root path",
        method: "POST",
        path_and_query: "/",
        signing_name: Some("apigateway"),
        target: None,
        claimed: false,
    },
    Case {
        name: "another service's signing name on this service's path",
        method: "GET",
        path_and_query: "/restapis",
        signing_name: Some("s3"),
        target: None,
        claimed: false,
    },
    Case {
        name: "another service's target on this service's path",
        method: "GET",
        path_and_query: "/restapis",
        signing_name: None,
        target: Some("DynamoDB_20120810.GetItem"),
        claimed: false,
    },
    Case {
        // No credential scope, no target, and a loopback host names no service.
        name: "no routing signal at all",
        method: "GET",
        path_and_query: "/restapis",
        signing_name: None,
        target: None,
        claimed: false,
    },
];

#[test]
fn only_this_services_own_path_space_is_claimed() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    for case in CASES {
        let parts = parts(case.method, case.path_and_query, case.signing_name, case.target);
        let claimed = service.can_handle(&parts);
        assert_eq!(
            claimed, case.claimed,
            "case {:?}: expected can_handle({} {}) to be {}, got {claimed}",
            case.name, case.method, case.path_and_query, case.claimed
        );
    }
}
