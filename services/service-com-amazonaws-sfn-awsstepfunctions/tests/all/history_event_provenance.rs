// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an execution's history REPORTS about the run it
//! belongs to, driven over this service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the
//! production `MultiServiceRouter`, so what is asserted is what a signed AWS SDK
//! request observes.
//!
//! The rules under test:
//!
//! 1. the `ExecutionStarted` event reports the execution role the run assumed —
//!    the `roleArn` of the state machine configuration the start PINNED, which an
//!    update to the state machine afterwards does not move;
//! 2. `previousEventId` names the event before it, except that the event written
//!    after an execution's own `ExecutionStarted` has no predecessor and reports
//!    the sentinel `0`, exactly as the start's own event does;
//! 3. a document the CALLER supplied is echoed back in the caller's own bytes
//!    wherever the history reports it — their whitespace and member order
//!    included — while a document the DEFINITION built is reported in the
//!    encoding the walk produced;
//! 4. a history page token is a CURSOR into this execution's own event order, and
//!    it serves only the read it was handed to: a cursor minted for another
//!    execution, the other sort order or another page size — or one this history
//!    never handed out at all — is the modelled `InvalidToken` (HTTP 400, message
//!    `Invalid Token`);
//! 5. an execution that ends through a `Fail` state ends its history with the
//!    `ExecutionFailed` event, carrying the `error` and `cause` that state
//!    declared;
//! 6. every element of `events` carries the awsJson type discriminator of the SWF
//!    base model's `HistoryEvent`, on every page of every read.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `string`) is the
//! one `state_machine_lifecycle` builds, the history reader is
//! `execution_history`'s and the refusal reader is `error_messages`'; `tests/all.rs`
//! is this binary's crate root, so each is reached through `crate::<suite>`.

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::service::wire;

use crate::error_messages::failure;
use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
    PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN,
};

/// A definition whose one `Pass` state carries its input straight through, so what
/// the state was entered with and what it emitted are both the caller's own
/// document.
const ECHO_DEFINITION: &str =
    r#"{"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}}"#;

/// A definition that ends at a `Fail` state after one `Pass`.
const PASS_THEN_FAIL: &str = r#"{
    "StartAt": "First",
    "States": {
        "First": { "Type": "Pass", "Next": "Stop" },
        "Stop": { "Type": "Fail", "Error": "CustomError", "Cause": "Something went wrong" }
    }
}"#;

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

/// The string at `pointer` inside `event`, which must be there.
fn at(event: &Value, pointer: &str, case: &str) -> String {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {pointer} of {event:?}"))
        .to_string()
}

/// Start one execution of `definition` under `role_arn` and return its history once
/// the clock has made the outcome due.
async fn ran(
    emulator: &Emulator,
    name: &str,
    definition: &str,
    input: &str,
) -> (String, Vec<Value>) {
    let machine = create_machine(emulator, name, definition).await;
    let execution = start_execution(emulator, &machine, "only", input).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let events = history(emulator, &execution, json!({})).await;
    (execution, events)
}

// ---------------------------------------------------------------------------
// Rule 1: the role the run assumed
// ---------------------------------------------------------------------------

