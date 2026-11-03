// SPDX-License-Identifier: CC0-1.0

//! The agent-runtime-endpoint surface of this control plane, end to end over HTTP.
//!
//! One rule, read off the Smithy model rather than off any one recorded request: an
//! agent runtime endpoint is a named, stable reference PINNED to one version OF an
//! agent runtime, persisted on that runtime. So
//!
//! * a create persists the name, the description, the tags and the runtime version the
//!   endpoint pins — the one the request names, or the version the runtime holds now
//!   when it names none — and reports the endpoint in the `CREATING` status the clock
//!   owes a transition for;
//! * every later read reports that same record, with no `liveVersion` until the
//!   clock-owed transition is due, and then `READY` serving the version it pins;
//! * the pin does not move: the runtime publishing further versions leaves the
//!   endpoint's `targetVersion` where it was;
//! * the endpoint is addressed ONLY through its runtime: a runtime this account does
//!   not hold, an `agentRuntimeVersion` that runtime never published, an endpoint name
//!   the runtime does not hold and another runtime's endpoint are each the modelled
//!   `404 ResourceNotFoundException`; and one name per runtime, so a repeat is the
//!   modelled `409 ConflictException`.
//!
//! Sources: `smithy_query(query="operation",operation="CreateAgentRuntimeEndpoint,\
//! GetAgentRuntimeEndpoint,CreateAgentRuntime,UpdateAgentRuntime")` for the routes, the
//! success statuses, the members and the errors, and
//! `smithy_query(query="shape",shape="AgentRuntimeEndpointStatus,EndpointName,\
//! AgentRuntimeVersion,AgentRuntimeEndpointArn,AgentRuntimeEndpointId")` for the status
//! values, the name and version grammars and the ARN and id forms asserted below.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AgentRole";

/// The `AgentRuntimeVersion` a created runtime carries: the grammar
/// `^([1-9][0-9]{0,4})$` starts at 1, and only an update publishes another.
const FIRST_VERSION: &str = "1";

/// Every `AgentRuntimeEndpointStatus` value
/// (`smithy_query shape=AgentRuntimeEndpointStatus`).
const ENDPOINT_STATUS_VALUES: [&str; 7] = [
    "CREATING",
    "CREATE_FAILED",
    "UPDATING",
    "UPDATE_FAILED",
    "READY",
    "DELETING",
    "DELETE_FAILED",
];

/// The settle spans `src/state.rs` gives the runtime `CREATING` -> `READY` transition
/// and then the endpoint's own, plus slack: an endpoint fronts a runtime version, so it
/// is ready one span after the version it serves is.
const PAST_ENDPOINT_SETTLE: Duration = Duration::from_millis(3_900);

/// An `agentRuntimeId` inside `AgentRuntimeId`'s pattern
/// `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$` that no row ever creates.
const ABSENT_RUNTIME_ID: &str = "noSuchRuntime-abcdefghij";

/// The highest `agentRuntimeVersion` the wire can carry: `AgentRuntimeVersion` is
/// `^([1-9][0-9]{0,4})$`, so five digits — and no row publishes anywhere near it.
const UNPUBLISHED_HIGHEST_VERSION: &str = "99999";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on, and what
/// the handlers read the ARN region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the status and the parsed body.
struct Answer {
    status: u16,
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
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

/// The member at `pointer`, or `Value::Null` when the answer carries none.
fn at(body: &Value, pointer: &str) -> Value {
    body.pointer(pointer).cloned().unwrap_or(Value::Null)
}

fn text(body: &Value, pointer: &str) -> String {
    at(body, pointer).as_str().unwrap_or_default().to_string()
}

/// The error code `restJson1` puts in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A container artifact whose URI satisfies `RuntimeContainerUri`'s ECR pattern.
fn container_artifact(tag: &str) -> Value {
    json!({"containerConfiguration": {
        "containerUri": format!("public.ecr.aws/agentcore/demo:{tag}")
    }})
}

/// Create one agent runtime and return its `agentRuntimeId`.
async fn create_runtime(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "agentRuntimeName": name,
        "agentRuntimeArtifact": container_artifact("v1"),
        "roleArn": ROLE_ARN,
    });
    let answer = send(router, "PUT", "/runtimes/", Some(&body)).await;
    assert_eq!(
        answer.status, 202,
        "creating the runtime {name:?} must succeed for this suite to say anything, got {} with {}",
        answer.status, answer.body
    );
    text(&answer.body, "/agentRuntimeId")
}

