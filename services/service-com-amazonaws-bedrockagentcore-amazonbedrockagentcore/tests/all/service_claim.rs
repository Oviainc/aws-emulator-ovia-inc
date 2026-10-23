// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS — the `Service::can_handle` seam.
//!
//! Amazon Bedrock AgentCore (the data plane) signs as `bedrock-agentcore`, and so does
//! the AgentCore CONTROL plane
//! (`services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol`):
//! a different service, with the same `restJson1` protocol, which AWS separates by
//! endpoint (`bedrock-agentcore` against `bedrock-agentcore-control`). The router resolves
//! a request's routing key from the SigV4 credential scope, so both services see the key
//! `bedrock-agentcore`; claiming on that key alone makes each service claim the other's
//! traffic, the router finds two claimants and answers `500 AmbiguousService` — a code no
//! AWS model carries — for every AgentCore request instead of serving either service.
//!
//! The first URI path segment cannot separate this pair, because the two models
//! INTERLEAVE their path spaces: both bind operations under `/memories`, `/runtimes`,
//! `/browsers`, `/browser-profiles`, `/code-interpreters`, `/capacity-providers`,
//! `/harnesses`, `/identities` and `/payments`. The whole ROUTE does: no
//! `(method, URI pattern)` is bound by both models. So the claim is the routing key AND a
//! route this service's own model binds, and this suite pins that rule over the class of
//! requests it generalises over — its own route families (nested, action and label-only
//! shapes, with and without a query string), the peer's routes under the segments the two
//! share, a route that differs from one of this model's only in its METHOD, and requests
//! naming another service or none.
//!
//! Source for every route below: this crate's `OPERATIONS.md` (the `Route:` line of each
//! operation), which is generated from the Smithy model.

use std::sync::Arc;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore::EmulatedService;
use smithy_server_runtime::Service;

