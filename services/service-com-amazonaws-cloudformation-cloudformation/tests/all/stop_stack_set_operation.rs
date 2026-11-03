// SPDX-License-Identifier: CC0-1.0

//! `StopStackSetOperation` over the awsQuery wire: stopping the operation a StackSet still
//! has in flight.
//!
//! The rule every case here holds the service to: **a stop acts on the operation record the
//! StackSet named by the request PERSISTS — resolved by name or by id within the
//! administrator account and Region the request was signed for — and only while that
//! operation has not concluded: it moves the operation to `STOPPING`, cancels the stack
//! instance deployments of that operation which had not started and leaves the ones already
//! in progress to complete, after which the clock concludes the operation `STOPPED` and
//! releases the StackSet's single-flight lock; an operation already in a terminal status is
//! refused `400 InvalidOperationException`, an id the StackSet holds no operation for `404
//! OperationNotFoundException` and a `StackSetName` that resolves to no StackSet `404
//! StackSetNotFoundException`, none of which changes the record.**
//!
//! The tables below span what the rule generalises over: all six `StackSetOperationStatus`
//! values (`smithy_query(query="shape", shape="StackSetOperationStatus")`, which
//! `state::stack_sets::operation_status` names) for what a stop accepts and refuses, all five
//! calls that start a StackSet operation — and so all four `StackSetOperationAction` values —
//! for what the stop does to one, and both ways a request addresses a StackSet.
//!
//! The statuses a wire call cannot leave an operation in (`QUEUED`, a terminal `FAILED`) are
//! seeded straight into the store through `state::...::record_stack_set`, the device
//! `list_stack_set_operations.rs` uses for the same reason. Every call goes through the real
//! request path (form-encoded body in, XML envelope out), so what is asserted is what a
//! client sees.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    detailed_status, drift_detection_status, instance_status, operation_action, operation_status,
    PendingOperationOutcome, StoredStackInstance, StoredStackSet, StoredStackSetOperation,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, member, router_with_engine, store, wait_for_the_transition, xml_text,
    ACCOUNT_ID, REGION,
};

/// Another Region, for the case that holds a StackSet to the Region it was created in.
const OTHER_REGION: &str = "us-west-2";

/// A second target account beside the emulator's own (`Account` is `@pattern("^[0-9]{12}$")`).
const ACCOUNT_A: &str = "111111111111";

/// The template a StackSet is created from here: one topic, really created in Amazon SNS by
/// each instance's stack.
const TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"stopped-fleet"}}}}"#;

/// How far out a held-open operation's conclusion is pushed: long enough that no row's own
/// work can reach it.
const HELD_OPEN: Duration = Duration::from_secs(600);

/// The `InvalidOperationException` message a stop of an operation that has already ended is
/// refused with, spelled out here rather than read from the crate: the sentence is the
/// requirement, so a reworded one must fail this suite. The status is the record's own.
fn cannot_be_stopped(status: &str) -> String {
    format!("Cannot Stop since Operation is in {status}")
}

/// `StopStackSetOperation` for one operation of one StackSet, signed for `REGION`.
async fn stop(
    router: &MultiServiceRouter,
    name_or_id: &str,
    operation_id: &str,
) -> (u16, String) {
    call(
        router,
        "StopStackSetOperation",
        &[
            member("StackSetName", name_or_id),
            member("OperationId", operation_id),
        ],
    )
    .await
}

/// `DescribeStackSetOperation`, which is what a caller monitors a stop through.
async fn describe_operation(
    router: &MultiServiceRouter,
    name_or_id: &str,
    operation_id: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackSetOperation",
        &[
            member("StackSetName", name_or_id),
            member("OperationId", operation_id),
        ],
    )
    .await
}

/// The `Status` `DescribeStackSetOperation` reports for one operation.
async fn reported_status(
    case: &str,
    router: &MultiServiceRouter,
    name_or_id: &str,
    operation_id: &str,
) -> String {
    let (http_status, body) = describe_operation(router, name_or_id, operation_id).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStackSetOperation to succeed, got {http_status} {body}"
    );
    xml_text(&body, "Status")
        .unwrap_or_else(|| panic!("case {case:?}: no Status in {body}"))
}

