// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the two refusal arms of the backlog-task plane
//! (`CreateBacklogTask` POST `/backlog/agent-space/{agentSpaceId}/tasks`,
//! `GetBacklogTask` GET `/backlog/agent-space/{agentSpaceId}/tasks/{taskId}`).
//!
//! The rules under test:
//!
//! 1. An operation addressed to a resource INSIDE an agent space authorises the
//!    caller against that space before it can know what it holds, so an
//!    `agentSpaceId` that resolves to no space fails CLOSED with HTTP 403
//!    `AccessDeniedException` and the wholly fixed message `Access denied`,
//!    disclosing no identifier — whatever the request names inside the space.
//! 2. Inside a space the caller reached, an identifier that names no task is
//!    HTTP 404 `ResourceNotFoundException` whose message is the fixed text
//!    `Task not found: ` and the requested `taskId` taken verbatim from the
//!    request path, with nothing else interpolated.
//!
//! Each table spans the class rather than one recorded row: the task-identifier rows
//! differ in length, character class, separator and form across the whole of
//! `ResourceId`'s `^[a-zA-Z0-9_.-]+$` (1..128) including both published length
//! bounds and one past the upper one; the agent-space rows differ in length,
//! character class and form, and include both published length bounds of
//! `AgentSpaceIdentifier` and the minted identifier of a space that was deleted. Both
//! arms are crossed with the operations of the plane, and the SERVED cases are
//! exercised too, so a refusal that ignored the store would fail here.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which is
/// how the router resolves which service claims a request. Signature verification is
/// switched off for these tests, so the signature is a placeholder.
const AUTHORIZATION: &str = concat!(
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/aidevops/aws4_request, ",
    "SignedHeaders=host, Signature=0123456789abcdef"
);

/// The published bounds of `ResourceId`, the shape `taskId` targets
/// (`@length(min: 1, max: 128)`, `@pattern ^[a-zA-Z0-9_.-]+$`), from
/// `smithy_query(query="shape", shape="ResourceId")`.
const TASK_ID_MIN_LEN: usize = 1;
const TASK_ID_MAX_LEN: usize = 128;

/// The published bounds of `AgentSpaceIdentifier`, the shape the backlog plane's
/// `agentSpaceId` label targets (`@length(min: 1, max: 2048)`, no pattern), from
/// `smithy_query(query="shape", shape="AgentSpaceIdentifier")`.
const AGENT_SPACE_IDENTIFIER_MIN_LEN: usize = 1;
const AGENT_SPACE_IDENTIFIER_MAX_LEN: usize = 2048;

/// The fail-closed wording, written out here rather than read from the service so the
/// test pins the text itself.
const ACCESS_DENIED: &str = "Access denied";

/// One emulator: a single shared engine behind the production router.
fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(engine),
    );
    router
}

/// One answer as it came off the wire.
struct Answer {
    status: u16,
    /// The error shape `x-amzn-errortype` names (restJson1's error code), falling back
    /// to the body's `__type`; empty on a success.
    error_type: String,
    body: Value,
}

impl Answer {
    /// The `message` of an error answer.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
    }
}

/// Send one request and return what came back.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body
        .as_ref()
        .map(|b| Bytes::from(serde_json::to_vec(b).expect("serialize request body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "aidevops.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    let error_type = if header.is_empty() {
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header
    };
    // Either source may carry a `namespace#Shape` or a trailing `:` — the shape name is
    // what names the error.
    let error_type = error_type
        .split(['#', ':'])
        .find(|part| !part.is_empty() && part.contains("Exception"))
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        error_type,
        body: parsed,
    }
}

/// Create an agent space and return its minted id.
async fn create_space(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(router, "POST", "/v1/agentspaces", Some(json!({"name": name}))).await;
    assert_eq!(
        answer.status, 201,
        "case {name:?}: expected 201, got {} with {}",
        answer.status, answer.body
    );
    answer.body["agentSpace"]["agentSpaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no agentSpaceId in {}", answer.body))
        .to_string()
}

/// A create request this plane serves, with the required members only.
fn create_request() -> Value {
    json!({
        "taskType": "INVESTIGATION",
        "title": "investigate the oncall backlog",
        "priority": "MEDIUM",
    })
}

/// `POST .../tasks` in `space`.
async fn create_task(router: &MultiServiceRouter, space: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/backlog/agent-space/{space}/tasks"),
        Some(create_request()),
    )
    .await
}

