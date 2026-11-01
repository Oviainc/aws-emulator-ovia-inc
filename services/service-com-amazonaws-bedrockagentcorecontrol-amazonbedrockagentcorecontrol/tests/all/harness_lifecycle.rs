// SPDX-License-Identifier: CC0-1.0

//! The harness surface of this control plane, end to end over HTTP.
//!
//! Three rules are under test, each read off the Smithy model rather than off any one
//! recorded request:
//!
//! 1. REACHABILITY. Every method + URI this service's model binds resolves to the
//!    operation that model binds there, and is answered in THAT operation's contract —
//!    its modelled success, its modelled error, or the scaffold's `501 NotImplemented`
//!    for one not written yet. It is never answered by the router's own
//!    `404 UnknownService` in the router's envelope (`application/x-amz-json-1.1`), which
//!    is what a route no service claims gets and is an error code no AWS model declares.
//!    The rows span the four harness operations and the harness-endpoint family, so the
//!    rule is checked across method, URI depth and implementation state.
//! 2. INPUT VALIDATION. A member outside its modelled constraint is refused `400` with
//!    `x-amzn-errortype: ValidationException` and a message that names the member — the
//!    boundary validator's doing, which only runs for a request that reaches the service.
//!    The rows span the shape of the offending value (leading digit, punctuation, empty,
//!    one past the `@pattern`'s 40-character stem) and include the bound itself.
//! 3. ROUND TRIP. `GetHarness` reports what `CreateHarness` persisted, member for member,
//!    and a harness that is not stored (never created, deleted, or read at a version it
//!    does not hold) is `404 ResourceNotFoundException`. The rows span the configuration
//!    the request carries: the model provider, tools (including the valueless union
//!    members of `HarnessGatewayOutboundAuth`), skills, hooks, memory, the environment and
//!    the integer limits.
//!
//! Sources: `smithy_query(query="operation",operation="CreateHarness,GetHarness,\
//! DeleteHarness,UpdateHarness")` for the routes, statuses, members and errors, and
//! `smithy_query(query="shape",shape="HarnessName,HarnessId,HarnessArn,HarnessStatus")`
//! for the patterns and the status values asserted below.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/HarnessRole";

/// The settle span `src/state.rs` gives the harness `CREATING` -> `READY` transition, plus
/// slack, so a row can wait the clock out and read a settled harness too.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// A `harnessId` inside `HarnessId`'s pattern
/// `^[a-zA-Z][a-zA-Z0-9_]{0,39}-[a-zA-Z0-9]{10}$` that no row ever creates.
const ABSENT_HARNESS_ID: &str = "noSuchHarness-abcdefghij";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on, and what
/// the handlers read the ARN and endpoint region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the status, the `x-amzn-errortype` header, the `content-type` header and
/// the parsed body.
struct Answer {
    status: u16,
    error_type: String,
    content_type: String,
    body: Value,
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    Answer {
        status: response.status().as_u16(),
        error_type: header("x-amzn-errortype"),
        content_type: header("content-type"),
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

/// The member at `pointer`, or `Value::Null` when the body carries none.
fn at(body: &Value, pointer: &str) -> Value {
    body.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// A `CreateHarness` body for `name`: the two `@required` members, the `model` this
/// service has no default for, and whatever else the row configures.
fn harness_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "harnessName": name,
        "executionRoleArn": ROLE_ARN,
        "model": {"bedrockModelConfig": {"modelId": "amazon.nova-pro-v1:0"}},
    });
    if let Value::Object(extra) = extra {
        for (key, value) in extra {
            body[key] = value;
        }
    }
    body
}

/// Create one harness and return (harnessId, the created `harness` member).
async fn create(router: &MultiServiceRouter, name: &str, extra: Value) -> (String, Value) {
    let answer = send(router, "POST", "/harnesses", Some(&harness_body(name, extra))).await;
    assert_eq!(
        answer.status, 201,
        "creating {name:?} must succeed for this row to say anything, got {} with {}",
        answer.status, answer.body
    );
    let harness = at(&answer.body, "/harness");
    let id = at(&harness, "/harnessId")
        .as_str()
        .unwrap_or_else(|| panic!("CreateHarness reports the minted harnessId, got {harness}"))
        .to_string();
    (id, harness)
}

