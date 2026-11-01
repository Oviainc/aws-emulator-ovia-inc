// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the three refusal arms of the Trigger plane
//! (`CreateTrigger` POST `/trigger/agent-space/{agentSpaceId}/triggers`,
//! `GetTrigger` GET `/trigger/agent-space/{agentSpaceId}/triggers/{triggerId}`).
//!
//! The rules under test:
//!
//! 1. A member whose value fails a regular-expression constraint this service
//!    enforces itself is refused with HTTP 400 `ValidationException` whose message is
//!    one sentence — `1 validation error detected. Value at '<json pointer>' failed to
//!    satisfy constraint: Member must satisfy regular expression pattern: <pattern>` —
//!    naming the member it validated as a JSON pointer and the pattern that member
//!    must satisfy, and echoing none of the rejected value. The identifier that names
//!    a Trigger is the identifier of the goal whose schedule it is, so the member is
//!    the goal identifier and the pattern is its UUID-shaped form.
//! 2. A well-formed identifier that names nothing in a space the caller reached is
//!    HTTP 404 `ResourceNotFoundException` naming the GOAL that was resolved —
//!    `Goal not found: <identifier>` — since a Trigger is a goal's schedule.
//! 3. An operation addressed to a resource INSIDE an agent space authorises the caller
//!    against that space before it can know what it holds, so an `agentSpaceId` that
//!    resolves to no space fails CLOSED with HTTP 403 `AccessDeniedException` and the
//!    wholly fixed message `Access denied`, disclosing no identifier.
//!
//! Each table spans the class rather than one recorded row: the identifier rows differ
//! in prefix, separator, character class, group count and group length; the agent-space
//! rows differ in length, character class and form; and both arms are crossed with the
//! operations of the plane, with the SERVED cases exercised too, so a refusal that
//! ignored the store would fail here.

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

/// The form a goal identifier takes, as the refusal states it. Written out here
/// rather than read from the service so the test pins the text itself.
const GOAL_ID_PATTERN: &str =
    "^goal-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";

/// The JSON pointer the refusal names the validated member by.
const GOAL_ID_PATH: &str = "/goalId";

/// The hexadecimal digits each dash-separated group of a UUID carries, which is what
/// [`GOAL_ID_PATTERN`] states after the prefix.
const UUID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// The published bounds of `AgentSpaceId`'s `^[a-zA-Z0-9-]{1,64}$`, from
/// `smithy_query(query="shape", shape="AgentSpaceId")`.
const AGENT_SPACE_ID_MIN_LEN: usize = 1;
const AGENT_SPACE_ID_MAX_LEN: usize = 64;

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

/// A create request this plane serves, firing on a fixed schedule.
fn schedule_request() -> Value {
    json!({
        "type": "SCHEDULE",
        "condition": {"schedule": {"expression": "cron(0 9 * * ? *)"}},
        "action": {"agent": "custom", "prompt": "review the oncall backlog"},
    })
}

/// `POST .../triggers` in `space`.
async fn create_trigger(router: &MultiServiceRouter, space: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/trigger/agent-space/{space}/triggers"),
        Some(schedule_request()),
    )
    .await
}

/// `GET .../triggers/{trigger_id}` in `space`.
async fn get_trigger(router: &MultiServiceRouter, space: &str, trigger_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/trigger/agent-space/{space}/triggers/{trigger_id}"),
        None,
    )
    .await
}

/// Create one Trigger in `space` and return its minted identifier.
async fn minted_trigger_id(router: &MultiServiceRouter, space: &str) -> String {
    let answer = create_trigger(router, space).await;
    assert_eq!(
        answer.status, 201,
        "case create in {space:?}: expected 201, got {} with {}",
        answer.status, answer.body
    );
    answer.body["trigger"]["triggerId"]
        .as_str()
        .unwrap_or_else(|| panic!("case create in {space:?}: no triggerId in {}", answer.body))
        .to_string()
}

/// Whether `value` has the goal-identifier form [`GOAL_ID_PATTERN`] states, decided
/// here rather than by the service so the test pins the rule.
fn is_goal_shaped(value: &str) -> bool {
    let Some(uuid) = value.strip_prefix("goal-") else {
        return false;
    };
    let groups: Vec<&str> = uuid.split('-').collect();
    groups.len() == UUID_GROUP_LENGTHS.len()
        && groups
            .iter()
            .zip(UUID_GROUP_LENGTHS)
            .all(|(group, length)| {
                group.len() == length && group.bytes().all(|b| b.is_ascii_hexdigit())
            })
}

