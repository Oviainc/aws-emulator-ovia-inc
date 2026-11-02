// SPDX-License-Identifier: CC0-1.0

//! The wire form of what this service reports about a gateway: the type every
//! `date-time` timestamp member takes on the wire, the shape of a gateway's endpoint URL,
//! and which members a gateway reports at all.
//!
//! Three rules are under test, each read off the Smithy model or the recorded responses
//! rather than off any one recorded value:
//!
//! 1. A member whose target carries `@timestampFormat("date-time")` is serialized on
//!    restJson1 as an RFC 3339 / ISO-8601 date-time STRING — never as restJson1's default
//!    epoch-seconds number. In this service that is EVERY timestamp member: all of them
//!    target `com.amazonaws.bedrockagentcorecontrol#DateTimestamp`, which the model marks
//!    `@timestampFormat("date-time")` (`smithy_query(query="shape",shape="DateTimestamp")`).
//!    It is checked end to end over HTTP on every implemented operation that reports a
//!    timestamp (so the rows differ along operation, shape, member, and whether the member
//!    is `@required`), and directly on the member schemas the codec reads the form off.
//! 2. A gateway's `gatewayUrl` is the endpoint AWS assigns the gateway: the host it was
//!    given, FOLLOWED by the transport path its `protocolType` is served on
//!    (`https://{gatewayId}.gateway.bedrock-agentcore.{region}.amazonaws.com/mcp` for MCP,
//!    the model's only protocol and its default), as the recorded
//!    `CreateGateway`/`UpdateGateway`/`GetGateway` responses report it. The whole rule, and
//!    the endpoint being the URL this emulator SERVES, is pinned in
//!    `gateway_endpoint_url.rs`; this row keeps the wire form beside the other wire rules.
//! 3. A gateway's `workloadIdentityDetails` carries a server-generated ARN: its text is
//!    minted by the service rather than derived from the request, so what is under test is
//!    the ARN's form and its invariants — stable across reads, one per gateway.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::ops::types::{
    AgentRuntime, CreateAgentRuntimeOutput, CreateGatewayOutput, CreateGatewayTargetOutput,
    GetAgentRuntimeOutput, GetGatewayOutput, GetGatewayTargetOutput, UpdateAgentRuntimeOutput,
    UpdateGatewayOutput, UpdateGatewayTargetOutput,
};
use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for unless one names another.
const REGION: &str = "us-east-1";

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRole";