/// One reachability row: a method + URI this model binds, the body to send, and the answer
/// the operation bound there owes.
struct Route {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    error_type: &'static str,
}

/// Rule 1: every modelled harness route reaches the operation bound there.
///
/// Rows differ along the method, the URI depth, the operation's implementation state
/// (implemented, scaffolded) and the answer's class (success, modelled error, 501). Every
/// row asserts the SAME thing about the envelope: this service answered, so the error code
/// is never the router's `UnknownService` and the content type is always this service's
/// `restJson1` `application/json` — never the router's `application/x-amz-json-1.1`.
#[tokio::test]
async fn every_modelled_harness_route_reaches_its_own_operation() {
    let router = router();
    let (existing, _) = create(&router, "routedHarness", json!({})).await;

    let rows: Vec<Route> = vec![
        Route {
            case: "CreateHarness, implemented",
            method: "POST",
            uri: "/harnesses".to_string(),
            body: Some(harness_body("reachableHarness", json!({}))),
            status: 201,
            error_type: "",
        },
        Route {
            case: "CreateHarness with a harnessName outside its @pattern",
            method: "POST",
            uri: "/harnesses".to_string(),
            body: Some(harness_body("1leadingDigit", json!({}))),
            status: 400,
            error_type: "ValidationException",
        },
        Route {
            case: "GetHarness on a harness that does not exist, implemented",
            method: "GET",
            uri: format!("/harnesses/{ABSENT_HARNESS_ID}"),
            body: None,
            status: 404,
            error_type: "ResourceNotFoundException",
        },
        Route {
            case: "GetHarness on a stored harness, implemented",
            method: "GET",
            uri: format!("/harnesses/{existing}"),
            body: None,
            status: 200,
            error_type: "",
        },
        Route {
            case: "DeleteHarness on a harness that does not exist, implemented",
            method: "DELETE",
            uri: format!("/harnesses/{ABSENT_HARNESS_ID}"),
            body: None,
            status: 404,
            error_type: "ResourceNotFoundException",
        },
        Route {
            case: "UpdateHarness, scaffolded",
            method: "PATCH",
            uri: format!("/harnesses/{existing}"),
            body: Some(json!({"maxIterations": 4})),
            status: 501,
            error_type: "NotImplemented",
        },
        Route {
            case: "ListHarnesses, scaffolded",
            method: "GET",
            uri: "/harnesses".to_string(),
            body: None,
            status: 501,
            error_type: "NotImplemented",
        },
        Route {
            case: "CreateHarnessEndpoint on a stored harness, implemented",
            method: "POST",
            uri: format!("/harnesses/{existing}/endpoints"),
            body: Some(json!({"endpointName": "live"})),
            status: 201,
            error_type: "",
        },
        Route {
            case: "GetHarnessEndpoint on the endpoint the row above created, implemented",
            method: "GET",
            uri: format!("/harnesses/{existing}/endpoints/live"),
            body: None,
            status: 200,
            error_type: "",
        },
        Route {
            case: "GetHarnessEndpoint on an endpoint the harness does not hold, implemented",
            method: "GET",
            uri: format!("/harnesses/{existing}/endpoints/absent"),
            body: None,
            status: 404,
            error_type: "ResourceNotFoundException",
        },
        Route {
            case: "DeleteHarnessEndpoint on the endpoint the create row added, implemented",
            method: "DELETE",
            uri: format!("/harnesses/{existing}/endpoints/live"),
            body: None,
            status: 200,
            error_type: "",
        },
        Route {
            case: "ListHarnessVersions on a stored harness, implemented",
            method: "GET",
            uri: format!("/harnesses/{existing}/versions"),
            body: None,
            status: 200,
            error_type: "",
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = send(&router, row.method, &row.uri, row.body.as_ref()).await;
        assert_ne!(
            answer.error_type, "UnknownService",
            "case {case:?}: {} {} is a route this model binds, so it must reach this service \
             rather than the router's UnknownService refusal, got {}",
            row.method, row.uri, answer.body
        );
        assert_eq!(
            answer.content_type, "application/json",
            "case {case:?}: this service speaks restJson1, so every answer is application/json, \
             got {:?} with {}",
            answer.content_type, answer.body
        );
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (row.status, row.error_type),
            "case {case:?}: expected HTTP {} {:?}, got {} {:?} with {}",
            row.status,
            row.error_type,
            answer.status,
            answer.error_type,
            answer.body
        );
    }
}