/// The refusal a value that is not a goal identifier gets: one sentence, the validated
/// member as a JSON pointer, and the pattern it must satisfy.
fn identifier_refusal() -> String {
    format!(
        "1 validation error detected. Value at '{GOAL_ID_PATH}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {GOAL_ID_PATTERN}"
    )
}

/// Values that satisfy `ResourceId` (`^[a-zA-Z0-9_.-]+$`, 1..128) — so the boundary
/// validator passes them and this service's own guard is what refuses them — and are
/// not goal identifiers. The rows differ in prefix, separator, character class, group
/// count and group length, including each UUID group one digit short and one long.
fn malformed_identifiers() -> Vec<String> {
    let mut rows = vec![
        // No prefix, a wrong prefix, the prefix alone, the prefix in the wrong case and
        // joined by the wrong separator, and the prefix not at the start.
        "00000000-0000-0000-0000-000000000000".to_string(),
        "trigger-00000000-0000-0000-0000-000000000000".to_string(),
        "goal".to_string(),
        "goal-".to_string(),
        "GOAL-00000000-0000-0000-0000-000000000000".to_string(),
        "goal_00000000-0000-0000-0000-000000000000".to_string(),
        "goal.00000000-0000-0000-0000-000000000000".to_string(),
        "my-goal-00000000-0000-0000-0000-000000000000".to_string(),
        // The prefix followed by something that is not a UUID at all, by a UUID with
        // its separators dropped, by a non-hexadecimal digit, and by too few or too
        // many groups.
        "goal-anything".to_string(),
        "goal-0000000000009999".to_string(),
        "goal-000000000000000000000000000000000000".to_string(),
        "goal-0000000z-0000-0000-0000-000000000000".to_string(),
        "goal-00000000-0000-0000-000000000000".to_string(),
        "goal-00000000-0000-0000-0000-0000-00000000000".to_string(),
    ];
    // Each group one digit short and one digit long.
    for (index, length) in UUID_GROUP_LENGTHS.iter().enumerate() {
        for adjusted in [length - 1, length + 1] {
            let groups: Vec<String> = UUID_GROUP_LENGTHS
                .iter()
                .enumerate()
                .map(|(position, group)| {
                    let width = if position == index { adjusted } else { *group };
                    "a".repeat(width)
                })
                .collect();
            rows.push(format!("goal-{}", groups.join("-")));
        }
    }
    rows
}

/// Goal identifiers that name no stored Trigger: the all-zero and all-`f` UUIDs, a
/// mixed-case one, and an upper-case one — the pattern admits either case.
fn absent_goal_identifiers() -> Vec<String> {
    vec![
        "goal-00000000-0000-0000-0000-000000000000".to_string(),
        "goal-ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        "goal-ABCDEF01-2345-6789-abcd-EF0123456789".to_string(),
        "goal-0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0".to_string(),
    ]
}

/// Identifiers that satisfy `AgentSpaceId`'s `^[a-zA-Z0-9-]{1,64}$` and name no stored
/// AgentSpace: the shortest admissible one, a digit-only one, one in the form this
/// service mints, a mixed-case hyphenated one, an all-hyphen one, and one at the
/// pattern's maximum length.
fn absent_agent_space_ids() -> Vec<String> {
    vec![
        "a".repeat(AGENT_SPACE_ID_MIN_LEN),
        "7".to_string(),
        "as-0000000000000000".to_string(),
        "Mixed-Case-Agent-Space-42".to_string(),
        "---".to_string(),
        "z".repeat(AGENT_SPACE_ID_MAX_LEN),
    ]
}

#[tokio::test]
async fn a_value_that_is_not_a_goal_identifier_is_refused_without_being_echoed() {
    let router = new_router();
    let space = create_space(&router, "trigger-identifier-validation").await;

    for case in malformed_identifiers() {
        let answer = get_trigger(&router, &space, &case).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ValidationException",
            "case {case:?}: expected ValidationException, got {:?} ({})",
            answer.error_type, answer.body
        );
        assert_eq!(
            answer.message(),
            identifier_refusal(),
            "case {case:?}: expected the validation wording naming {GOAL_ID_PATH} and the goal \
             pattern, got {:?}",
            answer.message()
        );
        // No echo of the rejected value: the class states the member and the pattern
        // only. (A row that is itself a substring of the pattern text cannot be told
        // apart from the pattern, so it is exempt from this check — the equality above
        // already pins the whole message for it.)
        if !GOAL_ID_PATTERN.contains(case.as_str()) {
            assert!(
                !answer.message().contains(case.as_str()),
                "case {case:?}: the refusal must not echo the rejected value, got {:?}",
                answer.message()
            );
        }
        // The member the refusal names is the goal identifier, not the wire member it
        // arrived on.
        let field_path = answer.body["fieldList"][0]["path"].as_str();
        assert_eq!(
            field_path,
            Some(GOAL_ID_PATH),
            "case {case:?}: expected the field path {GOAL_ID_PATH}, got {:?}",
            answer.body["fieldList"]
        );
    }
}

