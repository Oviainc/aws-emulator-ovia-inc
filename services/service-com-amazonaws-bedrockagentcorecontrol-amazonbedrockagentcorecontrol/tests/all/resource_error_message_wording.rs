// SPDX-License-Identifier: CC0-1.0

//! How this control plane WORDS the two refusals that are about a resource rather than
//! about the request's members, end to end over HTTP.
//!
//! `bedrock-agentcore-control` is one endpoint in front of several AgentCore services, one
//! per resource family, and the families do not word these refusals alike. Two rules are
//! under test, each asserted over a SET of operations and identifiers rather than over the
//! one request a report happened to cite:
//!
//! 1. A REQUEST FOR A RESOURCE THAT IS NOT THERE is refused with the operation's modelled
//!    `ResourceNotFoundException` (HTTP 404 for every operation below:
//!    `smithy_query(query="operation", operation="…")`), worded in the sentence its
//!    resource family words a missing resource in. The sentence's fixed text never varies
//!    with the asking operation, so a family's read, update and delete of a missing
//!    resource all answer alike; its variable parts are filled from the request and from
//!    this service's own state — never with the operation's verb. The gateway family's
//!    sentence states the failed RETRIEVE whatever was asked for and echoes no identifier
//!    at all; the runtime family's names the agent, the account the lookup ran in
//!    (`engine::emulator_account_id`) and the version asked for, spelling `null` for a
//!    request that named no version; the harness family's names the agent by name; and the
//!    memory family's names the operation that was asking and the id it could not retrieve.
//!    The rows differ along the family, the operation, the identifier and whether a version
//!    was asked for.
//! 2. AN OPERATION ASKED FOR WHILE THE GATEWAY CANNOT SERVE IT is refused with the
//!    operation's modelled `ValidationException` (HTTP 400) stating the operation, the
//!    status it found and the one remedy: "<Operation> operation can't be performed on
//!    gateway when it is in <Status> state. Retry the request when the gateway is in a
//!    valid state." The status is spelled as prose (`Creating`), not as the
//!    screaming-snake token the wire's `GatewayStatus` enum carries. The rows differ along
//!    the operation refused and along which transient status the gateway is in, so neither
//!    the operation nor one named status can be what the sentence is keyed to.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The settle span `src/state.rs` gives a gateway's transient statuses, plus slack, so a
/// row can wait the clock-owed transition out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AgentCoreRole";

/// The gateway family's whole not-found sentence. It carries no variable part: what failed
/// is the retrieve of the gateway, which every gateway operation performs alike.
const GATEWAY_NOT_FOUND: &str = "Failed to retrieve gateway because it doesn't exist. Retry the \
                                 request with a different resource identifier.";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the status, the error code and the `message` the body carries.
struct Answer {
    status: u16,
    code: String,
    message: String,
    body: Value,
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<&Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer {
        status,
        // The error code `restJson1` puts in `__type`.
        code: parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string(),
        message: parsed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        body: parsed,
    }
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The runtime family's not-found sentence for `id` and the version the request asked for,
/// which a request that named none has reported as `null`.
fn agent_not_found(id: &str, version: Option<&str>) -> String {
    format!(
        "Agent with agentId: {id}, accountId: {}, and version: {} not found!",
        ::engine::emulator_account_id(),
        version.unwrap_or("null")
    )
}

/// A container artifact whose URI satisfies `RuntimeContainerUri`'s ECR pattern.
fn container_artifact(tag: &str) -> Value {
    json!({"containerConfiguration": {
        "containerUri": format!("public.ecr.aws/agentcore/demo:{tag}")
    }})
}

/// An `UpdateAgentRuntime` body carrying what the model makes `@required`.
fn runtime_update_body() -> Value {
    json!({"agentRuntimeArtifact": container_artifact("probe"), "roleArn": ROLE_ARN})
}

/// An `UpdateGateway` body for a gateway created under `name`: `name`, `roleArn` and
/// `authorizerType` are all `@required`, and `name` must be the one the gateway carries.
fn gateway_update_body(name: &str) -> Value {
    json!({"name": name, "roleArn": ROLE_ARN, "authorizerType": "AWS_IAM"})
}

/// One refused request, and the whole message its family must answer with.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    message: String,
}

