// SPDX-License-Identifier: CC0-1.0

//! Which of several services sharing one SigV4 signing name claims a request, over the
//! two Bedrock-family signing names — and, for the `bedrock-agentcore` pair whose path
//! spaces OVERLAP, decided by the WHOLE modelled route.
//!
//! The router resolves a request's routing key from the SigV4 credential scope first, so
//! every service that signs under one name sees the same key: a service that claims on
//! the signing name ALONE claims its peers' traffic too, the router finds several
//! claimants and answers `500 AmbiguousService` — a code no AWS model declares — for all
//! of them. Six services sign as `bedrock` (the Bedrock control plane, Bedrock Runtime,
//! Bedrock Agent, Bedrock Agent Runtime, Bedrock Data Automation and its runtime) and two
//! as `bedrock-agentcore` (the AgentCore data plane and AgentCore Control). AWS tells them
//! apart by endpoint prefix (`bedrock` vs `bedrock-runtime` vs `bedrock-agentcore-control`
//! …), which an emulator cannot use because a client configured with one local endpoint
//! sends the same `Host` for all of them.
//!
//! The six `bedrock` services are separated by the modelled PATH SPACE
//! (`service_common::bedrock::claims_request`, their own `ROUTE_PATH_SEGMENTS`), which is
//! derived from the model and therefore covers modelled routes whose operation a crate
//! does not generate — an event-stream operation, say — plus that rule's shared-segment
//! split for the segments two family members both publish. The two `bedrock-agentcore`
//! services are not in that family: both their models bind operations under
//! `/runtimes/…`, `/memories/…`, `/browsers/…` and `/code-interpreters/…`, so the first
//! segment separates nothing and each would keep claiming the other's traffic. For them
//! the claim is the WHOLE modelled route — method plus URI template, from each service's
//! own generated route table (`service_common::claims_modeled_route`) — which does
//! separate them.
//!
//! The segments two `bedrock` peers both publish — `POST /tagResource`, `/untagResource`
//! and `/listTagsForResource` on the Bedrock control plane and Bedrock Data Automation,
//! and `/agents`, `/flows`, `/knowledgebases` and `/tags` on the two agent services — are
//! not rows here: `service_common::bedrock` owns that split and
//! `all/bedrock_signing_name_routing.rs` is where it is pinned.
//!
//! The table below asserts the rule on the claim seam itself: each row is a request
//! addressed at one service's modelled surface, and EXACTLY that service must claim it —
//! no second claimant (which the router answers `500 AmbiguousService`) and no empty
//! claim (`404 UnknownService`). The rows are the ones the sibling suite does NOT pin:
//! the `bedrock-agentcore` pair, whose path spaces overlap so only the whole route
//! separates them, plus the one negative row that holds for any shared name (one
//! family's own path signed for another service).
//! The `bedrock` family's own claim table — who claims `/guardrails`, `/model/…`,
//! `/blueprints/`, the awsJson1_1 root post, and the segments two members share — lives
//! in `all/bedrock_signing_name_routing.rs` and is not repeated here; the whole family is
//! still MOUNTED for every row, so a bedrock service claiming an AgentCore path would
//! fail these assertions too. Rows differ along what the rule generalises over: the
//! method, the number and the literal value of the path segments, the signing name, and
//! whether any model binds the route at all. The second test dispatches a
//! `CreateGuardrail` request through a router with the whole family mounted, once per
//! signing region, and pins the modelled 202 envelope it must answer.
//!
//! `all/bedrock_signing_name_routing.rs` is the sibling suite for the `bedrock` family's
//! own answers; this one pins the claim seam across both families, including the AgentCore
//! pair no path space can separate.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::{MultiServiceRouter, Service};

use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as control_plane;
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda as agent;
use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;
use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as agentcore_control;
use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice as agent_runtime;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice as data_automation;
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as data_automation_runtime;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice as runtime;

/// The region every request in this suite signs for; it is what the minted ARN must
/// carry.
const REGION: &str = "us-west-2";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A SigV4 `Authorization` header whose credential scope names `region` and
/// `signing_name` — the primary routing signal, and the one that makes every service
/// under a shared name see the same key. The signature itself is never checked here.
fn authorization_for(region: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=00"
    )
}

/// [`authorization_for`] in this suite's default region.
fn authorization(signing_name: &str) -> String {
    authorization_for(REGION, signing_name)
}

/// The label this suite reports a claiming service by. Nothing in the services depends on
/// it: it names the service under test so a failure says which one claimed what.
type Owner = &'static str;