#[tokio::test]
async fn the_identifier_form_is_decided_before_the_agent_space_is_resolved() {
    let router = new_router();
    create_space(&router, "trigger-identifier-precedence").await;

    // The same refusal whether or not the agent space exists: a value that names no
    // goal names no Trigger in any space, so nothing is looked up.
    for case in malformed_identifiers() {
        for space_id in absent_agent_space_ids() {
            let answer = get_trigger(&router, &space_id, &case).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} in absent space {space_id:?}: expected 400, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.message(),
                identifier_refusal(),
                "case {case:?} in absent space {space_id:?}: unexpected refusal message, got {:?}",
                answer.message()
            );
        }
    }
}

#[tokio::test]
async fn a_goal_identifier_that_names_nothing_reports_the_missing_goal() {
    let router = new_router();
    let space = create_space(&router, "trigger-absent-goal").await;
    let other_space = create_space(&router, "trigger-absent-goal-other").await;

    // A Trigger stored in another space, so the refusal is decided from what THIS
    // space holds rather than from the store being empty. Its identifier is the one
    // the service minted, so this row carries a value no literal here supplied.
    let elsewhere = minted_trigger_id(&router, &other_space).await;

    let mut rows = absent_goal_identifiers();
    rows.push(elsewhere.clone());
    for case in rows {
        let answer = get_trigger(&router, &space, &case).await;
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
            format!("Goal not found: {case}"),
            "case {case:?}: expected the missing-goal wording, got {:?}",
            answer.message()
        );
    }

    // The same identifier is served in the space that owns it, so the arm above is a
    // lookup in the store and not a blanket refusal.
    let served = get_trigger(&router, &other_space, &elsewhere).await;
    assert_eq!(
        served.status, 200,
        "case {elsewhere:?} in its own space: expected 200, got {} with {}",
        served.status, served.body
    );
}

#[tokio::test]
async fn an_agent_space_that_resolves_to_nothing_fails_closed() {
    let router = new_router();
    let present = create_space(&router, "trigger-fail-closed").await;
    let stored = minted_trigger_id(&router, &present).await;

    for space_id in absent_agent_space_ids() {
        assert_ne!(
            space_id, present,
            "the table must not name the stored AgentSpace"
        );
        // Both operations of the plane, and both a Trigger that exists elsewhere and
        // one that exists nowhere: inside an unresolvable space nothing about its
        // contents is reported.
        let mut answers = vec![(
            format!("CreateTrigger in {space_id:?}"),
            create_trigger(&router, &space_id).await,
        )];
        for trigger_id in [stored.as_str(), "goal-00000000-0000-0000-0000-000000000000"] {
            answers.push((
                format!("GetTrigger {trigger_id:?} in {space_id:?}"),
                get_trigger(&router, &space_id, trigger_id).await,
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
                "Access denied",
                "case {case}: expected the fixed access-denied wording, got {:?}",
                answer.message()
            );
            assert!(
                !answer.message().contains(space_id.as_str()),
                "case {case}: the refusal must disclose no identifier, got {:?}",
                answer.message()
            );
        }
    }

    // The space that does exist is still served, so the arm is decided from the store.
    let served = get_trigger(&router, &present, &stored).await;
    assert_eq!(
        served.status, 200,
        "case {stored:?} in {present:?}: expected 200, got {} with {}",
        served.status, served.body
    );
}

#[tokio::test]
async fn minted_trigger_identifiers_are_goal_shaped_and_read_back() {
    let router = new_router();
    let space = create_space(&router, "trigger-minted-identifiers").await;

    // Several mints in one space: every identifier the service hands out satisfies the
    // form its own refusal advertises, no two agree, and each reads back.
    let mut minted: Vec<String> = Vec::new();
    for index in 0..5 {
        let case = format!("mint {index}");
        let trigger_id = minted_trigger_id(&router, &space).await;
        assert!(
            is_goal_shaped(&trigger_id),
            "case {case}: minted identifier does not satisfy {GOAL_ID_PATTERN}: {trigger_id:?}"
        );
        assert!(
            !minted.contains(&trigger_id),
            "case {case}: minted identifier repeats an earlier one: {trigger_id:?}"
        );
        let answer = get_trigger(&router, &space, &trigger_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 reading back {trigger_id:?}, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["trigger"]["triggerId"].as_str(),
            Some(trigger_id.as_str()),
            "case {case}: the read must report the minted identifier, got {}",
            answer.body
        );
        minted.push(trigger_id);
    }
}
