// SPDX-License-Identifier: CC0-1.0

//! The harness-endpoint surface of this control plane, end to end over HTTP.
//!
//! One rule, read off the Smithy model rather than off any one recorded request: an
//! endpoint is a named, stable reference to one version OF a harness, persisted on that
//! harness. So
//!
//! * a create persists the name, the description, the tags and the harness version the
//!   endpoint points to, and reports the endpoint in the `CREATING` status the clock owes a
//!   transition for — with no `liveVersion`, because nothing is being served behind the name
//!   until the endpoint finishes transitioning onto its target version;
//! * every later read reports that same record, and once the clock-owed transition is due
//!   the endpoint is `READY` and its `liveVersion` is the version it points to;
//! * the endpoint is addressed ONLY through its harness: a harness this account does not
//!   hold, an endpoint that harness does not hold, and another harness's endpoint are each
//!   the modelled `404 ResourceNotFoundException`; one name per harness, so a repeat is the
//!   modelled `409 ConflictException`; and a `targetVersion` the harness does not hold names
//!   no stored version, which is as absent as an unknown harness;
//! * the harness is not destroyed while it still holds endpoints: `DeleteHarness` is the
//!   modelled `409 ConflictException` until every endpoint is deleted, and the identical
//!   call then succeeds.
//!
//! Sources: `smithy_query(query="operation",operation="CreateHarnessEndpoint,\
//! GetHarnessEndpoint,DeleteHarnessEndpoint,DeleteHarness")` for the routes, the success
//! statuses, the members and the errors, and
//! `smithy_query(query="shape",shape="HarnessEndpoint,HarnessEndpointStatus,\
//! HarnessEndpointName,HarnessEndpointArn")` for the endpoint's members, its ARN form and
//! the status values asserted below.

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
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/HarnessRole";

/// The `HarnessVersion` a created harness carries: the grammar `^([1-9][0-9]{0,4})$` starts
/// at 1, and only an update publishes another.
const FIRST_VERSION: &str = "1";

/// Every `HarnessEndpointStatus` value (`smithy_query shape=HarnessEndpointStatus`).
const ENDPOINT_STATUS_VALUES: [&str; 7] = [
    "CREATING",
    "CREATE_FAILED",
    "UPDATING",
    "UPDATE_FAILED",
    "READY",
    "DELETING",
    "DELETE_FAILED",
];

/// The settle spans `src/state.rs` gives the harness `CREATING` -> `READY` transition and
/// then the endpoint's own, plus slack: an endpoint fronts its harness, so it is ready one
/// span after the harness it serves is.
const PAST_ENDPOINT_SETTLE: Duration = Duration::from_millis(2_900);

/// A `harnessId` inside `HarnessId`'s pattern
/// `^[a-zA-Z][a-zA-Z0-9_]{0,39}-[a-zA-Z0-9]{10}$` that no row ever creates.
const ABSENT_HARNESS_ID: &str = "noSuchHarness-abcdefghij";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on, and what the
/// handlers read the ARN region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the status, the `x-amzn-errortype` header and the parsed body.
struct Answer {
    status: u16,
    error_type: String,
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
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Answer {
        status: response.status().as_u16(),
        error_type,
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

/// A `CreateHarness` body for `name`: the two `@required` members plus the `model` the
/// harness runs with.
fn harness_body(name: &str) -> Value {
    json!({
        "harnessName": name,
        "executionRoleArn": ROLE_ARN,
        "model": {"bedrockModelConfig": {"modelId": "amazon.nova-pro-v1:0"}},
    })
}

/// Create one harness and return its `harnessId`.
async fn create_harness(router: &MultiServiceRouter, name: &str) -> String {
    let answer = send(router, "POST", "/harnesses", Some(&harness_body(name))).await;
    assert_eq!(
        answer.status, 201,
        "creating the harness {name:?} must succeed for this suite to say anything, got {} with {}",
        answer.status, answer.body
    );
    text(&answer.body, "/harness/harnessId")
}

/// `POST /harnesses/{harnessId}/endpoints` — the route the model binds
/// `CreateHarnessEndpoint` at.
async fn create_endpoint(router: &MultiServiceRouter, harness_id: &str, body: &Value) -> Answer {
    send(
        router,
        "POST",
        &format!("/harnesses/{harness_id}/endpoints"),
        Some(body),
    )
    .await
}

/// `GET /harnesses/{harnessId}/endpoints/{endpointName}` — the route the model binds
/// `GetHarnessEndpoint` at.
async fn get_endpoint(router: &MultiServiceRouter, harness_id: &str, name: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/harnesses/{harness_id}/endpoints/{name}"),
        None,
    )
    .await
}

/// `DELETE /harnesses/{harnessId}/endpoints/{endpointName}`.
async fn delete_endpoint(router: &MultiServiceRouter, harness_id: &str, name: &str) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/harnesses/{harness_id}/endpoints/{name}"),
        None,
    )
    .await
}

