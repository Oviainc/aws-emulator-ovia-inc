// SPDX-License-Identifier: CC0-1.0

//! What a change set's STATUS surface refuses, and what the refusal says.
//!
//! Three rules, each held over its whole class rather than over one recorded call:
//!
//! 1. **A change set's `ExecutionStatus` follows the stack operation its execution
//!    started.** Executing a change set applies its plan to the stack, so the change set
//!    reports `EXECUTE_IN_PROGRESS` for as long as that operation is in flight — however
//!    long the stack is held — and reaches the terminal status that operation's outcome
//!    decides: `EXECUTE_COMPLETE` where it carried the change through, `EXECUTE_FAILED`
//!    where it did not.
//! 2. **CloudFormation will not discard a plan it is still working on.** `DeleteChangeSet`
//!    is refused with the modelled `InvalidChangeSetStatus` while the change set's creation
//!    has not concluded and while its execution is being applied; and a change set something
//!    has EXECUTED keeps the record of that execution when its plan is discarded, so a call
//!    asking for it to be executed again is answered on that execution instead of being told
//!    the change set never existed.
//! 3. **A refusal names the member whose value refuses the call.** A change set whose
//!    creation left nothing to execute is refused on its `ChangeSetStatus`; one whose
//!    creation completed is refused on its `ExecutionStatus`, the only member that says the
//!    plan has already been applied. A change set addressed within a stack this Region holds
//!    none of is refused about the STACK.
//!
//! The tables below span the model's `ExecutionStatus`, the change-set statuses this service
//! puts a change set into and both change-set types whose execution changes a stack, so the
//! recorded call this suite was written for is ONE row of each.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_cloudformation_cloudformation::state::{
    change_set_status, execution_status, status,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::{
    create_change_set, create_creating_change_set, describe_change_set, execute_change_set,
    settle_the_store,
};
use crate::harness::{call, create_stack, member, router_with_engine, store, xml_text, REGION};
use crate::stack_lifecycle::describe_stack;

const TOPIC: &str = r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#;
const TWO_TOPICS: &str =
    r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"},"B":{"Type":"AWS::SNS::Topic"}}}"#;

/// `OBSOLETE` has no constant in this service's `execution_status` module — nothing here
/// puts a change set into it — so the one spelling of it this suite needs lives here, and
/// using it is what makes the tables below cover a status the recorded calls never did.
///
/// Source: the model's `ExecutionStatus` enum,
/// `smithy_query(query="shape",shape="ExecutionStatus")` =
/// `[UNAVAILABLE, AVAILABLE, EXECUTE_IN_PROGRESS, EXECUTE_COMPLETE, EXECUTE_FAILED, OBSOLETE]`.
const OBSOLETE: &str = "OBSOLETE";

/// Every value of that enum, with what a `DeleteChangeSet` for a change set in it must
/// answer once the change set's own creation has concluded.
const EVERY_EXECUTION_STATUS: [(&str, bool); 6] = [
    // (the ExecutionStatus, whether the plan may be discarded in it)
    (execution_status::UNAVAILABLE, true),
    (execution_status::AVAILABLE, true),
    (OBSOLETE, true),
    // The one status in which CloudFormation is applying the plan right now.
    (execution_status::EXECUTE_IN_PROGRESS, false),
    (execution_status::EXECUTE_COMPLETE, true),
    (execution_status::EXECUTE_FAILED, true),
];

async fn delete_change_set(
    router: &MultiServiceRouter,
    change_set: &str,
    stack_name: &str,
) -> (u16, String) {
    call(
        router,
        "DeleteChangeSet",
        &[
            member("ChangeSetName", change_set),
            member("StackName", stack_name),
        ],
    )
    .await
}