const CONTROL_PLANE: Owner = "bedrock (control plane)";
const RUNTIME: Owner = "bedrock-runtime";
const AGENT: Owner = "bedrock-agent";
const AGENT_RUNTIME: Owner = "bedrock-agent-runtime";
const DATA_AUTOMATION: Owner = "bedrock-data-automation";
const DATA_AUTOMATION_RUNTIME: Owner = "bedrock-data-automation-runtime";
const AGENTCORE: Owner = "bedrock-agentcore";
const AGENTCORE_CONTROL: Owner = "bedrock-agentcore-control";

/// Every service that shares one of the two signing names under test, each paired with
/// its label, all on one engine exactly as `core/router/src/lib.rs` mounts them.
fn family(engine: &Arc<engine::InMemoryEngine>) -> Vec<(Owner, Box<dyn Service>)> {
    vec![
        (
            CONTROL_PLANE,
            Box::new(control_plane::EmulatedService::new(Arc::clone(engine))) as Box<dyn Service>,
        ),
        (
            RUNTIME,
            Box::new(runtime::EmulatedService::new(Arc::clone(engine))),
        ),
        (
            AGENT,
            Box::new(agent::EmulatedService::new(Arc::clone(engine))),
        ),
        (
            AGENT_RUNTIME,
            Box::new(agent_runtime::EmulatedService::new(Arc::clone(engine))),
        ),
        (
            DATA_AUTOMATION,
            Box::new(data_automation::EmulatedService::new(Arc::clone(engine))),
        ),
        (
            DATA_AUTOMATION_RUNTIME,
            Box::new(data_automation_runtime::EmulatedService::new(Arc::clone(
                engine,
            ))),
        ),
        (
            AGENTCORE,
            Box::new(agentcore::EmulatedService::new(Arc::clone(engine))),
        ),
        (
            AGENTCORE_CONTROL,
            Box::new(agentcore_control::EmulatedService::new(Arc::clone(engine))),
        ),
    ]
}

/// The same family mounted in a router, so a request can be dispatched end to end.
fn router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(control_plane::EmulatedService::new(Arc::clone(engine)));
    router.register_service(runtime::EmulatedService::new(Arc::clone(engine)));
    router.register_service(agent::EmulatedService::new(Arc::clone(engine)));
    router.register_service(agent_runtime::EmulatedService::new(Arc::clone(engine)));
    router.register_service(data_automation::EmulatedService::new(Arc::clone(engine)));
    router.register_service(data_automation_runtime::EmulatedService::new(Arc::clone(
        engine,
    )));
    router.register_service(agentcore::EmulatedService::new(Arc::clone(engine)));
    router.register_service(agentcore_control::EmulatedService::new(Arc::clone(engine)));
    router
}

/// One request to resolve: how it is signed, addressed, and (for the RPC member) which
/// service its `X-Amz-Target` names.
struct Call {
    signing_name: &'static str,
    method: &'static str,
    path: &'static str,
    target: Option<&'static str>,
}

/// The request `call` describes, as the borrowed `Parts` the claim seam is handed.
fn parts(call: &Call) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method(call.method)
        .uri(format!("https://emulator.local{}", call.path))
        .header("host", "emulator.local")
        .header("content-type", "application/json")
        .header("authorization", authorization(call.signing_name));
    if let Some(target) = call.target {
        builder = builder.header("x-amz-target", target);
    }
    builder
        .body(())
        .expect("a well-formed request")
        .into_parts()
        .0
}

/// Every mounted service whose `can_handle` claims `call`, by label.
fn claimants(family: &[(Owner, Box<dyn Service>)], call: &Call) -> Vec<Owner> {
    let parts = parts(call);
    family
        .iter()
        .filter(|(_, service)| service.can_handle(&parts))
        .map(|(owner, _)| *owner)
        .collect()
}

#[test]
fn each_service_sharing_a_signing_name_claims_exactly_its_own_modelled_routes() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let family = family(&engine);

    // (case, call, the one service that must claim it — `None` for a request no model in
    // either family binds). Every URI below is a route its owner's Smithy model declares
    // (`smithy_query query="operation"` on that service's model), and no row is a rename
    // of another: they differ in method, in segment count, in the literal segments, in
    // the signing name, and in whether any model binds the route.
    let rows: [(&str, Call, Option<Owner>); 5] = [
        (
            "ListAgentRuntimes on AgentCore Control",
            Call {
                signing_name: "bedrock-agentcore",
                method: "POST",
                path: "/runtimes/",
                target: None,
            },
            Some(AGENTCORE_CONTROL),
        ),
        (
            "InvokeAgentRuntime on the AgentCore data plane, same leading segment",
            Call {
                signing_name: "bedrock-agentcore",
                method: "POST",
                path: "/runtimes/agent-runtime-1/invocations",
                target: None,
            },
            Some(AGENTCORE),
        ),
        (
            "ListMemories on AgentCore Control",
            Call {
                signing_name: "bedrock-agentcore",
                method: "POST",
                path: "/memories/",
                target: None,
            },
            Some(AGENTCORE_CONTROL),
        ),
        (
            "ListSessions on the AgentCore data plane, same leading segment",
            Call {
                signing_name: "bedrock-agentcore",
                method: "POST",
                path: "/memories/mem-abc1234567/actor/actor-1/sessions",
                target: None,
            },
            Some(AGENTCORE),
        ),
        (
            // The control plane's own path, signed for a different service: the signing
            // name is still a conjunct of the claim, which no row of the sibling suite
            // pins for this family.
            "the control plane's path signed for another service",
            Call {
                signing_name: "s3",
                method: "POST",
                path: "/guardrails",
                target: None,
            },
            None,
        ),
    ];

    for (case, call, expected) in rows {
        let claimed = claimants(&family, &call);
        let expected_claim: Vec<Owner> = expected.into_iter().collect();
        assert_eq!(
            claimed, expected_claim,
            "case {case:?}: {} {} signed for {:?} must be claimed by {expected_claim:?}, \
             got {claimed:?}",
            call.method, call.path, call.signing_name
        );
    }
}