/// `GET .../tasks/{task_id}` in `space`.
async fn get_task(router: &MultiServiceRouter, space: &str, task_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/backlog/agent-space/{space}/tasks/{task_id}"),
        None,
    )
    .await
}

/// Create one task in `space` and return its minted identifier.
async fn minted_task_id(router: &MultiServiceRouter, space: &str) -> String {
    let answer = create_task(router, space).await;
    assert_eq!(
        answer.status, 201,
        "case create in {space:?}: expected 201, got {} with {}",
        answer.status, answer.body
    );
    answer.body["task"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case create in {space:?}: no taskId in {}", answer.body))
        .to_string()
}

/// Identifiers that satisfy `ResourceId` (`^[a-zA-Z0-9_.-]+$`, 1..128) and name no
/// stored task: the shortest admissible one, a digit-only one, one in the form this
/// service mints, each of the pattern's punctuation characters alone, a mixed-case
/// dotted one, and one at the pattern's maximum length.
fn absent_task_ids() -> Vec<String> {
    vec![
        "a".repeat(TASK_ID_MIN_LEN),
        "7".to_string(),
        "task-0000000000000000".to_string(),
        "_".to_string(),
        ".".to_string(),
        "-".to_string(),
        "Mixed.Case_Task-42".to_string(),
        "z".repeat(TASK_ID_MAX_LEN),
    ]
}

/// Values that satisfy `AgentSpaceIdentifier`'s length bound and name no stored agent
/// space: the shortest admissible one, a digit-only one, one in the form this service
/// mints, a mixed-case hyphenated one, an all-hyphen one, a value holding characters
/// no `AgentSpaceId` admits (the identifier shape of this plane carries no pattern),
/// and one at the length bound's maximum.
fn absent_agent_space_ids() -> Vec<String> {
    vec![
        "a".repeat(AGENT_SPACE_IDENTIFIER_MIN_LEN),
        "7".to_string(),
        "as-0000000000000000".to_string(),
        "Mixed-Case-Agent-Space-42".to_string(),
        "---".to_string(),
        "as_0000000000000000.v2".to_string(),
        "z".repeat(AGENT_SPACE_IDENTIFIER_MAX_LEN),
    ]
}

#[tokio::test]
async fn a_task_that_names_nothing_in_a_reachable_space_reports_the_missing_task() {
    let router = new_router();
    let space = create_space(&router, "backlog-absent-task").await;
    let other_space = create_space(&router, "backlog-absent-task-other").await;

    // A task stored in another space, so the refusal is decided from what THIS space
    // holds rather than from the store being empty. Its identifier is the one the
    // service minted, so this row carries a value no literal here supplied.
    let elsewhere = minted_task_id(&router, &other_space).await;
    // And a task stored in THIS space, so the space is non-empty too.
    let present = minted_task_id(&router, &space).await;

    let mut rows = absent_task_ids();
    rows.push(elsewhere.clone());
    for case in rows {
        assert_ne!(case, present, "the table must not name the stored task");
        let answer = get_task(&router, &space, &case).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?} ({})",
            answer.error_type, answer.body
        );
        assert_eq!(
            answer.message(),
            format!("Task not found: {case}"),
            "case {case:?}: expected the missing-task wording naming only the requested taskId, \
             got {:?}",
            answer.message()
        );
        // The refusal of a space the caller DID reach names the task alone: the
        // containing space is already known to them and is not repeated.
        assert!(
            !answer.message().contains(space.as_str()),
            "case {case:?}: the refusal must not name the agent space, got {:?}",
            answer.message()
        );
    }

    // The same identifier is served in the space that owns it, and so is this space's
    // own task, so the arm above is a lookup in the store and not a blanket refusal.
    for (case, space_id, task_id) in [
        ("task in its own space", other_space.as_str(), elsewhere.as_str()),
        ("task in this space", space.as_str(), present.as_str()),
    ] {
        let served = get_task(&router, space_id, task_id).await;
        assert_eq!(
            served.status, 200,
            "case {case}: expected 200, got {} with {}",
            served.status, served.body
        );
        assert_eq!(
            served.body["task"]["taskId"].as_str(),
            Some(task_id),
            "case {case}: the read must report the requested task, got {}",
            served.body
        );
    }
}