/// A SigV4 credential scope naming `signing_name`; the signature itself is never
/// inspected by the routing key.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The request parts `can_handle` is handed: the method, the URI, and the headers the
/// routing key is read from.
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
    // ---- this model's own routes -------------------------------------------------
    Case {
        name: "CreateEvent POST /memories/{memoryId}/events",
        method: "POST",
        path_and_query: "/memories/demo-memory-0123456789/events",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    Case {
        name: "GetEvent GET /memories/{id}/actor/{actorId}/sessions/{sessionId}/events/{eventId}",
        method: "GET",
        path_and_query: "/memories/demo-memory-0123456789/actor/a1/sessions/s1/events/1%23ab",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    Case {
        name: "ListMemoryRecords POST /memories/{memoryId}/memoryRecords",
        method: "POST",
        path_and_query: "/memories/demo-memory-0123456789/memoryRecords",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    Case {
        name: "InvokeAgentRuntime POST /runtimes/{agentRuntimeArn}/invocations",
        method: "POST",
        path_and_query: "/runtimes/arn%3Aaws%3Abedrock-agentcore%3Aus-east-1%3A0%3Aruntime%2Fr1/invocations?qualifier=DEFAULT",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    Case {
        name: "GetWorkloadAccessToken POST /identities/GetWorkloadAccessToken",
        method: "POST",
        path_and_query: "/identities/GetWorkloadAccessToken",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    Case {
        name: "GetResourceOauth2Token POST /identities/oauth2/token",
        method: "POST",
        path_and_query: "/identities/oauth2/token",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    Case {
        name: "StartBrowserSession PUT /browsers/{browserIdentifier}/sessions/start",
        method: "PUT",
        path_and_query: "/browsers/aws.browser.v1/sessions/start",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    Case {
        name: "StartCodeInterpreterSession PUT /code-interpreters/{id}/sessions/start",
        method: "PUT",
        path_and_query: "/code-interpreters/aws.codeinterpreter.v1/sessions/start",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    Case {
        name: "SearchRegistryRecords POST /registry-records/search",
        method: "POST",
        path_and_query: "/registry-records/search",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    Case {
        name: "ProcessPayment POST /payments/processPayment",
        method: "POST",
        path_and_query: "/payments/processPayment",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    Case {
        name: "ListABTests GET /ab-tests",
        method: "GET",
        path_and_query: "/ab-tests?maxResults=5",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    Case {
        // The signing name is normalised (trimmed, lowercased) before it is compared.
        name: "signed for BEDROCK-AGENTCORE in upper case",
        method: "POST",
        path_and_query: "/payments/processPayment",
        signing_name: Some("BEDROCK-AGENTCORE"),
        target: None,
        claimed: true,
    },
    // ---- the control plane's routes, under the segments the two models SHARE ------
    Case {
        name: "the peer's CreateMemory POST /memories/create",
        method: "POST",
        path_and_query: "/memories/create",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's GetMemory GET /memories/{memoryId}/details",
        method: "GET",
        path_and_query: "/memories/demo-memory-0123456789/details",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's CreateAgentRuntime PUT /runtimes/",
        method: "PUT",
        path_and_query: "/runtimes/",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's ListAgentRuntimes POST /runtimes/",
        method: "POST",
        path_and_query: "/runtimes/?maxResults=10",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's CreateWorkloadIdentity POST /identities/CreateWorkloadIdentity",
        method: "POST",
        path_and_query: "/identities/CreateWorkloadIdentity",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's GetBrowser GET /browsers/{browserId}",
        method: "GET",
        path_and_query: "/browsers/aws.browser.v1",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's CreatePaymentManager POST /payments/managers",
        method: "POST",
        path_and_query: "/payments/managers",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's ListTagsForResource GET /tags/{resourceArn}",
        method: "GET",
        path_and_query: "/tags/arn%3Aaws%3Abedrock-agentcore%3Aus-east-1%3A0%3Aruntime%2Fr1",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's CreateGateway POST /gateways/",
        method: "POST",
        path_and_query: "/gateways/",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    // ---- the claim is the whole route: method and path, exactly ------------------
    Case {
        name: "this model's path under a method it binds no route for",
        method: "DELETE",
        path_and_query: "/payments/processPayment",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        name: "a session action this model binds only as PUT",
        method: "POST",
        path_and_query: "/browsers/aws.browser.v1/sessions/start",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        name: "a path neither model binds",
        method: "GET",
        path_and_query: "/not-a-route",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        // `InvokeHarness` is the only operation this model binds under `/harnesses`, and
        // it is one of the streaming operations this emulator registers no handler for,
        // so no route accepts the path and neither service claims it.
        name: "a modelled path no registered operation serves",
        method: "POST",
        path_and_query: "/harnesses/invoke",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        // Every operation of this model is bound to a URI, so it claims no root post.
        name: "the root path",
        method: "POST",
        path_and_query: "/",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    // ---- requests that name another service, or none -----------------------------
    Case {
        name: "another service's signing name on this service's route",
        method: "POST",
        path_and_query: "/payments/processPayment",
        signing_name: Some("s3"),
        target: None,
        claimed: false,
    },
    Case {
        name: "another service's target on this service's route",
        method: "POST",
        path_and_query: "/payments/processPayment",
        signing_name: None,
        target: Some("DynamoDB_20120810.GetItem"),
        claimed: false,
    },
    Case {
        // No credential scope, no target, and a loopback host names no service.
        name: "no routing signal at all",
        method: "POST",
        path_and_query: "/payments/processPayment",
        signing_name: None,
        target: None,
        claimed: false,
    },
];

#[test]
fn only_this_models_own_routes_are_claimed() {
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

/// A route the model selects with a REQUIRED `@httpHeader` binding is claimed when the
/// request carries that header: `StopRuntimeSession` marks `runtimeSessionId`
/// (`X-Amzn-Bedrock-AgentCore-Runtime-Session-Id`) `@required` (`OPERATIONS.md`), and the
/// framework's route lookup rejects a candidate route whose required bindings the request
/// does not carry — so that header is part of what selects the route, and a claim keyed on
/// the route reads it.
#[test]
fn a_route_selected_by_a_required_header_is_claimed_with_that_header() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    const PATH: &str = "/runtimes/arn%3Aaws%3Abedrock-agentcore%3Aus-east-1%3A0%3Aruntime%2Fr1/stopruntimesession";
    let parts = http::Request::builder()
        .method("POST")
        .uri(format!("https://127.0.0.1:9297{PATH}"))
        .header("host", "127.0.0.1:9297")
        .header("authorization", authorization("bedrock-agentcore"))
        .header(
            "x-amzn-bedrock-agentcore-runtime-session-id",
            "session-0123456789012345678901234567",
        )
        .body(())
        .expect("the request is well formed")
        .into_parts()
        .0;

    let claimed = service.can_handle(&parts);
    assert!(
        claimed,
        "case \"StopRuntimeSession with its required session-id header\": expected \
         can_handle(POST {PATH}) to be true, got {claimed}"
    );
}
