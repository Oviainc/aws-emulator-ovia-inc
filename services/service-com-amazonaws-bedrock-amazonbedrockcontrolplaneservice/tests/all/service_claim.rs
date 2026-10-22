// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS — the `Service::can_handle` seam.
//!
//! The Amazon Bedrock control plane signs as `bedrock`, and so does every other service
//! in the Bedrock family: Bedrock Runtime (endpoint `bedrock-runtime`), Bedrock Agent
//! (`bedrock-agent`), Bedrock Agent Runtime (`bedrock-agent-runtime`), Bedrock Data
//! Automation (`bedrock-data-automation`) and Bedrock Data Automation Runtime
//! (`bedrock-data-automation-runtime`) — six distinct wire services, one
//! `aws.auth#sigv4` name. The router resolves a request's routing key from the SigV4
//! credential scope first, so all six see the key `bedrock`; claiming on that key alone
//! makes each of them a claimant for every Bedrock request, the router finds six
//! claimants and answers `500 AmbiguousService` — a code no AWS model carries — for all
//! of it, so nothing on the Bedrock control plane is reachable.
//!
//! The claim is therefore the routing key AND the request's own modelled wire surface:
//! the first URI path segment, matched by exact equality against this service's
//! model-derived `ROUTE_PATH_SEGMENTS` (`OPERATIONS.md`: `/guardrails`,
//! `/foundation-models`, `/custom-models`, `/logging/modelinvocations`,
//! `/model-customization-jobs`, `/evaluation-jobs`, `/inference-profiles`,
//! `/automated-reasoning-policies`, `/tagResource`, …), with the segments a sibling
//! model also publishes split by `service_common::bedrock`.
//!
//! This suite pins that rule over the class of requests it generalises over: each family
//! of this model's own first segments, collection and instance paths, a query string, the
//! three tagging paths it shares with Bedrock Data Automation, each sibling's path space,
//! near-miss spellings, and requests that name another service or none.

use std::sync::Arc;

use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService;
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
    // One row per family of this model's own modelled first segments, in both the
    // collection and the instance shape where it binds both.
    Case {
        name: "the issue's own call: POST /guardrails",
        method: "POST",
        path_and_query: "/guardrails",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        name: "instance path /guardrails/{guardrailIdentifier}",
        method: "GET",
        path_and_query: "/guardrails/abc123def456",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        name: "/guardrails with a query string",
        method: "GET",
        path_and_query: "/guardrails/abc123def456?guardrailVersion=1",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        name: "GET /foundation-models",
        method: "GET",
        path_and_query: "/foundation-models",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        name: "GET /foundation-models/{modelIdentifier}",
        method: "GET",
        path_and_query: "/foundation-models/anthropic.claude-v2",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        name: "the model-invocation logging singleton",
        method: "GET",
        path_and_query: "/logging/modelinvocations",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        name: "a two-segment path under /custom-models",
        method: "POST",
        path_and_query: "/custom-models/create-custom-model",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        name: "a deep path under /automated-reasoning-policies",
        method: "GET",
        path_and_query: "/automated-reasoning-policies/arn/build-workflows/w-1/annotations",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        name: "the camelCase /enforcedGuardrailsConfiguration",
        method: "GET",
        path_and_query: "/enforcedGuardrailsConfiguration",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        name: "/model-invocation-job (singular) and its stop path",
        method: "POST",
        path_and_query: "/model-invocation-job/job-1/stop",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    // The three tagging paths Bedrock Data Automation binds IDENTICALLY: nothing in such
    // a request tells the two models apart, so `service_common::bedrock` gives the
    // segment to this service and the rule must hand them here.
    Case {
        name: "POST /tagResource, bound identically by bedrock-data-automation",
        method: "POST",
        path_and_query: "/tagResource",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        name: "POST /untagResource",
        method: "POST",
        path_and_query: "/untagResource",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        name: "POST /listTagsForResource",
        method: "POST",
        path_and_query: "/listTagsForResource",
        signing_name: Some("bedrock"),
        target: None,
        claimed: true,
    },
    Case {
        // The signing name is normalised (trimmed, lowercased) before it is compared.
        name: "signed for BEDROCK in upper case",
        method: "POST",
        path_and_query: "/guardrails",
        signing_name: Some("BEDROCK"),
        target: None,
        claimed: true,
    },
    // Each sibling's own path space: this service must claim NONE of it, or the router is
    // ambiguous again.
    Case {
        name: "the runtime's POST /model/{modelId}/invoke",
        method: "POST",
        path_and_query: "/model/anthropic.claude-v2/invoke",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the runtime's POST /model/{modelId}/converse",
        method: "POST",
        path_and_query: "/model/anthropic.claude-v2/converse",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        // `guardrail` is the runtime's segment and `guardrails` is this service's: the
        // match is exact segment equality, never a prefix test.
        name: "the runtime's POST /guardrail/{id}/version/{v}/apply",
        method: "POST",
        path_and_query: "/guardrail/abc123def456/version/1/apply",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the runtime's /async-invoke",
        method: "GET",
        path_and_query: "/async-invoke",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the agent plane's PUT /knowledgebases/",
        method: "PUT",
        path_and_query: "/knowledgebases/",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the agent plane's PUT /agents/",
        method: "PUT",
        path_and_query: "/agents/",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the agent plane's GET /tags/{resourceArn}",
        method: "GET",
        path_and_query: "/tags/arn%3Aaws%3Abedrock%3Aus-east-1%3A%3Aagent%2Fa1",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the agent runtime's POST /retrieveAndGenerate",
        method: "POST",
        path_and_query: "/retrieveAndGenerate",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        name: "data automation's PUT /blueprints/",
        method: "PUT",
        path_and_query: "/blueprints/",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the data-automation runtime's awsJson1_1 root post",
        method: "POST",
        path_and_query: "/",
        signing_name: Some("bedrock"),
        target: Some("AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync"),
        claimed: false,
    },
    Case {
        // Every operation of this model is bound to a URI, so it claims no root post.
        name: "the root path with no target at all",
        method: "POST",
        path_and_query: "/",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        name: "near miss /guardrailsextra",
        method: "POST",
        path_and_query: "/guardrailsextra",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        name: "near miss /Guardrails in the wrong case",
        method: "POST",
        path_and_query: "/Guardrails",
        signing_name: Some("bedrock"),
        target: None,
        claimed: false,
    },
    Case {
        name: "another service's signing name on this service's path",
        method: "POST",
        path_and_query: "/guardrails",
        signing_name: Some("s3"),
        target: None,
        claimed: false,
    },
    Case {
        name: "another service's target on this service's path",
        method: "POST",
        path_and_query: "/guardrails",
        signing_name: None,
        target: Some("DynamoDB_20120810.GetItem"),
        claimed: false,
    },
    Case {
        // No credential scope, no target, and a loopback host names no service.
        name: "no routing signal at all",
        method: "POST",
        path_and_query: "/guardrails",
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
