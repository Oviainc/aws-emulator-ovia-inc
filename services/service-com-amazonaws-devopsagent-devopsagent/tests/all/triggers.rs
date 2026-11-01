// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Trigger slice of one AgentSpace: `CreateTrigger`
//! (POST `/trigger/agent-space/{agentSpaceId}/triggers`) and `GetTrigger`
//! (GET `/trigger/agent-space/{agentSpaceId}/triggers/{triggerId}`).
//!
//! Every case drives the service through the same router the emulator serves, so the
//! routes, status codes, protocol envelope and state are all exercised.
//!
//! The rules under test:
//!
//! * The identifier that names a Trigger is the identifier of the goal whose schedule
//!   it is, so an identifier without that form names no Trigger in any agent space and
//!   is refused as invalid (400 `ValidationException`) BEFORE any resource is looked
//!   up; a well-formed identifier that names nothing is refused as not found (404).
//! * A Trigger is persisted by its create and reported unchanged by every later read:
//!   the schedule it fires on, the action it performs, its status and its timestamps.
//! * A schedule condition carries exactly one of `expression` or `spec`; a structured
//!   `spec` is the source of truth and the EventBridge expression the Trigger fires on
//!   is derived from it, with both reported together.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_devopsagent_devopsagent::state::is_goal_id;

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which is
/// how the router resolves which service claims a request. Signature verification is
/// switched off for these tests, so the signature is a placeholder.
const AUTHORIZATION: &str = concat!(
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/aidevops/aws4_request, ",
    "SignedHeaders=host, Signature=0123456789abcdef"
);

/// The form a Trigger identifier takes — a goal identifier — as the refusal states
/// it.
const GOAL_ID_PATTERN: &str =
    "^goal-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";

/// The published bounds of `GetTriggerInput$triggerId` (`ResourceId`, `@length(min: 1,
/// max: 128)`), from `smithy_query(query="operation", operation="GetTrigger")`.
const TRIGGER_ID_MAX_LEN: usize = 128;

/// The `DayOfWeek` value set, from the model's enum
/// (`smithy_query(query="shape", shape="DayOfWeek")`), paired with the day-of-week
/// field an EventBridge cron expression names it with.
const DAYS_OF_WEEK: &[(&str, &str)] = &[
    ("MONDAY", "MON"),
    ("TUESDAY", "TUE"),
    ("WEDNESDAY", "WED"),
    ("THURSDAY", "THU"),
    ("FRIDAY", "FRI"),
    ("SATURDAY", "SAT"),
    ("SUNDAY", "SUN"),
];

/// The published bounds of `MonthlyRecurrence$dayOfMonth` (`@range(min: 1, max: 31)`),
/// from `smithy_query(query="shape", shape="MonthlyRecurrence")`.
const DAY_OF_MONTH_MIN: i64 = 1;
const DAY_OF_MONTH_MAX: i64 = 31;

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