#[tokio::test]
async fn an_agent_space_that_resolves_to_nothing_fails_closed() {
    let router = new_router();
    let present = create_space(&router, "backlog-fail-closed").await;
    let stored = minted_task_id(&router, &present).await;

    for space_id in absent_agent_space_ids() {
        assert_ne!(
            space_id, present,
            "the table must not name the stored AgentSpace"
        );
        // Both operations of the plane, and both a task that exists elsewhere and one
        // that exists nowhere: inside an unresolvable space nothing about its contents
        // is reported.
        let mut answers = vec![(
            format!("CreateBacklogTask in {space_id:?}"),
            create_task(&router, &space_id).await,
        )];
        for task_id in [stored.as_str(), "task-0000000000000000"] {
            answers.push((
                format!("GetBacklogTask {task_id:?} in {space_id:?}"),
                get_task(&router, &space_id, task_id).await,
            ));
        }
        for (case, answer) in answers {
            assert_eq!(
                answer.status, 403,
                "case {case}: expected 403, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, "AccessDeniedException",
                "case {case}: expected AccessDeniedException, got {:?} ({})",
                answer.error_type, answer.body
            );
            assert_eq!(
                answer.message(),
                ACCESS_DENIED,
                "case {case}: expected the fixed access-denied wording, got {:?}",
                answer.message()
            );
            assert!(
                !answer.message().contains(space_id.as_str())
                    && !answer.message().contains(stored.as_str()),
                "case {case}: the refusal must disclose no identifier, got {:?}",
                answer.message()
            );
        }
    }

    // The space that does exist is still served, so the arm is decided from the store.
    let served = get_task(&router, &present, &stored).await;
    assert_eq!(
        served.status, 200,
        "case {stored:?} in {present:?}: expected 200, got {} with {}",
        served.status, served.body
    );
}

#[tokio::test]
async fn a_space_that_was_deleted_fails_closed_for_the_tasks_it_held() {
    let router = new_router();
    let space = create_space(&router, "backlog-deleted-space").await;
    let stored = minted_task_id(&router, &space).await;

    // Served while the space exists.
    let served = get_task(&router, &space, &stored).await;
    assert_eq!(
        served.status, 200,
        "case before the delete: expected 200, got {} with {}",
        served.status, served.body
    );

    let deleted = call(&router, "DELETE", &format!("/v1/agentspaces/{space}"), None).await;
    assert_eq!(
        deleted.status, 204,
        "case delete: expected 204, got {} with {}",
        deleted.status, deleted.body
    );

    // The identifiers in both rows are ones the service minted, so neither value came
    // from a literal here: with the space gone the request reaches nothing inside it.
    for (case, task_id) in [
        ("the task it held", stored.as_str()),
        ("a task it never held", "task-0000000000000000"),
    ] {
        let answer = get_task(&router, &space, task_id).await;
        assert_eq!(
            answer.status, 403,
            "case {case}: expected 403 once the space is gone, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "AccessDeniedException",
            "case {case}: expected AccessDeniedException, got {:?} ({})",
            answer.error_type, answer.body
        );
        assert_eq!(
            answer.message(),
            ACCESS_DENIED,
            "case {case}: expected the fixed access-denied wording, got {:?}",
            answer.message()
        );
    }
}

#[tokio::test]
async fn a_task_identifier_past_its_pattern_bound_is_not_a_lookup_miss() {
    let router = new_router();
    let space = create_space(&router, "backlog-bound").await;

    // One character past `ResourceId`'s maximum length, and a value holding a
    // character its pattern does not admit: the request never reaches a lookup, so it
    // is a validation refusal rather than either refusal arm of this plane.
    for case in [
        "z".repeat(TASK_ID_MAX_LEN + 1),
        "task~0000000000000000".to_string(),
    ] {
        let answer = get_task(&router, &space, &case).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a taskId outside ResourceId's constraints, got {} \
             with {}",
            answer.status, answer.body
        );
        assert_ne!(
            answer.message(),
            format!("Task not found: {case}"),
            "case {case:?}: a value the constraints reject must not be answered as a lookup miss, \
             got {}",
            answer.body
        );
        assert_ne!(
            answer.message(),
            ACCESS_DENIED,
            "case {case:?}: a value the constraints reject must not be answered as an \
             authorization failure, got {}",
            answer.body
        );
    }
}
