// SPDX-License-Identifier: CC0-1.0

//! What a stack's RESOURCE views report as `ResourceStatusReason`, end to end over the
//! awsQuery wire.
//!
//! RULE under test: **a resource in a FAILURE status carries `ResourceStatusReason` —
//! "Success/failure message associated with the resource", the provider's own report of the
//! refusal that failed it — while a resource whose status speaks for itself OMITS the member
//! entirely rather than carrying an empty one; the same asymmetry holds in every per-resource
//! view (`ListStackResources`, `DescribeStackResources`, `DescribeStackResource`) and the
//! reason those views report is the one the resource's own event carries.**
//!
//! Why it matters: the reason is the only place the owning service's refusal survives into
//! CloudFormation's resource views, so a client diagnosing a failed deployment reads WHICH
//! downstream call was refused and why from it. A failed row without it reports that a
//! resource failed and nothing about the cause; a successful row WITH it (even empty) makes
//! "is there a reason?" useless as the test for "did this resource fail?".
//!
//! The rows differ along what the rule generalises over: the service that owns the failed
//! resource (AWS Systems Manager, Amazon SNS), where the refusal comes from (the owning
//! service, or CloudFormation itself for a declaration that describes no request any service
//! could be asked), the logical ids, and the type of the sibling that SUCCEEDS beside it.
//! Every row is deployed with `OnFailure=DO_NOTHING`, which is what keeps the failed state and
//! the resources the create had already made ("DO_NOTHING — if the stack creation fails, do
//! nothing"), so the views can be read where a caller reads them.

use serde_json::json;

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, member, resource_event_reason, router,
    wait_for_the_transition, xml_blocks, xml_text,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, resource_status, status,
};

/// One row: the stack, the template, the logical id whose creation is refused, the sibling
/// whose creation succeeds, and a fragment the failed resource's reason must carry.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: String,
    failed: &'static str,
    succeeded: &'static str,
    quotes: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a parameter Systems Manager refuses because the value breaks the \
                   AllowedPattern the declaration states",
            stack: "reason-rejected-parameter",
            template: json!({"Resources": {
                "GoodParam": {
                    "Type": "AWS::SSM::Parameter",
                    "Properties": {"Type": "String", "Value": "42"}
                },
                "RejectedParam": {
                    "Type": "AWS::SSM::Parameter",
                    "DependsOn": "GoodParam",
                    "Properties": {
                        "Type": "String",
                        "Value": "not-a-number",
                        "AllowedPattern": "^[0-9]+$"
                    }
                }
            }})
            .to_string(),
            failed: "RejectedParam",
            succeeded: "GoodParam",
            // The provider reports the Systems Manager operation it asked for, which is the
            // one call that "create[s] or update[s] a parameter in Parameter Store".
            quotes: "PutParameter",
        },
        Row {
            case: "a FIFO topic whose name does not carry the suffix Amazon SNS requires",
            stack: "reason-rejected-topic",
            template: json!({"Resources": {
                "Plain": {
                    "Type": "AWS::SNS::Topic",
                    "Properties": {"DisplayName": "reason-plain-topic"}
                },
                "Fifo": {
                    "Type": "AWS::SNS::Topic",
                    "DependsOn": "Plain",
                    "Properties": {
                        "TopicName": "reason-rejected-topic-name",
                        "FifoTopic": true
                    }
                }
            }})
            .to_string(),
            failed: "Fifo",
            succeeded: "Plain",
            // Amazon SNS's own wording for a topic name that breaks its rule.
            quotes: "Invalid parameter: Topic Name",
        },
        Row {
            case: "a declaration missing a property its type requires, refused before any \
                   owning service is asked",
            stack: "reason-trustless-role",
            template: json!({"Resources": {
                "Plain": {
                    "Type": "AWS::SNS::Topic",
                    "Properties": {"DisplayName": "reason-trustless-sibling"}
                },
                "Trustless": {
                    "Type": "AWS::IAM::Role",
                    "DependsOn": "Plain",
                    "Properties": {"RoleName": "reason-trustless-role-name"}
                }
            }})
            .to_string(),
            failed: "Trustless",
            succeeded: "Plain",
            quotes: "AssumeRolePolicyDocument",
        },
    ]
}

/// The member every view carries the diagnosis in, as the wire spells it.
const REASON_ELEMENT: &str = "<ResourceStatusReason>";

/// The `<member>` block of `body` whose `LogicalResourceId` is `logical_id`.
fn row_of(case: &str, view: &str, body: &str, logical_id: &str) -> String {
    xml_blocks(body, "member")
        .into_iter()
        .find(|block| xml_text(block, "LogicalResourceId").as_deref() == Some(logical_id))
        .unwrap_or_else(|| {
            panic!("case {case:?} on {view}: expected a row for {logical_id:?}, got {body}")
        })
}