/// `DescribeStackInstance` for one account and Region pair.
async fn describe_instance(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    account: &str,
    region: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackInstance",
        &[
            member("StackSetName", stack_set_name),
            member("StackInstanceAccount", account),
            member("StackInstanceRegion", region),
        ],
    )
    .await
}

/// The `OperationId` a call that starts a StackSet operation answered with.
fn operation_id_of(case: &str, http_status: u16, body: &str) -> String {
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected the call to succeed, got {http_status} {body}"
    );
    xml_text(body, "OperationId")
        .unwrap_or_else(|| panic!("case {case:?}: no OperationId in {body}"))
}

/// The `<Code>` and `<Message>` of a refusal.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (xml_text(body, "Code"), xml_text(body, "Message"))
}

/// Hold one in-flight operation of a StackSet open, so the stop under test lands on an
/// operation that really has not concluded.
///
/// This store concludes a StackSet operation one transition span after the call that started
/// it — half a second, which is shorter than a row's own wire work — so pushing the instant
/// that conclusion is due out is what makes a row deterministic. It is the only thing a row
/// reaches into the store for, and it moves nothing else: the stop itself is what re-dates
/// the conclusion, so a stopped operation still settles on the clock.
///
/// `pub(crate)` because `stack_set_operation_tokens_and_outcomes.rs` holds an operation open
/// for the same reason, and one copy of this device is enough for the target.
pub(crate) fn hold_the_operation_open(
    engine: &Arc<engine::InMemoryEngine>,
    stack_set_name: &str,
    operation_id: &str,
) {
    let state = store(engine);
    let mut held = state.lock();
    let stack_set = held
        .resolve_stack_set_mut(REGION, stack_set_name)
        .unwrap_or_else(|| panic!("the store holds StackSet {stack_set_name}"));
    let operation = stack_set
        .operations
        .iter_mut()
        .find(|operation| operation.operation_id == operation_id)
        .unwrap_or_else(|| panic!("StackSet {stack_set_name} holds operation {operation_id}"));
    operation.status = operation_status::RUNNING.to_string();
    operation.end = None;
    operation.pending = Some(PendingOperationOutcome {
        status: operation_status::SUCCEEDED.to_string(),
        status_reason: None,
        failed_stack_instances_count: Some(0),
        due_at: SystemTime::now() + HELD_OPEN,
    });
}

/// The instant one operation's conclusion is due, as the store holds it, or `None` for an
/// operation that has already concluded.
fn conclusion_due_at(
    engine: &Arc<engine::InMemoryEngine>,
    stack_set_name: &str,
    operation_id: &str,
) -> Option<SystemTime> {
    let state = store(engine);
    let mut held = state.lock();
    held.resolve_stack_set_mut(REGION, stack_set_name)?
        .operation(operation_id)?
        .pending
        .as_ref()
        .map(|pending| pending.due_at)
}

/// One row of the status table: the status the StackSet's operation is in, and whether a stop
/// may act on an operation in it.
struct StatusRow {
    case: &'static str,
    stack_set: &'static str,
    status: &'static str,
    stoppable: bool,
}

/// One row per `StackSetOperationStatus`: the three statuses an operation that has not
/// concluded holds, and the three terminal ones.
const STATUS_ROWS: &[StatusRow] = &[
    StatusRow {
        case: "an operation being performed",
        stack_set: "fleet-running",
        status: operation_status::RUNNING,
        stoppable: true,
    },
    StatusRow {
        case: "an operation queued behind a sequence",
        stack_set: "fleet-queued",
        status: operation_status::QUEUED,
        stoppable: true,
    },
    StatusRow {
        case: "an operation already stopping",
        stack_set: "fleet-stopping",
        status: operation_status::STOPPING,
        stoppable: true,
    },
    StatusRow {
        case: "an operation that completed",
        stack_set: "fleet-succeeded",
        status: operation_status::SUCCEEDED,
        stoppable: false,
    },
    StatusRow {
        case: "an operation that exceeded its failure tolerance",
        stack_set: "fleet-failed",
        status: operation_status::FAILED,
        stoppable: false,
    },
    StatusRow {
        case: "an operation the caller already stopped",
        stack_set: "fleet-stopped",
        status: operation_status::STOPPED,
        stoppable: false,
    },
];

