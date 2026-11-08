// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the redrive projection `DescribeExecution` reports and for
//! `RedriveExecution` itself, driven over this service's wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. `DescribeExecution` reports, for EVERY execution, whether `RedriveExecution`
//!    would redrive it: `redriveStatus` is `REDRIVABLE` exactly when that call would
//!    succeed and `NOT_REDRIVABLE` otherwise, with `redriveStatusReason` carrying one
//!    of the reasons the member documents only in the second case — and the two can
//!    never disagree, so the reason `DescribeExecution` reports is the message
//!    `RedriveExecution` then refuses with;
//! 2. a performed redrive is RECORDED on the execution: `redriveCount` counts the
//!    redrives, `redriveDate` is the instant of the last one and is stable across
//!    reads, and a repeated `clientToken` answers that same instant instead of
//!    redriving again;
//! 3. an execution's `output` is the document the run produced: a document no state
//!    TRANSFORMED keeps the BYTES it arrived with — the caller's `input` text reaches
//!    `output` byte for byte through a pass-through — while a `Pass` state's
//!    `Result`, a value of the parsed definition, is reported as that value's own
//!    encoding.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` already builds; `tests/all.rs` is this binary's crate
//! root, so it is reached through `crate::state_machine_lifecycle`.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    not_redrivable_message, EXECUTION_SETTLE_DELAY, NOT_REDRIVABLE_RUNNING,
    NOT_REDRIVABLE_SUCCEEDED,
};

use crate::state_machine_lifecycle::{
    create_machine, discriminated, error_code, string, Emulator, HELLO_DEFINITION, ROLE_ARN,
};

/// Long enough that an outcome the clock owed has become due.
///
/// `std::thread::sleep`, for the reason `state_machine_lifecycle` records: the
/// workspace's `tokio` carries no `time` feature, and each test runs on its own
/// single-threaded runtime with no concurrent task.
const PAST_THE_EXECUTION_DELAY: Duration =
    EXECUTION_SETTLE_DELAY.saturating_add(Duration::from_millis(300));

/// A definition that fails immediately, so the execution reaches a terminal status
/// AWS reports as redrivable.
const FAIL_DEFINITION: &str =
    r#"{"StartAt":"Bang","States":{"Bang":{"Type":"Fail","Error":"Boom","Cause":"on purpose"}}}"#;

/// A definition that carries its input through unchanged.
const ECHO_DEFINITION: &str =
    r#"{"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}}"#;

/// A definition whose only state names no integration call this emulator makes (a
/// bare resource ARN of a service whose resources are not work in themselves), so the
/// execution ends `FAILED` with ASL's own `States.Runtime`.
const TASK_DEFINITION: &str = r#"{"StartAt":"T","States":{"T":{"Type":"Task","Resource":"arn:aws:dynamodb:us-west-2:000000000000:table/t","End":true}}}"#;

/// A definition that waits long enough that the execution stays non-terminal for the
/// whole test.
const SLOW_DEFINITION: &str =
    r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":600,"Next":"Done"},"Done":{"Type":"Succeed"}}}"#;

async fn start_execution(
    emulator: &Emulator,
    state_machine_arn: &str,
    name: &str,
    input: &str,
) -> String {
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": state_machine_arn, "name": name, "input": input }),
        )
        .await;
    string(&started, "executionArn")
}