/// Publish one more version of `runtime_id` and return the version it published.
async fn update_runtime(router: &MultiServiceRouter, runtime_id: &str, tag: &str) -> String {
    let body = json!({
        "agentRuntimeArtifact": container_artifact(tag),
        "roleArn": ROLE_ARN,
    });
    let answer = send(router, "PUT", &format!("/runtimes/{runtime_id}/"), Some(&body)).await;
    assert_eq!(
        answer.status, 202,
        "publishing another version must succeed for this suite to say anything, got {} with {}",
        answer.status, answer.body
    );
    text(&answer.body, "/agentRuntimeVersion")
}

/// `PUT /runtimes/{agentRuntimeId}/runtime-endpoints/` — the route the model binds
/// `CreateAgentRuntimeEndpoint` at.
async fn create_endpoint(router: &MultiServiceRouter, runtime_id: &str, body: &Value) -> Answer {
    send(
        router,
        "PUT",
        &format!("/runtimes/{runtime_id}/runtime-endpoints/"),
        Some(body),
    )
    .await
}

/// `GET /runtimes/{agentRuntimeId}/runtime-endpoints/{endpointName}/` — the route the
/// model binds `GetAgentRuntimeEndpoint` at.
async fn get_endpoint(router: &MultiServiceRouter, runtime_id: &str, name: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/runtimes/{runtime_id}/runtime-endpoints/{name}/"),
        None,
    )
    .await
}