/// Rule 2: `harnessName` outside `HarnessName`'s `@pattern`
/// `^[a-zA-Z][a-zA-Z0-9_]{0,39}$` is refused 400 `ValidationException`, naming the member;
/// a name the pattern admits is created.
///
/// Rows span the shape of the value — leading digit, punctuation the pattern excludes, the
/// empty string, one character past the 40-character stem — and the admitted forms,
/// including the stem's own upper bound.
#[tokio::test]
async fn create_harness_refuses_a_name_outside_the_modelled_pattern() {
    let router = router();

    // (case, harnessName, admitted by the pattern)
    let rows: Vec<(&str, String, bool)> = vec![
        ("a leading digit", "1harnessProbe".to_string(), false),
        ("a dash, which the pattern excludes", "harness-probe".to_string(), false),
        ("a dot, which the pattern excludes", "harness.probe".to_string(), false),
        ("a space, which the pattern excludes", "harness probe".to_string(), false),
        ("the empty string", String::new(), false),
        // The pattern admits a leading letter plus up to 39 more characters.
        ("one past the 40-character stem", format!("h{}", "a".repeat(40)), false),
        ("a single letter, the shortest name", "h".to_string(), true),
        ("letters, digits and underscores", "harness_Probe_42".to_string(), true),
        ("exactly 40 characters, the bound itself", format!("h{}", "a".repeat(39)), true),
    ];

    for (case, name, admitted) in rows {
        let answer = send(&router, "POST", "/harnesses", Some(&harness_body(&name, json!({})))).await;
        if admitted {
            assert_eq!(
                answer.status, 201,
                "case {case:?}: {name:?} satisfies HarnessName's pattern, so it is created, got \
                 {} with {}",
                answer.status, answer.body
            );
            continue;
        }
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?}: {name:?} is outside HarnessName's pattern, so it is refused 400 \
             ValidationException, got {} {:?} with {}",
            answer.status,
            answer.error_type,
            answer.body
        );
        let message = at(&answer.body, "/message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains("harnessName"),
            "case {case:?}: the refusal names the member the caller has to fix, got {message:?}"
        );
        assert!(
            message.contains("constraint"),
            "case {case:?}: the refusal says which constraint failed, got {message:?}"
        );
    }
}