/// Put the change set into the `(ChangeSetStatus, ExecutionStatus)` pair a row is about.
///
/// The statuses a change set reaches by itself are driven through the wire in
/// [`an_execution_reports_the_status_of_the_stack_operation_it_started`]; the tables here are
/// about the GUARDS, so they state the pair directly and can therefore cover the statuses no
/// flow of this emulator produces (`OBSOLETE`) as well as the ones that need a held or
/// failing deployment.
fn put_in_status(
    engine: &Arc<engine::InMemoryEngine>,
    stack_name: &str,
    change_set_name: &str,
    status: &str,
    execution: &str,
) {
    let state = store(engine);
    let mut held = state.lock();
    let change_set = held
        .resolve_change_set_mut(REGION, change_set_name, Some(stack_name))
        .expect("the change set the case created");
    change_set.pending = None;
    change_set.status = status.to_string();
    change_set.execution_status = execution.to_string();
}

/// Rule 2, over every `ExecutionStatus` the model declares: the plan is discardable in every
/// status but the one CloudFormation is applying it in, and a refused delete leaves the
/// change set exactly where it was.
#[tokio::test]
async fn delete_change_set_is_refused_while_the_plan_is_being_applied() {
    let (router, engine) = router_with_engine();

    for (execution, deletable) in EVERY_EXECUTION_STATUS {
        let case = execution;
        let stack_name = format!("del-{}", execution.to_lowercase().replace('_', "-"));
        let change_set_name = format!("{stack_name}-set");
        let (code, body) =
            create_creating_change_set(&router, &stack_name, &change_set_name, TOPIC).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set the case acts on must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        put_in_status(
            &engine,
            &stack_name,
            &change_set_name,
            change_set_status::CREATE_COMPLETE,
            execution,
        );

        let (code, body) = delete_change_set(&router, &change_set_name, &stack_name).await;
        if deletable {
            assert_eq!(
                code, 200,
                "case {case:?}: a plan nothing is applying is discardable, got {code} {body}"
            );
            let (code, body) =
                describe_change_set(&router, &change_set_name, Some(&stack_name)).await;
            assert_eq!(
                code, 404,
                "case {case:?}: the discarded plan is no longer readable, got {code} {body}"
            );
            continue;
        }
        assert_eq!(
            code, 400,
            "case {case:?}: a plan being applied cannot be discarded, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("InvalidChangeSetStatus"),
            "case {case:?}: expected the modelled InvalidChangeSetStatus code, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message"),
            Some(format!("Cannot delete ChangeSet in execution status {execution}")),
            "case {case:?}: expected the refusal to name the execution status alone, got {body}"
        );
        // The refused delete changed nothing: the record is still addressable, in the very
        // status that refused the call.
        let (code, body) = describe_change_set(&router, &change_set_name, Some(&stack_name)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: a refused delete leaves the change set addressable, got {code} \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "ExecutionStatus").as_deref(),
            Some(execution),
            "case {case:?}: a refused delete leaves its execution status alone, got {body}"
        );
    }
}

/// Rule 2, on the other side of the guard: a change set whose CREATION has not concluded is
/// a plan CloudFormation is still computing, so it cannot be discarded either — and the same
/// request is accepted once that creation has settled.
#[tokio::test]
async fn delete_change_set_is_refused_while_the_plan_is_still_being_computed() {
    let (router, engine) = router_with_engine();
    let (code, body) = create_creating_change_set(&router, "computing", "computing-set", TOPIC)
        .await;
    assert_eq!(code, 200, "the change set is created, got {code} {body}");

    // Straight after the create the creation is still in flight, which is what
    // `DescribeChangeSet` reports as CREATE_PENDING with nothing executable yet.
    let (_, body) = describe_change_set(&router, "computing-set", Some("computing")).await;
    assert_eq!(
        (
            xml_text(&body, "Status").unwrap_or_default(),
            xml_text(&body, "ExecutionStatus").unwrap_or_default(),
        ),
        (
            change_set_status::CREATE_PENDING.to_string(),
            execution_status::UNAVAILABLE.to_string(),
        ),
        "case \"straight after the create\": the creation is still in flight, got {body}"
    );
    let (code, body) = delete_change_set(&router, "computing-set", "computing").await;
    assert_eq!(
        code, 400,
        "case \"a creation still in flight\": the plan cannot be discarded, got {code} {body}"
    );
    assert_eq!(
        (
            xml_text(&body, "Code").unwrap_or_default(),
            xml_text(&body, "Message").unwrap_or_default(),
        ),
        (
            "InvalidChangeSetStatus".to_string(),
            format!(
                "Cannot delete ChangeSet in execution status {}",
                execution_status::UNAVAILABLE
            ),
        ),
        "case \"a creation still in flight\": expected the modelled refusal naming the \
         execution status, got {body}"
    );

    // ...and the identical request is accepted once the creation has concluded.
    settle_the_store(&router, &engine).await;
    let (code, body) = delete_change_set(&router, "computing-set", "computing").await;
    assert_eq!(
        code, 200,
        "case \"a creation that has concluded\": the plan is discardable, got {code} {body}"
    );
    let (code, body) = describe_change_set(&router, "computing-set", Some("computing")).await;
    assert_eq!(
        code, 404,
        "case \"a creation that has concluded\": the plan is gone, got {code} {body}"
    );
}