/// `AgentRuntimeEndpointId`'s pattern, `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$`
/// (`smithy_query shape=AgentRuntimeEndpointId`), checked by hand because this crate
/// carries no regex engine.
fn is_modelled_endpoint_id(id: &str) -> bool {
    let Some((head, suffix)) = id.rsplit_once('-') else {
        return false;
    };
    suffix.len() == 10
        && suffix.chars().all(|c| c.is_ascii_alphanumeric())
        && !head.is_empty()
        && head.len() <= 100
        && head.starts_with(|c: char| c.is_ascii_alphabetic())
        && head.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// One create row: the runtime it is created on and what the request carries beyond the
/// required `name`.
struct Created {
    case: &'static str,
    runtime: &'static str,
    endpoint_name: String,
    extra: Value,
}

/// A created endpoint reports what the request persisted, and `GetAgentRuntimeEndpoint`
/// reports the same record member for member.
///
/// The rows differ along the shape of the name (the shortest the pattern admits, digits
/// and underscores, the 48-character bound itself), whether `agentRuntimeVersion` is
/// named or left to the version the runtime holds, the length of the description (the
/// shortest and the longest the modelled 1..256 admits) and how many tags the request
/// carries.
#[tokio::test]
async fn a_created_endpoint_reads_back_member_for_member() {
    let router = router();

    // `EndpointName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`: a leading letter plus up to 47
    // more characters. `AgentEndpointDescription` is len 1..256.
    let rows: Vec<Created> = vec![
        Created {
            case: "the shortest name the pattern admits, nothing else",
            runtime: "endpoint_runtime_a",
            endpoint_name: "e".to_string(),
            extra: json!({}),
        },
        Created {
            case: "digits and underscores, with the version named explicitly",
            runtime: "endpoint_runtime_b",
            endpoint_name: "live_v2_endpoint_42".to_string(),
            extra: json!({"agentRuntimeVersion": FIRST_VERSION}),
        },
        Created {
            case: "the 48-character bound itself, with a one-character description",
            runtime: "endpoint_runtime_c",
            endpoint_name: format!("e{}", "a".repeat(47)),
            extra: json!({"description": "x"}),
        },
        Created {
            case: "a description at the 256-character bound and three tags",
            runtime: "endpoint_runtime_d",
            endpoint_name: "prod".to_string(),
            extra: json!({
                "description": "d".repeat(256),
                "tags": {"team": "agents", "env": "prod", "owner": "platform"},
            }),
        },
        Created {
            case: "no description and an empty tag map",
            runtime: "endpoint_runtime_e",
            endpoint_name: "canary".to_string(),
            extra: json!({"tags": {}}),
        },
    ];

    for row in rows {
        let case = row.case;
        let runtime_id = create_runtime(&router, row.runtime).await;
        let mut body = row.extra.clone();
        body["name"] = json!(row.endpoint_name);

        let answer = create_endpoint(&router, &runtime_id, &body).await;
        assert_eq!(
            answer.status, 202,
            "case {case:?}: creating an endpoint on a stored runtime is HTTP 202, got {} with {}",
            answer.status, answer.body
        );
        let created = answer.body;

        assert_eq!(
            text(&created, "/agentRuntimeId"),
            runtime_id,
            "case {case:?}: the endpoint reports the runtime it belongs to, got {created}"
        );
        assert_eq!(
            text(&created, "/endpointName"),
            row.endpoint_name,
            "case {case:?}: the endpoint reports the name it was created with, got {created}"
        );
        let runtime_arn = format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:runtime/{runtime_id}");
        assert_eq!(
            text(&created, "/agentRuntimeArn"),
            runtime_arn,
            "case {case:?}: expected the runtime ARN {runtime_arn:?}, got {created}"
        );
        let expected_arn = format!("{runtime_arn}/runtime-endpoint/{}", row.endpoint_name);
        assert_eq!(
            text(&created, "/agentRuntimeEndpointArn"),
            expected_arn,
            "case {case:?}: expected the endpoint ARN {expected_arn:?}, got {created}"
        );
        assert_eq!(
            text(&created, "/status"),
            "CREATING",
            "case {case:?}: an endpoint is created, not ready — the transition to READY is the \
             clock's, got {created}"
        );
        assert!(
            ENDPOINT_STATUS_VALUES.contains(&text(&created, "/status").as_str()),
            "case {case:?}: the status is an AgentRuntimeEndpointStatus value, got {created}"
        );
        assert_eq!(
            text(&created, "/targetVersion"),
            FIRST_VERSION,
            "case {case:?}: the endpoint pins the version its runtime holds, got {created}"
        );

        let read = get_endpoint(&router, &runtime_id, &row.endpoint_name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the endpoint that was just created is readable, got {} with {}",
            read.status, read.body
        );
        let got = read.body;
        assert_eq!(
            text(&got, "/name"),
            row.endpoint_name,
            "case {case:?}: the read reports the name the create persisted, got {got}"
        );
        assert!(
            is_modelled_endpoint_id(&text(&got, "/id")),
            "case {case:?}: the endpoint carries an AgentRuntimeEndpointId, got {got}"
        );
        assert_eq!(
            text(&got, "/agentRuntimeEndpointArn"),
            text(&created, "/agentRuntimeEndpointArn"),
            "case {case:?}: the read reports the ARN the create reported, got {got}"
        );
        assert_eq!(
            text(&got, "/agentRuntimeArn"),
            runtime_arn,
            "case {case:?}: the read reports the runtime's ARN, got {got}"
        );
        assert_eq!(
            at(&got, "/description"),
            at(&row.extra, "/description"),
            "case {case:?}: the endpoint reports the description it was created with, got {got}"
        );
        assert_eq!(
            text(&got, "/targetVersion"),
            FIRST_VERSION,
            "case {case:?}: the read reports the version the create pinned, got {got}"
        );
        assert_eq!(
            at(&got, "/liveVersion"),
            Value::Null,
            "case {case:?}: nothing is served behind the name until the endpoint is READY, got \
             {got}"
        );
        assert_eq!(
            text(&got, "/status"),
            "CREATING",
            "case {case:?}: the read reports the status the create reported, got {got}"
        );
        assert_eq!(
            at(&got, "/failureReason"),
            Value::Null,
            "case {case:?}: an endpoint the clock drives to READY has no failure, got {got}"
        );
        assert_eq!(
            at(&got, "/createdAt"),
            at(&created, "/createdAt"),
            "case {case:?}: the read reports the instant the create reported, got {got}"
        );
        assert_eq!(
            at(&got, "/lastUpdatedAt"),
            at(&got, "/createdAt"),
            "case {case:?}: nothing has updated the endpoint since it was created, got {got}"
        );
    }
}

/// The clock-owed transition: an endpoint is `CREATING` with no `liveVersion` until its
/// settle span passes, then `READY` and serving the version it pins.
///
/// An endpoint fronts a runtime version, so it settles one span after that version
/// does; the row reads the same endpoint before and after that instant, and twice
/// afterwards, so the settled answer is stable rather than a one-off.
#[tokio::test]
async fn an_endpoint_becomes_ready_and_serves_the_version_it_pins() {
    let router = router();
    let runtime_id = create_runtime(&router, "settling_endpoint_runtime").await;
    let created = create_endpoint(&router, &runtime_id, &json!({"name": "live"})).await;
    assert_eq!(
        created.status, 202,
        "the create must succeed for this suite to say anything, got {} with {}",
        created.status, created.body
    );

    let fresh = get_endpoint(&router, &runtime_id, "live").await;
    assert_eq!(
        (
            text(&fresh.body, "/status"),
            at(&fresh.body, "/liveVersion")
        ),
        ("CREATING".to_string(), Value::Null),
        "case \"before the settle span\": the endpoint is still CREATING and serves nothing, got \
         {}",
        fresh.body
    );

    tokio::time::sleep(PAST_ENDPOINT_SETTLE).await;

    for case in ["the first read after the settle span", "the read after that"] {
        let settled = get_endpoint(&router, &runtime_id, "live").await;
        assert_eq!(
            (
                text(&settled.body, "/status"),
                text(&settled.body, "/liveVersion")
            ),
            ("READY".to_string(), FIRST_VERSION.to_string()),
            "case {case:?}: once the transition is due the endpoint is READY and serves the \
             version it pins, got {}",
            settled.body
        );
        assert_eq!(
            text(&settled.body, "/targetVersion"),
            text(&settled.body, "/liveVersion"),
            "case {case:?}: a settled endpoint serves the version it points to, got {}",
            settled.body
        );
    }
}

/// The pin does not move: the runtime publishing further versions leaves the endpoint
/// pointing at the version it was created against.
///
/// The rows differ along how the pin was established (named explicitly, or left to the
/// version the runtime held at create time) and how many further versions the runtime
/// publishes afterwards.
#[tokio::test]
async fn a_pinned_endpoint_keeps_its_version_across_runtime_updates() {
    // (case, the `agentRuntimeVersion` the create names, how many updates follow)
    let rows: Vec<(&str, Option<&str>, u32)> = vec![
        ("the version named explicitly, one later update", Some(FIRST_VERSION), 1),
        ("the version left implicit, one later update", None, 1),
        ("the version named explicitly, three later updates", Some(FIRST_VERSION), 3),
        ("the version left implicit, three later updates", None, 3),
    ];

    for (case, named_version, updates) in rows {
        let router = router();
        let runtime_id = create_runtime(&router, "pinned_endpoint_runtime").await;
        let mut body = json!({"name": "pinned"});
        if let Some(version) = named_version {
            body["agentRuntimeVersion"] = json!(version);
        }
        let created = create_endpoint(&router, &runtime_id, &body).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the create must succeed for this suite to say anything, got {} with {}",
            created.status, created.body
        );
        assert_eq!(
            text(&created.body, "/targetVersion"),
            FIRST_VERSION,
            "case {case:?}: the endpoint pins the version the runtime held at create time, got {}",
            created.body
        );

        let mut latest = FIRST_VERSION.to_string();
        for revision in 0..updates {
            latest = update_runtime(&router, &runtime_id, &format!("v{}", revision + 2)).await;
        }
        assert_ne!(
            latest, FIRST_VERSION,
            "case {case:?}: the updates must publish a version past the pinned one for this row \
             to say anything, got {latest}"
        );

        let read = get_endpoint(&router, &runtime_id, "pinned").await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the endpoint is still readable after the updates, got {} with {}",
            read.status, read.body
        );
        assert_eq!(
            text(&read.body, "/targetVersion"),
            FIRST_VERSION,
            "case {case:?}: the runtime publishing version {latest} does not move an endpoint \
             pinned to {FIRST_VERSION}, got {}",
            read.body
        );
    }
}

