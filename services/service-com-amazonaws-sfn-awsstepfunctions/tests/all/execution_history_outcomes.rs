// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `GetExecutionHistory` reports about the OUTCOME an
//! execution reached, driven over this service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! `tests/all/execution_history.rs` asserts the events of one attempt and how they
//! are paged; the rules here are the ones that span the whole life of an execution:
//!
//! 1. the event an execution's history ENDS on is the event of the outcome that
//!    execution reached, and its detail member reports what the attempt that WROTE
//!    the event ended with — the output of one that succeeded, the error and cause of
//!    one that failed or was stopped, the count of a redrive — with a value no writer
//!    had left unset rather than filled in. An execution's own `output`, `error` and
//!    `cause` are those of its LATEST attempt, so an event an earlier attempt wrote
//!    keeps reporting that attempt's values after a redrive has moved the record;
//! 2. "You can select whether execution data (input or output of a history event) is
//!    returned": `includeExecutionData: false` withholds the input and the output of
//!    an event, and nothing else — an error and a cause are not execution data;
//! 3. one page carries at most `maxResults` events of ONE order (ascending, or the
//!    descending order `reverseOrder` asks for), `nextToken` continues that same
//!    order, and the pages concatenate to exactly it — whatever page size the caller
//!    chooses, and `maxResults` outside the range the model declares is refused;
//! 4. the history is a REPORT: reading it, in any order and at any page size, changes
//!    nothing a later caller sees.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`)
//! is `state_machine_lifecycle`'s and the whole-history reader is
//! `execution_history`'s; `tests/all.rs` is this binary's crate root, so each is
//! reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::RUNTIME_ERROR;
use service_com_amazonaws_sfn_awsstepfunctions::state::DEFAULT_PAGE_SIZE;

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, Emulator, PAST_THE_EXECUTION_DELAY,
};

/// A definition whose one `Pass` state succeeds with a static result.
const SUCCEED_DEFINITION: &str =
    r#"{"StartAt":"Only","States":{"Only":{"Type":"Pass","Result":{"ok":true},"End":true}}}"#;

/// A definition whose one `Fail` state ends the execution with the error and cause it
/// declares.
const FAIL_DEFINITION: &str =
    r#"{"StartAt":"Bang","States":{"Bang":{"Type":"Fail","Error":"Boom","Cause":"on purpose"}}}"#;

/// A definition whose `Task` state names work this emulator makes no call for (a bare
/// resource ARN of a service whose resources are not work in themselves), so the
/// execution ends `FAILED` with ASL's own [`RUNTIME_ERROR`] and a cause naming what
/// was not run.
const UNRUNNABLE_DEFINITION: &str = r#"{"StartAt":"T","States":{"T":{"Type":"Task","Resource":"arn:aws:dynamodb:us-west-2:000000000000:table/absent","End":true}}}"#;

/// A definition that waits far longer than these tests run, so an execution of it is
/// still `RUNNING` when it is stopped.
const SLOW_DEFINITION: &str =
    r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":600,"Next":"Done"},"Done":{"Type":"Succeed"}}}"#;

/// A definition of four `Pass` states, which leaves a history long enough to page:
/// `ExecutionStarted`, a pair of events per state, and `ExecutionSucceeded`.
const CHAIN_DEFINITION: &str = r#"{"StartAt":"S1","States":{"S1":{"Type":"Pass","Next":"S2"},"S2":{"Type":"Pass","Next":"S3"},"S3":{"Type":"Pass","Next":"S4"},"S4":{"Type":"Pass","Result":{"ok":true},"End":true}}}"#;

/// The largest `maxResults` the model admits: `maxResults` targets `PageSize`, an
/// `integer` with `range` 0..1000
/// (`smithy_query(query="operation",operation="GetExecutionHistory")`).
const MAX_PAGE_SIZE: i64 = 1000;

/// The detail member an event reports its own values through: the event type in
/// lowerCamel with `EventDetails` appended, which is how the model names every one of
/// them (`ExecutionSucceeded` -> `executionSucceededEventDetails`).
fn detail_member(event_type: &str) -> String {
    let mut characters = event_type.chars();
    let first: String = characters
        .next()
        .map(|first| first.to_lowercase().collect())
        .unwrap_or_default();
    format!("{first}{}EventDetails", characters.as_str())
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

/// The last event of a history, which must hold one.
fn last<'a>(events: &'a [Value], case: &str) -> &'a Value {
    events
        .last()
        .unwrap_or_else(|| panic!("case {case:?}: expected a history with events in it"))
}

