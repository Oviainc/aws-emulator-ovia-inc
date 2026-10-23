// SPDX-License-Identifier: CC0-1.0

//! The claim cases `signing_name_routing.rs` beside this suite does not cover.
//!
//! That suite owns the rule — this service claims the routes its own model binds and no
//! more, under a SigV4 signing name it shares with the AgentCore data plane — over its
//! route families and the peer's. What is here is the rest of the request space a claim
//! predicate has to answer for, each row a shape of request that names no route of this
//! model or no service at all:
//!
//! * a path this model binds under a METHOD it binds no route for, and a collection it
//!   binds only under another method — the claim is the whole route, not the path;
//! * the ROOT path, which every operation of this `restJson1` model is bound away from;
//! * a request that names another service through `X-Amz-Target` rather than a credential
//!   scope, which is the second routing-key tier the framework reads;
//! * a request with NO routing signal at all (no credential scope, no target, a loopback
//!   host that names no service).
//!
//! Source for every route named below: this crate's `OPERATIONS.md` (the `Route:` line of
//! each operation), which is generated from the Smithy model.

use std::sync::Arc;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;
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
/// routing key is read from. The host is a bare loopback endpoint, so it names no
/// service and the credential scope or the target is the only signal.
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
    // One row of the positive class, so the table cannot pass by claiming nothing at all.
    Case {
        name: "ListAgentRuntimes, a route this model binds, with a query string",
        method: "POST",
        path_and_query: "/runtimes/?maxResults=10",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: true,
    },
    // The claim is the whole route: method and path, exactly.
    Case {
        name: "this model's path under a method it binds no route for",
        method: "PATCH",
        path_and_query: "/runtimes/",
        signing_name: Some("bedrock-agentcore"),
        target: None,
        claimed: false,
    },
    Case {
        name: "a collection GET this model binds only as POST",
        method: "GET",
        path_and_query: "/runtimes/",
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
    // Requests that name another service, or none.
    Case {
        name: "another service's target on this service's route",
        method: "POST",
        path_and_query: "/runtimes/",
        signing_name: None,
        target: Some("DynamoDB_20120810.GetItem"),
        claimed: false,
    },
    Case {
        // No credential scope, no target, and a loopback host names no service.
        name: "no routing signal at all",
        method: "POST",
        path_and_query: "/runtimes/",
        signing_name: None,
        target: None,
        claimed: false,
    },
];

#[test]
fn a_request_naming_no_route_of_this_model_and_no_service_is_not_claimed() {
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