/// One refusal row: the request that is refused and the answer it must get.
struct Refusal {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
    message: String,
}

/// An endpoint is reached only THROUGH the runtime that holds it, and one runtime holds
/// one endpoint per name.
///
/// The rows differ along what the request names that no stored endpoint answers to: a
/// runtime id nothing created, a version the runtime never published (the one past its
/// latest and the highest the grammar admits), an endpoint name its runtime does not
/// hold, another runtime's endpoint name, and a name the runtime already holds an
/// endpoint under.
#[tokio::test]
async fn an_endpoint_is_addressed_only_through_its_runtime() {
    let router = router();
    let owner = create_runtime(&router, "owner_runtime").await;
    let other = create_runtime(&router, "other_runtime").await;
    let created = create_endpoint(&router, &owner, &json!({"name": "onlyHere"})).await;
    assert_eq!(
        created.status, 202,
        "the create must succeed for this suite to say anything, got {} with {}",
        created.status, created.body
    );

    let rows: Vec<Refusal> = vec![
        Refusal {
            case: "creating an endpoint on a runtime nothing created",
            method: "PUT",
            uri: format!("/runtimes/{ABSENT_RUNTIME_ID}/runtime-endpoints/"),
            body: Some(json!({"name": "orphan"})),
            status: 404,
            code: "ResourceNotFoundException",
            message: format!("Agent with agentId: {ABSENT_RUNTIME_ID} not found"),
        },
        Refusal {
            case: "pinning the version just past the runtime's latest",
            method: "PUT",
            uri: format!("/runtimes/{owner}/runtime-endpoints/"),
            body: Some(json!({"name": "ahead", "agentRuntimeVersion": "2"})),
            status: 404,
            code: "ResourceNotFoundException",
            message: format!("Agent with agentId: {owner} not found"),
        },
        Refusal {
            case: "pinning the highest version the grammar admits",
            method: "PUT",
            uri: format!("/runtimes/{owner}/runtime-endpoints/"),
            body: Some(json!({
                "name": "highest",
                "agentRuntimeVersion": UNPUBLISHED_HIGHEST_VERSION,
            })),
            status: 404,
            code: "ResourceNotFoundException",
            message: format!("Agent with agentId: {owner} not found"),
        },
        Refusal {
            case: "a second endpoint under a name the runtime already holds",
            method: "PUT",
            uri: format!("/runtimes/{owner}/runtime-endpoints/"),
            body: Some(json!({"name": "onlyHere"})),
            status: 409,
            code: "ConflictException",
            message: "An endpoint with the specified name already exists. Check the request and \
                      try again."
                .to_string(),
        },
        Refusal {
            case: "reading an endpoint name its runtime does not hold",
            method: "GET",
            uri: format!("/runtimes/{owner}/runtime-endpoints/notThere/"),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
            message: "Agent endpoint with name notThere not found.".to_string(),
        },
        Refusal {
            case: "reading another runtime's endpoint",
            method: "GET",
            uri: format!("/runtimes/{other}/runtime-endpoints/onlyHere/"),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
            message: "Agent endpoint with name onlyHere not found.".to_string(),
        },
        Refusal {
            case: "reading an endpoint of a runtime nothing created",
            method: "GET",
            uri: format!("/runtimes/{ABSENT_RUNTIME_ID}/runtime-endpoints/onlyHere/"),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
            message: format!("Agent with agentId: {ABSENT_RUNTIME_ID} not found"),
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = send(&router, row.method, &row.uri, row.body.as_ref()).await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected HTTP {}, got {} with {}",
            row.status, answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer.body),
            row.code,
            "case {case:?}: expected {}, got {}",
            row.code,
            answer.body
        );
        assert_eq!(
            text(&answer.body, "/message"),
            row.message,
            "case {case:?}: expected the message {:?}, got {}",
            row.message,
            answer.body
        );
    }

    // The endpoint the refusals were aimed at is untouched: nothing above created,
    // renamed or moved it.
    let read = get_endpoint(&router, &owner, "onlyHere").await;
    assert_eq!(
        (read.status, text(&read.body, "/targetVersion")),
        (200, FIRST_VERSION.to_string()),
        "the refused requests left the stored endpoint exactly as it was, got {} with {}",
        read.status,
        read.body
    );
}

