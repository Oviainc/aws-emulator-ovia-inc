// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN a state machine's `loggingConfiguration` delivers its
//! executions' history events to CloudWatch Logs.
//!
//! The rule under test: a run's history events reach the configured log group once the
//! outcome the clock owed that run has come due, with NO further Step Functions call —
//! so the delivery is observable through CloudWatch Logs' own readers alone, whatever
//! workflow type the state machine is and whether or not its executions can be read
//! back through this service's own reporting APIs.
//!
//! That matters most for an EXPRESS workflow, whose executions `DescribeExecution` and
//! `GetExecutionHistory` do not serve: the delivered events are the ONLY record of such
//! a run, so a delivery waiting for a call that a client has no reason — and no way —
//! to make is a run that left no record at all.
//!
//! Every row below starts its execution asynchronously and then calls Step Functions no
//! more: it polls `FilterLogEvents`, exactly as a client watching the group does. The
//! rows differ along what the rule generalises over: the workflow type (EXPRESS and
//! STANDARD), the logging `level` (`ALL` and `ERROR`), `includeExecutionData`, and the
//! outcome the run reaches (succeeded and failed).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    observability, EXECUTION_SETTLE_DELAY,
};

use crate::execution_observability::{create, logging, Observed, FAILING_DEFINITION};
use crate::state_machine_lifecycle::{error_code, string, HELLO_DEFINITION, ROLE_ARN};

/// How long a row waits for the delivery before it fails.
///
/// The outcome is due one [`EXECUTION_SETTLE_DELAY`] after the start, and the clock
/// that materialises it wakes on its own schedule, so the wait is that span with ample
/// room for the wake-up around it — far longer than it takes, and bounded so a failing
/// row reports rather than hangs.
const DELIVERY_DEADLINE: Duration = EXECUTION_SETTLE_DELAY.saturating_add(Duration::from_secs(20));

/// How often a row looks at the log group while it waits.
const POLL: Duration = Duration::from_millis(25);

/// One row of the table: a state machine's logging configuration, the run it logs, and
/// what the group must then hold.
struct Row {
    /// What the row is about, named in every assertion it makes.
    case: &'static str,
    /// `STANDARD` or `EXPRESS`.
    sm_type: &'static str,
    /// The configuration's `level`.
    level: &'static str,
    /// The configuration's `includeExecutionData`.
    include_execution_data: bool,
    /// The definition the run interprets.
    definition: &'static str,
    /// The event types the group must hold, in the run's own history order.
    expected: &'static [&'static str],
    /// One value of the run a delivered event's `details` must carry EXACTLY: the
    /// document a state produced, which is the execution DATA and so is delivered only
    /// where the configuration asks for it, or the error a failure reported, which is
    /// the transition rather than the payload and is delivered either way.
    datum: &'static str,
    /// Whether the delivered messages must carry it.
    carries_datum: bool,
    /// Whether the delivered events must carry a payload member between them: the
    /// configuration asks for the execution data AND the level selects an event that
    /// has some.
    payload_delivered: bool,
    /// Whether the delivered ids are the whole contiguous `1..N` of the run's history,
    /// which only a level that logs every event delivers.
    whole_history: bool,
}