async fn describe_execution(emulator: &Emulator, execution_arn: &str) -> Value {
    emulator
        .ok(
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await
}

/// `redriveStatus` and `redriveStatusReason` as the body carries them.
fn redrive_projection(body: &Value) -> (Option<&str>, Option<&str>) {
    (
        body.get("redriveStatus").and_then(Value::as_str),
        body.get("redriveStatusReason").and_then(Value::as_str),
    )
}

/// The `__type` of an error body, without its namespace, plus its `message`.
fn error_of(body: &Value) -> (String, String) {
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string();
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (code, message)
}

// ---------------------------------------------------------------------------
// Rule 1: the projection is the eligibility of a redrive
// ---------------------------------------------------------------------------

/// One row of the projection table.
struct ProjectionCase {
    case: &'static str,
    definition: &'static str,
    input: &'static str,
    /// Whether the row's execution is stopped before it is described.
    stop: bool,
    status: &'static str,
    redrive_status: &'static str,
    /// The documented reason, or `None` when the execution is redrivable.
    reason: Option<&'static str>,
}

/// `redriveStatus`/`redriveStatusReason` follow the eligibility rule over a table
/// that varies the execution's terminal status and HOW it reached it: a run that is
/// still in flight, one that succeeded, one that failed at a `Fail` state, one that
/// failed because the emulator does not execute its state, and one a caller stopped.
#[tokio::test]
async fn the_redrive_projection_reports_whether_a_redrive_would_succeed() {
    let cases = [
        ProjectionCase {
            case: "a run still in flight",
            definition: SLOW_DEFINITION,
            input: "{}",
            stop: false,
            status: "RUNNING",
            redrive_status: "NOT_REDRIVABLE",
            reason: Some(NOT_REDRIVABLE_RUNNING),
        },
        ProjectionCase {
            case: "a succeeded run",
            definition: HELLO_DEFINITION,
            input: r#"{"a":1}"#,
            stop: false,
            status: "SUCCEEDED",
            redrive_status: "NOT_REDRIVABLE",
            reason: Some(NOT_REDRIVABLE_SUCCEEDED),
        },
        ProjectionCase {
            case: "a run that reached a Fail state",
            definition: FAIL_DEFINITION,
            input: "{}",
            stop: false,
            status: "FAILED",
            redrive_status: "REDRIVABLE",
            reason: None,
        },
        ProjectionCase {
            case: "a run whose state names no integration call this emulator makes",
            definition: TASK_DEFINITION,
            input: "{}",
            stop: false,
            status: "FAILED",
            redrive_status: "REDRIVABLE",
            reason: None,
        },
        ProjectionCase {
            case: "a run a caller stopped",
            definition: SLOW_DEFINITION,
            input: "{}",
            stop: true,
            status: "ABORTED",
            redrive_status: "REDRIVABLE",
            reason: None,
        },
    ];

    let emulator = Emulator::new();
    // Start every row's execution first, then wait once for the outcomes the clock
    // owes, so the whole table costs one settle delay.
    let mut started = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("redrive-projection-{index}"), case.definition).await;
        let execution = start_execution(&emulator, &machine, "run", case.input).await;
        if case.stop {
            emulator
                .ok("StopExecution", &json!({ "executionArn": execution }))
                .await;
        }
        started.push(execution);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (case, execution) in cases.iter().zip(&started) {
        let described = describe_execution(&emulator, execution).await;
        let label = case.case;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some(case.status),
            "case {label:?}: expected status {:?}, got {described:?}",
            case.status
        );
        let (status, reason) = redrive_projection(&described);
        assert_eq!(
            status,
            Some(case.redrive_status),
            "case {label:?}: expected redriveStatus {:?}, got {status:?}",
            case.redrive_status
        );
        assert_eq!(
            reason, case.reason,
            "case {label:?}: expected redriveStatusReason {:?}, got {reason:?}",
            case.reason
        );
        // A never-redriven execution reports no redrive of its own: "If you have not
        // yet redriven an execution, the redriveCount is 0 [and] the redriveDate is
        // null."
        assert_eq!(
            described.get("redriveCount").and_then(Value::as_i64),
            Some(0),
            "case {label:?}: expected redriveCount 0, got {described:?}"
        );
        assert!(
            described.get("redriveDate").is_none_or(Value::is_null),
            "case {label:?}: expected no redriveDate before a redrive, got {described:?}"
        );
        // "Indicates whether input or output was included in the response. Always
        // true for API calls" — and `outputDetails` only where there is an output.
        let details = discriminated(
            "CloudWatchEventsExecutionDataDetails",
            json!({ "included": true }),
        );
        assert_eq!(
            described.get("inputDetails"),
            Some(&details),
            "case {label:?}: expected inputDetails {details}, got {described:?}"
        );
        let has_output = described
            .get("output")
            .is_some_and(|output| !output.is_null());
        assert_eq!(
            described.get("outputDetails").cloned(),
            has_output.then(|| details.clone()),
            "case {label:?}: outputDetails must accompany an output and only an output, got {described:?}"
        );
    }
}

