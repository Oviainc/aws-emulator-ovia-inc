// SPDX-License-Identifier: CC0-1.0

//! The UPDATE path's per-resource event timeline and the recovery
//! `ContinueUpdateRollback` drives, end to end over the awsQuery wire.
//!
//! Two rules are under test.
//!
//! * An UpdateStack records one stack event per resource per transition it puts that
//!   resource through, beside the stack's own events, and `DescribeStackEvents` reports them
//!   newest first — so a client can tell which resources an update had already changed
//!   before it finished.
//! * `ContinueUpdateRollback` resumes the rollback of an update that could not be rolled
//!   back: it is accepted ONLY for a stack in `UPDATE_ROLLBACK_FAILED`, the resources
//!   `ResourcesToSkip` names are left exactly as the failed update made them and take
//!   `UPDATE_COMPLETE`, and the stack reaches `UPDATE_ROLLBACK_COMPLETE`.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out) through
//! the wire harness. The table for the first rule spans the class it generalises over: a
//! resource the update CHANGES, one it ADDS, one it REMOVES and one it leaves alone, over
//! both template spellings. The table for the second spans the statuses a stack can be in
//! when the call arrives, and the two spellings of what to skip.

use std::time::SystemTime;

use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, member, router, router_with_engine, store, wait_for_the_transition,
    xml_text, xml_texts, REGION,
};

/// Every `(LogicalResourceId, ResourceStatus)` pair one `DescribeStackEvents` response
/// carries, in wire order (newest first).
///
/// Read as PARALLEL member lists: every event reports both members, so the lists line up
/// one-to-one, and a nesting-naive split of the `StackEvents` block into its `<member>`
/// elements is not needed.
async fn stack_events(
    case: &str,
    router: &MultiServiceRouter,
    stack_name: &str,
) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStackEvents to succeed, got {status} {body}"
    );
    let logical_ids = xml_texts(&body, "LogicalResourceId");
    let statuses = xml_texts(&body, "ResourceStatus");
    assert_eq!(
        logical_ids.len(),
        statuses.len(),
        "case {case:?}: every event reports a logical id and a status, got {body}"
    );
    logical_ids.into_iter().zip(statuses).collect()
}

/// One row of the first table: a stack to create, the template to update it to, and the
/// per-resource `(LogicalResourceId, ResourceStatus)` events the update must record.
struct UpdateCase {
    case: &'static str,
    stack_name: &'static str,
    created: String,
    updated: String,
    /// Events the update's own log must carry.
    expected: Vec<(&'static str, &'static str)>,
    /// Events no update may record, because the resource was not acted on.
    forbidden: Vec<(&'static str, &'static str)>,
}

/// The class the rule covers: any update, over any template, acting on any subset of the
/// stack's resources.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "json template, one resource changed and one left alone",
            stack_name: "changed-one",
            created: r#"{"Resources":{
                 "Changed":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"before"}},
                 "Untouched":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"same"}}}}"#
                .to_string(),
            updated: r#"{"Resources":{
                 "Changed":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"after"}},
                 "Untouched":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"same"}}}}"#
                .to_string(),
            expected: vec![
                ("Changed", resource_status::UPDATE_IN_PROGRESS),
                ("Changed", resource_status::UPDATE_COMPLETE),
            ],
            forbidden: vec![
                ("Untouched", resource_status::UPDATE_IN_PROGRESS),
                ("Untouched", resource_status::UPDATE_COMPLETE),
            ],
        },
        UpdateCase {
            case: "yaml template, a resource added by the update",
            stack_name: "added-one",
            created: concat!(
                "Resources:\n",
                "  Kept:\n",
                "    Type: AWS::SSM::Parameter\n",
                "    Properties:\n",
                "      Type: String\n",
                "      Value: kept\n",
            )
            .to_string(),
            updated: concat!(
                "Resources:\n",
                "  Kept:\n",
                "    Type: AWS::SSM::Parameter\n",
                "    Properties:\n",
                "      Type: String\n",
                "      Value: kept\n",
                "  Added:\n",
                "    Type: AWS::SSM::Parameter\n",
                "    Properties:\n",
                "      Type: String\n",
                "      Value: added\n",
            )
            .to_string(),
            expected: vec![
                ("Added", resource_status::CREATE_IN_PROGRESS),
                ("Added", resource_status::CREATE_COMPLETE),
            ],
            forbidden: vec![("Kept", resource_status::UPDATE_COMPLETE)],
        },
        UpdateCase {
            case: "json template, a resource removed by the update",
            stack_name: "removed-one",
            created: r#"{"Resources":{
                 "Kept":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"kept"}},
                 "Dropped":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"dropped"}}}}"#
                .to_string(),
            updated: r#"{"Resources":{
                 "Kept":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"kept"}}}}"#
                .to_string(),
            expected: vec![
                ("Dropped", resource_status::DELETE_IN_PROGRESS),
                ("Dropped", resource_status::DELETE_COMPLETE),
            ],
            forbidden: vec![("Kept", resource_status::UPDATE_COMPLETE)],
        },
    ]
}