/// The messages `group` holds, waited for until the run's delivery has landed.
///
/// The group is read through the ONE reader of a delivered group this binary has
/// (`crate::vended_log_event_members::delivered`); what this adds is the WAIT, because
/// a delivery owed to the clock has not landed when the start answers. The messages come
/// back in the order CloudWatch Logs pages them, which is the order they were written in.
async fn delivered(observed: &Observed, group: &str, case: &str) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let messages = crate::vended_log_event_members::delivered(observed, case, group).await;
        if !messages.is_empty() {
            return messages;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the run's events in {group}, got none within \
             {DELIVERY_DEADLINE:?} and no further Step Functions call"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// The `type` of each delivered message.
fn types(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .map(|message| {
            message
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `id` of each delivered message.
///
/// A vended Step Functions log event carries its id, the event it follows and its
/// instant as TEXT, which is the form the message is read back in here: a number would
/// be another wire type, not another spelling of the same one.
fn ids(messages: &[Value], case: &str) -> Vec<i64> {
    messages
        .iter()
        .map(|message| {
            let text = message
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a delivered event to carry its id as text, got \
                         {message:?}"
                    )
                });
            text.parse::<i64>().unwrap_or_else(|error| {
                panic!("case {case:?}: expected a whole id, got {text:?} ({error})")
            })
        })
        .collect()
}

/// A run's events reach the configured log group with no second Step Functions call,
/// whatever workflow type it runs under and whichever events its `level` selects.
#[tokio::test]
async fn a_run_is_delivered_without_a_second_step_functions_call() {
    let observed = Observed::new();

    let rows: Vec<Row> = vec![
        Row {
            case: "an EXPRESS run that succeeds, logged at ALL with its data",
            sm_type: "EXPRESS",
            level: observability::LOG_LEVEL_ALL,
            include_execution_data: true,
            definition: HELLO_DEFINITION,
            expected: &[
                "ExecutionStarted",
                "PassStateEntered",
                "PassStateExited",
                "ExecutionSucceeded",
            ],
            datum: r#"{"msg":"hi"}"#,
            carries_datum: true,
            payload_delivered: true,
            whole_history: true,
        },
        Row {
            case: "an EXPRESS run that fails, logged at ALL with its data",
            sm_type: "EXPRESS",
            level: observability::LOG_LEVEL_ALL,
            include_execution_data: true,
            definition: FAILING_DEFINITION,
            expected: &["ExecutionStarted", "FailStateEntered", "ExecutionFailed"],
            datum: "Boom",
            carries_datum: true,
            payload_delivered: true,
            whole_history: true,
        },
        Row {
            case: "an EXPRESS run that fails, logged at ALL without its data",
            sm_type: "EXPRESS",
            level: observability::LOG_LEVEL_ALL,
            include_execution_data: false,
            definition: FAILING_DEFINITION,
            expected: &["ExecutionStarted", "FailStateEntered", "ExecutionFailed"],
            datum: "Boom",
            // The error the failure reported is the transition, not the payload, so a
            // configuration that excludes the execution data still delivers it.
            carries_datum: true,
            payload_delivered: false,
            whole_history: true,
        },
        Row {
            case: "an EXPRESS run that fails, logged at ERROR",
            sm_type: "EXPRESS",
            level: observability::LOG_LEVEL_ERROR,
            include_execution_data: true,
            definition: FAILING_DEFINITION,
            // A `Fail` state IS the failure, so entering it is of that tier too; the
            // run's opening event is not.
            expected: &["FailStateEntered", "ExecutionFailed"],
            datum: "Boom",
            carries_datum: true,
            // The state the run failed at was entered with the run's own input.
            payload_delivered: true,
            whole_history: false,
        },
        Row {
            case: "a STANDARD run that succeeds, logged at ALL with its data",
            sm_type: "STANDARD",
            level: observability::LOG_LEVEL_ALL,
            include_execution_data: true,
            definition: HELLO_DEFINITION,
            expected: &[
                "ExecutionStarted",
                "PassStateEntered",
                "PassStateExited",
                "ExecutionSucceeded",
            ],
            datum: r#"{"msg":"hi"}"#,
            carries_datum: true,
            payload_delivered: true,
            whole_history: true,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        // The group the configuration names, created by its account exactly as the
        // client that configures the delivery creates it.
        let group = format!("/aws/vendedlogs/states/clock-{index}");
        observed
            .logs("CreateLogGroup", &json!({ "logGroupName": group }))
            .await;

        let machine = create(
            &observed,
            &format!("clock-{index}"),
            row.definition,
            row.sm_type,
            Some(logging(
                row.level,
                &group,
                row.include_execution_data,
            )),
        )
        .await;
        // A distinct input per row, so the data a delivered event carries is this
        // run's own and not a value that happens to be there.
        let input = format!("{{\"row\":{index}}}");
        let started = observed
            .ok(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "run", "input": input }),
            )
            .await;
        let execution = string(&started, "executionArn");

        // From here on this row makes NO Step Functions call: the delivery is what a
        // client watching the log group sees, and nothing else drives it.
        let messages = delivered(&observed, &group, case).await;
        assert_eq!(
            types(&messages),
            row.expected,
            "case {case:?}: expected the event types the level selects, in history order, got \
             {messages:?}"
        );
        for message in &messages {
            assert_eq!(
                message.get("execution_arn").and_then(Value::as_str),
                Some(execution.as_str()),
                "case {case:?}: expected every delivered event to name its execution, got \
                 {message:?}"
            );
            // Every delivered event carries the details of the step it reports, so a
            // reader of any event reads that member rather than having to ask whether
            // this one has it.
            let details = message
                .get("details")
                .unwrap_or_else(|| panic!("case {case:?}: no details on {message:?}"));
            assert!(
                details.is_object(),
                "case {case:?}: expected the event's details to be an object, got {message:?}"
            );
            // The execution DATA is in it only where the configuration asks for it.
            for payload in ["input", "output"] {
                let present = details.get(payload).is_some();
                assert!(
                    !present || row.include_execution_data,
                    "case {case:?}: a configuration that excludes the execution data \
                     delivers no {payload}, got {message:?}"
                );
            }
        }
        if row.payload_delivered {
            assert!(
                messages.iter().any(|message| {
                    let details = message.get("details").unwrap_or(&Value::Null);
                    details.get("input").is_some() || details.get("output").is_some()
                }),
                "case {case:?}: expected the run's payload among the delivered details, got \
                 {messages:?}"
            );
        }
        // The opening event reports the execution's own identity as the history API
        // reports it: the input the run was started with, and the role it runs as.
        if let Some(opening) = messages
            .iter()
            .find(|message| message.get("type").and_then(Value::as_str) == Some("ExecutionStarted"))
        {
            let details = opening.get("details").unwrap_or(&Value::Null);
            assert_eq!(
                details.get("roleArn").and_then(Value::as_str),
                Some(ROLE_ARN),
                "case {case:?}: expected the opening event to name the role the run assumed, got \
                 {opening:?}"
            );
            let reported = details
                .get("input")
                .and_then(Value::as_str)
                .map(|text| text.to_string());
            let expected = row.include_execution_data.then(|| input.clone());
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the opening event's input to be {expected:?}, got \
                 {opening:?}"
            );
        }

        let delivered_ids = ids(&messages, case);
        if row.whole_history {
            let whole: Vec<i64> = (1..=delivered_ids.len() as i64).collect();
            assert_eq!(
                delivered_ids, whole,
                "case {case:?}: expected the contiguous history ids 1..N in order, got \
                 {delivered_ids:?}"
            );
        } else {
            assert!(
                delivered_ids.windows(2).all(|pair| pair[0] < pair[1]),
                "case {case:?}: expected the selected ids to keep the history's own order with \
                 no duplicates, got {delivered_ids:?}"
            );
            assert!(
                delivered_ids.first().is_some_and(|first| *first > 1),
                "case {case:?}: a level that selects a later event does not deliver the first \
                 one, got {delivered_ids:?}"
            );
        }

        // The datum is looked for among the DETAILS members alone, so the assertion is
        // about the data the configuration asks for and not about text that happens to
        // appear anywhere in the message.
        let carries = messages.iter().any(|message| {
            message
                .get("details")
                .and_then(Value::as_object)
                .is_some_and(|details| {
                    details
                        .values()
                        .filter_map(Value::as_str)
                        .any(|value| value == row.datum)
                })
        });
        assert_eq!(
            carries, row.carries_datum,
            "case {case:?}: expected the run's data to be delivered={}, got {messages:?}",
            row.carries_datum
        );

        // One stream per execution, so a reader can tell which run an event belongs to
        // from the stream alone.
        let described = observed
            .logs("DescribeLogStreams", &json!({ "logGroupName": group }))
            .await;
        let streams: Vec<String> = described
            .get("logStreams")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|stream| stream.get("logStreamName").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        assert_eq!(
            streams.len(),
            1,
            "case {case:?}: expected the one stream of the one run, got {described:?}"
        );
        assert!(
            streams[0].ends_with("/run"),
            "case {case:?}: expected the stream to name the execution it carries, got {streams:?}"
        );
    }
}

