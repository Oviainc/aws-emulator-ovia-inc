// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MEMBERS of the execution-history events a state machine's
//! `loggingConfiguration` vends to CloudWatch Logs, driven over each service's own wire
//! protocol through the production `MultiServiceRouter` with Step Functions and
//! CloudWatch Logs mounted on ONE engine — so the events asserted here are the ones
//! CloudWatch Logs' own reader answers with.
//!
//! The rule under test: a vended event MIRRORS the history event it was made from. Every
//! one of them carries its own identity as TEXT — `id`, `type`, `previous_event_id`,
//! `event_timestamp`, `execution_arn` and `redrive_count` ("0" for an execution that has
//! never been redriven) — and a `details` object holding exactly what
//! `GetExecutionHistory` reports as that event type's own detail, so a consumer can join
//! the two event for event. `includeExecutionData` decides the PAYLOAD inside `details`
//! (the document a step was handed or produced, and the `*Details` member that says
//! whether it was truncated): delivered when it asks for the data, left out when it does
//! not, with everything else about the event delivered either way.
//!
//! The rows differ along what the rule generalises over: `includeExecutionData` true and
//! false, and — within one run — every event type a definition of a `Pass` and a `Fail`
//! state produces (`ExecutionStarted`, `PassStateEntered`, `PassStateExited`,
//! `FailStateEntered`, `ExecutionFailed`), each compared against ITS OWN history event
//! rather than against a document written into this suite.
//!
//! The harness (`Observed`, `create`, `logging`) is the one `execution_observability`
//! builds for the same two services, and the history reader is `execution_history`'s;
//! `tests/all.rs` is this binary's crate root, so both are reached through
//! `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::observability;

use crate::execution_observability::{create, logging, Observed};
use crate::state_machine_lifecycle::{string, PAST_THE_EXECUTION_DELAY};

/// A definition whose run produces one state's entry and exit and then fails, so one
/// execution covers an execution-level event, a state-level pair and a failure.
const PASS_THEN_FAIL: &str = r#"{"StartAt":"Note","States":{
    "Note":{"Type":"Pass","Result":{"noted":true},"Next":"Nope"},
    "Nope":{"Type":"Fail","Error":"Boom","Cause":"the row fails on purpose"}}}"#;

/// The members every vended event carries beside its `details`, each as TEXT.
const ENVELOPE_MEMBERS: &[&str] = &[
    "id",
    "type",
    "previous_event_id",
    "event_timestamp",
    "execution_arn",
    "redrive_count",
];

/// The members of a `details` object that carry the execution's own PAYLOAD, which
/// `includeExecutionData` decides the delivery of.
const PAYLOAD_MEMBERS: &[&str] = &[
    "input",
    "inputDetails",
    "output",
    "outputDetails",
    "parameters",
];

/// The `redrive_count` an execution that has never been redriven reports.
const FIRST_RUN_REDRIVE_COUNT: &str = "0";

/// The detail object a history event reports, read as the ONE `*EventDetails` member that
/// event carries — or `None` for an event type the model gives no detail structure.
///
/// The `__type` discriminator an `awsJson` response carries on a nested structure is
/// dropped: it is a fact of that PROTOCOL, and a vended log event is a JSON document
/// rather than a response to an API call, so the comparison is over the members the two
/// both carry.
fn history_details(event: &Value) -> Option<Value> {
    event
        .as_object()?
        .iter()
        .find(|(member, value)| member.ends_with("EventDetails") && value.is_object())
        .map(|(_, value)| without_discriminators(value))
}

/// The member an `awsJson` response names a nested structure's shape in.
const TYPE_DISCRIMINATOR: &str = "__type";

/// `value` with every [`TYPE_DISCRIMINATOR`] member taken out, at every depth.
fn without_discriminators(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(member, _)| member.as_str() != TYPE_DISCRIMINATOR)
                .map(|(member, held)| (member.clone(), without_discriminators(held)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_discriminators).collect()),
        other => other.clone(),
    }
}

/// `details` with the payload members taken out, which is what a configuration that
/// excludes execution data delivers.
fn without_payload(details: &Value) -> Value {
    let mut object = details.as_object().cloned().unwrap_or_default();
    for member in PAYLOAD_MEMBERS {
        object.remove(*member);
    }
    Value::Object(object)
}