/// The operation id one status row's StackSet holds.
fn row_operation_id(row: &StatusRow) -> String {
    format!("operation-{}", row.status.to_lowercase())
}

/// One status row as the record the store holds: an operation that has not concluded carries
/// the conclusion the clock owes it, held open; a terminal one carries none.
fn stored_operation(row: &StatusRow, now: SystemTime) -> StoredStackSetOperation {
    let mut operation =
        StoredStackSetOperation::running(row_operation_id(row), operation_action::CREATE, now);
    operation.status = row.status.to_string();
    if row.stoppable {
        operation.pending = Some(PendingOperationOutcome {
            status: operation_status::SUCCEEDED.to_string(),
            status_reason: None,
            failed_stack_instances_count: Some(0),
            due_at: now + HELD_OPEN,
        });
    } else {
        operation.pending = None;
        operation.end = Some(now);
    }
    operation
}

#[tokio::test]
async fn a_stop_acts_on_an_operation_that_has_not_concluded_and_refuses_one_that_has() {
    let (router, engine) = router_with_engine();
    let now = SystemTime::now();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for row in STATUS_ROWS {
            let mut stack_set = StoredStackSet::new(row.stack_set, REGION);
            stack_set.record_operation(stored_operation(row, now));
            held.record_stack_set(stack_set);
        }
    }

    for row in STATUS_ROWS {
        let case = row.case;
        let operation_id = row_operation_id(row);
        let due_before = conclusion_due_at(&engine, row.stack_set, &operation_id);
        let (http_status, body) = stop(&router, row.stack_set, &operation_id).await;
        if !row.stoppable {
            assert_eq!(
                http_status, 400,
                "case {case:?}: expected the stop to be refused with 400, got {http_status} \
                 {body}"
            );
            let (code, message) = refusal(&body);
            assert_eq!(
                code.as_deref(),
                Some("InvalidOperationException"),
                "case {case:?}: expected the modelled refusal, got {body}"
            );
            let expected = cannot_be_stopped(row.status);
            assert_eq!(
                message.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            // The refusal changed nothing: the operation still reports the status it held.
            let reported = reported_status(case, &router, row.stack_set, &operation_id).await;
            assert_eq!(
                reported, row.status,
                "case {case:?}: expected the refused stop to leave the status alone, got \
                 {reported:?}"
            );
            continue;
        }

        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the stop to be served, got {http_status} {body}"
        );
        let reported = reported_status(case, &router, row.stack_set, &operation_id).await;
        assert_eq!(
            reported,
            operation_status::STOPPING,
            "case {case:?}: expected the operation to be stopping, got {reported:?}"
        );
        let due_after = conclusion_due_at(&engine, row.stack_set, &operation_id);
        if row.status == operation_status::STOPPING {
            // A stop of an operation ALREADY stopping is the stop being carried out: it
            // writes nothing, so repeating the call cannot push the conclusion it owes away.
            assert_eq!(
                due_after, due_before,
                "case {case:?}: expected the second stop to leave the conclusion where it \
                 was, got {due_after:?}"
            );
            continue;
        }
        assert!(
            due_after < due_before,
            "case {case:?}: expected the stop to bring the conclusion forward from \
             {due_before:?}, got {due_after:?}"
        );

        // "STOPPED: The user has canceled the operation", on the clock rather than on a read.
        wait_for_the_transition().await;
        let (http_status, body) = describe_operation(&router, row.stack_set, &operation_id).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DescribeStackSetOperation to succeed, got {http_status} \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(operation_status::STOPPED),
            "case {case:?}: expected the stop to conclude the operation, got {body}"
        );
        assert!(
            xml_text(&body, "EndTimestamp").is_some(),
            "case {case:?}: expected the concluded operation to carry an end instant, got \
             {body}"
        );
    }
}