/// `DELETE /harnesses/{harnessId}`.
async fn delete_harness(router: &MultiServiceRouter, harness_id: &str) -> Answer {
    send(router, "DELETE", &format!("/harnesses/{harness_id}"), None).await
}

/// One create row: what the request carries beyond the required `endpointName`.
struct Created {
    case: &'static str,
    harness: &'static str,
    endpoint_name: String,
    extra: Value,
}

/// A created endpoint reports what the request persisted, and `GetHarnessEndpoint` reports
/// the same record member for member.
///
/// The rows differ along the shape of the name (the shortest the pattern admits, digits and
/// underscores, the 48-character bound itself), whether `targetVersion` is named or left to
/// the version the harness holds, the length of the description (the shortest and the
/// longest the modelled 1..256 admits) and how many tags the request carries.
#[tokio::test]
async fn a_created_endpoint_reads_back_member_for_member() {
    let router = router();

    // `HarnessEndpointName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`: a leading letter plus up to
    // 47 more characters. `HarnessEndpointDescription` is len 1..256.
    let rows: Vec<Created> = vec![
        Created {
            case: "the shortest name the pattern admits, nothing else",
            harness: "endpointHarnessA",
            endpoint_name: "e".to_string(),
            extra: json!({}),
        },
        Created {
            case: "digits and underscores, with the version named explicitly",
            harness: "endpointHarnessB",
            endpoint_name: "live_v2_endpoint_42".to_string(),
            extra: json!({"targetVersion": FIRST_VERSION}),
        },
        Created {
            case: "the 48-character bound itself, with a one-character description",
            harness: "endpointHarnessC",
            endpoint_name: format!("e{}", "a".repeat(47)),
            extra: json!({"description": "x"}),
        },
        Created {
            case: "a description at the 256-character bound and three tags",
            harness: "endpointHarnessD",
            endpoint_name: "prod".to_string(),
            extra: json!({
                "description": "d".repeat(256),
                "tags": {"team": "agents", "env": "prod", "owner": "platform"},
            }),
        },
        Created {
            case: "no description and an empty tag map",
            harness: "endpointHarnessE",
            endpoint_name: "canary".to_string(),
            extra: json!({"tags": {}}),
        },
    ];

    for row in rows {
        let case = row.case;
        let harness_id = create_harness(&router, row.harness).await;
        let mut body = row.extra.clone();
        body["endpointName"] = json!(row.endpoint_name);

        let answer = create_endpoint(&router, &harness_id, &body).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: creating an endpoint on a stored harness is HTTP 201, got {} with {}",
            answer.status, answer.body
        );
        let created = at(&answer.body, "/endpoint");

        assert_eq!(
            text(&created, "/harnessId"),
            harness_id,
            "case {case:?}: the endpoint reports the harness it belongs to, got {created}"
        );
        assert_eq!(
            text(&created, "/harnessName"),
            row.harness,
            "case {case:?}: the endpoint reports its harness's name, got {created}"
        );
        assert_eq!(
            text(&created, "/endpointName"),
            row.endpoint_name,
            "case {case:?}: the endpoint reports the name it was created with, got {created}"
        );
        let expected_arn = format!(
            "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:harness/{harness_id}/harness-endpoint/{}",
            row.endpoint_name
        );
        assert_eq!(
            text(&created, "/arn"),
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
            "case {case:?}: the status is a HarnessEndpointStatus value, got {created}"
        );
        assert_eq!(
            text(&created, "/targetVersion"),
            FIRST_VERSION,
            "case {case:?}: the endpoint points at the version its harness holds, got {created}"
        );
        assert_eq!(
            at(&created, "/liveVersion"),
            Value::Null,
            "case {case:?}: nothing is served behind the name until the endpoint is READY, got \
             {created}"
        );
        assert_eq!(
            at(&created, "/description"),
            at(&row.extra, "/description"),
            "case {case:?}: the endpoint reports the description it was created with, got {created}"
        );
        assert_eq!(
            at(&created, "/failureReason"),
            Value::Null,
            "case {case:?}: an endpoint the clock drives to READY has no failure, got {created}"
        );
        assert_eq!(
            at(&created, "/createdAt"),
            at(&created, "/updatedAt"),
            "case {case:?}: nothing has updated the endpoint since it was created, got {created}"
        );

        let read = get_endpoint(&router, &harness_id, &row.endpoint_name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the endpoint that was just created is readable, got {} with {}",
            read.status, read.body
        );
        assert_eq!(
            at(&read.body, "/endpoint"),
            created,
            "case {case:?}: GetHarnessEndpoint reports what CreateHarnessEndpoint persisted, got \
             {} for {created}",
            at(&read.body, "/endpoint")
        );
    }
}

