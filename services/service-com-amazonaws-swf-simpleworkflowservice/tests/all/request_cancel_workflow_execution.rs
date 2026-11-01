// SPDX-License-Identifier: CC0-1.0

//! `RequestCancelWorkflowExecution`, end to end over the service's own wire protocol,
//! dispatched through the same `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: a cancel request addressed at an OPEN stored run
//! appends one `WorkflowExecutionCancelRequested` event to that run's persisted history
//! and marks the run as having a cancellation requested, WITHOUT closing it or touching
//! its open task counts — it is the decider that closes the run; and a request that
//! addresses anything other than an open stored run of that domain (unknown domain,
//! unknown `workflowId`, unknown `runId`, already-closed run) is refused with the
//! modelled `UnknownResourceFault`.
//!
//! The rows below vary what the rule generalises over: how the run is addressed
//! (explicit `runId` vs resolved from the open run), the `workflowId`'s shape and size
//! (including the modelled 256-character maximum), how many times the request is
//! repeated, and which registered defaults the run was started with. No row is
//! privileged — in particular the recorded unknown-execution case is one row of the
//! refusal table, not its definition.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::EVENT_TYPE_VALUES;

use super::workflow_execution_lifecycle::{
    full_defaults, i64_at, merge, seeded, string_at, whole_history, Emulator,
};

/// One cancel request against a freshly started run.
struct CancelCase {
    name: &'static str,
    /// Varies the `workflowId`'s shape and size; `WorkflowId` is modelled `len 1..256`.
    workflow_id: String,
    /// Whether the call names the `runId` or lets the service resolve the open run.
    pass_run_id: bool,
    /// How many times the same cancel request is repeated.
    calls: usize,
    /// The child policy the workflow type registers, so the rule is checked against
    /// runs configured differently.
    child_policy: &'static str,
}