/// A replayed `clientToken` answers with the endpoint that token first produced rather
/// than adding a second one, and a name is taken only on the runtime that holds it.
///
/// The rows differ along the token and the runtime: the same token replayed, a
/// different token under the same name on the same runtime (a conflict, because the
/// name is taken there), the same name on another runtime under its own token, and a
/// create carrying no token at all.
#[tokio::test]
async fn a_replayed_client_token_returns_the_first_endpoint() {
    let router = router();
    let first_runtime = create_runtime(&router, "idempotent_endpoint_runtime").await;
    let second_runtime = create_runtime(&router, "other_idempotent_runtime").await;
    // `ClientToken` is len 33..256, pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`.
    let token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let other_token = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let third_token = "cccccccccccccccccccccccccccccccccc";
    let request = json!({"name": "shared", "clientToken": token});

    let first = create_endpoint(&router, &first_runtime, &request).await;
    assert_eq!(
        first.status, 202,
        "the first create must succeed for this suite to say anything, got {} with {}",
        first.status, first.body
    );
    let replay = create_endpoint(&router, &first_runtime, &request).await;
    assert_eq!(
        (replay.status, replay.body.clone()),
        (202, first.body.clone()),
        "case \"the same token replayed\": the replay answers with the first result, got {} with {}",
        replay.status,
        replay.body
    );

    let repeated_name = create_endpoint(
        &router,
        &first_runtime,
        &json!({"name": "shared", "clientToken": other_token}),
    )
    .await;
    assert_eq!(
        repeated_name.status, 409,
        "case \"a different token under the same name\": the name is taken, got {} with {}",
        repeated_name.status, repeated_name.body
    );

    let elsewhere = create_endpoint(
        &router,
        &second_runtime,
        &json!({"name": "shared", "clientToken": third_token}),
    )
    .await;
    assert_eq!(
        elsewhere.status, 202,
        "case \"the same name on another runtime\": a name is taken only on the runtime that \
         holds it, got {} with {}",
        elsewhere.status, elsewhere.body
    );
    assert_ne!(
        text(&elsewhere.body, "/agentRuntimeEndpointArn"),
        text(&first.body, "/agentRuntimeEndpointArn"),
        "case \"the same name on another runtime\": the endpoint belongs to the runtime it was \
         created on, got {}",
        elsewhere.body
    );

    let untokened = create_endpoint(&router, &second_runtime, &json!({"name": "untokened"})).await;
    assert_eq!(
        untokened.status, 202,
        "case \"no token at all\": the token is optional, got {} with {}",
        untokened.status, untokened.body
    );

    // One endpoint per name, and the replay added none: each runtime holds exactly the
    // endpoints its own creates persisted, each with its own id.
    let first_read = get_endpoint(&router, &first_runtime, "shared").await;
    let second_read = get_endpoint(&router, &second_runtime, "shared").await;
    assert_eq!(
        (first_read.status, second_read.status),
        (200, 200),
        "both endpoints are readable, got {} and {}",
        first_read.body,
        second_read.body
    );
    assert_ne!(
        text(&first_read.body, "/id"),
        text(&second_read.body, "/id"),
        "two endpoints of two runtimes are two resources with two ids, got {} and {}",
        first_read.body,
        second_read.body
    );
}