/// `RedriveExecution` refuses exactly what `DescribeExecution` reports as
/// `NOT_REDRIVABLE`, with that same reason, and answers each malformed or unknown
/// request with the operation's own modelled error.
#[tokio::test]
async fn redrive_refuses_what_the_projection_reports_as_not_redrivable() {
    let emulator = Emulator::new();

    // Rows: the two eligibility reasons an execution can report here, each checked
    // against the reason `DescribeExecution` gave for the SAME execution.
    let not_redrivable = [
        ("a run still in flight", SLOW_DEFINITION, NOT_REDRIVABLE_RUNNING),
        ("a succeeded run", HELLO_DEFINITION, NOT_REDRIVABLE_SUCCEEDED),
    ];
    for (index, (case, definition, reason)) in not_redrivable.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("redrive-refusal-{index}"), definition).await;
        let execution = start_execution(&emulator, &machine, "run", "{}").await;
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);

        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            redrive_projection(&described),
            (Some("NOT_REDRIVABLE"), Some(*reason)),
            "case {case:?}: expected the documented projection, got {described:?}"
        );

        let (status, body) = emulator
            .call("RedriveExecution", &json!({ "executionArn": execution }))
            .await;
        let (code, message) = error_of(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "ExecutionNotRedrivable"),
            "case {case:?}: expected 400 ExecutionNotRedrivable, got {status} {body:?}"
        );
        // The refusal carries the reason describe reported, inside the wrapper
        // naming the execution it refused (`tests/all/client_error_wrappers.rs`
        // owns that construction); `redriveStatusReason` carries the reason alone.
        let expected = not_redrivable_message(&execution, reason);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
    }

    // Rows: a request no stored execution answers, an ARN that is not one of this
    // service's execution ARNs, and an execution of an EXPRESS state machine, which
    // this API action does not support.
    let standard = create_machine(&emulator, "redrive-standard", HELLO_DEFINITION).await;
    let express = string(
        &emulator
            .ok(
                "CreateStateMachine",
                &json!({
                    "name": "redrive-express",
                    "definition": HELLO_DEFINITION,
                    "roleArn": ROLE_ARN,
                    "type": "EXPRESS",
                }),
            )
            .await,
        "stateMachineArn",
    );
    let express_execution = start_execution(&emulator, &express, "run", "{}").await;
    // An execution ARN of the STANDARD machine that nothing started: this row is about
    // a record that is ABSENT, so the ARN is the form a STANDARD execution carries.
    let unknown = format!(
        "{}:nothing-started",
        standard.replacen(":stateMachine:", ":execution:", 1)
    );
    let bad_shape = [
        (
            "an execution ARN no execution is stored under",
            unknown,
            400,
            "ExecutionDoesNotExist",
        ),
        (
            "an ARN of another service",
            "arn:aws:lambda:us-west-2:000000000000:function:f".to_string(),
            400,
            "InvalidArn",
        ),
        (
            "a state machine ARN, not an execution ARN",
            standard,
            400,
            "InvalidArn",
        ),
        (
            // An EXPRESS execution is named by the `express` ARN its run carries its
            // own id in, a resource type no execution-ARN member accepts, so the ARN
            // is refused before any record is looked up.
            "an execution of an EXPRESS state machine",
            express_execution,
            400,
            "InvalidArn",
        ),
    ];
    for (case, arn, expected_status, expected_code) in bad_shape {
        let (status, body) = emulator
            .call("RedriveExecution", &json!({ "executionArn": arn }))
            .await;
        let (code, _) = error_of(&body);
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a redrive is recorded
// ---------------------------------------------------------------------------

/// A redrive re-runs the execution and is RECORDED on it: the count moves by one per
/// redrive, `redriveDate` is the instant the call answered and does not change
/// between reads, the execution is non-terminal again until the clock makes the
/// re-run's outcome due, and `ListExecutions` reports the same record.
#[tokio::test]
async fn a_redrive_is_recorded_on_the_execution_and_read_back() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "redrive-recorded", FAIL_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "run", r#"{"attempt":1}"#).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let before = describe_execution(&emulator, &execution).await;
    assert_eq!(
        redrive_projection(&before),
        (Some("REDRIVABLE"), None),
        "case {:?}: a failed execution must be redrivable, got {before:?}",
        "before the first redrive"
    );

    // Each redrive is one row: the count it must report afterwards, and the
    // `redriveDate` it answered.
    let mut previous_date: Option<Value> = None;
    for expected_count in 1..=2 {
        let case = format!("redrive {expected_count}");
        let redriven = emulator
            .ok(
                "RedriveExecution",
                &json!({ "executionArn": execution, "clientToken": format!("token-{expected_count}") }),
            )
            .await;
        let answered = redriven
            .get("redriveDate")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a redriveDate in {redriven:?}"));
        assert!(
            answered.as_f64().is_some(),
            "case {case:?}: expected an epoch-seconds redriveDate, got {answered:?}"
        );
        assert_ne!(
            Some(&answered),
            previous_date.as_ref(),
            "case {case:?}: a second redrive must answer its own instant"
        );

        // Straight after the call the re-run is in flight, so the execution is not
        // terminal and is therefore not redrivable again yet.
        let running = describe_execution(&emulator, &execution).await;
        assert_eq!(
            running.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: a redriven execution runs again, got {running:?}"
        );
        assert_eq!(
            redrive_projection(&running),
            (Some("NOT_REDRIVABLE"), Some(NOT_REDRIVABLE_RUNNING)),
            "case {case:?}: a re-running execution is not redrivable, got {running:?}"
        );
        assert_eq!(
            running.get("redriveCount").and_then(Value::as_i64),
            Some(expected_count),
            "case {case:?}: expected redriveCount {expected_count}, got {running:?}"
        );
        assert_eq!(
            running.get("redriveDate"),
            Some(&answered),
            "case {case:?}: describe must report the redriveDate the call answered, got {running:?}"
        );
        // The terminal values of the attempt being redriven are gone while it runs.
        assert!(
            running.get("stopDate").is_none_or(Value::is_null),
            "case {case:?}: a re-running execution has no stopDate, got {running:?}"
        );

        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let settled = describe_execution(&emulator, &execution).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: the re-run reaches the definition's own outcome, got {settled:?}"
        );
        assert_eq!(
            settled.get("error").and_then(Value::as_str),
            Some("Boom"),
            "case {case:?}: the re-run reports the Fail state's error, got {settled:?}"
        );
        assert_eq!(
            redrive_projection(&settled),
            (Some("REDRIVABLE"), None),
            "case {case:?}: the re-run's failure is redrivable again, got {settled:?}"
        );
        assert_eq!(
            settled.get("redriveDate"),
            Some(&answered),
            "case {case:?}: redriveDate is stable across reads, got {settled:?}"
        );
        assert_eq!(
            settled.get("redriveCount").and_then(Value::as_i64),
            Some(expected_count),
            "case {case:?}: expected redriveCount {expected_count}, got {settled:?}"
        );
        // The execution keeps the input the first attempt was given: a redrive "uses
        // the same input".
        assert_eq!(
            settled.get("input").and_then(Value::as_str),
            Some(r#"{"attempt":1}"#),
            "case {case:?}: a redrive reuses the stored input, got {settled:?}"
        );
        previous_date = Some(answered);
    }

    // `ListExecutions` reports the same record, and the listing scoped to the state
    // machine names no `redriveFilter`: "if you provide a state machine ARN in
    // redriveFilter, the API returns a validation exception", so the filter is served
    // for the Map Run selector alone.
    let listed = emulator
        .ok("ListExecutions", &json!({ "stateMachineArn": machine }))
        .await;
    let executions = listed
        .get("executions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an executions list in {listed:?}"));
    assert_eq!(
        executions.len(),
        1,
        "expected the one execution of the state machine, got {listed:?}"
    );
    for item in executions {
        assert_eq!(
            item.get("redriveCount").and_then(Value::as_i64),
            Some(2),
            "the list item carries the stored redriveCount, got {item:?}"
        );
        assert_eq!(
            item.get("redriveDate"),
            previous_date.as_ref(),
            "the list item carries the stored redriveDate, got {item:?}"
        );
    }
    for filter in ["REDRIVEN", "NOT_REDRIVEN"] {
        let (status, answered) = emulator
            .call(
                "ListExecutions",
                &json!({ "stateMachineArn": machine, "redriveFilter": filter }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {filter:?}: expected the filter to be refused beside a state machine ARN, got \
             {status} {answered:?}"
        );
    }
}