/// `cancelRequested` as `DescribeWorkflowExecution` reports it. The member models a
/// default of `false`, so an absent member is `false`.
fn cancel_requested(described: &Value) -> bool {
    described
        .pointer("/executionInfo/cancelRequested")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[tokio::test]
async fn a_cancel_request_is_recorded_on_the_open_run_and_leaves_it_open() {
    let cases = [
        CancelCase {
            name: "explicit runId",
            workflow_id: "wf-1".to_string(),
            pass_run_id: true,
            calls: 1,
            child_policy: "TERMINATE",
        },
        CancelCase {
            name: "runId omitted, resolved from the open run",
            workflow_id: "order-42".to_string(),
            pass_run_id: false,
            calls: 1,
            child_policy: "REQUEST_CANCEL",
        },
        CancelCase {
            name: "repeated twice",
            workflow_id: "wf repeated".to_string(),
            pass_run_id: true,
            calls: 2,
            child_policy: "ABANDON",
        },
        CancelCase {
            // The modelled `WorkflowId` maximum (len 1..256).
            name: "workflowId at the modelled maximum length",
            workflow_id: "w".repeat(256),
            pass_run_id: false,
            calls: 3,
            child_policy: "TERMINATE",
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("cancel-{index}");
        seeded(
            &emulator,
            case.name,
            &domain,
            ("demo-wf", "1.0"),
            full_defaults("demo-tasks", case.child_policy),
        )
        .await;
        let run_id = string_at(
            &emulator
                .ok(
                    case.name,
                    "StartWorkflowExecution",
                    &json!({
                        "domain": domain,
                        "workflowId": case.workflow_id,
                        "workflowType": { "name": "demo-wf", "version": "1.0" },
                    }),
                )
                .await,
            "/runId",
        );

        let describe = json!({
            "domain": domain,
            "execution": { "workflowId": case.workflow_id, "runId": run_id },
        });
        let before = emulator
            .ok(case.name, "DescribeWorkflowExecution", &describe)
            .await;
        assert!(
            !cancel_requested(&before),
            "case {:?}: a freshly started run has no cancellation requested, got {before:?}",
            case.name
        );
        let events_before =
            whole_history(&emulator, &domain, &case.workflow_id, &run_id).await;

        let mut body = json!({ "domain": domain, "workflowId": case.workflow_id });
        if case.pass_run_id {
            merge(&mut body, json!({ "runId": run_id }));
        }
        // The history as the FIRST accepted request left it; every repeat must leave it
        // exactly so (the request is a latch, so a repeat appends nothing at all).
        let mut latched: Option<Vec<Value>> = None;
        for call in 1..=case.calls {
            let answered = emulator
                .ok(case.name, "RequestCancelWorkflowExecution", &body)
                .await;
            assert_eq!(
                answered,
                json!({}),
                "case {:?} call {call}: RequestCancelWorkflowExecution's output shape is \
                 Unit, got {answered:?}",
                case.name
            );
            let events = whole_history(&emulator, &domain, &case.workflow_id, &run_id).await;
            match &latched {
                None => latched = Some(events),
                Some(first) => assert_eq!(
                    &events, first,
                    "case {:?} call {call}: a repeated cancel request records nothing \
                     further \u{2014} not even a second event \u{2014} got {events:?}",
                    case.name
                ),
            }
        }

        let after = emulator
            .ok(case.name, "DescribeWorkflowExecution", &describe)
            .await;
        assert!(
            cancel_requested(&after),
            "case {:?}: the stored run must report cancelRequested, got {after:?}",
            case.name
        );
        assert_eq!(
            string_at(&after, "/executionInfo/executionStatus"),
            "OPEN",
            "case {:?}: a cancel request does not close the run — the decider does, \
             got {after:?}",
            case.name
        );
        assert_eq!(
            after.pointer("/executionInfo/closeStatus"),
            None,
            "case {:?}: an open run carries no closeStatus, got {after:?}",
            case.name
        );
        assert_eq!(
            after.pointer("/executionInfo/closeTimestamp"),
            None,
            "case {:?}: an open run carries no closeTimestamp, got {after:?}",
            case.name
        );
        assert_eq!(
            i64_at(&after, "/openCounts/openDecisionTasks"),
            i64_at(&before, "/openCounts/openDecisionTasks"),
            "case {:?}: a cancel request consumes no task, got {after:?}",
            case.name
        );

        // The request "records a WorkflowExecutionCancelRequested event in the currently
        // running workflow execution", and it is a LATCH: the run carries exactly one of
        // them however many times the request is repeated (idempotent — a repeat is
        // accepted and appends nothing), with every earlier event untouched.
        let events_after = whole_history(&emulator, &domain, &case.workflow_id, &run_id).await;
        assert_eq!(
            events_after.len(),
            events_before.len() + 1,
            "case {:?}: {} cancel request(s) record exactly one cancel-requested event, \
             got {events_after:?}",
            case.name,
            case.calls
        );
        assert_eq!(
            &events_after[..events_before.len()],
            &events_before[..],
            "case {:?}: the events recorded before the cancel request are unchanged, \
             got {events_after:?}",
            case.name
        );
        for (offset, appended) in events_after[events_before.len()..].iter().enumerate() {
            let event_type = string_at(appended, "/eventType");
            assert_eq!(
                event_type, "WorkflowExecutionCancelRequested",
                "case {:?} appended event {offset}: expected a \
                 WorkflowExecutionCancelRequested event, got {appended:?}",
                case.name
            );
            assert!(
                EVENT_TYPE_VALUES.contains(&event_type.as_str()),
                "case {:?} appended event {offset}: the eventType must be in the model's \
                 EventType catalogue, got {event_type:?}",
                case.name
            );
            assert_eq!(
                i64_at(appended, "/eventId"),
                events_before.len() as i64 + offset as i64 + 1,
                "case {:?} appended event {offset}: eventIds stay contiguous and \
                 increasing, got {appended:?}",
                case.name
            );
            // A repeat must not re-timestamp the latched event either.
            assert!(
                appended.pointer("/eventTimestamp").is_some(),
                "case {:?} appended event {offset}: every stored event carries its \
                 timestamp, got {appended:?}",
                case.name
            );
            let attributes = appended
                .pointer("/workflowExecutionCancelRequestedEventAttributes")
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?} appended event {offset}: no cancel-requested attributes \
                         in {appended:?}",
                        case.name
                    )
                });
            // `externalWorkflowExecution`/`externalInitiatedEventId` describe a request
            // made by another execution's RequestCancelExternalWorkflowExecution
            // decision, and `cause` is set only when SWF generated the request itself;
            // this request came straight from the caller.
            assert_eq!(
                (
                    attributes.pointer("/externalWorkflowExecution"),
                    attributes.pointer("/cause"),
                ),
                (None, None),
                "case {:?} appended event {offset}: a caller's own request names no \
                 external execution and no cause, got {attributes:?}",
                case.name
            );
            assert_eq!(
                attributes
                    .pointer("/externalInitiatedEventId")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                0,
                "case {:?} appended event {offset}: there is no initiating event, so the \
                 member keeps its modelled default of 0, got {attributes:?}",
                case.name
            );
        }

        // The run is still cancellable AND still terminable: the request changed the
        // record, not the run's availability.
        emulator
            .ok(
                case.name,
                "TerminateWorkflowExecution",
                &json!({ "domain": domain, "workflowId": case.workflow_id, "runId": run_id }),
            )
            .await;
        let closed = emulator
            .ok(case.name, "DescribeWorkflowExecution", &describe)
            .await;
        assert_eq!(
            (
                string_at(&closed, "/executionInfo/executionStatus"),
                cancel_requested(&closed)
            ),
            ("CLOSED".to_string(), false),
            "case {:?}: `cancelRequested` reports a cancellation still outstanding against \
             a running execution, so a run that is no longer OPEN reports none, got \
             {closed:?}",
            case.name
        );
        // What the run was ASKED is history, and history does not change when the run
        // closes: the cancel-requested event the request recorded is still there.
        let events_closed = whole_history(&emulator, &domain, &case.workflow_id, &run_id).await;
        assert_eq!(
            events_closed
                .iter()
                .filter(|event| event
                    .pointer("/eventType")
                    .and_then(Value::as_str)
                    .is_some_and(|event_type| event_type == "WorkflowExecutionCancelRequested"))
                .count(),
            1,
            "case {:?}: the cancel-requested event stays in the closed run's history, \
             got {events_closed:?}",
            case.name
        );
        // A closed run is no longer a "currently running workflow execution", by either
        // addressing form.
        for repeat in [
            json!({ "domain": domain, "workflowId": case.workflow_id, "runId": run_id }),
            json!({ "domain": domain, "workflowId": case.workflow_id }),
        ] {
            emulator
                .refused(
                    case.name,
                    "RequestCancelWorkflowExecution",
                    &repeat,
                    "UnknownResourceFault",
                )
                .await;
        }
    }
}