/// One row of the execute-refusal table: the `(ChangeSetStatus, ExecutionStatus)` pair a
/// change set is in, and which member the refusal must report.
struct RefusalCase {
    case: &'static str,
    status: &'static str,
    execution: &'static str,
    /// `true` where the message must report the `ExecutionStatus` — a plan whose execution
    /// is what refuses the call — and `false` where it must report the `ChangeSetStatus`.
    reports_execution_status: bool,
}

/// Rule 3: `ExecuteChangeSet`'s refusal reports the member whose value refuses it, in
/// CloudFormation's wording, with the change set's ARN and that status as its only variable
/// parts.
#[tokio::test]
async fn execute_change_set_reports_the_status_that_refuses_it() {
    let (router, engine) = router_with_engine();
    let cases = [
        RefusalCase {
            case: "a creation that has not completed",
            status: change_set_status::CREATE_PENDING,
            execution: execution_status::UNAVAILABLE,
            reports_execution_status: false,
        },
        RefusalCase {
            case: "a creation that failed",
            status: change_set_status::FAILED,
            execution: execution_status::UNAVAILABLE,
            reports_execution_status: false,
        },
        RefusalCase {
            case: "a plan that is being applied",
            status: change_set_status::CREATE_COMPLETE,
            execution: execution_status::EXECUTE_IN_PROGRESS,
            reports_execution_status: true,
        },
        RefusalCase {
            case: "a plan that has been applied",
            status: change_set_status::CREATE_COMPLETE,
            execution: execution_status::EXECUTE_COMPLETE,
            reports_execution_status: true,
        },
        RefusalCase {
            case: "a plan whose application failed",
            status: change_set_status::CREATE_COMPLETE,
            execution: execution_status::EXECUTE_FAILED,
            reports_execution_status: true,
        },
        RefusalCase {
            case: "a plan another change set superseded",
            status: change_set_status::CREATE_COMPLETE,
            execution: OBSOLETE,
            reports_execution_status: true,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let stack_name = format!("exec-refused-{index}");
        let change_set_name = format!("{stack_name}-set");
        let (code, body) =
            create_creating_change_set(&router, &stack_name, &change_set_name, TOPIC).await;
        assert_eq!(
            code, 200,
            "case {:?}: the change set the case acts on must be created, got {code} {body}",
            case.case
        );
        settle_the_store(&router, &engine).await;
        let (_, described) = describe_change_set(&router, &change_set_name, Some(&stack_name)).await;
        let change_set_id =
            xml_text(&described, "ChangeSetId").expect("a described change set carries its ARN");
        put_in_status(
            &engine,
            &stack_name,
            &change_set_name,
            case.status,
            case.execution,
        );

        let (code, body) = execute_change_set(&router, &change_set_name, Some(&stack_name)).await;
        assert_eq!(
            code, 400,
            "case {:?}: the execution is refused, got {code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("InvalidChangeSetStatus"),
            "case {:?}: expected the modelled InvalidChangeSetStatus code, got {body}",
            case.case
        );
        let expected = if case.reports_execution_status {
            format!(
                "ChangeSet [{change_set_id}] cannot be executed in its current execution \
                 status of [{}]",
                case.execution
            )
        } else {
            format!(
                "ChangeSet [{change_set_id}] cannot be executed in its current status of [{}]",
                case.status
            )
        };
        assert_eq!(
            xml_text(&body, "Message"),
            Some(expected.clone()),
            "case {:?}: expected the message {expected:?}, got {body}",
            case.case
        );
    }
}

/// Rules 1 and 2 over the REAL path, for each change-set type whose execution changes a
/// stack: the execution reports itself in flight until the stack operation it started
/// concludes, cannot be discarded while it is, and — once it has run — keeps the record of
/// that execution when the plan is discarded, so the plan reads as gone while a second
/// execution is still refused on the execution that happened.
#[tokio::test]
async fn an_executed_change_set_keeps_its_execution_record_when_its_plan_is_discarded() {
    for (case, change_set_type, template) in [
        ("a CREATE change set", "CREATE", TOPIC),
        ("an UPDATE change set", "UPDATE", TWO_TOPICS),
    ] {
        let (router, engine) = router_with_engine();
        let stack_name = format!("executed-{change_set_type}").to_lowercase();
        let change_set_name = format!("{stack_name}-set");
        if change_set_type == "UPDATE" {
            let (code, body) = create_stack(&router, &stack_name, TOPIC, &[]).await;
            assert_eq!(
                code, 200,
                "case {case:?}: the stack the change set updates must exist, got {code} {body}"
            );
            settle_the_store(&router, &engine).await;
        }
        let (code, body) = create_change_set(
            &router,
            &stack_name,
            &change_set_name,
            &[
                member("ChangeSetType", change_set_type),
                member("TemplateBody", template),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (_, described) = describe_change_set(&router, &change_set_name, Some(&stack_name)).await;
        let change_set_id =
            xml_text(&described, "ChangeSetId").expect("a described change set carries its ARN");

        let (code, body) = execute_change_set(&router, &change_set_name, Some(&stack_name)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set executes, got {code} {body}"
        );

        // While the stack is applying the plan the change set reports the execution in
        // flight, the plan cannot be discarded, and a second execution is refused on that
        // very status.
        let (_, body) = describe_change_set(&router, &change_set_name, Some(&stack_name)).await;
        assert_eq!(
            xml_text(&body, "ExecutionStatus").as_deref(),
            Some(execution_status::EXECUTE_IN_PROGRESS),
            "case {case:?}: the execution is in flight with the stack, got {body}"
        );
        let (code, body) = delete_change_set(&router, &change_set_name, &stack_name).await;
        assert_eq!(
            code, 400,
            "case {case:?}: the plan being applied cannot be discarded, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Message"),
            Some(format!(
                "Cannot delete ChangeSet in execution status {}",
                execution_status::EXECUTE_IN_PROGRESS
            )),
            "case {case:?}: expected the refusal to name the execution in flight, got {body}"
        );
        let (code, body) = execute_change_set(&router, &change_set_name, Some(&stack_name)).await;
        assert_eq!(
            (
                code,
                xml_text(&body, "Code").unwrap_or_default(),
                xml_text(&body, "Message").unwrap_or_default(),
            ),
            (
                400,
                "InvalidChangeSetStatus".to_string(),
                format!(
                    "ChangeSet [{change_set_id}] cannot be executed in its current execution \
                     status of [{}]",
                    execution_status::EXECUTE_IN_PROGRESS
                ),
            ),
            "case {case:?}: a second execution is refused on the execution in flight, got \
             {body}"
        );

        // Once the stack has carried the change through, the execution reports itself
        // complete...
        settle_the_store(&router, &engine).await;
        let (_, body) = describe_change_set(&router, &change_set_name, Some(&stack_name)).await;
        assert_eq!(
            xml_text(&body, "ExecutionStatus").as_deref(),
            Some(execution_status::EXECUTE_COMPLETE),
            "case {case:?}: the execution completes with the stack, got {body}"
        );
        // ...the plan may now be discarded...
        let (code, body) = delete_change_set(&router, &change_set_name, &stack_name).await;
        assert_eq!(
            code, 200,
            "case {case:?}: an applied plan is discardable, got {code} {body}"
        );
        // ...and what is left is the record of the execution: the plan reads as gone and is
        // listed no more, while a call asking for it to be executed again is still answered
        // on the execution that happened rather than told it never existed.
        let (code, body) = describe_change_set(&router, &change_set_name, Some(&stack_name)).await;
        assert_eq!(
            code, 404,
            "case {case:?}: the discarded plan is no longer readable, got {code} {body}"
        );
        let (code, body) = call(
            &router,
            "ListChangeSets",
            &[member("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            (code, xml_text(&body, "ChangeSetName")),
            (200, None),
            "case {case:?}: the discarded plan is listed no more, got {body}"
        );
        let (code, body) = execute_change_set(&router, &change_set_name, Some(&stack_name)).await;
        assert_eq!(
            (
                code,
                xml_text(&body, "Code").unwrap_or_default(),
                xml_text(&body, "Message").unwrap_or_default(),
            ),
            (
                400,
                "InvalidChangeSetStatus".to_string(),
                format!(
                    "ChangeSet [{change_set_id}] cannot be executed in its current execution \
                     status of [{}]",
                    execution_status::EXECUTE_COMPLETE
                ),
            ),
            "case {case:?}: executing it again is answered on the execution that happened, \
             got {body}"
        );

        // The discarded plan's NAME is free again — nothing lists or reads it — so another
        // change set may be created under it, and that new plan is the one every call about
        // the name is then about: the execution record left behind must not shadow it.
        let (code, body) = create_change_set(
            &router,
            &stack_name,
            &change_set_name,
            &[
                member("ChangeSetType", "UPDATE"),
                member(
                    "TemplateBody",
                    r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"renamed"}}}}"#,
                ),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the name of a discarded plan takes a new change set, got {code} \
             {body}"
        );
        settle_the_store(&router, &engine).await;
        let (code, described) = describe_change_set(&router, &change_set_name, Some(&stack_name))
            .await;
        assert_eq!(
            (code, xml_text(&described, "ExecutionStatus").unwrap_or_default()),
            (200, execution_status::AVAILABLE.to_string()),
            "case {case:?}: the new plan is the one the name reads back, got {described}"
        );
        let (code, body) = execute_change_set(&router, &change_set_name, Some(&stack_name)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the new plan executes rather than being refused on the discarded \
             one's execution, got {code} {body}"
        );
    }
}

/// A template whose update is HELD at a wait condition for as long as `timeout` says, so the
/// stack operation an execution starts can be made to outlast the span the emulator's own
/// clock owes it.
fn held_update(timeout: &str) -> String {
    format!(
        r#"{{"Resources":{{
            "A":{{"Type":"AWS::SNS::Topic"}},
            "Gate":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
            "Wait":{{"Type":"AWS::CloudFormation::WaitCondition",
                    "Properties":{{"Handle":{{"Ref":"Gate"}},"Timeout":"{timeout}","Count":"1"}}}}
        }}}}"#
    )
}