/// The settle span `src/state.rs` gives the `CREATING` -> `READY` transition, plus slack,
/// so a row can wait the clock out and read a settled resource too.
const PAST_SETTLE: Duration = Duration::from_millis(900);

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`: what the router routes on,
/// and what the handlers read the ARN and endpoint region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(region))
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one request and require it to have succeeded: a row says nothing about a wire form
/// the service never produced.
async fn ok(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> Value {
    let (status, answer) = send(router, method, uri, region, body).await;
    assert!(
        (200..300).contains(&status),
        "{method} {uri} must succeed for this suite to say anything, got {status} with {answer}"
    );
    answer
}

/// The member at `pointer`, or `Value::Null` when the body carries none.
fn at(body: &Value, pointer: &str) -> Value {
    body.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// A `CreateGateway` body for `name` with `authorizer_type`, and the configuration the
/// model makes mandatory for `CUSTOM_JWT`.
fn gateway_body(name: &str, authorizer_type: &str) -> Value {
    let mut body = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "authorizerType": authorizer_type,
    });
    if authorizer_type == "CUSTOM_JWT" {
        body["authorizerConfiguration"] = json!({
            "customJWTAuthorizer": {
                "discoveryUrl": "https://example.com/.well-known/openid-configuration",
                "allowedClients": ["client-1"]
            }
        });
    }
    body
}

/// A `CreateAgentRuntime` body: a container artifact whose URI satisfies
/// `RuntimeContainerUri`'s ECR pattern.
fn runtime_body(name: &str, tag: &str) -> Value {
    json!({
        "agentRuntimeName": name,
        "agentRuntimeArtifact": {"containerConfiguration": {
            "containerUri": format!("public.ecr.aws/agentcore/demo:{tag}")
        }},
        "roleArn": "arn:aws:iam::000000000000:role/AgentRole",
        "environmentVariables": {"LOG_LEVEL": "debug"},
    })
}

/// Assert one `DateTimestamp` member's wire form: a JSON STRING that parses as an RFC 3339
/// date-time and renders back to the very same text, naming a real instant.
///
/// Parsed and re-rendered rather than matched against a literal: the instant itself comes
/// from this process's clock, so what is under test is the JSON type and the lexical form,
/// not the value.
fn assert_date_time(case: &str, member: &str, value: &Value) {
    assert!(
        !value.is_number(),
        "case {case:?}: {member} targets DateTimestamp (@timestampFormat date-time), so it must \
         be an RFC 3339 string and never an epoch-seconds number, got {value:?}"
    );
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 date-time string, got {value:?}")
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must parse as a date-time ({error}), got {text:?}")
    });
    let rendered = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {member} does not re-render ({error})"));
    assert_eq!(
        rendered, text,
        "case {case:?}: {member} must already be in the canonical date-time form, got {text:?} \
         against {rendered:?}"
    );
    assert!(
        parsed.secs() > 0,
        "case {case:?}: {member} must name a real instant, got {text:?}"
    );
}

/// Every `DateTimestamp` member an implemented operation reports is an RFC 3339 string on
/// the wire.
///
/// The rows span operation, shape, member and `@required`-ness: the two gateway envelopes,
/// the two gateway-target envelopes, the three agent-runtime envelopes and the
/// `ListAgentRuntimes` row, each read both while the resource is still in its transient
/// status and after the clock-owed transition settled it.
#[tokio::test]
async fn every_reported_date_time_member_is_an_rfc3339_string() {
    let router = router();

    let created_gateway = ok(
        &router,
        "POST",
        "/gateways/",
        REGION,
        Some(&gateway_body("wireshape", "AWS_IAM")),
    )
    .await;
    let gateway_id = at(&created_gateway, "/gatewayId")
        .as_str()
        .expect("the create reports the minted gatewayId")
        .to_string();
    let creating_gateway = ok(
        &router,
        "GET",
        &format!("/gateways/{gateway_id}/"),
        REGION,
        None,
    )
    .await;

    // The gateway has to be READY before it takes a target, so the clock-owed
    // `CREATING` -> `READY` transition is waited out here — after the `CREATING` read
    // above, which is what this row needs the transient status for.
    tokio::time::sleep(PAST_SETTLE).await;

    let created_target = ok(
        &router,
        "POST",
        &format!("/gateways/{gateway_id}/targets/"),
        REGION,
        Some(&json!({
            "name": "openapi-target",
            "targetConfiguration": {
                "mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.0.0\"}"}}
            }
        })),
    )
    .await;
    let target_id = at(&created_target, "/targetId")
        .as_str()
        .expect("the create reports the minted targetId")
        .to_string();
    let fetched_target = ok(
        &router,
        "GET",
        &format!("/gateways/{gateway_id}/targets/{target_id}/"),
        REGION,
        None,
    )
    .await;
    let updated_target = ok(
        &router,
        "PUT",
        &format!("/gateways/{gateway_id}/targets/{target_id}/"),
        REGION,
        Some(&json!({
            // `credentialProviderConfigurations` is `@required` on the response, so an
            // update that defines none is refused before it reports any wire form.
            "credentialProviderConfigurations": [{"credentialProviderType": "GATEWAY_IAM_ROLE"}],
            "targetConfiguration": {
                "mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.1.0\"}"}}
            }
        })),
    )
    .await;

    let created_runtime = ok(
        &router,
        "PUT",
        "/runtimes/",
        REGION,
        Some(&runtime_body("wire_shape_agent", "v1")),
    )
    .await;
    let runtime_id = at(&created_runtime, "/agentRuntimeId")
        .as_str()
        .expect("the create reports the minted agentRuntimeId")
        .to_string();
    let fetched_runtime = ok(
        &router,
        "GET",
        &format!("/runtimes/{runtime_id}/"),
        REGION,
        None,
    )
    .await;
    let updated_runtime = ok(
        &router,
        "PUT",
        &format!("/runtimes/{runtime_id}/"),
        REGION,
        Some(&json!({
            "agentRuntimeArtifact": {"containerConfiguration": {
                "containerUri": "public.ecr.aws/agentcore/demo:v2"
            }},
            "roleArn": "arn:aws:iam::000000000000:role/AgentRoleV2",
        })),
    )
    .await;
    let listed_runtimes = ok(&router, "POST", "/runtimes/", REGION, None).await;

    // The same resources once the clock-owed transition is due: a settled read renders its
    // timestamps through the same member schemas, and `lastUpdatedAt` has moved on.
    tokio::time::sleep(PAST_SETTLE).await;
    let settled_gateway = ok(
        &router,
        "GET",
        &format!("/gateways/{gateway_id}/"),
        REGION,
        None,
    )
    .await;
    let settled_runtime = ok(
        &router,
        "GET",
        &format!("/runtimes/{runtime_id}/"),
        REGION,
        None,
    )
    .await;

    // (case, body, member pointer) — one row per `DateTimestamp` member each implemented
    // operation reports.
    let rows: Vec<(&str, &Value, &str)> = vec![
        ("CreateGateway", &created_gateway, "/createdAt"),
        ("CreateGateway", &created_gateway, "/updatedAt"),
        ("GetGateway, still CREATING", &creating_gateway, "/createdAt"),
        ("GetGateway, still CREATING", &creating_gateway, "/updatedAt"),
        ("GetGateway, settled", &settled_gateway, "/createdAt"),
        ("GetGateway, settled", &settled_gateway, "/updatedAt"),
        ("CreateGatewayTarget", &created_target, "/createdAt"),
        ("CreateGatewayTarget", &created_target, "/updatedAt"),
        ("GetGatewayTarget", &fetched_target, "/createdAt"),
        ("GetGatewayTarget", &fetched_target, "/updatedAt"),
        ("UpdateGatewayTarget", &updated_target, "/createdAt"),
        ("UpdateGatewayTarget", &updated_target, "/updatedAt"),
        ("CreateAgentRuntime", &created_runtime, "/createdAt"),
        ("GetAgentRuntime", &fetched_runtime, "/createdAt"),
        ("GetAgentRuntime", &fetched_runtime, "/lastUpdatedAt"),
        ("GetAgentRuntime, settled", &settled_runtime, "/createdAt"),
        ("GetAgentRuntime, settled", &settled_runtime, "/lastUpdatedAt"),
        ("UpdateAgentRuntime", &updated_runtime, "/createdAt"),
        ("UpdateAgentRuntime", &updated_runtime, "/lastUpdatedAt"),
        (
            "ListAgentRuntimes/AgentRuntime",
            &listed_runtimes,
            "/agentRuntimes/0/lastUpdatedAt",
        ),
    ];

    for (case, body, pointer) in rows {
        let value = at(body, pointer);
        assert!(
            !value.is_null(),
            "case {case:?}: {pointer} is @required, so the answer must carry it, got {body}"
        );
        assert_date_time(case, pointer, &value);
    }

    // `lastSynchronizedAt` is the optional timestamp of this family: nothing synchronized
    // this target (`SynchronizeGatewayTargets` is not implemented), so it is absent rather
    // than an epoch-zero instant.
    for (case, body) in [
        ("CreateGatewayTarget", &created_target),
        ("GetGatewayTarget", &fetched_target),
        ("UpdateGatewayTarget", &updated_target),
    ] {
        assert!(
            at(body, "/lastSynchronizedAt").is_null(),
            "case {case:?}: nothing synchronized this target, so lastSynchronizedAt is absent, \
             got {}",
            at(body, "/lastSynchronizedAt")
        );
    }

    // A read reports the instant the create stored, in the same rendering: the wire form is
    // not re-derived per response. `updatedAt` is compared against the read taken BEFORE a
    // target was added to the gateway, which is a change to the gateway and moves it.
    for (case, body, member) in [
        ("gateway createdAt", &settled_gateway, "/createdAt"),
        ("gateway createdAt, first read", &creating_gateway, "/createdAt"),
        ("gateway updatedAt, first read", &creating_gateway, "/updatedAt"),
    ] {
        assert_eq!(
            at(body, member),
            at(&created_gateway, member),
            "case {case:?}: GetGateway reports the instant CreateGateway stored, got {} for {}",
            at(body, member),
            at(&created_gateway, member)
        );
    }
}

/// The member schemas the codec reads the wire form off: every `DateTimestamp` member of
/// the shapes this service reports carries `@timestampFormat("date-time")`, and a member
/// the model does not annotate carries none.
///
/// This is what fails if any of these schemas goes back to the vendored codegen's output,
/// which drops the trait (see `src/ops/types/date_time_wire.rs`).
#[test]
fn every_reported_timestamp_member_schema_carries_the_date_time_format() {
    // Source: `smithy_query(query="shape",shape="CreateGatewayResponse,GetGatewayResponse,
    // UpdateGatewayResponse,CreateGatewayTargetResponse,GetGatewayTargetResponse,
    // UpdateGatewayTargetResponse,CreateAgentRuntimeResponse,GetAgentRuntimeResponse,
    // UpdateAgentRuntimeResponse,AgentRuntime")` — every member of these shapes targeting
    // `DateTimestamp`.
    let date_time_members: Vec<(&str, &Schema)> = vec![
        ("CreateGatewayResponse$createdAt", &CreateGatewayOutput::M_CREATED_AT),
        ("CreateGatewayResponse$updatedAt", &CreateGatewayOutput::M_UPDATED_AT),
        ("GetGatewayResponse$createdAt", &GetGatewayOutput::M_CREATED_AT),
        ("GetGatewayResponse$updatedAt", &GetGatewayOutput::M_UPDATED_AT),
        ("UpdateGatewayResponse$createdAt", &UpdateGatewayOutput::M_CREATED_AT),
        ("UpdateGatewayResponse$updatedAt", &UpdateGatewayOutput::M_UPDATED_AT),
        (
            "CreateGatewayTargetResponse$createdAt",
            &CreateGatewayTargetOutput::M_CREATED_AT,
        ),
        (
            "CreateGatewayTargetResponse$updatedAt",
            &CreateGatewayTargetOutput::M_UPDATED_AT,
        ),
        (
            "CreateGatewayTargetResponse$lastSynchronizedAt",
            &CreateGatewayTargetOutput::M_LAST_SYNCHRONIZED_AT,
        ),
        (
            "GetGatewayTargetResponse$createdAt",
            &GetGatewayTargetOutput::M_CREATED_AT,
        ),
        (
            "GetGatewayTargetResponse$updatedAt",
            &GetGatewayTargetOutput::M_UPDATED_AT,
        ),
        (
            "GetGatewayTargetResponse$lastSynchronizedAt",
            &GetGatewayTargetOutput::M_LAST_SYNCHRONIZED_AT,
        ),
        (
            "UpdateGatewayTargetResponse$createdAt",
            &UpdateGatewayTargetOutput::M_CREATED_AT,
        ),
        (
            "UpdateGatewayTargetResponse$updatedAt",
            &UpdateGatewayTargetOutput::M_UPDATED_AT,
        ),
        (
            "UpdateGatewayTargetResponse$lastSynchronizedAt",
            &UpdateGatewayTargetOutput::M_LAST_SYNCHRONIZED_AT,
        ),
        (
            "CreateAgentRuntimeResponse$createdAt",
            &CreateAgentRuntimeOutput::M_CREATED_AT,
        ),
        ("GetAgentRuntimeResponse$createdAt", &GetAgentRuntimeOutput::M_CREATED_AT),
        (
            "GetAgentRuntimeResponse$lastUpdatedAt",
            &GetAgentRuntimeOutput::M_LAST_UPDATED_AT,
        ),
        (
            "UpdateAgentRuntimeResponse$createdAt",
            &UpdateAgentRuntimeOutput::M_CREATED_AT,
        ),
        (
            "UpdateAgentRuntimeResponse$lastUpdatedAt",
            &UpdateAgentRuntimeOutput::M_LAST_UPDATED_AT,
        ),
        ("AgentRuntime$lastUpdatedAt", &AgentRuntime::M_LAST_UPDATED_AT),
    ];

    for (case, schema) in date_time_members {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case:?}: the member schema must carry @timestampFormat(date-time), got \
             {format:?}"
        );
    }

    // A member the model annotates no format for keeps the protocol default, so nothing
    // above put a format where the model names none.
    for (case, schema) in [
        ("CreateGatewayResponse$gatewayUrl", &CreateGatewayOutput::M_GATEWAY_URL),
        ("GetGatewayResponse$status", &GetGatewayOutput::M_STATUS),
        (
            "GetGatewayTargetResponse$targetConfiguration",
            &GetGatewayTargetOutput::M_TARGET_CONFIGURATION,
        ),
        ("AgentRuntime$agentRuntimeArn", &AgentRuntime::M_AGENT_RUNTIME_ARN),
    ] {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            format.is_none(),
            "case {case:?}: the model annotates no format for this member, got {format:?}"
        );
    }
}