/// The `ExecutionStarted` event reports the `roleArn` of the configuration the start
/// pinned, whichever ARN the execution was started with.
///
/// The rows differ along what the rule generalises over: the role itself (several
/// ARNs, including one at a path and one at the modelled maximum length), and HOW the
/// execution was started — by the state machine's own ARN, by a published version's
/// ARN and through an alias.
#[tokio::test]
async fn the_execution_started_event_reports_the_role_the_run_assumed() {
    let emulator = Emulator::new();

    // A role ARN is `@length 1..256` (`smithy_query(query="shape",
    // shape="ExecutionStartedEventDetails")` -> roleArn: Arn, len 1..256).
    let long_role = format!(
        "arn:aws:iam::000000000000:role/{}",
        "r".repeat(256 - "arn:aws:iam::000000000000:role/".len())
    );
    let roles: Vec<(&str, String)> = vec![
        ("the harness role", ROLE_ARN.to_string()),
        (
            "a role at a path",
            "arn:aws:iam::000000000000:role/service-role/sfn/Runner".to_string(),
        ),
        ("a role ARN at the modelled maximum", long_role),
    ];

    for (index, (case, role_arn)) in roles.iter().enumerate() {
        let created = emulator
            .ok(
                "CreateStateMachine",
                &json!({
                    "name": format!("role-in-history-{index}"),
                    "definition": ECHO_DEFINITION,
                    "roleArn": role_arn,
                    "publish": true,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let version = string(&created, "stateMachineVersionArn");
        let alias = string(
            &emulator
                .ok(
                    "CreateStateMachineAlias",
                    &json!({
                        "name": format!("live{index}"),
                        "routingConfiguration": [
                            { "stateMachineVersionArn": version, "weight": 100 }
                        ],
                    }),
                )
                .await,
            "stateMachineAliasArn",
        );

        for (started_by, arn) in [
            ("the state machine's own ARN", &machine),
            ("a published version's ARN", &version),
            ("an alias", &alias),
        ] {
            let case = format!("{case} started by {started_by}");
            // One execution NAME per start: the three ARNs resolve to the same state
            // machine, so a repeated name would be the same execution.
            let name = format!("run-{}", started_by.len());
            let execution = start_execution(&emulator, arn, &name, "{}").await;
            std::thread::sleep(PAST_THE_EXECUTION_DELAY);
            let events = history(&emulator, &execution, json!({})).await;
            let started = only(&events, &case, "ExecutionStarted");
            assert_eq!(
                at(started, "/executionStartedEventDetails/roleArn", &case),
                *role_arn,
                "case {case:?}: expected the role the run assumed, got {started:?}"
            );
        }
    }
}

/// An execution is PINNED to the configuration it started with — "all currently
/// running executions will continue to use the previous definition and roleArn" — so
/// its history keeps reporting the role it really ran under after the state machine
/// has been given another one.
#[tokio::test]
async fn the_history_keeps_the_role_the_run_was_pinned_to() {
    let emulator = Emulator::new();
    let case = "a state machine given another role after the run";
    let machine = create_machine(&emulator, "role-pinned", ECHO_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "only", "{}").await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let moved_on = "arn:aws:iam::000000000000:role/SomeOtherRole";
    emulator
        .ok(
            "UpdateStateMachine",
            &json!({ "stateMachineArn": machine, "roleArn": moved_on }),
        )
        .await;

    let events = history(&emulator, &execution, json!({})).await;
    let started = only(&events, case, "ExecutionStarted");
    assert_eq!(
        at(started, "/executionStartedEventDetails/roleArn", case),
        ROLE_ARN,
        "case {case:?}: expected the role the run was pinned to, got {started:?}"
    );
    // The positive control: an execution started AFTER the update runs under the new
    // role, so the member is read from the record and not from a constant.
    let later = start_execution(&emulator, &machine, "later", "{}").await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let later_events = history(&emulator, &later, json!({})).await;
    let later_started = only(&later_events, case, "ExecutionStarted");
    assert_eq!(
        at(
            later_started,
            "/executionStartedEventDetails/roleArn",
            case
        ),
        moved_on,
        "case {case:?}: expected the role the later run assumed, got {later_started:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: what each event names as the one before it
// ---------------------------------------------------------------------------

/// `previousEventId` is the id of the event before, and the sentinel `0` on the
/// `ExecutionStarted` event and on the event written after it.
///
/// The rows differ along what the rule generalises over: the number of events the run
/// writes (two, four and more), the OUTCOME it reaches (succeeded, failed, timed out),
/// how many states it walks, and the ORDER the page is read in.
#[tokio::test]
async fn the_event_after_the_start_has_no_previous_event() {
    let emulator = Emulator::new();

    // (case, definition, input)
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a run of one Pass state",
            ECHO_DEFINITION.to_string(),
            "{}",
        ),
        (
            "a run of three states",
            json!({
                "StartAt": "One",
                "States": {
                    "One": { "Type": "Pass", "Next": "Two" },
                    "Two": { "Type": "Pass", "Next": "Three" },
                    "Three": { "Type": "Pass", "End": true }
                }
            })
            .to_string(),
            "{}",
        ),
        (
            "a run that ends at a Fail state",
            PASS_THEN_FAIL.to_string(),
            "{}",
        ),
        (
            "a run that ends at a Succeed state after a Choice",
            json!({
                "StartAt": "Which",
                "States": {
                    "Which": {
                        "Type": "Choice",
                        "Choices": [{ "Variable": "$.n", "NumericEquals": 1, "Next": "Done" }],
                        "Default": "Done"
                    },
                    "Done": { "Type": "Succeed" }
                }
            })
            .to_string(),
            r#"{"n":1}"#,
        ),
        (
            "a run of a Map state over two items",
            json!({
                "StartAt": "Each",
                "States": {
                    "Each": {
                        "Type": "Map",
                        "ItemsPath": "$.items",
                        "ItemProcessor": {
                            "StartAt": "One",
                            "States": { "One": { "Type": "Pass", "End": true } }
                        },
                        "End": true
                    }
                }
            })
            .to_string(),
            r#"{"items":[1,2]}"#,
        ),
    ];

    for (index, (case, definition, input)) in rows.iter().enumerate() {
        let (execution, events) =
            ran(&emulator, &format!("chained-{index}"), definition, input).await;
        assert!(
            events.len() >= 2,
            "case {case:?}: the rule is only pinned by a history of at least two events, got \
             {:?}",
            types(&events)
        );
        for (position, event) in events.iter().enumerate() {
            let id = event.get("id").and_then(Value::as_i64);
            let previous = event.get("previousEventId").and_then(Value::as_i64);
            let expected_id = i64::try_from(position + 1).expect("the position fits");
            // Event 1 is the start, which has no predecessor; event 2 follows the
            // start, which is the execution beginning rather than a step of its walk,
            // so it has none either.
            let expected_previous = if expected_id <= 2 { 0 } else { expected_id - 1 };
            assert_eq!(
                (id, previous),
                (Some(expected_id), Some(expected_previous)),
                "case {case:?}: expected event {expected_id} to report previousEventId \
                 {expected_previous}, got {event:?}"
            );
        }
        // "Lists events in descending order of their timeStamp": the same events with
        // the same ids and predecessors, read from the end.
        let reversed = history(&emulator, &execution, json!({ "reverseOrder": true })).await;
        let mut forwards: Vec<(Option<i64>, Option<i64>)> = events
            .iter()
            .map(|event| {
                (
                    event.get("id").and_then(Value::as_i64),
                    event.get("previousEventId").and_then(Value::as_i64),
                )
            })
            .collect();
        forwards.reverse();
        let backwards: Vec<(Option<i64>, Option<i64>)> = reversed
            .iter()
            .map(|event| {
                (
                    event.get("id").and_then(Value::as_i64),
                    event.get("previousEventId").and_then(Value::as_i64),
                )
            })
            .collect();
        assert_eq!(
            backwards, forwards,
            "case {case:?}: expected the reverse order to carry the same chain, got {reversed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the caller's own bytes
// ---------------------------------------------------------------------------

/// Every place the history echoes the document the caller supplied, it echoes the
/// bytes the caller wrote.
///
/// The rows differ along what the rule generalises over: the WHITESPACE the caller
/// wrote (none, a space after each colon, spaces inside the braces, newlines and
/// tabs), the member ORDER (not the serializer's), and the JSON TYPE of the payload
/// (an object, a nested object, an array, a string, a number and a boolean).
#[tokio::test]
async fn the_history_echoes_the_payload_the_caller_wrote() {
    let emulator = Emulator::new();

    // (case, the exact bytes the caller sends)
    let payloads: Vec<(&str, &str)> = vec![
        ("a compact object", r#"{"a":1}"#),
        ("a space after the colon", r#"{"a": 1}"#),
        ("spaces inside the braces", r#"{ "a" : 1 }"#),
        (
            "members in an order of the caller's own",
            r#"{"z": 1, "a": 2}"#,
        ),
        (
            "a nested document written over several lines",
            "{\n\t\"order\": \"o-1\",\n\t\"items\": [1, 2, 3]\n}",
        ),
        ("an array", r#"[1, 2, 3]"#),
        ("a string", r#""plain""#),
        ("a number", r#"42"#),
        ("a boolean", r#"true"#),
        ("an empty object with a space", r#"{ }"#),
    ];

    for (index, (case, payload)) in payloads.iter().enumerate() {
        let (execution, events) = ran(
            &emulator,
            &format!("echoed-{index}"),
            ECHO_DEFINITION,
            payload,
        )
        .await;
        // The start's own event, the state's two lifecycle events and the terminal
        // event all report the SAME document — the one the caller wrote.
        for (event_type, pointer) in [
            ("ExecutionStarted", "/executionStartedEventDetails/input"),
            ("PassStateEntered", "/stateEnteredEventDetails/input"),
            ("PassStateExited", "/stateExitedEventDetails/output"),
            ("ExecutionSucceeded", "/executionSucceededEventDetails/output"),
        ] {
            let event = only(&events, case, event_type);
            assert_eq!(
                at(event, pointer, case),
                *payload,
                "case {case:?}: expected {event_type}{pointer} to echo the caller's own bytes, \
                 got {event:?}"
            );
        }
        // Read back through the execution record itself, which is the other side of
        // the same rule.
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            (
                described.get("input").and_then(Value::as_str),
                described.get("output").and_then(Value::as_str),
            ),
            (Some(*payload), Some(*payload)),
            "case {case:?}: expected the record to echo the caller's own bytes, got {described:?}"
        );
    }
}

/// The boundary of the same rule: a document the DEFINITION built is reported in the
/// encoding the walk produced it in, not in any caller's spacing.
#[tokio::test]
async fn a_document_the_definition_built_is_reported_as_built() {
    let emulator = Emulator::new();
    let case = "a Pass state that declares its own Result";
    let definition = json!({
        "StartAt": "Only",
        "States": {
            "Only": { "Type": "Pass", "Result": { "ok": true, "n": 1 }, "End": true }
        }
    })
    .to_string();
    let (_, events) = ran(&emulator, "built-result", &definition, r#"{"a": 1}"#).await;

    // Entered WITH the caller's document, so that is echoed...
    let entered = only(&events, case, "PassStateEntered");
    assert_eq!(
        at(entered, "/stateEnteredEventDetails/input", case),
        r#"{"a": 1}"#,
        "case {case:?}: expected the caller's own bytes on the way in, got {entered:?}"
    );
    // ...and the document the state BUILT is reported as the walk built it.
    let exited = only(&events, case, "PassStateExited");
    assert_eq!(
        at(exited, "/stateExitedEventDetails/output", case),
        serde_json::to_string(&json!({ "ok": true, "n": 1 })).expect("the result serializes"),
        "case {case:?}: expected the built document as the walk encoded it, got {exited:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a page token serves only the read it was issued for
// ---------------------------------------------------------------------------

/// One page of a history read, as `(events, nextToken)`.
async fn page(emulator: &Emulator, request: &Value) -> (Vec<Value>, Option<String>) {
    let answered = emulator.ok("GetExecutionHistory", request).await;
    let events = answered
        .get("events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an events list in {answered:?}"))
        .clone();
    let token = answered
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (events, token)
}

/// A token this read issued walks the rest of that read, and the pages concatenate
/// to exactly the history that read answers.
#[tokio::test]
async fn a_token_the_read_issued_walks_the_rest_of_it() {
    let emulator = Emulator::new();
    let (execution, whole) = ran(&emulator, "token-walk", PASS_THEN_FAIL, "{}").await;

    for reverse in [false, true] {
        for size in [1_i32, 2, 3] {
            let case = format!("reverseOrder {reverse} in pages of {size}");
            let expected: Vec<String> = if reverse {
                let mut reversed = types(&whole);
                reversed.reverse();
                reversed
            } else {
                types(&whole)
            };
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut request = json!({
                    "executionArn": execution.clone(),
                    "maxResults": size,
                    "reverseOrder": reverse,
                });
                if let Some(token) = token.as_deref() {
                    request["nextToken"] = json!(token);
                }
                let (events, next) = page(&emulator, &request).await;
                assert!(
                    events.len() <= size as usize,
                    "case {case:?}: expected at most {size} events a page, got {}",
                    events.len()
                );
                walked.extend(types(&events));
                match next {
                    Some(next) => token = Some(next),
                    None => break,
                }
                assert!(
                    walked.len() <= whole.len(),
                    "case {case:?}: expected the pages to end, got {walked:?}"
                );
            }
            assert_eq!(
                walked, expected,
                "case {case:?}: expected the pages to concatenate to that read's history"
            );
        }
    }
}

/// A `nextToken` that is not one THIS read was handed is the modelled
/// `InvalidToken`, worded as the history words it.
///
/// The rows differ along what the rule generalises over: a cursor of the OTHER sort
/// order (both ways round), a cursor of ANOTHER execution, a cursor of a read with
/// another page size, the first position (which no token names), a position past the
/// end of this history, text that is no position at all, and a token at the modelled
/// `PageToken` maximum of 1024.
#[tokio::test]
async fn a_cursor_another_read_was_handed_is_refused() {
    let emulator = Emulator::new();
    let (execution, whole) = ran(&emulator, "token-bound", PASS_THEN_FAIL, "{}").await;
    let (other, _) = ran(&emulator, "token-bound-other", PASS_THEN_FAIL, "{}").await;

    let issued = |arn: String, reverse: bool, size: i32| {
        let emulator = &emulator;
        async move {
            let (_, token) = page(
                emulator,
                &json!({ "executionArn": arn, "maxResults": size, "reverseOrder": reverse }),
            )
            .await;
            token.unwrap_or_else(|| panic!("expected a token while events remain"))
        }
    };
    // This execution is handed one cursor of each order, in pages of one; the other
    // execution is handed one in pages of two, which this one was never handed.
    let forwards = issued(execution.clone(), false, 1).await;
    let backwards = issued(execution.clone(), true, 1).await;
    let elsewhere = issued(other.clone(), false, 2).await;
    assert_ne!(
        forwards, backwards,
        "the two orders continue from different positions in the same history: {forwards} and \
         {backwards}"
    );

    // The control: the cursor this read was handed IS served, so the rows below are
    // refused for the read they belong to and not because every token is refused.
    let (served, _) = page(
        &emulator,
        &json!({
            "executionArn": execution.clone(),
            "maxResults": 1,
            "nextToken": forwards.clone(),
        }),
    )
    .await;
    assert_eq!(
        served.len(),
        1,
        "expected the cursor this read was handed to serve its next page, got {served:?}"
    );

    // (case, the request the cursor is replayed on)
    let rows: Vec<(String, Value)> = vec![
        (
            "a cursor of the backwards read, replayed forwards".to_string(),
            json!({
                "executionArn": execution.clone(),
                "maxResults": 1,
                "nextToken": backwards.clone(),
            }),
        ),
        (
            "a cursor of the forwards read, replayed backwards".to_string(),
            json!({
                "executionArn": execution.clone(),
                "maxResults": 1,
                "reverseOrder": true,
                "nextToken": forwards.clone(),
            }),
        ),
        (
            "a cursor of another execution".to_string(),
            json!({
                "executionArn": execution.clone(),
                "maxResults": 2,
                "nextToken": elsewhere.clone(),
            }),
        ),
        (
            "a cursor of a read with another page size".to_string(),
            json!({
                "executionArn": execution.clone(),
                "maxResults": 2,
                "nextToken": forwards.clone(),
            }),
        ),
        (
            "the first position, which no token names".to_string(),
            json!({ "executionArn": execution.clone(), "maxResults": 1, "nextToken": "0" }),
        ),
        (
            "a position past the end of this history".to_string(),
            json!({
                "executionArn": execution.clone(),
                "maxResults": 1,
                "nextToken": (whole.len() + 1).to_string(),
            }),
        ),
        (
            "text that is no position at all".to_string(),
            json!({ "executionArn": execution.clone(), "nextToken": "not-a-token" }),
        ),
        (
            "a token at the modelled maximum".to_string(),
            json!({ "executionArn": execution.clone(), "nextToken": "z".repeat(1024) }),
        ),
    ];

    for (case, request) in rows {
        let (status, code, message) =
            failure(&emulator, &case, "GetExecutionHistory", &request).await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "InvalidToken", "Invalid Token"),
            "case {case:?}: expected 400 InvalidToken with the condition alone, got {status} \
             {code} with {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the event an execution that failed ends on
// ---------------------------------------------------------------------------

/// An execution that ends through a `Fail` state ends its history with
/// `ExecutionFailed`, carrying the error and cause that state declared.
///
/// The rows differ along what the rule generalises over: where the `Fail` state sits
/// (the definition's `StartAt`, after a `Pass`, behind a `Choice`), whether its
/// `Error`/`Cause` are literals or read from the state's input through a `*Path`, and
/// whether it declares a cause at all.
#[tokio::test]
async fn an_execution_that_ends_at_a_fail_state_ends_its_history_with_the_failure() {
    let emulator = Emulator::new();

    // (case, definition, input, expected error, expected cause)
    let rows: Vec<(&str, String, &str, &str, Option<&str>)> = vec![
        (
            "a Fail state that is the definition's StartAt",
            json!({
                "StartAt": "Stop",
                "States": {
                    "Stop": { "Type": "Fail", "Error": "Immediate", "Cause": "no work at all" }
                }
            })
            .to_string(),
            "{}",
            "Immediate",
            Some("no work at all"),
        ),
        (
            "a Fail state after a Pass",
            PASS_THEN_FAIL.to_string(),
            "{}",
            "CustomError",
            Some("Something went wrong"),
        ),
        (
            "a Fail state behind a Choice",
            json!({
                "StartAt": "Which",
                "States": {
                    "Which": {
                        "Type": "Choice",
                        "Choices": [{ "Variable": "$.go", "BooleanEquals": true, "Next": "Stop" }],
                        "Default": "Stop"
                    },
                    "Stop": { "Type": "Fail", "Error": "Chosen", "Cause": "the branch said so" }
                }
            })
            .to_string(),
            r#"{"go": true}"#,
            "Chosen",
            Some("the branch said so"),
        ),
        (
            "a Fail state whose error and cause come from its input",
            json!({
                "StartAt": "Stop",
                "States": {
                    "Stop": {
                        "Type": "Fail",
                        "ErrorPath": "$.error",
                        "CausePath": "$.cause"
                    }
                }
            })
            .to_string(),
            r#"{"error": "FromInput", "cause": "read off the document"}"#,
            "FromInput",
            Some("read off the document"),
        ),
        (
            "a Fail state that declares no cause",
            json!({
                "StartAt": "Stop",
                "States": { "Stop": { "Type": "Fail", "Error": "BareError" } }
            })
            .to_string(),
            "{}",
            "BareError",
            None,
        ),
    ];

    for (index, (case, definition, input, error, cause)) in rows.iter().enumerate() {
        let (execution, events) =
            ran(&emulator, &format!("failing-{index}"), definition, input).await;
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected the execution to have failed, got {described:?}"
        );
        // The LAST event is the execution's own failure, not the state that reached
        // it: the terminal event is where "the documented place to read why an
        // execution failed" is.
        let ordered = types(&events);
        assert_eq!(
            ordered.last().map(String::as_str),
            Some("ExecutionFailed"),
            "case {case:?}: expected the history to end with ExecutionFailed, got {ordered:?}"
        );
        assert_eq!(
            ordered.iter().filter(|event| *event == "FailStateEntered").count(),
            1,
            "case {case:?}: expected the Fail state's own entered event before it, got \
             {ordered:?}"
        );
        let failed = only(&events, case, "ExecutionFailed");
        assert_eq!(
            failed
                .pointer("/executionFailedEventDetails/error")
                .and_then(Value::as_str),
            Some(*error),
            "case {case:?}: expected the Fail state's error, got {failed:?}"
        );
        assert_eq!(
            failed
                .pointer("/executionFailedEventDetails/cause")
                .and_then(Value::as_str),
            *cause,
            "case {case:?}: expected the Fail state's cause, got {failed:?}"
        );
        // And the same values the record reports, so the history and the record
        // cannot disagree about why the run failed.
        assert_eq!(
            (
                described.get("error").and_then(Value::as_str),
                described.get("cause").and_then(Value::as_str),
            ),
            (Some(*error), *cause),
            "case {case:?}: expected the record to report the same failure, got {described:?}"
        );
    }
}

/// A REDRIVEN attempt continues the failed execution from the step it stopped at,
/// and that step's entered event is already in the history the redrive preserves —
/// so the attempt appends the outcome it reaches, not a second entry for that state.
///
/// The rows differ along what the rule generalises over: where the step the redrive
/// continues from sits (the definition's `StartAt`, after a `Pass`, behind a
/// `Choice`), and therefore how many events the preserved attempt left.
#[tokio::test]
async fn a_redriven_attempt_does_not_re_enter_the_step_it_continues_from() {
    let emulator = Emulator::new();

    // (case, definition, the error and cause its Fail state declares)
    let rows: Vec<(&str, String, &str, &str)> = vec![
        (
            "a Fail state that is the definition's StartAt",
            json!({
                "StartAt": "Stop",
                "States": {
                    "Stop": { "Type": "Fail", "Error": "CustomError", "Cause": "went wrong" }
                }
            })
            .to_string(),
            "CustomError",
            "went wrong",
        ),
        (
            "a Fail state after a Pass",
            PASS_THEN_FAIL.to_string(),
            "CustomError",
            "Something went wrong",
        ),
        (
            "a Fail state behind a Choice",
            json!({
                "StartAt": "Which",
                "States": {
                    "Which": {
                        "Type": "Choice",
                        "Choices": [{ "Variable": "$.n", "NumericEquals": 1, "Next": "Stop" }],
                        "Default": "Stop"
                    },
                    "Stop": { "Type": "Fail", "Error": "Chosen", "Cause": "the branch said so" }
                }
            })
            .to_string(),
            "Chosen",
            "the branch said so",
        ),
    ];

    for (index, (case, definition, error, cause)) in rows.iter().enumerate() {
        let (execution, first) =
            ran(&emulator, &format!("redriven-{index}"), definition, r#"{"n":1}"#).await;
        let before = types(&first);
        emulator
            .ok(
                "RedriveExecution",
                &json!({ "executionArn": execution.clone() }),
            )
            .await;
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);

        let events = history(&emulator, &execution, json!({})).await;
        let after = types(&events);
        // "Redriven executions append their event history to the existing event
        // history": what the first attempt wrote is still there, unchanged...
        assert_eq!(
            after.get(..before.len()).map(<[String]>::to_vec),
            Some(before.clone()),
            "case {case:?}: expected the preserved attempt's events first, got {after:?}"
        );
        // ...and what the redrive appended is its own opening event and the outcome
        // the step it continued from reached — no second entry for that state.
        assert_eq!(
            after.get(before.len()..).map(<[String]>::to_vec),
            Some(vec![
                "ExecutionRedriven".to_string(),
                "ExecutionFailed".to_string(),
            ]),
            "case {case:?}: expected the redrive to append its opening event and the outcome, \
             got {after:?}"
        );
        // The numbering runs on through the appended events, each naming the one
        // before it.
        for (position, event) in events.iter().enumerate() {
            let id = i64::try_from(position + 1).expect("the position fits");
            let previous = if id <= 2 { 0 } else { id - 1 };
            assert_eq!(
                (
                    event.get("id").and_then(Value::as_i64),
                    event.get("previousEventId").and_then(Value::as_i64),
                ),
                (Some(id), Some(previous)),
                "case {case:?}: expected event {id} to follow {previous}, got {event:?}"
            );
        }
        // The attempt's own terminal event carries the failure again, so a reader of
        // the redriven history finds why THIS attempt failed.
        let last = events.last().unwrap_or_else(|| panic!("case {case:?}: expected events"));
        assert_eq!(
            (
                last.pointer("/executionFailedEventDetails/error")
                    .and_then(Value::as_str),
                last.pointer("/executionFailedEventDetails/cause")
                    .and_then(Value::as_str),
            ),
            (Some(*error), Some(*cause)),
            "case {case:?}: expected the redriven attempt's own failure, got {last:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6: the discriminator every history event carries
// ---------------------------------------------------------------------------

/// Every element of `events` carries `__type` naming the SWF base model's
/// `HistoryEvent`, beside that event's own members — on every page of every read.
///
/// The rows differ along what the rule generalises over: the page SIZE (one event a
/// page, two, and the default that holds the whole history), the sort ORDER, and
/// whether the read asked for execution data — and the rule is asserted on every
/// event of every page, whatever that event's own type and detail member.
#[tokio::test]
async fn every_history_event_carries_the_history_event_discriminator() {
    let emulator = Emulator::new();
    let (execution, whole) = ran(
        &emulator,
        "discriminated-history",
        PASS_THEN_FAIL,
        r#"{"a": 1}"#,
    )
    .await;
    let expected = wire::discriminator("HistoryEvent");
    assert_eq!(
        expected, "com.amazonaws.swf.base.model#HistoryEvent",
        "the discriminator is the shape id in the SWF base-model namespace, got {expected:?}"
    );

    // (case, the members the read carries beside the execution ARN)
    let reads: Vec<(String, Value)> = vec![
        ("the whole history".to_string(), json!({})),
        (
            "the whole history without execution data".to_string(),
            json!({ "includeExecutionData": false }),
        ),
        (
            "the whole history in reverse".to_string(),
            json!({ "reverseOrder": true }),
        ),
        ("pages of one".to_string(), json!({ "maxResults": 1 })),
        ("pages of two".to_string(), json!({ "maxResults": 2 })),
        (
            "pages of two in reverse".to_string(),
            json!({ "maxResults": 2, "reverseOrder": true }),
        ),
    ];

    for (case, extra) in reads {
        let mut seen = 0;
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "executionArn": execution.clone() });
            let members = request.as_object_mut().expect("the request is an object");
            for (member, value) in extra.as_object().expect("the extras are an object") {
                members.insert(member.clone(), value.clone());
            }
            if let Some(token) = token.as_deref() {
                members.insert("nextToken".to_string(), json!(token));
            }
            let (events, next) = page(&emulator, &request).await;
            for event in &events {
                let object = event
                    .as_object()
                    .unwrap_or_else(|| panic!("case {case:?}: expected an event object"));
                assert_eq!(
                    object.get("__type").and_then(Value::as_str),
                    Some(expected.as_str()),
                    "case {case:?}: expected every event to carry __type {expected:?}, got \
                     {event:?}"
                );
                // The discriminator stands BESIDE the event's own members, which the
                // reader needs: its identity and the detail its type names.
                assert!(
                    object.contains_key("id")
                        && object.contains_key("type")
                        && object.contains_key("timestamp"),
                    "case {case:?}: the discriminator never displaces the event's own \
                     members, got {event:?}"
                );
                seen += 1;
            }
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen,
            whole.len(),
            "case {case:?}: expected every one of the {} events to be covered, got {seen}",
            whole.len()
        );
    }
}

/// A walk DEEPER than the cursors one execution remembers still completes, and two
/// callers walking the same read interleave without displacing each other: one entry
/// per read holds how far that read has got, so a cursor stays good however many
/// other pages are read in between.
#[tokio::test]
async fn interleaved_walks_of_one_read_do_not_displace_each_other() {
    use service_com_amazonaws_sfn_awsstepfunctions::state::MAX_HISTORY_PAGE_CURSORS;

    let emulator = Emulator::new();
    // A chain long enough that walking it one event a page needs more cursors than
    // an execution remembers reads.
    let states: serde_json::Map<String, Value> = (0..MAX_HISTORY_PAGE_CURSORS)
        .map(|index| {
            let state = if index + 1 == MAX_HISTORY_PAGE_CURSORS {
                json!({ "Type": "Pass", "End": true })
            } else {
                json!({ "Type": "Pass", "Next": format!("S{}", index + 1) })
            };
            (format!("S{index}"), state)
        })
        .collect();
    let definition = json!({ "StartAt": "S0", "States": states }).to_string();
    let (execution, whole) = ran(&emulator, "token-interleaved", &definition, "{}").await;
    assert!(
        whole.len() > MAX_HISTORY_PAGE_CURSORS,
        "the rule is only pinned by a history longer than the {MAX_HISTORY_PAGE_CURSORS} reads \
         one execution remembers, got {} events",
        whole.len()
    );

    // Two walkers of the SAME read (forwards, one event a page), each taking a page
    // in turn.
    let mut walked: [Vec<String>; 2] = [Vec::new(), Vec::new()];
    let mut tokens: [Option<String>; 2] = [None, None];
    let mut done = [false, false];
    while !done[0] || !done[1] {
        for walker in 0..2 {
            if done[walker] {
                continue;
            }
            let mut request = json!({ "executionArn": execution.clone(), "maxResults": 1 });
            if let Some(token) = tokens[walker].as_deref() {
                request["nextToken"] = json!(token);
            }
            let (events, next) = page(&emulator, &request).await;
            walked[walker].extend(types(&events));
            match next {
                Some(next) => tokens[walker] = Some(next),
                None => done[walker] = true,
            }
        }
    }
    for (walker, seen) in walked.iter().enumerate() {
        assert_eq!(
            *seen,
            types(&whole),
            "case {walker}: expected the interleaved walk to report the whole history"
        );
    }
}

/// The ARNs above are this endpoint's own, which is what makes the reads here reach
/// the records the starts wrote.
#[tokio::test]
async fn the_executions_read_here_are_this_endpoints_own() {
    let emulator = Emulator::new();
    let (execution, _) = ran(&emulator, "region-check", ECHO_DEFINITION, "{}").await;
    assert!(
        execution.starts_with(&format!("arn:aws:states:{REGION}:")),
        "expected an execution ARN in this endpoint's region, got {execution:?}"
    );
}
