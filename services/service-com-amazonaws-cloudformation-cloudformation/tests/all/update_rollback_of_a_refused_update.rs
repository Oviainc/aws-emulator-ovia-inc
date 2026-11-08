// SPDX-License-Identifier: CC0-1.0

//! The ROLLBACK a failed UPDATE owes however far that update got, end to end over the
//! awsQuery wire.
//!
//! RULE: an update that FAILS is undone — "if the stack update fails, CloudFormation rolls
//! back the stack to the previous stack configuration" — so the stack goes
//! `UPDATE_IN_PROGRESS` -> `UPDATE_ROLLBACK_IN_PROGRESS` -> `UPDATE_ROLLBACK_COMPLETE`
//! whatever the update got through before it failed: how far it got decides how much the
//! rollback has to put back in the services that own the resources, never whether the stack
//! rolls back at all. The stack-level `UPDATE_FAILED` park state belongs to the one update
//! whose caller asked for its failed state to be kept (`DisableRollback`), which is the
//! status `ContinueUpdateRollback` and `RollbackStack` then act on. The failing resource's
//! own `UPDATE_FAILED` event carries the owning service's refusal as its
//! `ResourceStatusReason`, exactly once.
//!
//! The rows span the class the rule generalises over: how far the update got before the
//! refusal (the refused resource is the only one the update asks anything of, so nothing was
//! changed anywhere, versus a resource the owning service had already accepted a change for),
//! which parameter the refusal is of (a bounded range and a value list), whether the caller
//! disabled the rollback, and which operation drove the failed update (`UpdateStack` and the
//! execution of an `UPDATE` change set, which deploys the same way).
//!
//! The second case is the other half of the same rule, for the failure no call of the
//! client's reports: an UPDATE change set whose execution really changed a resource and is
//! then HELD at a wait condition nothing signals is failed by the clock, and the rollback
//! that failure owes is CARRIED OUT in the service that owns the resource before the stack
//! reports itself rolled back.
//!
//! The refusal itself is Amazon RDS's own, answered through its own wire: the `Parameters` a
//! stack declares for an `AWS::RDS::DBParameterGroup` reach RDS as `ModifyDBParameterGroup`,
//! which refuses a value the parameter does not admit — so what is asserted here is what a
//! client that deploys a parameter group by template and then reads it back with
//! `DescribeDBParameters` observes.

use serde_json::{json, Map, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, rds, router, wait_for_the_transition, xml_blocks,
    xml_text, xml_texts,
};
use crate::update_rollback_recovery::{
    parameter, parameter_value, template as ssm_template, template_with_an_unsignalled_gate,
    WAITED_OUT,
};

/// The engine family every group here is created in.
const FAMILY: &str = "mysql8.0";

/// One parameter group of a row: the declaration the stack is created with, the one the
/// update states for it, and the value Amazon RDS must report for it once the stack has
/// settled.
struct Group {
    logical_id: &'static str,
    /// `DBParameterGroupName`, so the row addresses the group through RDS's own API by the
    /// name it declared.
    name: &'static str,
    /// The parameter the declarations set. Each one's published domain is what
    /// `DescribeEngineDefaultParameters` reports for this family under `AllowedValues` — a
    /// bounded range for `max_connections` and `max_allowed_packet`, a value list for
    /// `innodb_flush_log_at_trx_commit` — so `updated` being outside it is what RDS refuses.
    parameter: &'static str,
    created: &'static str,
    updated: &'static str,
    /// The value in force once the stack has settled.
    settled_value: &'static str,
}

/// How the row submits the update that fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Driver {
    /// `UpdateStack` with the revised body.
    UpdateStack,
    /// `CreateChangeSet` with the revised body, then `ExecuteChangeSet`.
    ChangeSet,
}

/// One row of the table: the stack, the groups it declares, how the failing update is
/// submitted, and where the stack must end up.
struct RollbackCase {
    case: &'static str,
    stack: &'static str,
    groups: Vec<Group>,
    /// `DisableRollback`, exactly as the update carries it (never sent when false, which is
    /// the recorded default).
    disable_rollback: bool,
    driver: Driver,
    /// The logical id whose change Amazon RDS refuses.
    refused: &'static str,
    /// The status the stack must settle in.
    settles_to: &'static str,
}