/// A gateway's `gatewayUrl` is the host it was assigned plus the transport path of the
/// protocol it speaks, and the read reports the same URL the create did.
///
/// The rows differ along what the rule generalises over: the name's shape (which the
/// gateway id is derived from) and the Region the request is signed for (which the host
/// carries).
#[tokio::test]
async fn a_gateway_url_is_the_assigned_host_and_transport_path() {
    let router = router();

    // (case, name, region)
    let rows: [(&str, &str, &str); 5] = [
        ("all lower case, us-east-1", "plaingateway", "us-east-1"),
        ("mixed case, eu-west-1", "MixedCaseGateway", "eu-west-1"),
        ("dashes between characters, ap-southeast-2", "my-mcp-gateway", "ap-southeast-2"),
        ("a name ending in a dash, us-west-2", "trailing-dash-", "us-west-2"),
        ("a single character, eu-central-1", "g", "eu-central-1"),
    ];

    for (case, name, region) in rows {
        let created = ok(
            &router,
            "POST",
            "/gateways/",
            region,
            Some(&gateway_body(name, "AWS_IAM")),
        )
        .await;
        let id = at(&created, "/gatewayId")
            .as_str()
            .expect("the create reports the minted gatewayId")
            .to_string();
        let expected =
            format!("https://{id}.gateway.bedrock-agentcore.{region}.amazonaws.com/mcp");
        let url = at(&created, "/gatewayUrl");
        assert_eq!(
            url,
            json!(expected),
            "case {case:?}: expected the assigned gateway endpoint {expected:?}, got {url}"
        );

        let fetched = ok(&router, "GET", &format!("/gateways/{id}/"), region, None).await;
        assert_eq!(
            at(&fetched, "/gatewayUrl"),
            url,
            "case {case:?}: GetGateway reports the URL CreateGateway stored, got {} for {url}",
            at(&fetched, "/gatewayUrl")
        );
    }
}