/// Assert the asymmetry on one view's block pair: the failed resource's block states the
/// reason and quotes `quotes`, and the successful one carries no reason element at all.
fn assert_asymmetry(case: &str, view: &str, failed: &str, succeeded: &str, quotes: &str) {
    let reason = xml_text(failed, "ResourceStatusReason").unwrap_or_default();
    assert!(
        !reason.is_empty(),
        "case {case:?} on {view}: expected the failed resource to carry a \
         ResourceStatusReason, got {failed}"
    );
    assert!(
        reason.contains(quotes),
        "case {case:?} on {view}: expected the reason to quote {quotes:?}, got {reason:?}"
    );
    assert!(
        !succeeded.contains(REASON_ELEMENT),
        "case {case:?} on {view}: expected the successful resource to omit \
         ResourceStatusReason entirely, got {succeeded}"
    );
}

#[tokio::test]
async fn a_failed_resource_states_its_reason_in_every_view_and_a_successful_one_omits_it() {
    for row in rows() {
        let router = router();
        // "DO_NOTHING — if the stack creation fails, do nothing": the failed state and the
        // resources the create had already made are what the views below are read from.
        let extra = acknowledging_iam(&[member("OnFailure", "DO_NOTHING")]);
        let (code, body) = create_stack(&router, row.stack, &row.template, &extra).await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must be accepted, got {code} {body}",
            row.case
        );
        wait_for_the_transition().await;

        // The stack rests in the failed status, and its own reason names the logical id that
        // failed — and nothing of the diagnosis, which lives on the resource.
        let (code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: DescribeStacks must answer 200, got {code} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_FAILED),
            "case {:?}: expected the failed state to be kept, got {described}",
            row.case
        );
        let stack_reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
        let summary = resource_failure_summary(failed_action::CREATE, [row.failed]);
        assert_eq!(
            stack_reason, summary,
            "case {:?}: expected the stack reason to name the failed resource as {summary:?}, \
             got {stack_reason:?}",
            row.case
        );

        // The two list-shaped views: the failed row states the reason, the successful row
        // omits the member.
        for (view, action) in [
            ("ListStackResources", "ListStackResources"),
            ("DescribeStackResources", "DescribeStackResources"),
        ] {
            let (code, body) = call(&router, action, &[member("StackName", row.stack)]).await;
            assert_eq!(
                code, 200,
                "case {:?}: {view} must answer 200, got {code} {body}",
                row.case
            );
            let failed = row_of(row.case, view, &body, row.failed);
            let succeeded = row_of(row.case, view, &body, row.succeeded);
            assert_eq!(
                xml_text(&failed, "ResourceStatus").as_deref(),
                Some(resource_status::CREATE_FAILED),
                "case {:?} on {view}: expected the refused resource to be {}, got {failed}",
                row.case,
                resource_status::CREATE_FAILED
            );
            assert_eq!(
                xml_text(&succeeded, "ResourceStatus").as_deref(),
                Some(resource_status::CREATE_COMPLETE),
                "case {:?} on {view}: expected the sibling to be {}, got {succeeded}",
                row.case,
                resource_status::CREATE_COMPLETE
            );
            assert_asymmetry(row.case, view, &failed, &succeeded, row.quotes);
        }

        // ...and the single-resource view, read once per resource.
        let view = "DescribeStackResource";
        let mut detail_of = Vec::new();
        for logical_id in [row.failed, row.succeeded] {
            let (code, body) = call(
                &router,
                view,
                &[
                    member("StackName", row.stack),
                    member("LogicalResourceId", logical_id),
                ],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {:?} on {view}({logical_id}): must answer 200, got {code} {body}",
                row.case
            );
            let detail = xml_blocks(&body, "StackResourceDetail")
                .into_iter()
                .next()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?} on {view}({logical_id}): expected a StackResourceDetail, \
                         got {body}",
                        row.case
                    )
                });
            detail_of.push(detail);
        }
        assert_asymmetry(row.case, view, &detail_of[0], &detail_of[1], row.quotes);

        // The views and the event stream cannot disagree: the reason a view reports for the
        // failed resource is the one its own event carries.
        let on_the_event = resource_event_reason(
            &router,
            row.stack,
            row.failed,
            resource_status::CREATE_FAILED,
        )
        .await;
        let in_the_view = xml_text(&detail_of[0], "ResourceStatusReason").unwrap_or_default();
        assert_eq!(
            in_the_view, on_the_event,
            "case {:?}: expected the view and the event to carry one reason, got \
             {in_the_view:?} and {on_the_event:?}",
            row.case
        );
    }
}