/// Rule 3: `GetHarness` reports what `CreateHarness` persisted, member for member.
///
/// Rows span the configuration the request carries — the model provider, every tool kind
/// (including the two VALUELESS members of `HarnessGatewayOutboundAuth`, which are the
/// reason the scaffold could not generate these operations at all), skills, hooks, memory,
/// the environment artifact, the environment's network and lifecycle settings, the integer
/// limits and the environment variables.
#[tokio::test]
async fn a_created_harness_reads_back_member_for_member() {
    let router = router();

    // (case, the members the row adds to the create body)
    let rows: Vec<(&str, Value)> = vec![
        ("the two required members and the model only", json!({})),
        (
            "a system prompt, tools and allowedTools",
            json!({
                "systemPrompt": [{"text": "You are a careful assistant."}],
                "tools": [
                    {"type": "remote_mcp", "name": "docs", "config": {"remoteMcp": {
                        "url": "https://mcp.example.com/sse",
                        "headers": {"x-api-version": "2026-01-01"}
                    }}},
                    {"type": "agentcore_code_interpreter", "name": "python"}
                ],
                "allowedTools": ["*", "@builtin", "@docs/search"],
            }),
        ),
        (
            "a gateway tool authenticating with awsIam, a union member carrying no value",
            json!({
                "tools": [{"type": "agentcore_gateway", "config": {"agentCoreGateway": {
                    "gatewayArn": "arn:aws:bedrock-agentcore:us-east-1:000000000000:gateway/probe-gateway-abcdefghij",
                    "outboundAuth": {"awsIam": {}}
                }}}],
            }),
        ),
        (
            "a gateway tool authenticating with none, the other valueless member",
            json!({
                "tools": [{"type": "agentcore_gateway", "config": {"agentCoreGateway": {
                    "gatewayArn": "arn:aws:bedrock-agentcore:us-east-1:000000000000:gateway/probe-gateway-abcdefghij",
                    "outboundAuth": {"none": {}}
                }}}],
            }),
        ),
        (
            "skills from three sources and a container artifact",
            json!({
                "skills": [
                    {"path": "/opt/skills/triage"},
                    {"s3": {"uri": "s3://example-bucket/skills/triage/"}},
                    {"awsSkills": {"paths": ["core-skills/*"]}}
                ],
                "environmentArtifact": {"containerConfiguration": {
                    "containerUri": "public.ecr.aws/agentcore/harness:v1"
                }},
            }),
        ),
        (
            "a VPC environment, environment variables and the integer limits",
            json!({
                "environment": {"agentCoreRuntimeEnvironment": {
                    "lifecycleConfiguration": {"idleRuntimeSessionTimeout": 120, "maxLifetime": 3600},
                    "networkConfiguration": {"networkMode": "VPC", "networkModeConfig": {
                        "securityGroups": ["sg-0123456789abcdef0"],
                        "subnets": ["subnet-0123456789abcdef0"]
                    }}
                }},
                "environmentVariables": {"LOG_LEVEL": "debug", "STAGE": "test"},
                "maxIterations": 12,
                "maxTokens": 4096,
                "timeoutSeconds": 300,
            }),
        ),
        (
            "memory disabled, a summarization truncation strategy and a lambda hook",
            json!({
                "memory": {"disabled": {}},
                "truncation": {"strategy": "summarization", "config": {"summarization": {
                    "preserveRecentMessages": 5,
                    "summarizationSystemPrompt": "Summarize the conversation."
                }}},
                "hooks": [{"beforeInvocation": {"name": "audit", "target": {"lambda": {
                    "arn": "arn:aws:lambda:us-east-1:000000000000:function:audit"
                }}}}],
            }),
        ),
        (
            "an OpenAI model provider and AgentCore memory",
            json!({
                "model": {"openAiModelConfig": {
                    "modelId": "gpt-4o",
                    "apiKeyArn": "arn:aws:bedrock-agentcore:us-east-1:000000000000:token-vault/default/apikeycredentialprovider/openai",
                    "temperature": 0.5,
                    "topP": 0.9,
                    "apiFormat": "responses"
                }},
                "memory": {"agentCoreMemoryConfiguration": {
                    "arn": "arn:aws:bedrock-agentcore:us-east-1:000000000000:memory/probeMemory-abcdefghij",
                    "actorId": "actor-1",
                    "messagesCount": 20
                }},
            }),
        ),
    ];

    for (index, (case, extra)) in rows.into_iter().enumerate() {
        let name = format!("roundTrip{index}");
        let (id, created) = create(&router, &name, extra).await;

        // What the service minted, in the forms the model publishes.
        assert_eq!(
            at(&created, "/harnessName"),
            json!(name),
            "case {case:?}: the harness reports the name it was created with, got {created}"
        );
        assert!(
            id.starts_with(&format!("{name}-")) && id.len() == name.len() + 11,
            "case {case:?}: a harnessId is the name plus a dash and ten characters \
             (HarnessId's pattern), got {id:?}"
        );
        let expected_arn = format!(
            "arn:aws:bedrock-agentcore:{REGION}:{}:harness/{id}",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            at(&created, "/arn"),
            json!(expected_arn),
            "case {case:?}: expected the harness ARN {expected_arn:?}, got {}",
            at(&created, "/arn")
        );
        assert_eq!(
            at(&created, "/harnessVersion"),
            json!("1"),
            "case {case:?}: a created harness is at its first version, got {}",
            at(&created, "/harnessVersion")
        );
        assert_eq!(
            at(&created, "/status"),
            json!("CREATING"),
            "case {case:?}: a harness is created, not ready — the transition to READY is the \
             clock's — got {}",
            at(&created, "/status")
        );
        // The two `@timestampFormat("date-time")` members: RFC 3339 strings, never
        // restJson1's default epoch-seconds number.
        for member in ["/createdAt", "/updatedAt"] {
            let value = at(&created, member);
            let text = value.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: {member} must be an RFC 3339 string, got {value:?}")
            });
            DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
                panic!("case {case:?}: {member} must parse as a date-time ({error}), got {text:?}")
            });
        }
        // The environment is `@required` on the response and optional on the request, so
        // every harness reports the AgentCore Runtime it runs in.
        assert_eq!(
            at(&created, "/environment/agentCoreRuntimeEnvironment/agentRuntimeId"),
            json!(id),
            "case {case:?}: the runtime a harness runs in is reported with the harness, got {}",
            at(&created, "/environment")
        );

        // The read reports the stored harness, member for member. `status` is compared
        // apart: the clock may have settled it between the two calls.
        let fetched = send(&router, "GET", &format!("/harnesses/{id}"), None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the harness that was just created is readable, got {} with {}",
            fetched.status, fetched.body
        );
        let read = at(&fetched.body, "/harness");
        let strip = |harness: &Value| {
            let mut value = harness.clone();
            value
                .as_object_mut()
                .expect("the harness is a JSON object")
                .remove("status");
            value
        };
        assert_eq!(
            strip(&read),
            strip(&created),
            "case {case:?}: GetHarness reports what CreateHarness persisted, got {read} for \
             {created}"
        );
        assert!(
            ["CREATING", "READY"].contains(&at(&read, "/status").as_str().unwrap_or_default()),
            "case {case:?}: a stored harness is CREATING until the clock owes it READY, got {}",
            at(&read, "/status")
        );
    }

    // The clock-owed transition, materialised by a read once it is due: a harness AWS
    // returns in a non-terminal status reaches its terminal one without any further call.
    let (settling, _) = create(&router, "settlingHarness", json!({})).await;
    tokio::time::sleep(PAST_SETTLE).await;
    let settled = send(&router, "GET", &format!("/harnesses/{settling}"), None).await;
    assert_eq!(
        at(&settled.body, "/harness/status"),
        json!("READY"),
        "case \"past the settle span\": the CREATING -> READY transition is due, got {}",
        at(&settled.body, "/harness/status")
    );
}