/// A gateway's `workloadIdentityDetails` is reported as a server-generated ARN that is
/// stable across reads and unique per gateway — the invariants such a value has, since its
/// text is minted by the service and not derived from the request.
///
/// The rows differ along the authorizer the gateway is created with, the one member whose
/// value changes what the request must carry. Source for the value list: `AuthorizerType`'s
/// enum (`smithy_query(query="shape",shape="AuthorizerType")`).
#[tokio::test]
async fn a_workload_identity_arn_is_stable_across_reads_and_unique_per_resource() {
    let router = router();

    let rows: [(&str, &str); 4] = [
        ("AWS_IAM", "AWS_IAM"),
        ("NONE", "NONE"),
        ("AUTHENTICATE_ONLY", "AUTHENTICATE_ONLY"),
        ("CUSTOM_JWT", "CUSTOM_JWT"),
    ];

    // The prefix every emulated workload identity ARN carries: the token vault's `default`
    // directory under this service's ARN namespace (`src/state.rs::workload_identity_arn`).
    let prefix = format!(
        "arn:aws:bedrock-agentcore:{REGION}:{}:workload-identity-directory/default/\
         workload-identity/",
        engine::EMULATOR_ACCOUNT_ID
    );

    let mut seen: Vec<String> = Vec::new();
    for (index, (case, authorizer_type)) in rows.into_iter().enumerate() {
        let name = format!("widgw{index}");
        let created = ok(
            &router,
            "POST",
            "/gateways/",
            REGION,
            Some(&gateway_body(&name, authorizer_type)),
        )
        .await;
        let arn = at(&created, "/workloadIdentityDetails/workloadIdentityArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.starts_with(&prefix),
            "case {case:?}: expected a workload identity ARN under {prefix:?}, got {arn:?}"
        );
        assert!(
            !seen.contains(&arn),
            "case {case:?}: each gateway's workload identity is its own, got {arn:?} twice"
        );
        seen.push(arn.clone());

        let id = at(&created, "/gatewayId")
            .as_str()
            .expect("the create reports the minted gatewayId")
            .to_string();
        for attempt in 0..2 {
            let fetched = ok(&router, "GET", &format!("/gateways/{id}/"), REGION, None).await;
            assert_eq!(
                at(&fetched, "/workloadIdentityDetails/workloadIdentityArn"),
                json!(arn),
                "case {case:?}: read {attempt} must report the stored workload identity, got {}",
                at(&fetched, "/workloadIdentityDetails/workloadIdentityArn")
            );
        }
    }

    // The runtime half of the same rule: a runtime is created with a workload identity of
    // its own, which its own reads report unchanged.
    let runtime = ok(
        &router,
        "PUT",
        "/runtimes/",
        REGION,
        Some(&runtime_body("workload_identity_agent", "v1")),
    )
    .await;
    let runtime_arn = at(&runtime, "/workloadIdentityDetails/workloadIdentityArn");
    assert!(
        runtime_arn
            .as_str()
            .is_some_and(|arn| arn.starts_with(&prefix)),
        "case \"CreateAgentRuntime\": a runtime is created with a workload identity, so it \
         reports one, got {runtime_arn}"
    );
    let runtime_id = at(&runtime, "/agentRuntimeId")
        .as_str()
        .expect("the create reports the minted agentRuntimeId")
        .to_string();
    let fetched_runtime = ok(
        &router,
        "GET",
        &format!("/runtimes/{runtime_id}/"),
        REGION,
        None,
    )
    .await;
    assert_eq!(
        at(&fetched_runtime, "/workloadIdentityDetails/workloadIdentityArn"),
        runtime_arn,
        "case \"GetAgentRuntime\": the read reports the stored workload identity, got {}",
        at(&fetched_runtime, "/workloadIdentityDetails/workloadIdentityArn")
    );
}