#[tokio::test]
async fn a_cancel_request_for_anything_but_an_open_stored_run_is_an_unknown_resource() {
    let emulator = Emulator::new();
    seeded(
        &emulator,
        "unknown",
        "known",
        ("demo-wf", "1.0"),
        full_defaults("demo-tasks", "TERMINATE"),
    )
    .await;
    let run_id = string_at(
        &emulator
            .ok(
                "unknown",
                "StartWorkflowExecution",
                &json!({
                    "domain": "known",
                    "workflowId": "wf-1",
                    "workflowType": { "name": "demo-wf", "version": "1.0" },
                }),
            )
            .await,
        "/runId",
    );
    // A second domain with a run of the SAME workflowId, so "resolved in the named
    // domain" is checked and not just "resolved anywhere".
    seeded(
        &emulator,
        "unknown",
        "other",
        ("demo-wf", "1.0"),
        full_defaults("demo-tasks", "TERMINATE"),
    )
    .await;
    let other_run_id = string_at(
        &emulator
            .ok(
                "unknown",
                "StartWorkflowExecution",
                &json!({
                    "domain": "other",
                    "workflowId": "wf-1",
                    "workflowType": { "name": "demo-wf", "version": "1.0" },
                }),
            )
            .await,
        "/runId",
    );

    let cases: Vec<(&str, Value)> = vec![
        (
            "unknown domain",
            json!({ "domain": "nope", "workflowId": "wf-1" }),
        ),
        (
            "unknown domain, explicit runId",
            json!({ "domain": "nope", "workflowId": "wf-1", "runId": run_id }),
        ),
        (
            "unknown workflowId",
            json!({ "domain": "known", "workflowId": "wf-9" }),
        ),
        (
            "unknown runId",
            json!({
                "domain": "known",
                "workflowId": "wf-1",
                "runId": "0".repeat(64),
            }),
        ),
        (
            "another domain's runId",
            json!({ "domain": "known", "workflowId": "wf-1", "runId": other_run_id }),
        ),
    ];
    for (case, body) in &cases {
        emulator
            .refused(case, "RequestCancelWorkflowExecution", body, "UnknownResourceFault")
            .await;
    }

    // None of the refusals touched the stored run: it is still open, still has no
    // cancellation recorded, and its history is untouched.
    let described = emulator
        .ok(
            "unknown",
            "DescribeWorkflowExecution",
            &json!({
                "domain": "known",
                "execution": { "workflowId": "wf-1", "runId": run_id },
            }),
        )
        .await;
    assert_eq!(
        (
            string_at(&described, "/executionInfo/executionStatus"),
            cancel_requested(&described)
        ),
        ("OPEN".to_string(), false),
        "case \"refusals\": a refused cancel request writes nothing, got {described:?}"
    );
    let events = whole_history(&emulator, "known", "wf-1", &run_id).await;
    assert_eq!(
        events.len(),
        2,
        "case \"refusals\": a refused cancel request appends no event, got {events:?}"
    );
}

#[tokio::test]
async fn a_workflow_id_the_naming_rule_rejects_is_a_validation_error() {
    let emulator = Emulator::new();
    seeded(
        &emulator,
        "naming-rule",
        "naming-rule",
        ("demo-wf", "1.0"),
        full_defaults("demo-tasks", "TERMINATE"),
    )
    .await;

    // SWF's naming rule, stated on `workflowId` itself: "The specified string must not
    // start or end with whitespace. It must not contain a : (colon), / (slash), |
    // (vertical bar), or any control characters (\u0000-\u001f | \u007f-\u009f). Also,
    // it must not be the literal string arn." One row per clause, plus a name the rule
    // allows (checked to reach the resource lookup instead, i.e. 400
    // UnknownResourceFault, not a validation error).
    for rejected in [
        " leading",
        "trailing ",
        "has:colon",
        "has/slash",
        "has|bar",
        "has\u{1}control",
        "has\u{7f}control",
        "arn",
    ] {
        emulator
            .refused(
                rejected,
                "RequestCancelWorkflowExecution",
                &json!({ "domain": "naming-rule", "workflowId": rejected }),
                "ValidationException",
            )
            .await;
    }
    emulator
        .refused(
            "a legal but unknown workflowId",
            "RequestCancelWorkflowExecution",
            &json!({ "domain": "naming-rule", "workflowId": "legal name_1.0" }),
            "UnknownResourceFault",
        )
        .await;
}