/// Rule 3's other half: a harness that is not stored is `404 ResourceNotFoundException`.
///
/// Rows span the ways a request can name no stored harness — an id that was never created,
/// one whose harness was deleted, and a stored harness read at a version it does not hold
/// (including the highest `HarnessVersion`'s pattern admits) — and each asserts the
/// modelled error code rather than only the status, which `404 UnknownService` shares.
#[tokio::test]
async fn an_absent_harness_is_reported_as_resource_not_found() {
    let router = router();
    let (stored, _) = create(&router, "storedHarness", json!({})).await;
    let (deleted, deleted_harness) = create(&router, "deletedHarness", json!({})).await;

    let delete = send(&router, "DELETE", &format!("/harnesses/{deleted}"), None).await;
    assert_eq!(
        delete.status, 200,
        "deleting a stored harness succeeds, got {} with {}",
        delete.status, delete.body
    );
    assert_eq!(
        at(&delete.body, "/harness/harnessId"),
        json!(deleted),
        "DeleteHarness reports the harness it removed, got {}",
        delete.body
    );
    assert_eq!(
        at(&delete.body, "/harness/status"),
        json!("DELETING"),
        "the harness a delete removed is reported as DELETING, got {}",
        at(&delete.body, "/harness/status")
    );
    assert_eq!(
        at(&delete.body, "/harness/arn"),
        at(&deleted_harness, "/arn"),
        "DeleteHarness reports the stored harness, not a fresh one, got {}",
        delete.body
    );

    // (case, uri)
    let rows: Vec<(&str, String)> = vec![
        (
            "an id inside HarnessId's pattern that was never created",
            format!("/harnesses/{ABSENT_HARNESS_ID}"),
        ),
        (
            "a harness that was deleted",
            format!("/harnesses/{deleted}"),
        ),
        (
            "a stored harness at a version it does not hold",
            format!("/harnesses/{stored}?harnessVersion=2"),
        ),
        (
            "a stored harness at the highest version HarnessVersion admits",
            format!("/harnesses/{stored}?harnessVersion=99999"),
        ),
    ];

    for (case, uri) in rows {
        let answer = send(&router, "GET", &uri, None).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: {uri} names no stored harness, so it is the operation's modelled \
             ResourceNotFoundException, got {} {:?} with {}",
            answer.status,
            answer.error_type,
            answer.body
        );
    }

    // The version the harness DOES hold still reads, so the rows above refuse the version
    // rather than the query parameter.
    let held = send(
        &router,
        "GET",
        &format!("/harnesses/{stored}?harnessVersion=1"),
        None,
    )
    .await;
    assert_eq!(
        held.status, 200,
        "case \"the version the harness holds\": expected the harness, got {} with {}",
        held.status, held.body
    );
}