/// The modelled `CreateGuardrail` envelope, end to end through a router with the WHOLE
/// shared-name family mounted: the issue's request was answered `500 AmbiguousService`
/// with none of the response members, because six services claimed it.
///
/// The model (`smithy_query query="operation" operation="CreateGuardrail"`) binds
/// `POST /guardrails` with `@http` code 202 and a response whose `guardrailId`,
/// `guardrailArn`, `version` and `createdAt` are all `@required`, with `version`
/// constrained to the pattern `^DRAFT$`. The id and the instant are generated per
/// environment, so they are asserted by shape, not against a recorded value.
///
/// Run once per signing REGION, so the ARN is proved to be computed from the request's
/// credential scope rather than fixed to one region.
#[tokio::test]
async fn create_guardrail_answers_the_modelled_envelope_with_the_family_mounted() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router(&engine);

    // (case, the region the request is signed for, the guardrail name it creates).
    let rows: [(&str, &str, &str); 2] = [
        (
            "CreateGuardrail signed for us-west-2",
            REGION,
            "routing-check-guardrail-west",
        ),
        (
            "CreateGuardrail signed for eu-central-1",
            "eu-central-1",
            "routing-check-guardrail-eu",
        ),
    ];

    for (case, region, name) in rows {
        let body = serde_json::json!({
            "name": name,
            "blockedInputMessaging": "blocked input",
            "blockedOutputsMessaging": "blocked output",
        });
        let request = http::Request::builder()
            .method("POST")
            .uri("https://emulator.local/guardrails")
            .header("host", "emulator.local")
            .header("content-type", "application/json")
            .header("authorization", authorization_for(region, "bedrock"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("a serializable body"),
            ))
            .expect("a well-formed request");

        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let rendered = String::from_utf8_lossy(response.body()).to_string();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);

        assert_eq!(
            status, 202,
            "case {case:?}: the model binds @http code 202, got {status} with {rendered}"
        );
        assert_eq!(
            content_type, "application/json",
            "case {case:?}: restJson1 answers application/json, got {content_type:?} with \
             {rendered}"
        );
        assert_eq!(
            parsed.get("version").and_then(Value::as_str),
            Some("DRAFT"),
            "case {case:?}: version is constrained to ^DRAFT$, got {rendered}"
        );
        let id = parsed
            .get("guardrailId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {case:?}: guardrailId is required and matches ^[a-z0-9]+$, got {rendered}"
        );
        assert_eq!(
            parsed.get("guardrailArn").and_then(Value::as_str),
            Some(format!("arn:aws:bedrock:{region}:{ACCOUNT}:guardrail/{id}").as_str()),
            "case {case:?}: guardrailArn is required and carries the SIGNING region and \
             the minted id, got {rendered}"
        );
        // `createdAt` is `@required`, so the envelope must carry it, and its target
        // `com.amazonaws.bedrock#Timestamp` carries `@timestampFormat("date-time")`, so
        // the wire form is an RFC 3339 STRING rather than restJson1's default
        // epoch-seconds number. The INSTANT is minted per environment, so it is parsed
        // rather than compared to a recorded value; the service crate's
        // `tests/all/guardrail_timestamp_wire_form.rs` is where that rule is the subject.
        let created_at = parsed.get("createdAt").unwrap_or(&Value::Null);
        assert!(
            !created_at.is_null(),
            "case {case:?}: createdAt is required, got {rendered}"
        );
        let created_at = created_at.as_str().unwrap_or_else(|| {
            panic!(
                "case {case:?}: createdAt is a date-time member, so it is a string, got \
                 {rendered}"
            )
        });
        assert!(
            aws_smithy_types::DateTime::from_str(
                created_at,
                aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok(),
            "case {case:?}: createdAt must be an RFC 3339 date-time, got {created_at:?}"
        );
    }
}