/// RULE 1: each family words a resource it does not hold in its own sentence, the same one
/// whatever operation asked, with its variable parts filled from the request.
#[tokio::test]
async fn a_missing_resource_is_worded_in_its_familys_own_sentence() {
    let router = router();

    // Identifiers inside each label's modelled pattern (`smithy_query(query="operation",
    // operation="GetGateway,GetAgentRuntime,GetHarness,GetMemory")`), two per family so a
    // sentence cannot pass by quoting one recorded id.
    let rows = vec![
        Row {
            case: "GetGateway on an unknown gateway",
            method: "GET",
            uri: "/gateways/missinggateway-abcdefghij/".to_string(),
            body: None,
            message: GATEWAY_NOT_FOUND.to_string(),
        },
        Row {
            case: "UpdateGateway on an unknown gateway: the same sentence, not the verb",
            method: "PUT",
            uri: "/gateways/no-such-gw-0123456789/".to_string(),
            body: Some(gateway_update_body("probegateway")),
            message: GATEWAY_NOT_FOUND.to_string(),
        },
        Row {
            case: "DeleteGateway on an unknown gateway: the same sentence again",
            method: "DELETE",
            uri: "/gateways/goneaway-9876543210/".to_string(),
            body: None,
            message: GATEWAY_NOT_FOUND.to_string(),
        },
        Row {
            case: "GetAgentRuntime on an unknown runtime, no version asked for",
            method: "GET",
            uri: "/runtimes/missingruntime-abcdefghij/".to_string(),
            body: None,
            message: agent_not_found("missingruntime-abcdefghij", None),
        },
        Row {
            case: "GetAgentRuntime on another unknown runtime, a version asked for",
            method: "GET",
            uri: "/runtimes/a_runtime2-0123456789/?version=3".to_string(),
            body: None,
            message: agent_not_found("a_runtime2-0123456789", Some("3")),
        },
        Row {
            // The highest version the wire can carry: `AgentRuntimeVersion` is
            // `^([1-9][0-9]{0,4})$`, so five digits.
            case: "GetAgentRuntime with the highest modelled version",
            method: "GET",
            uri: "/runtimes/missingruntime-abcdefghij/?version=99999".to_string(),
            body: None,
            message: agent_not_found("missingruntime-abcdefghij", Some("99999")),
        },
        Row {
            case: "DeleteAgentRuntime on an unknown runtime, no version asked for",
            method: "DELETE",
            uri: "/runtimes/missingruntime-abcdefghij/".to_string(),
            body: None,
            message: agent_not_found("missingruntime-abcdefghij", None),
        },
        Row {
            case: "DeleteAgentRuntime on an unknown runtime, one version asked for",
            method: "DELETE",
            uri: "/runtimes/a_runtime2-0123456789/?version=2".to_string(),
            body: None,
            message: agent_not_found("a_runtime2-0123456789", Some("2")),
        },
        Row {
            // An update addresses the runtime, not one of its versions, so it asked for
            // no version at all.
            case: "UpdateAgentRuntime on an unknown runtime",
            method: "PUT",
            uri: "/runtimes/missingruntime-abcdefghij/".to_string(),
            body: Some(runtime_update_body()),
            message: agent_not_found("missingruntime-abcdefghij", None),
        },
        Row {
            case: "GetHarness on an unknown harness",
            method: "GET",
            uri: "/harnesses/missingharness-abcdefghij".to_string(),
            body: None,
            message: "Agent with name missingharness-abcdefghij not found.".to_string(),
        },
        Row {
            case: "GetHarness on another unknown harness",
            method: "GET",
            uri: "/harnesses/h_probe2-0123456789".to_string(),
            body: None,
            message: "Agent with name h_probe2-0123456789 not found.".to_string(),
        },
        Row {
            case: "GetMemory on an unknown memory",
            method: "GET",
            uri: "/memories/missingmemory-1234567890/details".to_string(),
            body: None,
            message: "Resource not found during GetMemory: Failed to retrieve memory item for \
                      ID: missingmemory-1234567890"
                .to_string(),
        },
        Row {
            case: "UpdateMemory on another unknown memory: the asking operation is named",
            method: "PUT",
            uri: "/memories/m_probe2-abcdefghij/update".to_string(),
            body: Some(json!({"description": "a change nothing can take"})),
            message: "Resource not found during UpdateMemory: Failed to retrieve memory item \
                      for ID: m_probe2-abcdefghij"
                .to_string(),
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = send(&router, row.method, &row.uri, row.body.as_ref()).await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a resource that is not there is refused 404 \
             ResourceNotFoundException, got {} {:?} ({})",
            answer.status,
            answer.code,
            answer.body
        );
        assert_eq!(
            answer.message, row.message,
            "case {case:?}: expected the family's sentence {:?}, got {:?}",
            row.message, answer.message
        );
    }
}

/// RULE 1, the gateway family's half that no identifier fills: the sentence is the same
/// text for two different unknown identifiers, and neither appears in it.
#[tokio::test]
async fn the_gateway_sentence_echoes_no_identifier() {
    let router = router();

    for (case, identifier) in [
        ("a short identifier", "shortgw-abcdefghij"),
        (
            "a long hyphenated identifier",
            "a-much-longer-gateway-identifier-0123456789",
        ),
    ] {
        let answer = send(&router, "GET", &format!("/gateways/{identifier}/"), None).await;
        assert_eq!(
            answer.message, GATEWAY_NOT_FOUND,
            "case {case:?}: expected {GATEWAY_NOT_FOUND:?}, got {:?}",
            answer.message
        );
        assert!(
            !answer.message.contains(identifier),
            "case {case:?}: the sentence must not echo the identifier {identifier:?}, got {:?}",
            answer.message
        );
    }
}