#[tokio::test]
async fn an_update_records_one_event_per_resource_it_acts_on() {
    let router = router();

    for case in update_cases() {
        let (http_status, body) =
            create_stack(&router, case.stack_name, &case.created, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateStack to succeed, got {http_status} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let before = stack_events(case.case, &router, case.stack_name).await;
        let (http_status, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", case.stack_name),
                member("TemplateBody", &case.updated),
            ],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected UpdateStack to succeed, got {http_status} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let events = stack_events(case.case, &router, case.stack_name).await;
        // The update's own events are the ones the create's log did not already carry.
        let added: Vec<(String, String)> = events
            .iter()
            .filter(|event| !before.contains(event))
            .cloned()
            .collect();
        for (logical_id, resource) in &case.expected {
            let wanted = ((*logical_id).to_string(), (*resource).to_string());
            assert!(
                added.contains(&wanted),
                "case {:?}: expected the update to record {wanted:?}, got {added:?}",
                case.case
            );
        }
        for (logical_id, resource) in &case.forbidden {
            let unwanted = ((*logical_id).to_string(), (*resource).to_string());
            assert!(
                !added.contains(&unwanted),
                "case {:?}: expected no {unwanted:?} event for a resource the update did not \
                 act on, got {added:?}",
                case.case
            );
        }
        // The stack's own terminal event closes the operation, after the resources'.
        assert!(
            added.contains(&(
                case.stack_name.to_string(),
                status::UPDATE_COMPLETE.to_string()
            )),
            "case {:?}: expected the stack's own completion event, got {added:?}",
            case.case
        );
        let stack_terminal = added
            .iter()
            .position(|event| {
                event == &(
                    case.stack_name.to_string(),
                    status::UPDATE_COMPLETE.to_string(),
                )
            })
            .unwrap_or_default();
        for (logical_id, resource) in &case.expected {
            let wanted = ((*logical_id).to_string(), (*resource).to_string());
            let at = added
                .iter()
                .position(|event| event == &wanted)
                .unwrap_or_default();
            // Newest first, so the stack's terminal event comes BEFORE every resource
            // event of the same operation in the reported order.
            assert!(
                at > stack_terminal,
                "case {:?}: expected {wanted:?} to be reported after the stack's own \
                 completion in a newest-first log, got {added:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn continuing_a_rollback_is_refused_unless_the_rollback_failed() {
    let router = router();

    let template = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
         "Properties":{"DisplayName":"settled"}}}}"#;
    let (http_status, body) = create_stack(&router, "settled-stack", template, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );

    // While the create is still in flight, and once it has completed: neither is a status a
    // rollback can be continued from, and the stack is left exactly where it is.
    for case in ["while the create is in progress", "after the create completed"] {
        let (http_status, body) = call(
            &router,
            "ContinueUpdateRollback",
            &[member("StackName", "settled-stack")],
        )
        .await;
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected the call to be refused with 400, got {http_status} {body}"
        );
        assert!(
            body.contains("can not be continued"),
            "case {case:?}: expected the refusal to say the stack cannot be continued, got \
             {body}"
        );
        wait_for_the_transition().await;
    }

    // A stack this Region does not hold is refused too, and nothing is created for it.
    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[member("StackName", "no-such-stack")],
    )
    .await;
    assert_eq!(
        http_status, 400,
        "case {:?}: expected an unknown stack to be refused with 400, got {http_status} {body}",
        "a stack this Region does not hold"
    );

    // "ForceRollback and ResourcesToSkip are mutually exclusive. Specifying both in the same
    // request returns a validation error."
    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[
            member("StackName", "settled-stack"),
            member("ForceRollback", "true"),
            member("ResourcesToSkip.member.1", "Topic"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 400,
        "case {:?}: expected both spellings together to be refused, got {http_status} {body}",
        "ForceRollback beside ResourcesToSkip"
    );
    assert!(
        body.contains("ForceRollback") && body.contains("ResourcesToSkip"),
        "case {:?}: expected the refusal to name both members, got {body}",
        "ForceRollback beside ResourcesToSkip"
    );
}

