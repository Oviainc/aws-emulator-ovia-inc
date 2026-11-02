// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetExecutionHistory`, driven over this service's own wire
//! protocol (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`)
//! through the production `MultiServiceRouter`.
//!
//! The rules under test:
//!
//! 1. the history is the events the execution REALLY wrote — the walk's state
//!    transitions, the scheduling, claim and closure of each activity task, and the
//!    outcome the clock made due — numbered from 1 with each event naming the one
//!    before it, "in ascending order of the timeStamp of the events", and
//!    `reverseOrder` is that same list read from the end;
//! 2. an event's detail member carries what the call that wrote the event knew: the
//!    execution's input on `ExecutionStarted`, the state's name on every
//!    `<Type>StateEntered`/`<Type>StateExited`, the activity, the input and the bounds
//!    on `ActivityScheduled`, the `workerName` on `ActivityStarted`, the worker's
//!    output on `ActivitySucceeded` and the error and cause on the terminal event —
//!    and `includeExecutionData: false` leaves the execution DATA out;
//! 3. a page is `maxResults` long (`0` meaning the default), `nextToken` walks the
//!    rest, and the pages of one history concatenate to exactly that history;
//! 4. the request is judged before the lookup: an ARN that is not an execution ARN of
//!    this service in this endpoint's region is `InvalidArn`, an ARN the store holds no
//!    execution under is `ExecutionDoesNotExist`, a `nextToken` this service did not
//!    issue is `InvalidToken`, and an EXPRESS execution — whose history "is not
//!    supported by EXPRESS state machines" — is `InvalidArn`.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `failure`) and the
//! activity helpers are the ones `state_machine_lifecycle`, `error_messages` and
//! `activity_tasks` build; `tests/all.rs` is this binary's crate root, so each is
//! reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    DEFAULT_PAGE_SIZE, EVENT_ACTIVITY_SCHEDULED, EVENT_ACTIVITY_STARTED, EVENT_ACTIVITY_SUCCEEDED,
    EVENT_EXECUTION_STARTED,
};

use crate::activity_tasks::{activity_definition, claimed, create_activity};
use crate::error_messages::failure;
use crate::state_machine_lifecycle::{
    create_machine, string, Emulator, ACCOUNT, PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN,
};

/// A definition whose one `Pass` state leaves a short, fixed history.
const PASS_DEFINITION: &str =
    r#"{"StartAt":"Only","States":{"Only":{"Type":"Pass","Result":{"ok":true},"End":true}}}"#;

/// The events of one execution, as `GetExecutionHistory` answers them.
///
/// `pub(crate)`, not private: a sibling suite that asserts what an execution's own
/// events show drives this one rather than carrying a second copy of it.
pub(crate) async fn history(emulator: &Emulator, execution_arn: &str, extra: Value) -> Vec<Value> {
    let mut request = json!({ "executionArn": execution_arn });
    let members = request.as_object_mut().expect("the request is an object");
    for (member, value) in extra.as_object().expect("the extras are an object") {
        members.insert(member.clone(), value.clone());
    }
    let answered = emulator.ok("GetExecutionHistory", &request).await;
    answered
        .get("events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an events list in {answered:?}"))
        .clone()
}

/// The `type` of each event, in the order they were answered.
fn types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| {
            event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("expected a type in {event:?}"))
                .to_string()
        })
        .collect()
}

/// The one event of `event_type`, which the history must carry exactly once.
fn only<'a>(events: &'a [Value], case: &str, event_type: &str) -> &'a Value {
    let matching: Vec<&Value> = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(event_type))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one {event_type} event, got {:?}",
        types(events)
    );
    matching[0]
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the events that happened, and what each one carries
// ---------------------------------------------------------------------------