/// A repeated `clientToken` answers the redrive it already performed, and performs no
/// second one: "the API will return idempotent responses for the last 10 client
/// tokens used to successfully redrive the execution."
#[tokio::test]
async fn a_repeated_client_token_answers_the_same_redrive() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "redrive-idempotent", FAIL_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "run", "{}").await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let first = emulator
        .ok(
            "RedriveExecution",
            &json!({ "executionArn": execution, "clientToken": "the-same-token" }),
        )
        .await;
    let answered = first.get("redriveDate").cloned();

    // Rows: the same token twice more, once while the re-run is in flight and once
    // after it has settled — the token answers the same instant either way, and the
    // execution is redriven only once.
    for case in ["while the re-run is in flight", "after the re-run settled"] {
        let repeat = emulator
            .ok(
                "RedriveExecution",
                &json!({ "executionArn": execution, "clientToken": "the-same-token" }),
            )
            .await;
        assert_eq!(
            repeat.get("redriveDate"),
            answered.as_ref(),
            "case {case:?}: a repeated client token answers the first redriveDate, got {repeat:?}"
        );
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("redriveCount").and_then(Value::as_i64),
            Some(1),
            "case {case:?}: a repeated client token redrives nothing more, got {described:?}"
        );
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    }

    // A DIFFERENT token is a different request, so it redrives again.
    let other = emulator
        .ok(
            "RedriveExecution",
            &json!({ "executionArn": execution, "clientToken": "another-token" }),
        )
        .await;
    assert_ne!(
        other.get("redriveDate"),
        answered.as_ref(),
        "a different client token performs its own redrive, got {other:?}"
    );
    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        described.get("redriveCount").and_then(Value::as_i64),
        Some(2),
        "a different client token redrives again, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the output is the document's own bytes