#[tokio::test]
async fn continuing_a_rollback_leaves_the_skipped_resources_as_the_failed_update_made_them() {
    let (router, engine) = router_with_engine();

    let template = r#"{"Resources":{
         "Stuck":{"Type":"AWS::SSM::Parameter",
           "Properties":{"Type":"String","Value":"stuck"}},
         "Revertible":{"Type":"AWS::SSM::Parameter",
           "Properties":{"Type":"String","Value":"revertible"}}}}"#;
    let (http_status, body) = create_stack(&router, "failed-rollback", template, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;

    // The state a failed update rollback leaves behind, seeded directly: an owning service
    // that refuses to put one resource back is what produces it, and no wire call of this
    // suite can make a service refuse.
    {
        let state = store(&engine);
        let mut held = state.lock();
        // The create's own clock-owed completion is materialised FIRST, so the status seeded
        // below is not overwritten by the transition the next call would settle.
        held.settle(SystemTime::now());
        let stack = held
            .resolve_mut(REGION, "failed-rollback")
            .expect("the stack just created");
        stack.status = status::UPDATE_ROLLBACK_FAILED.to_string();
        stack.status_reason = Some("the owning service refused to put Stuck back".to_string());
        for deployed in &mut stack.resources {
            if deployed.resource.logical_id == "Stuck" {
                deployed.status = resource_status::UPDATE_FAILED.to_string();
                deployed.status_timestamp = SystemTime::now();
            }
        }
    }

    // "You can specify only resources that are in the UPDATE_FAILED state because a rollback
    // failed": a logical id outside that set is refused rather than silently skipped.
    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[
            member("StackName", "failed-rollback"),
            member("ResourcesToSkip.member.1", "Revertible"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 400,
        "case {:?}: expected a resource that did not fail to be refused, got {http_status} \
         {body}",
        "skipping a resource that is not UPDATE_FAILED"
    );

    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[
            member("StackName", "failed-rollback"),
            member("ResourcesToSkip.member.1", "Stuck"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {:?}: expected the rollback to be continued, got {http_status} {body}",
        "skipping the resource the rollback could not put back"
    );

    // Answered while the rollback is in progress; the terminal status falls due on the
    // clock, exactly as every other stack operation's does.
    let (http_status, body) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", "failed-rollback")],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected DescribeStacks to succeed, got {http_status} {body}"
    );
    let reported = xml_text(&body, "StackStatus").unwrap_or_default();
    assert!(
        reported == status::UPDATE_ROLLBACK_IN_PROGRESS
            || reported == status::UPDATE_ROLLBACK_COMPLETE,
        "case {:?}: expected the stack to be rolling back or rolled back, got {reported:?} in \
         {body}",
        "skipping the resource the rollback could not put back"
    );

    wait_for_the_transition().await;

    let (http_status, body) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", "failed-rollback")],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected DescribeStacks to succeed, got {http_status} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::UPDATE_ROLLBACK_COMPLETE),
        "case {:?}: expected the continued rollback to complete, got {body}",
        "skipping the resource the rollback could not put back"
    );

    // "CloudFormation sets the status of the specified resources to UPDATE_COMPLETE and
    // continues to roll back the stack": the skipped resource keeps the configuration it
    // had and takes that status, and the event log says so.
    let (http_status, body) = call(
        &router,
        "DescribeStackResources",
        &[
            member("StackName", "failed-rollback"),
            member("LogicalResourceId", "Stuck"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected DescribeStackResources to succeed, got {http_status} {body}"
    );
    assert_eq!(
        xml_text(&body, "ResourceStatus").as_deref(),
        Some(resource_status::UPDATE_COMPLETE),
        "case {:?}: expected the skipped resource to be reported UPDATE_COMPLETE, got {body}",
        "skipping the resource the rollback could not put back"
    );
    let events = stack_events(
        "skipping the resource the rollback could not put back",
        &router,
        "failed-rollback",
    )
    .await;
    assert!(
        events.contains(&(
            "Stuck".to_string(),
            resource_status::UPDATE_COMPLETE.to_string()
        )),
        "case {:?}: expected the skipped resource's own event, got {events:?}",
        "skipping the resource the rollback could not put back"
    );
    assert!(
        events.contains(&(
            "failed-rollback".to_string(),
            status::UPDATE_ROLLBACK_COMPLETE.to_string()
        )),
        "case {:?}: expected the stack's own rollback completion event, got {events:?}",
        "skipping the resource the rollback could not put back"
    );
}