/// Rule 1: a change set's execution is the stack operation it started, so it is in flight
/// exactly as long as that operation is — a stack held at a wait condition keeps reporting
/// `EXECUTE_IN_PROGRESS` long past the span a transition is otherwise due in — and it ends
/// in what that operation ended in: the wait that runs out fails the update, and the change
/// set reports the modelled `EXECUTE_FAILED` rather than an execution that completed.
#[tokio::test]
async fn an_execution_reports_the_status_of_the_stack_operation_it_started() {
    // (case, the wait's Timeout in seconds, the ExecutionStatus the change set must end in)
    for (case, timeout, settles_to) in [
        (
            "an update held at a wait condition nobody signals",
            "3600",
            execution_status::EXECUTE_IN_PROGRESS,
        ),
        (
            "an update whose wait runs out",
            "1",
            execution_status::EXECUTE_FAILED,
        ),
    ] {
        let (router, engine) = router_with_engine();
        let stack_name = format!("held-{timeout}");
        let (code, body) = create_stack(&router, &stack_name, TOPIC, &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (code, body) = create_change_set(
            &router,
            &stack_name,
            "held-set",
            &[
                member("ChangeSetType", "UPDATE"),
                member("TemplateBody", &held_update(timeout)),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (code, body) = execute_change_set(&router, "held-set", Some(&stack_name)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set executes, got {code} {body}"
        );

        // Everything the store owes is materialised — the held case's wait is not the
        // clock's to release, so pulling the due instants into the past settles whatever
        // ELSE was owed — and the expiring case is given the second its own `Timeout` asked
        // for. What decides the execution's status either way is the stack operation, not
        // the span a transition is otherwise due in.
        if settles_to == execution_status::EXECUTE_FAILED {
            tokio::time::sleep(Duration::from_millis(1_500)).await;
        }
        settle_the_store(&router, &engine).await;
        settle_the_store(&router, &engine).await;
        let (_, body) = describe_change_set(&router, "held-set", Some(&stack_name)).await;
        let reported = xml_text(&body, "ExecutionStatus").unwrap_or_default();
        let (_, stack) = describe_stack(&router, &stack_name).await;
        let stack_status = xml_text(&stack, "StackStatus").unwrap_or_default();
        assert_eq!(
            reported, settles_to,
            "case {case:?}: expected the execution to report {settles_to}, got {reported:?} \
             with the stack in {stack_status:?}"
        );
        if settles_to == execution_status::EXECUTE_FAILED {
            assert_eq!(
                stack_status,
                status::UPDATE_ROLLBACK_COMPLETE,
                "case {case:?}: the operation the execution started was rolled back, got \
                 {stack:?}"
            );
        } else {
            assert_eq!(
                stack_status,
                status::UPDATE_IN_PROGRESS,
                "case {case:?}: the operation the execution started is still in flight, got \
                 {stack:?}"
            );
        }
    }
}

/// Rule 3, at the stack level: a change set is addressed WITHIN a stack, so a read naming a
/// stack this Region holds none of is refused about that stack — the change-set-not-found of
/// a lookup that never had a stack to look in would tell a caller its change set is missing
/// from a stack that does not exist.
#[tokio::test]
async fn a_change_set_read_on_a_stack_that_is_not_there_is_refused_about_the_stack() {
    let (router, engine) = router_with_engine();
    // One stack that DOES exist, so the cases below differ in the stack they name rather
    // than in the store being empty.
    create_creating_change_set(&router, "present", "present-set", TOPIC).await;
    settle_the_store(&router, &engine).await;

    // (case, the stack named, the change set named, the expected code and message)
    let cases = [
        (
            "a change set of a stack that was never created",
            "never-created",
            "never-created-set",
            "ValidationError",
            Some("Stack [never-created] does not exist".to_string()),
        ),
        (
            "a change set name no stack of this Region carries",
            "present",
            "not-a-set",
            "ChangeSetNotFound",
            Some("ChangeSet [not-a-set] does not exist".to_string()),
        ),
    ];
    for (case, stack_name, change_set_name, expected_code, expected_message) in cases {
        let (code, body) = describe_change_set(&router, change_set_name, Some(stack_name)).await;
        assert_eq!(
            (
                xml_text(&body, "Code").unwrap_or_default(),
                xml_text(&body, "Message"),
            ),
            (expected_code.to_string(), expected_message),
            "case {case:?}: expected the {expected_code} refusal, got {code} {body}"
        );
    }
}