/// The `id` of an event.
fn id_of(event: &Value, case: &str) -> i64 {
    event
        .get("id")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected an id in {event:?}"))
}

/// The one event of `event_type` a history must hold exactly once.
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

/// The detail member of `event`, which the event's own type names.
fn details<'a>(event: &'a Value, case: &str, event_type: &str) -> &'a Value {
    let member = detail_member(event_type);
    event
        .get(&member)
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} in {event:?}"))
}

/// A string member of a detail structure, absent when the detail does not carry it.
fn text<'a>(details: &'a Value, member: &str) -> Option<&'a str> {
    details.get(member).and_then(Value::as_str)
}

/// What the `cause` of one terminal event must be.
///
/// A cause the INTERPRETER composes says what it could not run, so it is asserted by
/// the value it has to name rather than by its whole wording; a cause a caller or a
/// definition supplied is that string exactly.
#[derive(Debug)]
enum Cause {
    Absent,
    Exactly(&'static str),
    Naming(&'static str),
}

/// One row of the outcome table: an execution, and the terminal event it must end on.
#[derive(Debug)]
struct Row {
    case: &'static str,
    execution: String,
    event_type: &'static str,
    output: Option<&'static str>,
    error: Option<&'static str>,
    cause: Cause,
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the terminal event, and what `includeExecutionData` withholds
// ---------------------------------------------------------------------------

/// Every outcome an execution can reach ends its history on that outcome's own event,
/// carrying the values the attempt reached it with — and `includeExecutionData: false`
/// withholds the output while leaving the error and the cause.
///
/// The rows differ along the OUTCOME: a success with an output, a failure the
/// definition declared, a failure the interpreter composed, and a stop, with and
/// without the error and cause a stop may supply.
#[tokio::test]
async fn the_terminal_event_reports_what_the_attempt_ended_with() {
    let emulator = Emulator::new();

    let succeeding = create_machine(&emulator, "outcome-succeeded", SUCCEED_DEFINITION).await;
    let succeeded = start_execution(&emulator, &succeeding, "only", r#"{"n":1}"#).await;
    let failing = create_machine(&emulator, "outcome-failed", FAIL_DEFINITION).await;
    let failed = start_execution(&emulator, &failing, "only", "{}").await;
    let unrunnable = create_machine(&emulator, "outcome-runtime", UNRUNNABLE_DEFINITION).await;
    let runtime = start_execution(&emulator, &unrunnable, "only", "{}").await;
    let slow = create_machine(&emulator, "outcome-stopped", SLOW_DEFINITION).await;
    let stopped = start_execution(&emulator, &slow, "stopped", "{}").await;
    emulator
        .ok(
            "StopExecution",
            &json!({
                "executionArn": stopped.clone(),
                "error": "StoppedByCaller",
                "cause": "the caller stopped it",
            }),
        )
        .await;
    let bare = start_execution(&emulator, &slow, "bare", "{}").await;
    emulator
        .ok("StopExecution", &json!({ "executionArn": bare.clone() }))
        .await;

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let rows = vec![
        Row {
            case: "a Pass state that succeeded with a result",
            execution: succeeded,
            event_type: "ExecutionSucceeded",
            output: Some(r#"{"ok":true}"#),
            error: None,
            cause: Cause::Absent,
        },
        Row {
            case: "a Fail state that declared an error and a cause",
            execution: failed,
            event_type: "ExecutionFailed",
            output: None,
            error: Some("Boom"),
            cause: Cause::Exactly("on purpose"),
        },
        Row {
            case: "a Task state naming work this emulator runs nothing for",
            execution: runtime,
            event_type: "ExecutionFailed",
            output: None,
            error: Some(RUNTIME_ERROR),
            cause: Cause::Naming("arn:aws:dynamodb:us-west-2:000000000000:table/absent"),
        },
        Row {
            case: "an execution stopped with an error and a cause",
            execution: stopped,
            event_type: "ExecutionAborted",
            output: None,
            error: Some("StoppedByCaller"),
            cause: Cause::Exactly("the caller stopped it"),
        },
        Row {
            case: "an execution stopped with neither an error nor a cause",
            execution: bare,
            event_type: "ExecutionAborted",
            output: None,
            error: None,
            cause: Cause::Absent,
        },
    ];

    for row in &rows {
        let case = row.case;
        let events = history(&emulator, &row.execution, json!({})).await;
        let terminal = last(&events, case);
        assert_eq!(
            terminal.get("type").and_then(Value::as_str),
            Some(row.event_type),
            "case {case:?}: expected the history to end on {}, got {:?}",
            row.event_type,
            types(&events)
        );
        // The terminal event happens once, and the numbering runs to it.
        let same = only(&events, case, row.event_type);
        assert_eq!(
            id_of(same, case),
            i64::try_from(events.len()).expect("the event count fits"),
            "case {case:?}: expected the terminal event to be the last of {} events, got \
             {terminal:?}",
            events.len()
        );

        let reported = details(terminal, case, row.event_type);
        assert_eq!(
            text(reported, "output"),
            row.output,
            "case {case:?}: expected the output the attempt ended with, got {reported:?}"
        );
        assert_eq!(
            text(reported, "error"),
            row.error,
            "case {case:?}: expected the error the attempt ended with, got {reported:?}"
        );
        match row.cause {
            Cause::Absent => assert!(
                text(reported, "cause").is_none(),
                "case {case:?}: expected no cause, got {reported:?}"
            ),
            Cause::Exactly(expected) => assert_eq!(
                text(reported, "cause"),
                Some(expected),
                "case {case:?}: expected the cause the attempt ended with, got {reported:?}"
            ),
            Cause::Naming(expected) => assert!(
                text(reported, "cause").is_some_and(|cause| cause.contains(expected)),
                "case {case:?}: expected a cause naming {expected:?}, got {reported:?}"
            ),
        }
        if row.output.is_some() {
            assert_eq!(
                details(terminal, case, row.event_type)
                    .get("outputDetails")
                    .and_then(|data| data.get("truncated"))
                    .and_then(Value::as_bool),
                Some(false),
                "case {case:?}: expected the output to be reported whole, got {reported:?}"
            );
        }

        // Rule 2: the DATA goes, the rest stays.
        let withheld = history(
            &emulator,
            &row.execution,
            json!({ "includeExecutionData": false }),
        )
        .await;
        assert_eq!(
            types(&withheld),
            types(&events),
            "case {case:?}: expected the same events without execution data, got {withheld:?}"
        );
        let terminal = last(&withheld, case);
        let reported = details(terminal, case, row.event_type);
        assert!(
            text(reported, "output").is_none(),
            "case {case:?}: expected no output without execution data, got {reported:?}"
        );
        assert_eq!(
            (text(reported, "error"), text(reported, "cause").is_some()),
            (row.error, !matches!(row.cause, Cause::Absent)),
            "case {case:?}: expected the error and cause to be reported anyway, got {reported:?}"
        );
    }
}

/// A redrive moves the execution's own `error`, `cause` and `output`, and the event
/// the stopped attempt ended on keeps reporting what THAT attempt ended with.
#[tokio::test]
async fn an_earlier_attempts_terminal_event_keeps_its_own_values() {
    let emulator = Emulator::new();
    let case = "an execution stopped, then redriven to success";
    let machine = create_machine(&emulator, "outcome-redriven", SUCCEED_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"n":2}"#).await;

    // Stopped while it was still RUNNING, so the attempt ended on `ExecutionAborted`.
    emulator
        .ok(
            "StopExecution",
            &json!({
                "executionArn": execution.clone(),
                "error": "StoppedByCaller",
                "cause": "stopped before it could finish",
            }),
        )
        .await;
    emulator
        .ok("RedriveExecution", &json!({ "executionArn": execution.clone() }))
        .await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    // The record now reports the SECOND attempt: it succeeded, so it carries an
    // output and neither an error nor a cause.
    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        (
            described.get("status").and_then(Value::as_str),
            described.get("error").and_then(Value::as_str),
        ),
        (Some("SUCCEEDED"), None),
        "case {case:?}: expected the redriven execution to have succeeded, got {described:?}"
    );

    let events = history(&emulator, &execution, json!({})).await;
    let aborted = only(&events, case, "ExecutionAborted");
    let reported = details(aborted, case, "ExecutionAborted");
    assert_eq!(
        (text(reported, "error"), text(reported, "cause")),
        (
            Some("StoppedByCaller"),
            Some("stopped before it could finish")
        ),
        "case {case:?}: expected the stopped attempt's own error and cause, got {reported:?}"
    );
    let succeeded = only(&events, case, "ExecutionSucceeded");
    let reported = details(succeeded, case, "ExecutionSucceeded");
    assert_eq!(
        text(reported, "output"),
        Some(r#"{"ok":true}"#),
        "case {case:?}: expected the second attempt's own output, got {reported:?}"
    );
    // "Redriven executions append their event history to the existing event history":
    // the stopped attempt's events come first, and the numbering runs on through them.
    let ordered = types(&events);
    let aborted_at = ordered
        .iter()
        .position(|event| event == "ExecutionAborted")
        .unwrap_or_else(|| panic!("case {case:?}: expected an ExecutionAborted in {ordered:?}"));
    let redriven_at = ordered
        .iter()
        .position(|event| event == "ExecutionRedriven")
        .unwrap_or_else(|| panic!("case {case:?}: expected an ExecutionRedriven in {ordered:?}"));
    assert!(
        aborted_at < redriven_at,
        "case {case:?}: expected the stopped attempt's events first, got {ordered:?}"
    );
    for (index, event) in events.iter().enumerate() {
        let expected = i64::try_from(index + 1).expect("the index fits");
        // The event written after the start names no predecessor: the start is the
        // execution beginning rather than a step of its walk.
        let expected_previous = if expected <= 2 { 0 } else { expected - 1 };
        assert_eq!(
            (
                id_of(event, case),
                event.get("previousEventId").and_then(Value::as_i64)
            ),
            (expected, Some(expected_previous)),
            "case {case:?}: expected event {expected} to follow {expected_previous}, got {event:?}"
        );
    }
}

/// Each `ExecutionRedriven` event reports the number of redrives ITS redrive reached,
/// so a history of several redrives counts 1, 2, 3 and not the latest count three
/// times.
#[tokio::test]
async fn each_redriven_event_reports_the_count_its_own_redrive_reached() {
    let emulator = Emulator::new();
    let case = "an execution redriven three times";
    let machine = create_machine(&emulator, "outcome-redrive-count", FAIL_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "only", "{}").await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let redrives = 3;
    for attempt in 1..=redrives {
        emulator
            .ok(
                "RedriveExecution",
                &json!({ "executionArn": execution.clone() }),
            )
            .await;
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("redriveCount").and_then(Value::as_i64),
            Some(attempt),
            "case {case:?}: expected the record to count redrive {attempt}, got {described:?}"
        );
    }

    let events = history(&emulator, &execution, json!({})).await;
    let counted: Vec<Option<i64>> = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("ExecutionRedriven"))
        .map(|event| {
            event
                .get("executionRedrivenEventDetails")
                .and_then(|reported| reported.get("redriveCount"))
                .and_then(Value::as_i64)
        })
        .collect();
    assert_eq!(
        counted,
        (1..=redrives).map(Some).collect::<Vec<Option<i64>>>(),
        "case {case:?}: expected each redrive to be counted at the count it reached, got \
         {counted:?}"
    );
    // Every attempt failed the same way, so each attempt's terminal event reports the
    // error and the cause the definition declares.
    for event in events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("ExecutionFailed"))
    {
        let reported = details(event, case, "ExecutionFailed");
        assert_eq!(
            (text(reported, "error"), text(reported, "cause")),
            (Some("Boom"), Some("on purpose")),
            "case {case:?}: expected every attempt's failure to be reported, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: one order, cut into pages
// ---------------------------------------------------------------------------

/// Every event of one history, read one page at a time at `max_results`, in the order
/// `reverse_order` asks for.
async fn walked(
    emulator: &Emulator,
    case: &str,
    execution_arn: &str,
    max_results: i64,
    reverse_order: bool,
) -> Vec<Value> {
    let limit = if max_results <= 0 {
        DEFAULT_PAGE_SIZE
    } else {
        usize::try_from(max_results).unwrap_or(DEFAULT_PAGE_SIZE)
    };
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({
            "executionArn": execution_arn,
            "maxResults": max_results,
            "reverseOrder": reverse_order,
        });
        if let Some(token) = token.as_deref() {
            request
                .as_object_mut()
                .expect("the request is an object")
                .insert("nextToken".to_string(), json!(token));
        }
        let answered = emulator.ok("GetExecutionHistory", &request).await;
        let page: Vec<Value> = answered
            .get("events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected events in {answered:?}"))
            .clone();
        assert!(
            page.len() <= limit,
            "case {case:?}: expected a page of at most {limit} events, got {}",
            page.len()
        );
        collected.extend(page);
        pages += 1;
        assert!(
            pages <= collected.len() + 1,
            "case {case:?}: expected the pages to end, got {pages} of them for {} events",
            collected.len()
        );
        match answered.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    collected
}

/// The pages of a history concatenate to exactly the order they were asked for,
/// whatever page size the caller chooses, and the descending order is the ascending
/// one read from the end.
///
/// The sizes span smaller than, equal to and larger than the history, plus `0`, which
/// "uses the default".
#[tokio::test]
async fn the_pages_of_either_order_concatenate_to_that_order() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "outcome-paged", CHAIN_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "only", "{}").await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let whole = history(&emulator, &execution, json!({})).await;
    assert_eq!(
        types(&whole),
        vec![
            "ExecutionStarted",
            "PassStateEntered",
            "PassStateExited",
            "PassStateEntered",
            "PassStateExited",
            "PassStateEntered",
            "PassStateExited",
            "PassStateEntered",
            "PassStateExited",
            "ExecutionSucceeded",
        ],
        "expected the events of the four states the walk passed, got {whole:?}"
    );
    let mut backwards = whole.clone();
    backwards.reverse();

    for size in [1_i64, 2, 3, 5, 9, 10, 11, MAX_PAGE_SIZE, 0] {
        for reverse_order in [false, true] {
            let case = format!("maxResults {size}, reverseOrder {reverse_order}");
            let expected = if reverse_order { &backwards } else { &whole };
            let pages = walked(&emulator, &case, &execution, size, reverse_order).await;
            assert_eq!(
                types(&pages),
                types(expected),
                "case {case:?}: expected the pages to concatenate to that one order, got {:?}",
                types(&pages)
            );
            let ids: Vec<i64> = pages.iter().map(|event| id_of(event, &case)).collect();
            let ordered = ids
                .windows(2)
                .all(|pair| if reverse_order { pair[0] > pair[1] } else { pair[0] < pair[1] });
            assert!(
                ordered,
                "case {case:?}: expected the ids to run one way through the pages, got {ids:?}"
            );
        }
    }
}

/// `maxResults` is refused outside the range its shape declares, and accepted on each
/// published bound.
#[tokio::test]
async fn max_results_outside_its_modelled_range_is_refused() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "outcome-page-bounds", SUCCEED_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "only", "{}").await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let whole = types(&history(&emulator, &execution, json!({})).await);
    for (case, max_results, admitted) in [
        ("the minimum the model admits, which uses the default", 0, true),
        ("the maximum the model admits", MAX_PAGE_SIZE, true),
        ("one past the maximum", MAX_PAGE_SIZE + 1, false),
        ("one below the minimum", -1, false),
    ] {
        let request = json!({ "executionArn": execution.clone(), "maxResults": max_results });
        if admitted {
            let answered = emulator.ok("GetExecutionHistory", &request).await;
            let events: Vec<Value> = answered
                .get("events")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: expected events in {answered:?}"))
                .clone();
            assert_eq!(
                types(&events),
                whole,
                "case {case:?}: expected the whole history on one page, got {events:?}"
            );
        } else {
            let (status, code) = emulator.err("GetExecutionHistory", &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected maxResults={max_results} refused, got {status} {code}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the history is a report
// ---------------------------------------------------------------------------

/// Reading the history, in either order and at any page size, changes nothing a later
/// caller sees.
#[tokio::test]
async fn reading_the_history_changes_nothing() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "outcome-report", CHAIN_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"n":3}"#).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let before = history(&emulator, &execution, json!({})).await;
    let described_before = describe_execution(&emulator, &execution).await;
    for size in [1_i64, 3, 0] {
        for reverse_order in [false, true] {
            let case = format!("a repeated walk at maxResults {size}, reverseOrder {reverse_order}");
            walked(&emulator, &case, &execution, size, reverse_order).await;
        }
    }
    history(
        &emulator,
        &execution,
        json!({ "includeExecutionData": false }),
    )
    .await;

    let after = history(&emulator, &execution, json!({})).await;
    assert_eq!(
        after, before,
        "expected the history unchanged by reading it, got {before:?} then {after:?}"
    );
    let described_after = describe_execution(&emulator, &execution).await;
    assert_eq!(
        described_after, described_before,
        "expected the execution unchanged by reading its history, got {described_before:?} then \
         {described_after:?}"
    );
}