/// Every event the group `name` holds, as the parsed JSON message of each, in the order
/// CloudWatch Logs reports them.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so a sibling
/// suite that waits for a delivery to land reads the group through this one reader
/// rather than carrying a second copy of it.
pub(crate) async fn delivered(observed: &Observed, case: &str, name: &str) -> Vec<Value> {
    let found = observed
        .logs("FilterLogEvents", &json!({ "logGroupName": name }))
        .await;
    found
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|event| event.get("message").and_then(Value::as_str))
        .map(|message| {
            serde_json::from_str(message).unwrap_or_else(|error| {
                panic!("case {case:?}: a delivered event is a JSON object, got {error} for {message:?}")
            })
        })
        .collect()
}

/// A vended event mirrors the history event of the same id: the envelope members are
/// strings naming that event and its execution, and `details` is the history's own detail
/// object with the payload members delivered only where `includeExecutionData` asks for
/// them.
#[tokio::test]
async fn every_vended_event_mirrors_its_history_event_member_for_member() {
    let observed = Observed::new();
    for (index, (case, include)) in [
        ("includeExecutionData asks for the data", true),
        ("includeExecutionData excludes it", false),
    ]
    .into_iter()
    .enumerate()
    {
        let group = format!("/aws/vendedlogs/states/mirror-{index}");
        let machine = create(
            &observed,
            &format!("mirror-{index}"),
            PASS_THEN_FAIL,
            "STANDARD",
            Some(logging(observability::LOG_LEVEL_ALL, &group, include)),
        )
        .await;
        let started = observed
            .ok(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": "run",
                    "input": r#"{"marker":"mirrored"}"#,
                }),
            )
            .await;
        let execution = string(&started, "executionArn");
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        observed
            .ok("DescribeExecution", &json!({ "executionArn": execution }))
            .await;

        // The history the vended events must mirror, read with the execution data so the
        // payload members are there to be compared against whichever way the
        // configuration is set.
        let answered = observed
            .ok(
                "GetExecutionHistory",
                &json!({ "executionArn": execution, "includeExecutionData": true }),
            )
            .await;
        let events = answered
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            !events.is_empty(),
            "case {case:?}: expected the run to have a history, got {answered:?}"
        );

        let vended = delivered(&observed, case, &group).await;
        assert_eq!(
            vended.len(),
            events.len(),
            "case {case:?}: level ALL delivers every history event, got {vended:?} for {events:?}"
        );

        for event in &events {
            let id = event.get("id").and_then(Value::as_i64).unwrap_or_default();
            let kind = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let row = format!("{case}: event {id} ({kind})");
            let delivered_event = vended
                .iter()
                .find(|candidate| {
                    candidate.get("id").and_then(Value::as_str) == Some(id.to_string().as_str())
                })
                .unwrap_or_else(|| {
                    panic!("case {row:?}: expected a delivered event of that id, got {vended:?}")
                });

            // The envelope: every member present, every one of them TEXT.
            for member in ENVELOPE_MEMBERS {
                let value = delivered_event.get(*member);
                assert!(
                    value.and_then(Value::as_str).is_some(),
                    "case {row:?}: expected {member} as text, got {value:?}"
                );
            }
            assert_eq!(
                delivered_event.get("type").and_then(Value::as_str),
                Some(kind.as_str()),
                "case {row:?}: expected the history event's type, got {delivered_event:?}"
            );
            assert_eq!(
                delivered_event
                    .get("previous_event_id")
                    .and_then(Value::as_str),
                Some(
                    event
                        .get("previousEventId")
                        .and_then(Value::as_i64)
                        .unwrap_or_default()
                        .to_string()
                        .as_str()
                ),
                "case {row:?}: expected the history event's previousEventId, got \
                 {delivered_event:?}"
            );
            assert_eq!(
                delivered_event.get("execution_arn").and_then(Value::as_str),
                Some(execution.as_str()),
                "case {row:?}: expected the event to name its execution, got {delivered_event:?}"
            );
            assert_eq!(
                delivered_event.get("redrive_count").and_then(Value::as_str),
                Some(FIRST_RUN_REDRIVE_COUNT),
                "case {row:?}: expected the redrive count of a first run, got {delivered_event:?}"
            );

            // The detail: the history's own object, with the payload members delivered
            // only where the configuration asks for the execution data.
            let want = history_details(event).map(|details| {
                if include {
                    details
                } else {
                    without_payload(&details)
                }
            });
            assert_eq!(
                delivered_event.get("details"),
                want.as_ref(),
                "case {row:?}: expected the history event's details, got {delivered_event:?}"
            );
        }

        // The whole point of the payload half of the rule, read off the run: the document
        // the execution was started with reaches the log only where the configuration asks
        // for the execution data.
        let carries_payload = vended
            .iter()
            .any(|event| event.to_string().contains("mirrored"));
        assert_eq!(
            carries_payload, include,
            "case {case:?}: expected the execution data to be delivered={include}, got {vended:?}"
        );
    }
}