/// A `clientToken` replayed to `CreateHarness` answers with the harness it first produced,
/// rather than minting a second one — the `@idempotencyToken` the model marks it with.
///
/// The rows differ along what the replayed request carries: the same body, and a body whose
/// other members changed, which the token still short-circuits.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_first_harness() {
    let router = router();

    // `ClientToken`'s @length is 33..256.
    let token = "a".repeat(40);

    let first = send(
        &router,
        "POST",
        "/harnesses",
        Some(&harness_body(
            "idempotentHarness",
            json!({"clientToken": token}),
        )),
    )
    .await;
    assert_eq!(
        first.status, 201,
        "the first create must succeed for this suite to say anything, got {} with {}",
        first.status, first.body
    );
    let created = at(&first.body, "/harness");

    // (case, the members the replay adds)
    let rows: Vec<(&str, Value)> = vec![
        ("the same body", json!({"clientToken": token.clone()})),
        (
            "a body whose other members changed",
            json!({"clientToken": token.clone(), "maxIterations": 9}),
        ),
    ];

    for (case, extra) in rows {
        let replay = send(
            &router,
            "POST",
            "/harnesses",
            Some(&harness_body("idempotentHarness", extra)),
        )
        .await;
        assert_eq!(
            replay.status, 201,
            "case {case:?}: a replayed idempotency token is not an error, got {} with {}",
            replay.status, replay.body
        );
        assert_eq!(
            at(&replay.body, "/harness/harnessId"),
            at(&created, "/harnessId"),
            "case {case:?}: the replay answers with the harness the token first produced, got {}",
            replay.body
        );
    }

    // Without the token, the same name is a conflict rather than a second harness.
    let conflict = send(
        &router,
        "POST",
        "/harnesses",
        Some(&harness_body("idempotentHarness", json!({}))),
    )
    .await;
    assert_eq!(
        (conflict.status, conflict.error_type.as_str()),
        (409, "ConflictException"),
        "case \"the same name without a token\": expected 409 ConflictException, got {} {:?} \
         with {}",
        conflict.status,
        conflict.error_type,
        conflict.body
    );
}

/// `Harness.model` is `@required` on every response while `CreateHarnessRequest.model` is
/// optional, and this service has no default model to run a harness with, so a create that
/// names none is refused with the operation's own modelled `ValidationException` naming
/// the member — never served with an invented model id.
#[tokio::test]
async fn create_harness_refuses_a_request_that_names_no_model() {
    let router = router();

    let answer = send(
        &router,
        "POST",
        "/harnesses",
        Some(&json!({
            "harnessName": "modellessHarness",
            "executionRoleArn": ROLE_ARN,
        })),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "ValidationException"),
        "expected 400 ValidationException for a create that names no model, got {} {:?} with {}",
        answer.status,
        answer.error_type,
        answer.body
    );
    assert_eq!(
        at(&answer.body, "/fieldList/0/name"),
        json!("model"),
        "the modelled ValidationException names the member in its fieldList, got {}",
        answer.body
    );
}

/// One refusal-wording row: the request, the member it puts outside its `@pattern`, and
/// the expression the model declares for that member.
struct PatternRow {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    member: &'static str,
    modelled: &'static str,
}