/// One row of the not-found table: the call a row makes, and the refusal it must answer.
struct MissingRow {
    case: &'static str,
    stack_set: String,
    operation_id: String,
    region: &'static str,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_stop_that_resolves_nothing_names_what_it_could_not_find() {
    const STACK_SET: &str = "fleet-addressed";
    const OPERATION: &str = "operation-addressed";
    let (router, engine) = router_with_engine();
    let stack_set_id = {
        let state = store(&engine);
        let mut held = state.lock();
        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        stack_set.record_operation(StoredStackSetOperation::running(
            OPERATION,
            operation_action::UPDATE,
            SystemTime::now(),
        ));
        let stack_set_id = stack_set.stack_set_id.clone();
        held.record_stack_set(stack_set);
        stack_set_id
    };

    let rows = vec![
        MissingRow {
            case: "a StackSet name nothing holds",
            stack_set: "fleet-never-created".to_string(),
            operation_id: OPERATION.to_string(),
            region: REGION,
            code: "StackSetNotFoundException",
            message: "StackSet fleet-never-created not found".to_string(),
        },
        MissingRow {
            case: "a StackSet of another Region",
            stack_set: STACK_SET.to_string(),
            operation_id: OPERATION.to_string(),
            region: OTHER_REGION,
            code: "StackSetNotFoundException",
            message: format!("StackSet {STACK_SET} not found"),
        },
        MissingRow {
            case: "a StackSet id of another Region",
            stack_set: stack_set_id.clone(),
            operation_id: OPERATION.to_string(),
            region: OTHER_REGION,
            code: "StackSetNotFoundException",
            message: format!("StackSet {stack_set_id} not found"),
        },
        MissingRow {
            case: "an operation id the StackSet holds no record for",
            stack_set: STACK_SET.to_string(),
            operation_id: "operation-never-started".to_string(),
            region: REGION,
            code: "OperationNotFoundException",
            message: "Operation not found".to_string(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let (http_status, body) = call_in(
            &router,
            row.region,
            "StopStackSetOperation",
            &[
                member("StackSetName", &row.stack_set),
                member("OperationId", &row.operation_id),
            ],
        )
        .await;
        assert_eq!(
            http_status, 404,
            "case {case:?}: expected the stop to be refused with 404, got {http_status} {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some(row.code),
            "case {case:?}: expected the modelled refusal, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(row.message.as_str()),
            "case {case:?}: expected {:?}, got {message:?}",
            row.message
        );
    }

    // The same operation, addressed the two ways `StackSetName` addresses a StackSet in the
    // Region that holds it: "the name or unique ID of the StackSet".
    for (case, name_or_id) in [
        ("by the StackSet's id", stack_set_id.as_str()),
        ("by the StackSet's name", STACK_SET),
    ] {
        let (http_status, body) = stop(&router, name_or_id, OPERATION).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the stop to be served, got {http_status} {body}"
        );
        let reported = reported_status(case, &router, STACK_SET, OPERATION).await;
        assert_eq!(
            reported,
            operation_status::STOPPING,
            "case {case:?}: expected the operation to be stopping, got {reported:?}"
        );
    }
}

/// One row of the in-flight table: the call that starts the operation to stop, and the action
/// the record it writes must report.
struct StartedRow {
    case: &'static str,
    stack_set: &'static str,
    action: &'static str,
    starts: &'static str,
    members: Vec<(String, String)>,
}

/// `CreateStackSet` with the suite's template, then one instance deployed and settled: the
/// StackSet each in-flight row starts its operation on.
async fn stack_set_with_an_instance(router: &MultiServiceRouter, stack_set_name: &str) {
    let (http_status, body) = call(
        router,
        "CreateStackSet",
        &[
            member("StackSetName", stack_set_name),
            member("TemplateBody", TEMPLATE),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet({stack_set_name}) to succeed, got {http_status} {body}"
    );
    let (http_status, body) = call(
        router,
        "CreateStackInstances",
        &[
            member("StackSetName", stack_set_name),
            member("Accounts.member.1", ACCOUNT_ID),
            member("Regions.member.1", REGION),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackInstances({stack_set_name}) to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
}

/// One mutation of a StackSet, for the two probes of its single-flight lock: refused while
/// the stop is in progress, served once the stop concluded. `UpdateStackSet` with the
/// template already in force is the mutation that needs nothing of the instances — a row
/// whose stopped operation DELETED the StackSet's only instance has none left.
async fn probe_the_lock(router: &MultiServiceRouter, stack_set_name: &str) -> (u16, String) {
    call(
        router,
        "UpdateStackSet",
        &[
            member("StackSetName", stack_set_name),
            member("UsePreviousTemplate", "true"),
        ],
    )
    .await
}

#[tokio::test]
async fn a_stop_ends_the_operation_any_mutation_started_and_then_frees_the_stack_set() {
    let rows = vec![
        StartedRow {
            case: "the deployment of a second instance",
            stack_set: "fleet-create",
            action: operation_action::CREATE,
            starts: "CreateStackInstances",
            members: vec![
                member("StackSetName", "fleet-create"),
                member("Accounts.member.1", ACCOUNT_A),
                member("Regions.member.1", REGION),
            ],
        },
        StartedRow {
            case: "an update of the StackSet",
            stack_set: "fleet-update-set",
            action: operation_action::UPDATE,
            starts: "UpdateStackSet",
            members: vec![
                member("StackSetName", "fleet-update-set"),
                member("UsePreviousTemplate", "true"),
            ],
        },
        StartedRow {
            case: "an update of one instance",
            stack_set: "fleet-update-instances",
            action: operation_action::UPDATE,
            starts: "UpdateStackInstances",
            members: vec![
                member("StackSetName", "fleet-update-instances"),
                member("Accounts.member.1", ACCOUNT_ID),
                member("Regions.member.1", REGION),
            ],
        },
        StartedRow {
            case: "a deletion of one instance",
            stack_set: "fleet-delete-instances",
            action: operation_action::DELETE,
            starts: "DeleteStackInstances",
            members: vec![
                member("StackSetName", "fleet-delete-instances"),
                member("RetainStacks", "true"),
                member("Accounts.member.1", ACCOUNT_ID),
                member("Regions.member.1", REGION),
            ],
        },
        StartedRow {
            case: "a drift detection over the StackSet",
            stack_set: "fleet-detect-drift",
            action: operation_action::DETECT_DRIFT,
            starts: "DetectStackSetDrift",
            members: vec![member("StackSetName", "fleet-detect-drift")],
        },
    ];

    for row in &rows {
        let case = row.case;
        let (router, engine) = router_with_engine();
        stack_set_with_an_instance(&router, row.stack_set).await;

        let (http_status, body) = call(&router, row.starts, &row.members).await;
        let operation_id = operation_id_of(case, http_status, &body);
        hold_the_operation_open(&engine, row.stack_set, &operation_id);

        let (http_status, body) = stop(&router, row.stack_set, &operation_id).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the stop to be served, got {http_status} {body}"
        );
        let (http_status, body) = describe_operation(&router, row.stack_set, &operation_id).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DescribeStackSetOperation to succeed, got {http_status} \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "Action").as_deref(),
            Some(row.action),
            "case {case:?}: expected the stopped operation's own action, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(operation_status::STOPPING),
            "case {case:?}: expected the operation to be stopping, got {body}"
        );
        assert_eq!(
            xml_text(&body, "EndTimestamp"),
            None,
            "case {case:?}: expected no end instant while the operation is stopping, got {body}"
        );

        // An operation that is STOPPING has not concluded, so the StackSet is still
        // single-flight: "Only one operation can be performed for a StackSet at a given
        // time."
        let (http_status, body) = probe_the_lock(&router, row.stack_set).await;
        assert_eq!(
            http_status, 409,
            "case {case:?}: expected a mutation to be refused while the stop is in progress, \
             got {http_status} {body}"
        );

        wait_for_the_transition().await;
        let (http_status, body) = describe_operation(&router, row.stack_set, &operation_id).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DescribeStackSetOperation to succeed, got {http_status} \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(operation_status::STOPPED),
            "case {case:?}: expected the stop to conclude the operation, got {body}"
        );
        assert!(
            xml_text(&body, "EndTimestamp").is_some(),
            "case {case:?}: expected the concluded operation to carry an end instant, got \
             {body}"
        );
        if row.action == operation_action::DETECT_DRIFT {
            // "STOPPED: The user has canceled the drift detection operation."
            assert_eq!(
                xml_text(&body, "DriftDetectionStatus").as_deref(),
                Some(drift_detection_status::STOPPED),
                "case {case:?}: expected the canceled detection to report that, got {body}"
            );
        }

        if row.action == operation_action::DELETE {
            // "StackSets will cancel all the unstarted stack instance deployments": the
            // deletion the stop caught had yet to start in that account and Region, so it was
            // cancelled there and the instance is still one of the StackSet's.
            let (http_status, body) =
                describe_instance(&router, row.stack_set, ACCOUNT_ID, REGION).await;
            assert_eq!(
                http_status, 200,
                "case {case:?}: expected the cancelled deletion to leave the instance, got \
                 {http_status} {body}"
            );
            assert_eq!(
                xml_text(&body, "DetailedStatus").as_deref(),
                Some(detailed_status::CANCELLED),
                "case {case:?}: expected the cancelled deletion to report that, got {body}"
            );
        }

        // The lock the stopped operation held is free: the identical mutation is served now.
        let (http_status, body) = probe_the_lock(&router, row.stack_set).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the mutation to be served once the stop concluded, got \
             {http_status} {body}"
        );
    }
}