/// Send one request and return its status and parsed body (`Value::Null` for an empty
/// body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
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
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error response.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` of an error response.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Create an agent space and return its id.
async fn create_space(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "POST", "/v1/agentspaces", Some(json!({"name": name}))).await;
    assert_eq!(status, 201, "case {name:?}: expected 201, got {status}");
    body["agentSpace"]["agentSpaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no agentSpaceId in {body}"))
        .to_string()
}

/// `POST .../triggers` in `space`.
async fn create_trigger(
    router: &MultiServiceRouter,
    space: &str,
    request: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/trigger/agent-space/{space}/triggers"),
        Some(request),
    )
    .await
}

/// `GET .../triggers/{trigger_id}` in `space`.
async fn get_trigger(router: &MultiServiceRouter, space: &str, trigger_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/trigger/agent-space/{space}/triggers/{trigger_id}"),
        None,
    )
    .await
}

/// A create request firing on `expression`, which every schedule-shape case varies
/// from.
fn schedule_request(expression: &str) -> Value {
    json!({
        "type": "SCHEDULE",
        "condition": {"schedule": {"expression": expression}},
        "action": {"agent": "custom", "prompt": "review the oncall backlog"},
    })
}

/// The refusal a Trigger identifier without the goal form gets, as the service words
/// it: one sentence naming the validated member and the pattern it must satisfy, with
/// no echo of the rejected value.
fn identifier_refusal() -> String {
    format!(
        "1 validation error detected. Value at '/goalId' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {GOAL_ID_PATTERN}"
    )
}

/// An identifier without the goal form is refused as invalid, and refused the same way
/// whether or not the agent space it is asked for exists: the form of the identifier is
/// decided before anything is looked up.
#[tokio::test]
async fn get_trigger_refuses_an_identifier_that_is_not_a_goal_identifier() {
    let router = new_router();
    let space = create_space(&router, "trigger-identifier-form").await;
    let absent_space = "as-ffffffffffffffff";

    // Each row satisfies `ResourceId` (`^[a-zA-Z0-9_.-]+$`, 1..128), so the boundary
    // validator passes it and the handler is what refuses it: a bare resource name, a
    // differently-typed identifier, the prefix with no goal after it, the prefix in the
    // wrong case, the prefix joined by the wrong separator, the prefix buried inside the
    // value rather than starting it, and the prefix followed by something that is not a
    // UUID.
    // The last row sits exactly on the published 128-character bound of `triggerId`,
    // so the modelled constraint admits it and the goal form is what refuses it.
    let longest = format!("goal-{}", "a".repeat(TRIGGER_ID_MAX_LEN - "goal-".len()));
    let malformed = [
        "0001",
        "trigger-0000000000000001",
        "task-0000000000000001",
        "goal",
        "goal-",
        "GOAL-00000000-0000-0000-0000-000000000000",
        "goal_00000000-0000-0000-0000-000000000000",
        "goal.00000000-0000-0000-0000-000000000000",
        "my-goal-00000000-0000-0000-0000-000000000000",
        "goal-anything",
        "goal-0000000000009999",
        "goal-00000000-0000-0000-0000-00000000000",
        longest.as_str(),
    ];
    for case in malformed {
        for space_id in [space.as_str(), absent_space] {
            let (status, body) = get_trigger(&router, space_id, case).await;
            assert_eq!(
                status, 400,
                "case {case:?} in space {space_id:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} in space {space_id:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                identifier_refusal(),
                "case {case:?} in space {space_id:?}: unexpected refusal message"
            );
        }
    }

    // A goal identifier that names no stored Trigger is a different failure: the
    // request is well formed, the resource is simply absent.
    let well_formed = [
        "goal-00000000-0000-0000-0000-000000000000",
        "goal-ffffffff-ffff-ffff-ffff-ffffffffffff",
        "goal-ABCDEF01-2345-6789-abcd-ef0123456789",
    ];
    for case in well_formed {
        assert_eq!(
            case.len(),
            case.chars().count(),
            "case {case:?}: the fixture row must be ASCII"
        );
        let (status, body) = get_trigger(&router, &space, case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Goal not found: {case}"),
            "case {case:?}: unexpected refusal message"
        );
    }

    // One character past that bound is outside the modelled length, so the boundary
    // validator refuses it before the handler sees it — still a 400, by the modelled
    // constraint rather than by the identifier's form.
    let too_long = format!("goal-{}", "a".repeat(TRIGGER_ID_MAX_LEN));
    let (status, body) = get_trigger(&router, &space, &too_long).await;
    assert_eq!(
        status, 400,
        "case {too_long:?}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {too_long:?}: expected ValidationException, got {body}"
    );

    // A well-formed identifier in an agent space that does not exist fails closed: the
    // caller is authorised against the space before anything inside it can be known,
    // and the refusal names no identifier at all.
    let (status, body) = get_trigger(
        &router,
        absent_space,
        "goal-00000000-0000-0000-0000-000000000000",
    )
    .await;
    assert_eq!(
        status, 403,
        "case absent space: expected 403, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "case absent space: expected AccessDeniedException, got {body}"
    );
    assert_eq!(
        error_message(&body),
        "Access denied",
        "case absent space: unexpected refusal message"
    );
}

/// What a create persists is what every later read reports, across the members a
/// Trigger carries, and a read changes nothing a later caller sees.
#[tokio::test]
async fn created_triggers_round_trip_through_get_trigger() {
    let router = new_router();
    let space = create_space(&router, "trigger-round-trip").await;
    let other_space = create_space(&router, "trigger-round-trip-other").await;

    // The rows differ along every member the create takes: the trigger type, the
    // schedule expression form (cron and rate), the action document's shape, and
    // whether the caller named the initial status.
    let rows: Vec<(&str, Value)> = vec![
        (
            "cron expression, no status",
            json!({
                "type": "SCHEDULE",
                "condition": {"schedule": {"expression": "cron(0 9 * * ? *)"}},
                "action": {"agent": "custom", "prompt": "review the oncall backlog"},
            }),
        ),
        (
            "rate expression, caller status",
            json!({
                "type": "CUSTOM_AGENT_SCHEDULE",
                "condition": {"schedule": {"expression": "rate(1 day)"}},
                "action": {"agent": "custom", "parameters": {"depth": 3, "dryRun": true}},
                "status": "DISABLED",
            }),
        ),
        (
            "action is a list document",
            json!({
                "type": "SCHEDULE",
                "condition": {"schedule": {"expression": "rate(12 hours)"}},
                "action": {"steps": ["triage", "summarise"]},
                "status": "ENABLED",
            }),
        ),
    ];

    for (case, request) in rows {
        let (status, created) = create_trigger(&router, &space, request.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created}"
        );
        let trigger = &created["trigger"];
        let trigger_id = trigger["triggerId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no triggerId in {created}"));
        assert!(
            is_goal_id(trigger_id),
            "case {case:?}: minted identifier is not a goal identifier: {trigger_id:?}"
        );
        assert_eq!(
            trigger["agentSpaceId"].as_str(),
            Some(space.as_str()),
            "case {case:?}: expected the creating agent space, got {trigger}"
        );
        assert_eq!(
            trigger["type"], request["type"],
            "case {case:?}: expected the submitted type, got {trigger}"
        );
        assert_eq!(
            trigger["action"], request["action"],
            "case {case:?}: expected the submitted action, got {trigger}"
        );
        assert_eq!(
            trigger["condition"]["schedule"]["expression"],
            request["condition"]["schedule"]["expression"],
            "case {case:?}: expected the submitted expression, got {trigger}"
        );
        assert!(
            trigger["condition"]["schedule"].get("spec").is_none(),
            "case {case:?}: an expression-only schedule has no structured spec, got {trigger}"
        );
        // An omitted status takes the status a created Trigger starts in.
        let expected_status = request
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("ENABLED");
        assert_eq!(
            trigger["status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected status {expected_status:?}, got {trigger}"
        );
        for member in ["createdAt", "updatedAt"] {
            assert!(
                trigger[member].is_number(),
                "case {case:?}: {member} must be a timestamp, got {trigger}"
            );
        }

        // The read reports exactly what was stored, twice over: a report must not
        // change what a later caller sees.
        let (first_status, first) = get_trigger(&router, &space, trigger_id).await;
        assert_eq!(
            first_status, 200,
            "case {case:?}: expected 200, got {first_status} with {first}"
        );
        assert_eq!(
            first["trigger"], *trigger,
            "case {case:?}: the read must report the created Trigger"
        );
        let (_, second) = get_trigger(&router, &space, trigger_id).await;
        assert_eq!(
            second, first,
            "case {case:?}: two reads of one Trigger must agree"
        );

        // A Trigger is reachable only through the agent space that owns it.
        let (status, body) = get_trigger(&router, &other_space, trigger_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from another agent space, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Goal not found: {trigger_id}"),
            "case {case:?}: unexpected refusal message from another agent space"
        );
    }
}

/// A structured schedule is the source of truth and the expression the Trigger fires
/// on is derived from it; both are reported, by the create and by every later read.
#[tokio::test]
async fn structured_schedules_report_the_derived_expression_beside_the_spec() {
    let router = new_router();
    let space = create_space(&router, "trigger-structured-schedule").await;

    // (case, spec, the EventBridge expression it derives). A cron spec already carries
    // the expression that anchors its window; a time range is anchored at its
    // `startAfter` time of day, with the recurrence choosing the day fields.
    let mut rows: Vec<(String, Value, String)> = vec![
        (
            "cron spec".to_string(),
            json!({"cron": {"expression": "cron(15 6 * * ? *)"}}),
            "cron(15 6 * * ? *)".to_string(),
        ),
        (
            "rate spec".to_string(),
            json!({"cron": {"expression": "rate(30 minutes)"}}),
            "rate(30 minutes)".to_string(),
        ),
        (
            "daily time range".to_string(),
            json!({"timeRange": {
                "startAfter": "09:30",
                "startBefore": "10:00",
                "recurrence": {"daily": {}},
            }}),
            "cron(30 9 * * ? *)".to_string(),
        ),
        (
            "daily time range wrapping past midnight".to_string(),
            json!({"timeRange": {
                "startAfter": "23:50",
                "startBefore": "00:20",
                "recurrence": {"daily": {}},
            }}),
            "cron(50 23 * * ? *)".to_string(),
        ),
    ];
    // Every `DayOfWeek` value, and both published bounds of `dayOfMonth`.
    for (day, field) in DAYS_OF_WEEK {
        rows.push((
            format!("weekly time range on {day}"),
            json!({"timeRange": {
                "startAfter": "00:05",
                "startBefore": "01:05",
                "recurrence": {"weekly": {"dayOfWeek": day}},
            }}),
            format!("cron(5 0 ? * {field} *)"),
        ));
    }
    for day_of_month in [DAY_OF_MONTH_MIN, DAY_OF_MONTH_MAX] {
        rows.push((
            format!("monthly time range on day {day_of_month}"),
            json!({"timeRange": {
                "startAfter": "18:00",
                "startBefore": "18:30",
                "recurrence": {"monthly": {"dayOfMonth": day_of_month}},
            }}),
            format!("cron(0 18 {day_of_month} * ? *)"),
        ));
    }

    for (case, spec, expected_expression) in rows {
        let request = json!({
            "type": "SCHEDULE",
            "condition": {"schedule": {"spec": spec}},
            "action": {"agent": "custom"},
        });
        let (status, created) = create_trigger(&router, &space, request.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created}"
        );
        let trigger = &created["trigger"];
        assert_eq!(
            trigger["condition"]["schedule"]["expression"].as_str(),
            Some(expected_expression.as_str()),
            "case {case:?}: expected the derived expression, got {trigger}"
        );
        assert_eq!(
            trigger["condition"]["schedule"]["spec"],
            request["condition"]["schedule"]["spec"],
            "case {case:?}: expected the submitted spec, got {trigger}"
        );

        let trigger_id = trigger["triggerId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no triggerId in {created}"));
        let (read_status, read) = get_trigger(&router, &space, trigger_id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200, got {read_status} with {read}"
        );
        assert_eq!(
            read["trigger"], *trigger,
            "case {case:?}: the read must report the created Trigger"
        );
    }

    // One past the published `dayOfMonth` bound is outside the modelled range, so the
    // request is refused instead of stored.
    let request = json!({
        "type": "SCHEDULE",
        "condition": {"schedule": {"spec": {"timeRange": {
            "startAfter": "18:00",
            "startBefore": "18:30",
            "recurrence": {"monthly": {"dayOfMonth": DAY_OF_MONTH_MAX + 1}},
        }}}},
        "action": {"agent": "custom"},
    });
    let (status, body) = create_trigger(&router, &space, request).await;
    assert_eq!(
        status, 400,
        "case dayOfMonth past its bound: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case dayOfMonth past its bound: expected ValidationException, got {body}"
    );
}

/// A schedule condition carries exactly one of `expression` or `spec`, and a repeat of
/// one create with its own idempotency token is that create, not a second Trigger.
#[tokio::test]
async fn create_trigger_requires_exactly_one_schedule_source_and_replays_its_token() {
    let router = new_router();
    let space = create_space(&router, "trigger-schedule-source").await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "neither expression nor spec",
            json!({"schedule": {}}),
        ),
        (
            "both expression and spec",
            json!({"schedule": {
                "expression": "cron(0 9 * * ? *)",
                "spec": {"cron": {"expression": "cron(0 9 * * ? *)"}},
            }}),
        ),
    ];
    for (case, condition) in rows {
        let (status, body) = create_trigger(
            &router,
            &space,
            json!({
                "type": "SCHEDULE",
                "condition": condition,
                "action": {"agent": "custom"},
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert!(
            error_message(&body).contains("exactly one of expression or spec"),
            "case {case:?}: the refusal must state the requirement, got {}",
            error_message(&body)
        );
    }

    // `@idempotencyToken`: the same token twice is one Trigger.
    let mut request = schedule_request("cron(0 7 * * ? *)");
    request["clientToken"] = json!("token-one");
    let (first_status, first) = create_trigger(&router, &space, request.clone()).await;
    assert_eq!(
        first_status, 201,
        "case replayed token: expected 201, got {first_status} with {first}"
    );
    let (second_status, second) = create_trigger(&router, &space, request).await;
    assert_eq!(
        second_status, 201,
        "case replayed token: expected 201, got {second_status} with {second}"
    );
    assert_eq!(
        second["trigger"], first["trigger"],
        "case replayed token: a repeat must replay the first Trigger"
    );

    // A create against an agent space that does not exist stores nothing, and fails
    // closed: a Trigger lives inside the space that owns it, so the caller is
    // authorised against that space before anything inside it can be known.
    let absent_space = "as-ffffffffffffffff";
    let (status, body) =
        create_trigger(&router, absent_space, schedule_request("rate(1 hour)")).await;
    assert_eq!(
        status, 403,
        "case absent space: expected 403, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "case absent space: expected AccessDeniedException, got {body}"
    );
    assert_eq!(
        error_message(&body),
        "Access denied",
        "case absent space: unexpected refusal message"
    );
}