/// An EXPRESS execution's history API stays refused while the log group carries that
/// same run: the delivered events are the record, and `GetExecutionHistory` is not it.
#[tokio::test]
async fn an_express_runs_history_api_is_refused_while_its_logs_carry_it() {
    let observed = Observed::new();
    let group = "/aws/vendedlogs/states/express-history";
    observed
        .logs("CreateLogGroup", &json!({ "logGroupName": group }))
        .await;
    let machine = create(
        &observed,
        "express-history",
        HELLO_DEFINITION,
        "EXPRESS",
        Some(logging(observability::LOG_LEVEL_ALL, group, true)),
    )
    .await;
    let started = observed
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "run", "input": "{}" }),
        )
        .await;
    let execution = string(&started, "executionArn");
    // "StartExecution" on an EXPRESS machine answers the execution and the instant it
    // started, and nothing about how it ends.
    assert_eq!(
        started.get("status"),
        None,
        "expected an asynchronous start to report no status, got {started:?}"
    );
    assert!(
        execution.contains(":express:express-history:run:"),
        "expected an EXPRESS run to be named by its express execution ARN, got {execution:?}"
    );

    let messages = delivered(&observed, group, "the express run").await;
    let kinds = types(&messages);
    assert!(
        kinds.contains(&"ExecutionSucceeded".to_string()),
        "expected the run's outcome among the delivered events, got {kinds:?}"
    );

    let (status, answer) = observed
        .call("GetExecutionHistory", &json!({ "executionArn": execution }))
        .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "InvalidArn"),
        "expected the EXPRESS execution's history API to be refused, got {status} with {answer:?}"
    );
}