// ---------------------------------------------------------------------------

/// One row of the document table.
struct DocumentCase {
    case: &'static str,
    definition: &'static str,
    input: &'static str,
    /// The `output` the execution must report, byte for byte.
    output: &'static str,
}

/// An execution's `output` carries the document's own BYTES wherever no state
/// transformed it, over a table that varies its JSON type (object, array, string,
/// number), its inter-token spacing and the state type that carried it — and reports
/// a `Pass` state's `Result`, which is a value of the parsed definition, as that
/// value's own encoding.
#[tokio::test]
async fn the_output_keeps_the_bytes_the_document_was_written_with() {
    let cases = [
        DocumentCase {
            case: "an object passed through with a space after the colon",
            definition: ECHO_DEFINITION,
            input: r#"{"probe": true}"#,
            output: r#"{"probe": true}"#,
        },
        DocumentCase {
            case: "an object passed through with spaces around every token",
            definition: ECHO_DEFINITION,
            input: r#"{ "a" : [ 1, 2 ] , "b" : { "c" : null } }"#,
            output: r#"{ "a" : [ 1, 2 ] , "b" : { "c" : null } }"#,
        },
        DocumentCase {
            case: "an array passed through",
            definition: ECHO_DEFINITION,
            input: r#"[{"id": "a"}, {"id": "b"}]"#,
            output: r#"[{"id": "a"}, {"id": "b"}]"#,
        },
        DocumentCase {
            case: "a string document passed through",
            definition: ECHO_DEFINITION,
            input: r#""plain text""#,
            output: r#""plain text""#,
        },
        DocumentCase {
            case: "a number document passed through a Succeed state",
            definition: r#"{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}}"#,
            input: "1.50",
            output: "1.50",
        },
        DocumentCase {
            case: "a Result the definition wrote with spacing, reported as the parsed value",
            definition: r#"{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","Result":{"msg": "hi", "n": 1},"End":true}}}"#,
            input: r#"{"ignored": true}"#,
            output: r#"{"msg":"hi","n":1}"#,
        },
        DocumentCase {
            case: "the last Result of a chain, not the first",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":{"step": "a"},"Next":"B"},"B":{"Type":"Pass","Result":[ "b" ],"Next":"C"},"C":{"Type":"Succeed"}}}"#,
            input: "{}",
            output: r#"["b"]"#,
        },
    ];

    let emulator = Emulator::new();
    let mut started = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("document-bytes-{index}"), case.definition).await;
        started.push(start_execution(&emulator, &machine, "run", case.input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (case, execution) in cases.iter().zip(&started) {
        let described = describe_execution(&emulator, execution).await;
        let label = case.case;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {label:?}: expected a succeeded run, got {described:?}"
        );
        // The input comes back as the caller wrote it, and so does the document the
        // run produced.
        assert_eq!(
            described.get("input").and_then(Value::as_str),
            Some(case.input),
            "case {label:?}: expected the input byte-identical, got {described:?}"
        );
        assert_eq!(
            described.get("output").and_then(Value::as_str),
            Some(case.output),
            "case {label:?}: expected output {:?} byte for byte, got {:?}",
            case.output,
            described.get("output")
        );
    }
}