/// The clock-owed transition: an endpoint is `CREATING` with no `liveVersion` until its
/// settle span passes, then `READY` and serving the version it points to.
///
/// An endpoint fronts its harness, so it settles one span after the harness does; the row
/// reads the same endpoint before and after that instant, and twice afterwards, so the
/// settled answer is stable rather than a one-off.
#[tokio::test]
async fn an_endpoint_becomes_ready_and_serves_its_target_version() {
    let router = router();
    let harness_id = create_harness(&router, "settlingEndpointHarness").await;
    let created = create_endpoint(&router, &harness_id, &json!({"endpointName": "live"})).await;
    assert_eq!(
        created.status, 201,
        "the create must succeed for this suite to say anything, got {} with {}",
        created.status, created.body
    );

    let fresh = get_endpoint(&router, &harness_id, "live").await;
    assert_eq!(
        (
            text(&fresh.body, "/endpoint/status"),
            at(&fresh.body, "/endpoint/liveVersion")
        ),
        ("CREATING".to_string(), Value::Null),
        "case \"before the settle span\": the endpoint is still CREATING and serves nothing, got {}",
        fresh.body
    );

    tokio::time::sleep(PAST_ENDPOINT_SETTLE).await;

    for case in ["the first read after the settle span", "the read after that"] {
        let settled = get_endpoint(&router, &harness_id, "live").await;
        assert_eq!(
            (
                text(&settled.body, "/endpoint/status"),
                text(&settled.body, "/endpoint/liveVersion")
            ),
            ("READY".to_string(), FIRST_VERSION.to_string()),
            "case {case:?}: once the transition is due the endpoint is READY and serves its \
             target version, got {}",
            settled.body
        );
        assert_eq!(
            text(&settled.body, "/endpoint/targetVersion"),
            text(&settled.body, "/endpoint/liveVersion"),
            "case {case:?}: a settled endpoint serves the version it points to, got {}",
            settled.body
        );
    }
}

/// An endpoint is reached only THROUGH the harness that holds it.
///
/// The rows differ along what the request names that no stored endpoint answers to: a
/// harness id nothing created, an endpoint name its harness does not hold, and another
/// harness's endpoint name. A missing harness is reported as the harness it is; a missing
/// endpoint as the endpoint.
#[tokio::test]
async fn an_endpoint_is_addressed_only_through_its_harness() {
    let router = router();
    let first = create_harness(&router, "ownerHarness").await;
    let second = create_harness(&router, "otherHarness").await;
    let created = create_endpoint(&router, &first, &json!({"endpointName": "onlyHere"})).await;
    assert_eq!(
        created.status, 201,
        "the create must succeed for this suite to say anything, got {} with {}",
        created.status, created.body
    );

    // (case, harnessId, endpointName, the message the refusal carries)
    let rows: Vec<(&str, String, &str, String)> = vec![
        (
            "a harnessId inside its pattern that names no stored harness",
            ABSENT_HARNESS_ID.to_string(),
            "onlyHere",
            format!("Agent with name {ABSENT_HARNESS_ID} not found."),
        ),
        (
            "an endpoint name the harness does not hold",
            first.clone(),
            "neverCreated",
            "Agent endpoint with name neverCreated not found.".to_string(),
        ),
        (
            "another harness's endpoint name",
            second.clone(),
            "onlyHere",
            "Agent endpoint with name onlyHere not found.".to_string(),
        ),
    ];

    for (case, harness_id, endpoint_name, message) in rows {
        for (verb, answer) in [
            ("read", get_endpoint(&router, &harness_id, endpoint_name).await),
            (
                "delete",
                delete_endpoint(&router, &harness_id, endpoint_name).await,
            ),
        ] {
            assert_eq!(
                (answer.status, answer.error_type.as_str(), text(&answer.body, "/message")),
                (404, "ResourceNotFoundException", message.clone()),
                "case {case:?} ({verb}): expected HTTP 404 ResourceNotFoundException {message:?}, \
                 got {} {:?} with {}",
                answer.status,
                answer.error_type,
                answer.body
            );
        }
    }

    // A create for a harness this account does not hold is refused the same way.
    let orphan = create_endpoint(&router, ABSENT_HARNESS_ID, &json!({"endpointName": "live"})).await;
    assert_eq!(
        (orphan.status, orphan.error_type.as_str(), text(&orphan.body, "/message")),
        (
            404,
            "ResourceNotFoundException",
            format!("Agent with name {ABSENT_HARNESS_ID} not found.")
        ),
        "case \"a create for a harness that does not exist\": expected the modelled 404, got {} \
         {:?} with {}",
        orphan.status,
        orphan.error_type,
        orphan.body
    );

    // The endpoint its own harness holds still reads, so each refusal above is about what
    // the row changed rather than about the endpoint.
    let held = get_endpoint(&router, &first, "onlyHere").await;
    assert_eq!(
        held.status, 200,
        "case \"the endpoint its harness holds\": expected the endpoint, got {} with {}",
        held.status, held.body
    );
}