/// RULE 1's variable parts are read from the request and the store, not from fixed text: a
/// runtime that IS stored, asked for at a version it never published, is reported with its
/// own id and the version the request asked for.
#[tokio::test]
async fn a_version_a_stored_runtime_never_published_is_reported_with_that_version() {
    let router = router();
    let created = send(
        &router,
        "PUT",
        "/runtimes/",
        Some(&json!({
            "agentRuntimeName": "versionProbe",
            "agentRuntimeArtifact": container_artifact("v1"),
            "roleArn": ROLE_ARN,
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: CreateAgentRuntime must succeed for this test to say anything, got {} ({})",
        created.status, created.body
    );
    let runtime_id = text(&created.body, "agentRuntimeId");

    for (case, version) in [
        ("the next version, which nothing published", "2"),
        ("a far later version", "412"),
    ] {
        let answer = send(
            &router,
            "GET",
            &format!("/runtimes/{runtime_id}/?version={version}"),
            None,
        )
        .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} {:?} ({})",
            answer.status,
            answer.code,
            answer.body
        );
        let expected = agent_not_found(&runtime_id, Some(version));
        assert_eq!(
            answer.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message
        );
    }
}

/// RULE 2: a gateway operation asked for while the gateway is in a status that cannot serve
/// it is refused with the operation it was, the status spelled as prose, and the remedy.
#[tokio::test]
async fn an_operation_a_gateways_status_cannot_serve_is_refused_in_one_sentence() {
    // (case, the operation under test, the request that moves the gateway out of CREATING
    // — none at all leaves it CREATING —, the status it then reports, that status as the
    // sentence spells it)
    let rows: [(&str, &str, Option<&str>, &str, &str); 5] = [
        (
            "DeleteGateway while the gateway is still being created",
            "DeleteGateway",
            None,
            "CREATING",
            "Creating",
        ),
        (
            "UpdateGateway while the gateway is still being created",
            "UpdateGateway",
            None,
            "CREATING",
            "Creating",
        ),
        (
            "DeleteGateway while an update is in flight",
            "DeleteGateway",
            Some("PUT"),
            "UPDATING",
            "Updating",
        ),
        (
            "UpdateGateway while an update is in flight",
            "UpdateGateway",
            Some("PUT"),
            "UPDATING",
            "Updating",
        ),
        (
            "UpdateGateway once the deletion was accepted",
            "UpdateGateway",
            Some("DELETE"),
            "DELETING",
            "Deleting",
        ),
    ];

    for (case, operation, setup, status_name, status_prose) in rows {
        let router = router();
        let name = "statusprobe";
        let created = send(
            &router,
            "POST",
            "/gateways/",
            Some(&json!({"name": name, "roleArn": ROLE_ARN, "authorizerType": "AWS_IAM"})),
        )
        .await;
        assert_eq!(
            (created.status, text(&created.body, "status").as_str()),
            (202, "CREATING"),
            "case {case:?}: the setup create must leave the gateway CREATING, got {} {} ({})",
            created.status,
            text(&created.body, "status"),
            created.body
        );
        let gateway_id = text(&created.body, "gatewayId");

        // A row that wants another status waits the create out first, so the status under
        // test is the one its own setup call left behind.
        if let Some(method) = setup {
            tokio::time::sleep(PAST_SETTLE).await;
            let body = (method == "PUT").then(|| gateway_update_body(name));
            let moved = send(
                &router,
                method,
                &format!("/gateways/{gateway_id}/"),
                body.as_ref(),
            )
            .await;
            assert_eq!(
                (moved.status, text(&moved.body, "status").as_str()),
                (202, status_name),
                "case {case:?}: the setup call must leave the gateway in {status_name}, got {} {} \
                 ({})",
                moved.status,
                text(&moved.body, "status"),
                moved.body
            );
        }

        let (method, body) = match operation {
            "DeleteGateway" => ("DELETE", None),
            _ => ("PUT", Some(gateway_update_body(name))),
        };
        let refused = send(
            &router,
            method,
            &format!("/gateways/{gateway_id}/"),
            body.as_ref(),
        )
        .await;
        assert_eq!(
            (refused.status, refused.code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {} {:?} ({})",
            refused.status,
            refused.code,
            refused.body
        );
        let expected = format!(
            "{operation} operation can't be performed on gateway when it is in {status_prose} \
             state. Retry the request when the gateway is in a valid state."
        );
        assert_eq!(
            refused.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.message
        );
    }
}