/// The history of a walked execution is its own events, numbered and chained, with
/// each event's detail member carrying what the record holds.
#[tokio::test]
async fn the_history_is_the_events_the_execution_wrote() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "history-pass", PASS_DEFINITION).await;
    let execution = crate::state_machine_lifecycle::start_execution(
        &emulator,
        &machine,
        "only",
        r#"{"n":7}"#,
    )
    .await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let case = "a Pass state that succeeded";
    let events = history(&emulator, &execution, json!({})).await;
    assert_eq!(
        types(&events),
        vec![
            "ExecutionStarted",
            "PassStateEntered",
            "PassStateExited",
            "ExecutionSucceeded",
        ],
        "case {case:?}: expected the events the walk wrote, got {events:?}"
    );
    // Numbered from 1, each event naming the one before it — and the event written
    // after the start naming none, because the start is the execution beginning
    // rather than a step of its walk.
    for (index, event) in events.iter().enumerate() {
        let id = event.get("id").and_then(Value::as_i64);
        let previous = event.get("previousEventId").and_then(Value::as_i64);
        let expected = i64::try_from(index + 1).expect("the index fits");
        let expected_previous = if expected <= 2 { 0 } else { expected - 1 };
        assert_eq!(
            (id, previous),
            (Some(expected), Some(expected_previous)),
            "case {case:?}: expected event {expected} to follow {expected_previous}, got {event:?}"
        );
        assert!(
            event.get("timestamp").is_some(),
            "case {case:?}: expected a timestamp on {event:?}"
        );
    }
    // The detail members come from the record: the execution's input, and the state
    // each transition was written for.
    let started = only(&events, case, EVENT_EXECUTION_STARTED);
    assert_eq!(
        started
            .get("executionStartedEventDetails")
            .and_then(|details| details.get("input"))
            .and_then(Value::as_str),
        Some(r#"{"n":7}"#),
        "case {case:?}: expected the execution's own input, got {started:?}"
    );
    for (event_type, member) in [
        ("PassStateEntered", "stateEnteredEventDetails"),
        ("PassStateExited", "stateExitedEventDetails"),
    ] {
        let event = only(&events, case, event_type);
        assert_eq!(
            event
                .get(member)
                .and_then(|details| details.get("name"))
                .and_then(Value::as_str),
            Some("Only"),
            "case {case:?}: expected {member} to name the state, got {event:?}"
        );
    }
    let succeeded = only(&events, case, "ExecutionSucceeded");
    assert_eq!(
        succeeded
            .get("executionSucceededEventDetails")
            .and_then(|details| details.get("output"))
            .and_then(Value::as_str),
        Some(r#"{"ok":true}"#),
        "case {case:?}: expected the execution's own output, got {succeeded:?}"
    );

    // "Lists events in descending order of their timeStamp": the same events, read
    // from the end.
    let reversed = history(&emulator, &execution, json!({ "reverseOrder": true })).await;
    let mut forwards = types(&events);
    forwards.reverse();
    assert_eq!(
        types(&reversed),
        forwards,
        "case {case:?}: expected the reverse order to be the same events read from the end, got \
         {reversed:?}"
    );

    // "You can select whether execution data (input or output of a history event) is
    // returned": asked for without it, the data is left out and the events are not.
    let without = history(
        &emulator,
        &execution,
        json!({ "includeExecutionData": false }),
    )
    .await;
    assert_eq!(
        types(&without),
        types(&events),
        "case {case:?}: expected the same events without execution data, got {without:?}"
    );
    let started = only(&without, case, EVENT_EXECUTION_STARTED);
    assert!(
        started
            .get("executionStartedEventDetails")
            .and_then(|details| details.get("input"))
            .is_none_or(Value::is_null),
        "case {case:?}: expected no execution data, got {started:?}"
    );
}

/// An activity task's own events carry what the task carried: the activity it was
/// scheduled for with the input and the bounds, the worker that claimed it, and the
/// output the worker answered.
#[tokio::test]
async fn an_activity_tasks_events_carry_what_the_task_carried() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "history-activity").await;
    let definition = activity_definition(
        &activity,
        json!({ "TimeoutSeconds": 300, "HeartbeatSeconds": 60 }),
    );
    let machine = create_machine(&emulator, "history-activity-machine", &definition).await;
    let execution = crate::state_machine_lifecycle::start_execution(
        &emulator,
        &machine,
        "only",
        r#"{"n":1}"#,
    )
    .await;

    let case = "an activity task a worker claimed and answered";
    let (token, _) = claimed(&emulator, case, &activity, Some("worker-7")).await;
    emulator
        .ok(
            "SendTaskSuccess",
            &json!({ "taskToken": token, "output": r#"{"done":true}"# }),
        )
        .await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let events = history(&emulator, &execution, json!({})).await;
    let scheduled = only(&events, case, EVENT_ACTIVITY_SCHEDULED);
    let details = scheduled
        .get("activityScheduledEventDetails")
        .unwrap_or_else(|| panic!("case {case:?}: expected scheduling details in {scheduled:?}"));
    assert_eq!(
        details.get("resource").and_then(Value::as_str),
        Some(activity.as_str()),
        "case {case:?}: expected the activity the task was scheduled for, got {details:?}"
    );
    assert_eq!(
        details.get("input").and_then(Value::as_str),
        Some(r#"{"n":1}"#),
        "case {case:?}: expected the input the task was scheduled with, got {details:?}"
    );
    assert_eq!(
        (
            details.get("timeoutInSeconds").and_then(Value::as_i64),
            details.get("heartbeatInSeconds").and_then(Value::as_i64),
        ),
        (Some(300), Some(60)),
        "case {case:?}: expected the bounds the state declared, got {details:?}"
    );

    let started = only(&events, case, EVENT_ACTIVITY_STARTED);
    assert_eq!(
        started
            .get("activityStartedEventDetails")
            .and_then(|details| details.get("workerName"))
            .and_then(Value::as_str),
        Some("worker-7"),
        "case {case:?}: expected the worker that claimed the task, got {started:?}"
    );

    let answered = only(&events, case, EVENT_ACTIVITY_SUCCEEDED);
    assert_eq!(
        answered
            .get("activitySucceededEventDetails")
            .and_then(|details| details.get("output"))
            .and_then(Value::as_str),
        Some(r#"{"done":true}"#),
        "case {case:?}: expected the output the worker reported, got {answered:?}"
    );
}

/// A task the clock closed writes the failure it closed with, and the execution's
/// terminal event carries the same error and cause the record reports.
#[tokio::test]
async fn a_timed_out_task_writes_the_failure_it_closed_with() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "history-timeout").await;
    let definition = activity_definition(&activity, json!({ "TimeoutSeconds": 1 }));
    let machine = create_machine(&emulator, "history-timeout-machine", &definition).await;
    let execution =
        crate::state_machine_lifecycle::start_execution(&emulator, &machine, "only", "{}").await;

    let case = "an activity task the clock closed";
    claimed(&emulator, case, &activity, Some("worker-1")).await;
    std::thread::sleep(std::time::Duration::from_millis(1_300));

    let events = history(&emulator, &execution, json!({})).await;
    let timed_out = only(&events, case, "ActivityTimedOut");
    let details = timed_out
        .get("activityTimedOutEventDetails")
        .unwrap_or_else(|| panic!("case {case:?}: expected timeout details in {timed_out:?}"));
    assert_eq!(
        details.get("error").and_then(Value::as_str),
        Some("States.Timeout"),
        "case {case:?}: expected the language's own timeout error, got {details:?}"
    );
    assert!(
        details
            .get("cause")
            .and_then(Value::as_str)
            .is_some_and(|cause| cause.contains("TimeoutSeconds")),
        "case {case:?}: expected a cause naming the bound that ran out, got {details:?}"
    );
    let failed = only(&events, case, "ExecutionFailed");
    assert_eq!(
        failed
            .get("executionFailedEventDetails")
            .and_then(|details| details.get("error"))
            .and_then(Value::as_str),
        Some("States.Timeout"),
        "case {case:?}: expected the terminal event to carry the same error, got {failed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: paging
// ---------------------------------------------------------------------------

/// `maxResults` pages the history and `nextToken` walks the rest, with the pages of
/// one history concatenating to exactly that history.
///
/// Rows span page sizes smaller than, equal to and larger than the history, plus `0`,
/// which "uses the default".
#[tokio::test]
async fn the_history_is_paged_by_max_results_and_walked_by_next_token() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "history-paged", PASS_DEFINITION).await;
    let execution =
        crate::state_machine_lifecycle::start_execution(&emulator, &machine, "only", "{}").await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let whole = types(&history(&emulator, &execution, json!({})).await);
    assert_eq!(
        whole.len(),
        4,
        "expected the four events the walk wrote, got {whole:?}"
    );
    assert!(
        whole.len() < DEFAULT_PAGE_SIZE,
        "expected this history to fit inside one default page of {DEFAULT_PAGE_SIZE}, got {} \
         events",
        whole.len()
    );

    for size in [1_i32, 2, 3, 4, 7, 0] {
        let case = format!("maxResults {size}");
        let expected_page = if size <= 0 {
            whole.len()
        } else {
            (size as usize).min(whole.len())
        };
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut extra = json!({ "maxResults": size });
            if let Some(token) = token.as_deref() {
                extra
                    .as_object_mut()
                    .expect("the extras are an object")
                    .insert("nextToken".to_string(), json!(token));
            }
            let mut request = json!({ "executionArn": execution.clone() });
            let members = request.as_object_mut().expect("the request is an object");
            for (member, value) in extra.as_object().expect("the extras are an object") {
                members.insert(member.clone(), value.clone());
            }
            let answered = emulator.ok("GetExecutionHistory", &request).await;
            let page: Vec<Value> = answered
                .get("events")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: expected events in {answered:?}"))
                .clone();
            if pages == 0 {
                assert_eq!(
                    page.len(),
                    expected_page,
                    "case {case:?}: expected a first page of {expected_page}, got {}",
                    page.len()
                );
            }
            walked.extend(types(&page));
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: expected the pages to end, got {pages} of them"
            );
            match answered.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the pages to concatenate to the whole history, got {walked:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: what the request is refused for
// ---------------------------------------------------------------------------

/// A request this operation cannot serve is refused with that reason's own modelled
/// error — never with a 501 and never with an empty history.
#[tokio::test]
async fn get_execution_history_refuses_a_request_it_cannot_serve() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "history-refusals", PASS_DEFINITION).await;
    let execution =
        crate::state_machine_lifecycle::start_execution(&emulator, &machine, "only", "{}").await;

    // An EXPRESS execution keeps no history this action reads.
    let express = string(
        &emulator
            .ok(
                "CreateStateMachine",
                &json!({
                    "name": "history-express",
                    "definition": PASS_DEFINITION,
                    "roleArn": ROLE_ARN,
                    "type": "EXPRESS",
                }),
            )
            .await,
        "stateMachineArn",
    );
    let express_execution =
        crate::state_machine_lifecycle::start_execution(&emulator, &express, "only", "{}").await;

    // (case, the request body, the expected error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "an ARN that is not an ARN at all",
            json!({ "executionArn": "not-an-arn" }),
            "InvalidArn",
        ),
        (
            "an ARN of another service",
            json!({ "executionArn": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:queue") }),
            "InvalidArn",
        ),
        (
            "an execution ARN of another region",
            json!({
                "executionArn": format!("arn:aws:states:eu-west-1:{ACCOUNT}:execution:m:e"),
            }),
            "InvalidArn",
        ),
        (
            "an execution the store holds nothing under",
            json!({
                "executionArn": format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:history-refusals:absent"),
            }),
            "ExecutionDoesNotExist",
        ),
        (
            "a nextToken this service did not issue",
            json!({ "executionArn": execution.clone(), "nextToken": "not-a-token" }),
            "InvalidToken",
        ),
        (
            "an EXPRESS execution, whose history this action does not read",
            json!({ "executionArn": express_execution.clone() }),
            "InvalidArn",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, code, message) =
            failure(&emulator, case, "GetExecutionHistory", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} {code} with message \
             {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: expected a message saying what was refused, got {message:?}"
        );
    }
}