#[tokio::test]
async fn a_stop_cancels_the_deployments_that_had_not_started_and_waits_for_the_rest() {
    const STACK_SET: &str = "fleet-partly-deployed";
    const OPERATION: &str = "operation-partly-deployed";
    let (router, engine) = router_with_engine();
    let now = SystemTime::now();
    {
        let state = store(&engine);
        let mut held = state.lock();
        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        // One deployment under way and one that has not started, both of the same operation:
        // the two halves of "StackSets will cancel all the unstarted stack instance
        // deployments and wait for those are in-progress to complete".
        let mut under_way = StoredStackInstance::new(ACCOUNT_ID, REGION, OPERATION);
        under_way.detailed_status = detailed_status::RUNNING.to_string();
        stack_set.record_instance(under_way);
        stack_set.record_instance(StoredStackInstance::new(ACCOUNT_A, REGION, OPERATION));
        let mut operation =
            StoredStackSetOperation::running(OPERATION, operation_action::CREATE, now);
        operation.pending = Some(PendingOperationOutcome {
            status: operation_status::SUCCEEDED.to_string(),
            status_reason: None,
            failed_stack_instances_count: Some(0),
            due_at: now + HELD_OPEN,
        });
        stack_set.record_operation(operation);
        held.record_stack_set(stack_set);
    }

    let (http_status, body) = stop(&router, STACK_SET, OPERATION).await;
    assert_eq!(
        http_status, 200,
        "expected the stop to be served, got {http_status} {body}"
    );

    // Straight after the stop: the unstarted deployment is canceled, the one under way is
    // still running.
    for (case, account, expected) in [
        (
            "the deployment under way",
            ACCOUNT_ID,
            detailed_status::RUNNING,
        ),
        (
            "the deployment that had not started",
            ACCOUNT_A,
            detailed_status::CANCELLED,
        ),
    ] {
        let (http_status, body) = describe_instance(&router, STACK_SET, account, REGION).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DescribeStackInstance to succeed, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "DetailedStatus").as_deref(),
            Some(expected),
            "case {case:?}: expected {expected:?} straight after the stop, got {body}"
        );
    }

    wait_for_the_transition().await;
    let reported = reported_status("the stopped deployment", &router, STACK_SET, OPERATION).await;
    assert_eq!(
        reported,
        operation_status::STOPPED,
        "expected the stop to conclude the operation, got {reported:?}"
    );

    // The deployment the stop waited for completed; the canceled one did not, so it keeps the
    // state the stop put it in and is not up to date with the StackSet.
    for (case, account, expected_detailed, expected_status) in [
        (
            "the deployment the stop waited for",
            ACCOUNT_ID,
            detailed_status::SUCCEEDED,
            instance_status::CURRENT,
        ),
        (
            "the canceled deployment",
            ACCOUNT_A,
            detailed_status::CANCELLED,
            instance_status::OUTDATED,
        ),
    ] {
        let (http_status, body) = describe_instance(&router, STACK_SET, account, REGION).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DescribeStackInstance to succeed, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "DetailedStatus").as_deref(),
            Some(expected_detailed),
            "case {case:?}: expected {expected_detailed:?} once the stop concluded, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(expected_status),
            "case {case:?}: expected {expected_status:?} once the stop concluded, got {body}"
        );
    }
}