/// One endpoint per name on one harness, and one version per endpoint: a repeated name is
/// the modelled conflict, and a `targetVersion` the harness does not hold names no stored
/// version.
///
/// The rows differ along the version asked for: the one the harness holds, the next one
/// nothing published, the highest `HarnessVersion`'s pattern admits, and one past it (which
/// the modelled pattern itself refuses).
#[tokio::test]
async fn a_repeated_name_conflicts_and_an_unheld_version_is_absent() {
    let router = router();
    let harness_id = create_harness(&router, "uniqueEndpointHarness").await;

    let first = create_endpoint(&router, &harness_id, &json!({"endpointName": "live"})).await;
    assert_eq!(
        first.status, 201,
        "the first create must succeed for this suite to say anything, got {} with {}",
        first.status, first.body
    );

    let repeat = create_endpoint(
        &router,
        &harness_id,
        &json!({"endpointName": "live", "description": "a second endpoint under one name"}),
    )
    .await;
    assert_eq!(
        (repeat.status, repeat.error_type.as_str()),
        (409, "ConflictException"),
        "case \"a repeated endpoint name\": one endpoint per name on one harness, so the repeat is \
         the modelled 409 ConflictException, got {} {:?} with {}",
        repeat.status,
        repeat.error_type,
        repeat.body
    );
    let read = get_endpoint(&router, &harness_id, "live").await;
    assert_eq!(
        at(&read.body, "/endpoint/description"),
        Value::Null,
        "case \"a repeated endpoint name\": the refused create changed nothing, got {}",
        read.body
    );

    // A different name on the same harness is a second endpoint, not a conflict.
    let other = create_endpoint(&router, &harness_id, &json!({"endpointName": "canary"})).await;
    assert_eq!(
        other.status, 201,
        "case \"another name on the same harness\": expected a second endpoint, got {} with {}",
        other.status, other.body
    );

    // (case, targetVersion, the status owed, the errortype owed)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        ("the version the harness holds", FIRST_VERSION.to_string(), 201, ""),
        ("the next version, which nothing published", "2".to_string(), 404, "ResourceNotFoundException"),
        (
            "the highest version HarnessVersion admits",
            "99999".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "one past that bound, which the @pattern itself refuses",
            "100000".to_string(),
            400,
            "ValidationException",
        ),
    ];

    for (index, (case, target_version, status, error_type)) in rows.into_iter().enumerate() {
        let answer = create_endpoint(
            &router,
            &harness_id,
            &json!({"endpointName": format!("pinned{index}"), "targetVersion": target_version}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (status, error_type),
            "case {case:?}: expected HTTP {status} {error_type:?} for targetVersion \
             {target_version:?}, got {} {:?} with {}",
            answer.status,
            answer.error_type,
            answer.body
        );
    }
}