fn rollback_cases() -> Vec<RollbackCase> {
    vec![
        RollbackCase {
            case: "the refused resource is the only one the update asks anything of",
            stack: "refused-only",
            groups: vec![Group {
                logical_id: "Alpha",
                name: "rollback-only-alpha",
                parameter: "max_connections",
                created: "150",
                // Past the upper bound of the range that parameter publishes.
                updated: "200000",
                settled_value: "150",
            }],
            disable_rollback: false,
            driver: Driver::UpdateStack,
            refused: "Alpha",
            settles_to: status::UPDATE_ROLLBACK_COMPLETE,
        },
        RollbackCase {
            case: "a resource the update had already changed is put back",
            stack: "refused-after-a-change",
            groups: vec![
                Group {
                    logical_id: "Alpha",
                    name: "rollback-pair-alpha",
                    parameter: "max_connections",
                    created: "150",
                    updated: "300",
                    // The rollback puts the accepted change back.
                    settled_value: "150",
                },
                Group {
                    logical_id: "Beta",
                    name: "rollback-pair-beta",
                    parameter: "innodb_flush_log_at_trx_commit",
                    created: "1",
                    // Outside the value list that parameter publishes.
                    updated: "3",
                    settled_value: "1",
                },
            ],
            disable_rollback: false,
            driver: Driver::UpdateStack,
            refused: "Beta",
            settles_to: status::UPDATE_ROLLBACK_COMPLETE,
        },
        // "DisableRollback: Preserve the state of previously provisioned resources when an
        // operation fails": the caller said what it wants left behind, so the stack parks at
        // UPDATE_FAILED and the change the owning service had accepted stays applied.
        RollbackCase {
            case: "DisableRollback keeps the failed update as it failed",
            stack: "refused-rollback-disabled",
            groups: vec![
                Group {
                    logical_id: "Alpha",
                    name: "rollback-kept-alpha",
                    parameter: "max_connections",
                    created: "150",
                    updated: "300",
                    settled_value: "300",
                },
                Group {
                    logical_id: "Beta",
                    name: "rollback-kept-beta",
                    parameter: "innodb_flush_log_at_trx_commit",
                    created: "1",
                    updated: "3",
                    settled_value: "1",
                },
            ],
            disable_rollback: true,
            driver: Driver::UpdateStack,
            refused: "Beta",
            settles_to: status::UPDATE_FAILED,
        },
        RollbackCase {
            case: "the failed update is the execution of an UPDATE change set",
            stack: "refused-change-set",
            groups: vec![Group {
                logical_id: "Alpha",
                name: "rollback-set-alpha",
                parameter: "max_allowed_packet",
                created: "4194304",
                // Below the lower bound of the range that parameter publishes.
                updated: "512",
                settled_value: "4194304",
            }],
            disable_rollback: false,
            driver: Driver::ChangeSet,
            refused: "Alpha",
            settles_to: status::UPDATE_ROLLBACK_COMPLETE,
        },
    ]
}

/// The template a row deploys: one `AWS::RDS::DBParameterGroup` per group, carrying either
/// the created or the updated value of its parameter.
fn template(groups: &[Group], updated: bool) -> String {
    let mut resources = Map::new();
    for group in groups {
        let value = if updated { group.updated } else { group.created };
        resources.insert(
            group.logical_id.to_string(),
            json!({
                "Type": "AWS::RDS::DBParameterGroup",
                "Properties": {
                    "Description": format!("the {} rollback probe", group.logical_id),
                    "Family": FAMILY,
                    "DBParameterGroupName": group.name,
                    "Parameters": {group.parameter: value},
                }
            }),
        );
    }
    Value::Object(Map::from_iter([(
        "Resources".to_string(),
        Value::Object(resources),
    )]))
    .to_string()
}

