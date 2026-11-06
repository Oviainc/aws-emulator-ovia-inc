// SPDX-License-Identifier: CC0-1.0

//! What a stack claims about a resource NOTHING created.
//!
//! The rule under test: **a stack reports a resource as created only where the service
//! that owns it created it.** Two cases follow from that, and this suite pins both
//! through the wire of a router that does not carry Amazon Data Firehose — which is
//! exactly the shape of "the call did not happen" for the type it owns:
//!
//! 1. A resource type this emulator PROVISIONS (an `AWS::KinesisFirehose::DeliveryStream`)
//!    whose owning service does not answer fails the create — so the stack is rolled back
//!    to `ROLLBACK_COMPLETE`, with the reason naming the resource — instead of a
//!    `CREATE_COMPLETE` stack whose stream does not exist.
//! 2. A resource type it has no provisioner for is DECLARED ONLY: nothing is called for
//!    it, so the stack completes and its outputs still resolve from the identifiers
//!    CloudFormation's documented naming rules give the resource — and the stack's
//!    deletion, which has nothing to delete, completes too. (What those identifiers ARE is
//!    `stack_outputs.rs`'s table; what this row adds is that no call was made for them and
//!    that the stack's lifecycle completes anyway.)
//!
//! The same stack with the owning service mounted beside CloudFormation is
//! `core/router/tests/all/cloudformation_provisions_stack_resources.rs`, which is where
//! the create really happens.

use serde_json::json;

use crate::stack_lifecycle::{call, member, wait_for_the_transition, xml_text, xml_texts};

/// Create a stack from `template` and answer the `DescribeStacks` body for it once its
/// create has settled.
async fn create_and_settle(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateStack",
        &[
            member("StackName", stack_name),
            member("TemplateBody", template),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    wait_for_the_transition().await;
    let (status, body) = call(
        router,
        "DescribeStacks",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// A resource whose owning service cannot be reached is not reported as created: the stack
/// fails, naming the resource, and the failure carries what the call answered.
#[tokio::test]
async fn a_resource_whose_owning_service_does_not_answer_fails_the_stack() {
    let router = crate::stack_lifecycle::router();
    let case = "a Firehose stream with no Firehose service mounted";
    let template = json!({
        "Resources": {
            "Stream": {
                "Type": "AWS::KinesisFirehose::DeliveryStream",
                "Properties": {
                    "DeliveryStreamName": "unreachable-stream",
                    "ExtendedS3DestinationConfiguration": {
                        "BucketARN": "arn:aws:s3:::a-bucket",
                        "RoleARN": "arn:aws:iam::000000000000:role/a-role"
                    }
                }
            }
        },
        "Outputs": { "StreamName": { "Value": { "Ref": "Stream" } } }
    })
    .to_string();

    let body = create_and_settle(&router, case, "unreachable-stack", &template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back rather than report a \
         stream nothing created, got {body}"
    );
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Stream"),
        "case {case:?}: expected the reason to name the resource that failed, got {reason:?}"
    );
}

/// A resource type this emulator has no provisioner for is declared only: nothing is
/// called for it, the stack completes, its outputs resolve from the identifiers
/// CloudFormation's documented naming rules give the resource, and the stack's deletion —
/// which has nothing to delete anywhere — completes.
#[tokio::test]
async fn resource_types_with_no_provisioner_are_declared_only_and_the_stack_completes() {
    let router = crate::stack_lifecycle::router();
    let case = "an alarm and a log group, neither of which this emulator provisions";
    let stack_name = "declared-only-stack";
    let template = json!({
        "Resources": {
            "Alarm": {
                "Type": "AWS::CloudWatch::Alarm",
                "Properties": { "AlarmName": "declared-only-alarm" }
            },
            "Group": {
                "Type": "AWS::Logs::LogGroup",
                "Properties": { "LogGroupName": "declared-only-group" }
            }
        },
        "Outputs": {
            "AlarmName": { "Value": { "Ref": "Alarm" } },
            "GroupArn": { "Value": { "Fn::GetAtt": ["Group", "Arn"] } }
        }
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let keys = xml_texts(&body, "OutputKey");
    let values = xml_texts(&body, "OutputValue");
    assert_eq!(
        keys.len(),
        2,
        "case {case:?}: expected both declared outputs, got {keys:?} in {body}"
    );
    assert_eq!(
        values,
        vec![
            "declared-only-alarm".to_string(),
            "arn:aws:logs:us-east-1:000000000000:log-group:declared-only-group:*".to_string(),
        ],
        "case {case:?}: expected the documented Ref and Fn::GetAtt values, got {values:?}"
    );

    // Deleting the stack has nothing to delete in any service, so the deletion completes.
    let (status, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    wait_for_the_transition().await;
    let (status, body) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the deleted stack to be unaddressable by name, got {status} \
         with body {body}"
    );
}