/// A `clientToken` replayed to `CreateHarnessEndpoint` answers with the endpoint it first
/// produced, rather than adding a second one — the `@idempotencyToken` the model marks it
/// with.
///
/// The rows differ along what the replayed request carries: the same body, and a body whose
/// other members changed, which the token still short-circuits.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_first_endpoint() {
    let router = router();
    let harness_id = create_harness(&router, "idempotentEndpointHarness").await;

    // `ClientToken`'s @length is 33..256.
    let token = "b".repeat(40);
    let first = create_endpoint(
        &router,
        &harness_id,
        &json!({"endpointName": "live", "clientToken": token}),
    )
    .await;
    assert_eq!(
        first.status, 201,
        "the first create must succeed for this suite to say anything, got {} with {}",
        first.status, first.body
    );
    let created = at(&first.body, "/endpoint");

    // (case, the body the replay sends)
    let rows: Vec<(&str, Value)> = vec![
        ("the same body", json!({"endpointName": "live", "clientToken": token.clone()})),
        (
            "a body whose other members changed",
            json!({
                "endpointName": "replayed",
                "clientToken": token.clone(),
                "description": "a different endpoint entirely",
            }),
        ),
    ];

    for (case, body) in rows {
        let replay = create_endpoint(&router, &harness_id, &body).await;
        assert_eq!(
            replay.status, 201,
            "case {case:?}: a replayed idempotency token is not an error, got {} with {}",
            replay.status, replay.body
        );
        assert_eq!(
            at(&replay.body, "/endpoint/endpointName"),
            at(&created, "/endpointName"),
            "case {case:?}: the replay answers with the endpoint the token first produced, got {}",
            replay.body
        );
        assert_eq!(
            at(&replay.body, "/endpoint/arn"),
            at(&created, "/arn"),
            "case {case:?}: the replay reports the stored endpoint, not a fresh one, got {}",
            replay.body
        );
    }

    // The replays added nothing: the name they carried never became an endpoint of its own.
    let invented = get_endpoint(&router, &harness_id, "replayed").await;
    assert_eq!(
        invented.status, 404,
        "case \"the name the replay carried\": a replay mints no second endpoint, got {} with {}",
        invented.status, invented.body
    );
}

/// A harness that still holds endpoints is not destroyed: the delete is refused with the
/// service's fixed sentence until every endpoint is deleted, and the identical call then
/// succeeds.
///
/// The rows differ along how many endpoints the harness holds, so the refusal is the same
/// one for the first endpoint as for several, and each endpoint has to go before the
/// harness can.
#[tokio::test]
async fn a_harness_is_not_deleted_while_it_holds_endpoints() {
    let router = router();

    // (case, harnessName, the endpoints created on it)
    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        ("one endpoint", "oneEndpointHarness", vec!["live"]),
        (
            "three endpoints",
            "threeEndpointHarness",
            vec!["live", "canary", "beta_2"],
        ),
    ];

    for (case, harness_name, endpoints) in rows {
        let harness_id = create_harness(&router, harness_name).await;
        for name in &endpoints {
            let created = create_endpoint(&router, &harness_id, &json!({"endpointName": name})).await;
            assert_eq!(
                created.status, 201,
                "case {case:?}: creating the endpoint {name:?} must succeed for this row to say \
                 anything, got {} with {}",
                created.status, created.body
            );
        }

        let expected_message = format!(
            "Harness {harness_name} has endpoints that must be deleted before the harness can be \
             deleted."
        );
        // Every endpoint but the last is deleted in turn: the refusal holds while ANY is
        // attached, not only while all of them are.
        for (deleted, name) in endpoints.iter().enumerate() {
            let blocked = delete_harness(&router, &harness_id).await;
            assert_eq!(
                (
                    blocked.status,
                    blocked.error_type.as_str(),
                    text(&blocked.body, "/message")
                ),
                (409, "ConflictException", expected_message.clone()),
                "case {case:?}: with {} endpoint(s) still attached the delete is refused 409 \
                 ConflictException {expected_message:?}, got {} {:?} with {}",
                endpoints.len() - deleted,
                blocked.status,
                blocked.error_type,
                blocked.body
            );
            let read = send(&router, "GET", &format!("/harnesses/{harness_id}"), None).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the refused delete left the harness stored, got {} with {}",
                read.status, read.body
            );

            let removed = delete_endpoint(&router, &harness_id, name).await;
            assert_eq!(
                removed.status, 200,
                "case {case:?}: deleting the endpoint {name:?} is HTTP 200, got {} with {}",
                removed.status, removed.body
            );
            assert_eq!(
                text(&removed.body, "/endpoint/status"),
                "DELETING",
                "case {case:?}: the endpoint a delete removed is reported as DELETING, got {}",
                removed.body
            );
            let gone = get_endpoint(&router, &harness_id, name).await;
            assert_eq!(
                (gone.status, gone.error_type.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: a deleted endpoint is gone from its harness, got {} {:?} with {}",
                gone.status,
                gone.error_type,
                gone.body
            );
        }

        let accepted = delete_harness(&router, &harness_id).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: with every endpoint deleted the identical delete succeeds, got {} \
             with {}",
            accepted.status, accepted.body
        );
        let gone = send(&router, "GET", &format!("/harnesses/{harness_id}"), None).await;
        assert_eq!(
            (gone.status, gone.error_type.as_str(), text(&gone.body, "/message")),
            (
                404,
                "ResourceNotFoundException",
                format!("Agent with name {harness_id} not found.")
            ),
            "case {case:?}: the deleted harness is gone, got {} {:?} with {}",
            gone.status,
            gone.error_type,
            gone.body
        );
    }
}