/// A refusal quotes the member's `@pattern` as the expression a value has to satisfy —
/// WITHOUT the `^` / `$` anchors the model writes to make the match total — and words the
/// clause the way the validating walk does otherwise: the count, the submitted value, the
/// member path and the requirement.
///
/// Rows span the operation, the member, where the member is bound (body, URI label, query
/// string) and the shape of the modelled expression, plus one row whose constraint is NOT
/// a pattern, so the rule is confined to patterns.
#[tokio::test]
async fn a_refused_pattern_is_quoted_without_the_models_anchors() {
    let router = router();

    // Source for every pattern: `smithy_query(query="shape",shape="HarnessName,HarnessId,\
    // HarnessVersion,RoleArn,GatewayName")`.
    let rows: Vec<PatternRow> = vec![
        PatternRow {
            case: "CreateHarness harnessName, a body member",
            method: "POST",
            uri: "/harnesses".to_string(),
            body: Some(harness_body("1leadingDigit", json!({}))),
            member: "harnessName",
            modelled: "^[a-zA-Z][a-zA-Z0-9_]{0,39}$",
        },
        PatternRow {
            case: "CreateHarness executionRoleArn, whose class negation is not an anchor",
            method: "POST",
            uri: "/harnesses".to_string(),
            body: Some(json!({
                "harnessName": "patternProbe",
                "executionRoleArn": "not-an-arn",
                "model": {"bedrockModelConfig": {"modelId": "amazon.nova-pro-v1:0"}},
            })),
            member: "executionRoleArn",
            modelled: "^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$",
        },
        PatternRow {
            case: "GetHarness harnessId, a URI label",
            method: "GET",
            uri: "/harnesses/1notAnId".to_string(),
            body: None,
            member: "harnessId",
            modelled: "^[a-zA-Z][a-zA-Z0-9_]{0,39}-[a-zA-Z0-9]{10}$",
        },
        PatternRow {
            case: "GetHarness harnessVersion, a query parameter",
            method: "GET",
            uri: format!("/harnesses/{ABSENT_HARNESS_ID}?harnessVersion=0"),
            body: None,
            member: "harnessVersion",
            modelled: "^([1-9][0-9]{0,4})$",
        },
        PatternRow {
            // Another resource family of the same service: the rule is the service's, not
            // the harness operations'.
            case: "CreateGateway name, another operation family",
            method: "POST",
            uri: "/gateways/".to_string(),
            body: Some(json!({
                "name": "a name with spaces",
                "roleArn": ROLE_ARN,
                "authorizerType": "AWS_IAM",
            })),
            member: "name",
            modelled: "^([0-9a-zA-Z][-]?){1,48}$",
        },
    ];

    for row in rows {
        let case = row.case;
        let member = row.member;
        let modelled = row.modelled;
        let answer = send(&router, row.method, &row.uri, row.body.as_ref()).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a value outside its @pattern is refused 400 ValidationException, \
             got {} {:?} with {}",
            answer.status,
            answer.error_type,
            answer.body
        );
        let message = at(&answer.body, "/message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected = modelled
            .strip_prefix('^')
            .and_then(|rest| rest.strip_suffix('$'))
            .unwrap_or(modelled);
        assert!(
            message.contains(&format!(
                "at '{member}' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: {expected}"
            )),
            "case {case:?}: expected the clause to quote the unanchored expression \
             {expected:?} at {member:?}, got {message:?}"
        );
        assert!(
            !message.contains(modelled),
            "case {case:?}: the clause must not quote the anchored form {modelled:?}, got \
             {message:?}"
        );
        assert!(
            message.starts_with("1 validation error detected: Value '"),
            "case {case:?}: the envelope and the value echo stay the validating walk's, got \
             {message:?}"
        );
    }

    // A constraint that is NOT a pattern keeps the framework's clause verbatim, so nothing
    // above rewords a requirement this service does not state differently.
    let short_token = send(
        &router,
        "POST",
        "/harnesses",
        Some(&harness_body("lengthProbe", json!({"clientToken": "too-short"}))),
    )
    .await;
    let message = at(&short_token.body, "/message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        message.contains(
            "at 'clientToken' failed to satisfy constraint: Member must have length greater \
             than or equal to 33"
        ),
        "case \"clientToken shorter than its @length\": a non-pattern requirement is the \
         framework's own wording, got {message:?}"
    );
}