/// The `StackStatus` the stack reports right now — the one reader both tests here use.
async fn stack_status(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks must answer 200, got {code} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The value Amazon RDS reports for a group's own `parameter`, read through its own API as a
/// client reads it.
async fn value_in_rds(router: &MultiServiceRouter, case: &RollbackCase, group: &Group) -> String {
    let (code, body) = rds(
        router,
        "DescribeDBParameters",
        &[
            ("DBParameterGroupName", group.name),
            ("Source", "user"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {:?}: DescribeDBParameters({}) must answer 200, got {code} {body}",
        case.case, group.name
    );
    xml_texts(&body, "ParameterName")
        .into_iter()
        .zip(xml_texts(&body, "ParameterValue"))
        .find(|(name, _)| name == group.parameter)
        .map(|(_, value)| value)
        .unwrap_or_default()
}

/// Every `(LogicalResourceId, ResourceStatus, ResourceStatusReason)` the stack's event stream
/// carries.
async fn events(router: &MultiServiceRouter, case: &RollbackCase) -> Vec<(String, String, String)> {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", case.stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {:?}: DescribeStackEvents must answer 200, got {code} {body}",
        case.case
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|entry| {
            (
                xml_text(entry, "LogicalResourceId").unwrap_or_default(),
                xml_text(entry, "ResourceStatus").unwrap_or_default(),
                xml_text(entry, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect()
}

/// Submit the row's failing update and answer once it has been accepted.
async fn submit_the_update(router: &MultiServiceRouter, case: &RollbackCase) {
    let body = template(&case.groups, true);
    match case.driver {
        Driver::UpdateStack => {
            let mut members = vec![
                member("StackName", case.stack),
                member("TemplateBody", &body),
            ];
            if case.disable_rollback {
                members.push(member("DisableRollback", "true"));
            }
            let (code, answered) = call(router, "UpdateStack", &members).await;
            assert_eq!(
                code, 200,
                "case {:?}: UpdateStack must answer 200, got {code} {answered}",
                case.case
            );
        }
        Driver::ChangeSet => {
            let (code, answered) = call(
                router,
                "CreateChangeSet",
                &[
                    member("StackName", case.stack),
                    member("ChangeSetName", "refused-revision"),
                    member("ChangeSetType", "UPDATE"),
                    member("TemplateBody", &body),
                ],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {:?}: CreateChangeSet must answer 200, got {code} {answered}",
                case.case
            );
            let change_set = xml_text(&answered, "Id").unwrap_or_default();
            wait_for_the_transition().await;
            let mut members = vec![member("ChangeSetName", &change_set)];
            if case.disable_rollback {
                members.push(member("DisableRollback", "true"));
            }
            let (code, answered) = call(router, "ExecuteChangeSet", &members).await;
            assert_eq!(
                code, 200,
                "case {:?}: ExecuteChangeSet must answer 200, got {code} {answered}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_refused_update_is_rolled_back_however_far_it_got() {
    for case in rollback_cases() {
        let router = router();
        let (code, body) = create_stack(&router, case.stack, &template(&case.groups, false), &[])
            .await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must answer 200, got {code} {body}",
            case.case
        );
        wait_for_the_transition().await;
        let created = stack_status(&router, case.case, case.stack).await;
        assert_eq!(
            created,
            status::CREATE_COMPLETE,
            "case {:?}: expected the create to complete before the update, got {created:?}",
            case.case
        );
        for group in &case.groups {
            let held = value_in_rds(&router, &case, group).await;
            assert_eq!(
                held, group.created,
                "case {:?}: expected Amazon RDS to hold {:?} for {}/{} after the create, got \
                 {held:?}",
                case.case, group.created, group.logical_id, group.parameter
            );
        }

        submit_the_update(&router, &case).await;
        wait_for_the_transition().await;

        // Where the stack ENDS UP: rolled back unless the caller asked for the failed state
        // to be kept — not decided by how much the update had managed to change.
        let settled = stack_status(&router, case.case, case.stack).await;
        assert_eq!(
            settled, case.settles_to,
            "case {:?}: expected the stack to settle at {}, got {settled:?}",
            case.case, case.settles_to
        );

        // And what the owning service holds: every change the update made is back as it was
        // when the stack rolled back, while the refused one never landed either way.
        for group in &case.groups {
            let held = value_in_rds(&router, &case, group).await;
            assert_eq!(
                held, group.settled_value,
                "case {:?}: expected Amazon RDS to hold {:?} for {}/{} once the stack had \
                 settled, got {held:?}",
                case.case, group.settled_value, group.logical_id, group.parameter
            );
        }

        let reported = events(&router, &case).await;
        // "Success/failure message associated with the resource": the refusal belongs to the
        // resource that was refused, on exactly one event of its own.
        let refusals: Vec<&(String, String, String)> = reported
            .iter()
            .filter(|(logical_id, resource_status, _)| {
                logical_id == case.refused && resource_status == resource_status::UPDATE_FAILED
            })
            .collect();
        assert_eq!(
            refusals.len(),
            1,
            "case {:?}: expected exactly one {} event for {:?}, got {reported:?}",
            case.case,
            resource_status::UPDATE_FAILED,
            case.refused
        );
        let refused_parameter = case
            .groups
            .iter()
            .find(|group| group.logical_id == case.refused)
            .map(|group| group.parameter)
            .unwrap_or_default();
        let refused_value = case
            .groups
            .iter()
            .find(|group| group.logical_id == case.refused)
            .map(|group| group.updated)
            .unwrap_or_default();
        let reason = &refusals[0].2;
        // RDS words its refusal from the parameter and the value the declaration carried,
        // in the wording that parameter's published domain takes (a range refusal for the
        // bounded parameters, a value-list refusal for the enumerated one), so the reason
        // the event carries must name both.
        assert!(
            reason.contains(refused_parameter) && reason.contains(refused_value),
            "case {:?}: expected the refused resource's reason to carry Amazon RDS's own \
             refusal of {refused_parameter:?}, got {reason:?}",
            case.case
        );
        // The stack's own events say whether it rolled back, which is what a client waiting
        // on the update reads.
        let rolled_back = reported.iter().any(|(logical_id, reported_status, _)| {
            logical_id == case.stack && reported_status == status::UPDATE_ROLLBACK_IN_PROGRESS
        });
        assert_eq!(
            rolled_back, !case.disable_rollback,
            "case {:?}: expected the rollback to be reported exactly where one was owed, got \
             {reported:?}",
            case.case
        );
        // ...and the update's own failure is an event of the STACK only where the stack
        // RESTS in `UPDATE_FAILED`: a caller that asked for the failed state to be kept gets
        // that status and the event reporting it, while a stack rolled straight back goes
        // from `UPDATE_IN_PROGRESS` into `UPDATE_ROLLBACK_IN_PROGRESS`, whose own event
        // carries the failure. Either way the resource that failed has its own event, which
        // is what the reason above was read from.
        let rested_in_the_failure = reported.iter().any(|(logical_id, reported_status, _)| {
            logical_id == case.stack && reported_status == status::UPDATE_FAILED
        });
        assert_eq!(
            rested_in_the_failure, case.disable_rollback,
            "case {:?}: expected the stack's own {} event exactly where the failed state is \
             kept, got {reported:?}",
            case.case,
            status::UPDATE_FAILED
        );
    }
}

/// An UPDATE change set whose execution really changed a resource and is then HELD at a wait
/// condition nothing signals is failed by the CLOCK — no call of the client's reports it — and
/// the rollback that failure owes is CARRIED OUT: the resource is put back in the service that
/// owns it before the stack reports `UPDATE_ROLLBACK_COMPLETE`, so the rolled-back status is
/// never one nothing performed.
#[tokio::test]
async fn a_change_set_execution_failed_by_the_clock_is_rolled_back_in_the_owning_service() {
    let case = "an executed UPDATE change set held at a wait condition that runs out";
    let stack = "change-set-waited-out";
    let held_parameter = "/change-set-waited-out/p";
    let router = router();

    let created = ssm_template(json!({"Param": parameter(held_parameter, "before")}));
    let (code, body) = create_stack(&router, stack, &created, &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code} {body}"
    );
    wait_for_the_transition().await;
    let settled = stack_status(&router, case, stack).await;
    assert_eq!(
        settled,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the create to complete before the change set, got {settled:?}"
    );

    // The change set revises the parameter and adds the gate that holds the execution open.
    let revised = template_with_an_unsignalled_gate(
        json!({"Param": parameter(held_parameter, "after")}),
    );
    let (code, body) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", stack),
            member("ChangeSetName", "waited-out-revision"),
            member("ChangeSetType", "UPDATE"),
            member("TemplateBody", &revised),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateChangeSet must answer 200, got {code} {body}"
    );
    let change_set = xml_text(&body, "Id").unwrap_or_default();
    wait_for_the_transition().await;
    let (code, body) = call(
        &router,
        "ExecuteChangeSet",
        &[member("ChangeSetName", &change_set)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: ExecuteChangeSet must answer 200, got {code} {body}"
    );

    // The execution really made its change in the owning service and is HELD at the gate.
    let applied = parameter_value(&router, held_parameter).await;
    assert_eq!(
        applied.as_deref(),
        Some("after"),
        "case {case:?}: expected the execution to have applied its change before the gate, \
         got {applied:?}"
    );
    let in_flight = stack_status(&router, case, stack).await;
    assert_eq!(
        in_flight,
        status::UPDATE_IN_PROGRESS,
        "case {case:?}: expected the execution to be held at its wait condition, got \
         {in_flight:?}"
    );

    // The wait runs out, which fails the update with no call of the client's: what the next
    // call materialises is that failure and the rollback it owes.
    tokio::time::sleep(WAITED_OUT).await;
    let settling = stack_status(&router, case, stack).await;
    assert!(
        settling == status::UPDATE_ROLLBACK_IN_PROGRESS
            || settling == status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected the failed execution to be rolling back, got {settling:?}"
    );
    wait_for_the_transition().await;
    let settled = stack_status(&router, case, stack).await;
    assert_eq!(
        settled,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected the stack to settle rolled back, got {settled:?}"
    );
    // The rollback was PERFORMED, not merely reported: the owning service holds the value
    // that was in force before the execution.
    let reverted = parameter_value(&router, held_parameter).await;
    assert_eq!(
        reverted.as_deref(),
        Some("before"),
        "case {case:?}: expected the rollback to put the previous value back in Parameter \
         Store, got {reverted:?}"
    );
}
