// SPDX-License-Identifier: CC0-1.0

//! The branches of a REFUSED stack deletion `stack_delete_failure_reason.rs` does not
//! reach, end to end over the awsQuery wire.
//!
//! RULE under test, the same one that suite pins: **a deletion an owning service refuses
//! leaves the stack in `DELETE_FAILED` whose `StackStatusReason` is the fixed
//! resource-failure summary alone, and leaves every resource reporting the conclusion that
//! deletion walk reached about IT — `DELETE_FAILED` carrying the owning service's own
//! refusal for the resource it would not delete, `DELETE_COMPLETE` for each resource the
//! same walk did delete (a refusal stops nothing the refused resource is not NEEDED BY),
//! and the status its create left for one that resource still needs, which cannot be
//! deleted while it is there.**
//!
//! `StackResource$ResourceStatus` is that resource's "Current status" and
//! `StackResource$ResourceStatusReason` the "success/failure message associated with the
//! resource", so the per-resource diagnosis is read THERE and on that resource's
//! `StackEvent` — never folded into `Stack$StackStatusReason`, which is the one field a
//! client parses to recover the SET of failed logical ids.
//!
//! The rows here extend the class that suite spans (Amazon RDS refusing a protected DB
//! instance, Amazon S3 refusing a bucket that still holds an object) along the two
//! dimensions it leaves open: a THIRD owning service with a refusal of its own (IAM, which
//! will not delete a role an inline policy still holds), and a resource the refused one
//! DEPENDS ON, which must still report the status its create left and must still be there in
//! the service that owns it. Each row reads the owning services back, so a
//! `DELETE_COMPLETE` record means the resource really went and an untouched record means it
//! really stayed.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, deploy, describe_stack, iam, member, rds, resource_event_reason, router, sns,
    wait_for_the_transition, xml_blocks, xml_text,
};
use crate::stack_resources::sns_arn;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, resource_status, status,
};

/// A trust policy IAM accepts: a policy document carrying a `Statement`.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "ec2.amazonaws.com"},
            "Action": "sts:AssumeRole"
        }]
    })
}

/// The declaration of a DB instance whose deletion protection is ON: Amazon RDS refuses
/// `DeleteDBInstance` for one, which is what makes the stack's deletion fail.
fn protected_instance(identifier: &str) -> Value {
    json!({
        "Type": "AWS::RDS::DBInstance",
        "Properties": {
            "DBInstanceIdentifier": identifier,
            "Engine": "mysql",
            "DBInstanceClass": "db.t3.micro",
            "DeletionProtection": true
        }
    })
}

/// The declaration of a topic Amazon SNS deletes without complaint.
fn topic(name: &str) -> Value {
    json!({"Type": "AWS::SNS::Topic", "Properties": {"TopicName": name}})
}

/// What a row needs to be so BEFORE the deletion, so the owning service refuses it.
enum Given {
    /// Nothing: the declaration itself is what the owning service will not delete.
    Nothing,
    /// An inline policy put on this role out of band, which is what IAM refuses to delete a
    /// role while it holds.
    AnInlinePolicyOn(&'static str),
}

/// One row: a stack whose deletion one owning service refuses, and what every view of it
/// must report afterwards.
struct Refused {
    case: &'static str,
    stack: &'static str,
    template: String,
    given: Given,
    /// The logical id whose deletion the owning service refuses.
    refused: &'static str,
    /// A fragment of that service's OWN refusal, which belongs on the resource and never on
    /// the stack.
    diagnosis: &'static str,
    /// Every resource of the stack with the resource status the refused deletion must leave
    /// it reporting.
    expected: &'static [(&'static str, &'static str)],
    /// A DB instance that must have survived the refused deletion, still protected.
    surviving_instance: Option<&'static str>,
    /// The topics the walk really deleted, which Amazon SNS must no longer hold.
    deleted_topics: &'static [&'static str],
    /// A topic the refused resource NEEDS, which the deletion left alone and Amazon SNS must
    /// still hold.
    untouched_topic: Option<&'static str>,
}

fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "a protected DB instance between two topics it does not depend on",
            stack: "refused-delete-middle",
            template: json!({
                "Resources": {
                    "ATopic": topic("refused-delete-a-topic"),
                    "Db": protected_instance("refused-delete-middle-db"),
                    "ZTopic": topic("refused-delete-z-topic")
                }
            })
            .to_string(),
            given: Given::Nothing,
            refused: "Db",
            // Amazon RDS's own wording for an instance its deletion protection holds.
            diagnosis: "deletion protection is enabled",
            // The walk runs newest first, and nothing here depends on the database: both
            // topics are deleted and the database alone is reported as the failure.
            expected: &[
                ("ATopic", resource_status::DELETE_COMPLETE),
                ("Db", resource_status::DELETE_FAILED),
                ("ZTopic", resource_status::DELETE_COMPLETE),
            ],
            surviving_instance: Some("refused-delete-middle-db"),
            deleted_topics: &["refused-delete-a-topic", "refused-delete-z-topic"],
            untouched_topic: None,
        },
        Refused {
            case: "a protected DB instance that DEPENDS ON a topic, beside one it does not",
            stack: "refused-delete-needed",
            template: json!({
                "Resources": {
                    "ADep": topic("refused-delete-needed-topic"),
                    "Db": {
                        "Type": "AWS::RDS::DBInstance",
                        "DependsOn": "ADep",
                        "Properties": {
                            "DBInstanceIdentifier": "refused-delete-needed-db",
                            "Engine": "mysql",
                            "DBInstanceClass": "db.t3.micro",
                            "DeletionProtection": true
                        }
                    },
                    "ZTopic": topic("refused-delete-needed-z-topic")
                }
            })
            .to_string(),
            given: Given::Nothing,
            refused: "Db",
            diagnosis: "deletion protection is enabled",
            // The topic the database DEPENDS ON cannot be deleted while the database is
            // still there, so the walk leaves it exactly as its create left it; the topic
            // the database has nothing to do with is deleted.
            expected: &[
                ("ADep", resource_status::CREATE_COMPLETE),
                ("Db", resource_status::DELETE_FAILED),
                ("ZTopic", resource_status::DELETE_COMPLETE),
            ],
            surviving_instance: Some("refused-delete-needed-db"),
            deleted_topics: &["refused-delete-needed-z-topic"],
            untouched_topic: Some("refused-delete-needed-topic"),
        },
        Refused {
            case: "a role IAM will not delete while an inline policy holds it",
            stack: "refused-delete-role",
            template: json!({
                "Resources": {
                    "Role": {
                        "Type": "AWS::IAM::Role",
                        "Properties": {
                            "RoleName": "refused-delete-role-name",
                            "AssumeRolePolicyDocument": trust_policy()
                        }
                    },
                    "ZTopic": topic("refused-delete-role-topic")
                }
            })
            .to_string(),
            given: Given::AnInlinePolicyOn("refused-delete-role-name"),
            refused: "Role",
            // IAM's own wording for an entity subordinate entities still hold.
            diagnosis: "Cannot delete entity",
            expected: &[
                ("Role", resource_status::DELETE_FAILED),
                ("ZTopic", resource_status::DELETE_COMPLETE),
            ],
            surviving_instance: None,
            deleted_topics: &["refused-delete-role-topic"],
            untouched_topic: None,
        },
    ]
}

/// Make a row's `Given` so, through the owning service's own wire.
async fn given(router: &MultiServiceRouter, case: &str, given: &Given) {
    match given {
        Given::Nothing => {}
        Given::AnInlinePolicyOn(role) => {
            let document = json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "s3:ListAllMyBuckets",
                    "Resource": "*"
                }]
            })
            .to_string();
            let (code, body) = iam(
                router,
                "PutRolePolicy",
                &[
                    ("RoleName", role),
                    ("PolicyName", "holds-the-role"),
                    ("PolicyDocument", &document),
                ],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: the setup PutRolePolicy must succeed, got {code} {body}"
            );
        }
    }
}

/// `(logical id, resource status, resource status reason)` for every row of a
/// `DescribeStackResources` response.
fn reported_resources(xml: &str) -> Vec<(String, String, String)> {
    xml_blocks(xml, "member")
        .iter()
        .filter_map(|entry| {
            let logical_id = xml_text(entry, "LogicalResourceId")?;
            Some((
                logical_id,
                xml_text(entry, "ResourceStatus").unwrap_or_default(),
                xml_text(entry, "ResourceStatusReason").unwrap_or_default(),
            ))
        })
        .collect()
}

#[tokio::test]
async fn a_refused_deletion_leaves_the_resources_it_did_not_reach_alone() {
    for row in refused_rows() {
        let router = router();
        deploy(&router, row.stack, &row.template).await;
        given(&router, row.case, &row.given).await;

        let (code, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            code, 200,
            "case {:?}: DeleteStack must be accepted — the stack reports the outcome — got \
             {code} {body}",
            row.case
        );
        wait_for_the_transition().await;

        // The STACK: the failed status, and the summary sentence alone as its reason.
        let (code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: DescribeStacks must answer 200, got {code} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::DELETE_FAILED),
            "case {:?}: expected the refused deletion to fail the stack, got {described}",
            row.case
        );
        let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
        let expected_reason = resource_failure_summary(failed_action::DELETE, [row.refused]);
        assert_eq!(
            reason, expected_reason,
            "case {:?}: expected the stack reason to be {expected_reason:?}, got {reason:?}",
            row.case
        );
        assert!(
            !reason.contains(row.diagnosis),
            "case {:?}: the stack reason must not carry the owning service's diagnosis \
             {:?}, got {reason:?}",
            row.case,
            row.diagnosis
        );

        // The RESOURCES: each one as the walk left it.
        let (code, resources) = call(
            &router,
            "DescribeStackResources",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: DescribeStackResources must answer 200, got {code} {resources}",
            row.case
        );
        let reported = reported_resources(&resources);
        let logical_ids: Vec<&str> = reported
            .iter()
            .map(|(logical_id, _, _)| logical_id.as_str())
            .collect();
        let declared: Vec<&str> = row.expected.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            logical_ids, declared,
            "case {:?}: expected a record per declared resource, got {resources}",
            row.case
        );
        for (logical_id, expected_status) in row.expected {
            let (_, actual_status, actual_reason) = reported
                .iter()
                .find(|(reported_id, _, _)| reported_id == logical_id)
                .unwrap_or_else(|| panic!("case {:?}: no record for {logical_id}", row.case));
            assert_eq!(
                actual_status, expected_status,
                "case {:?}: expected {logical_id} to report {expected_status}, got \
                 {actual_status:?}",
                row.case
            );
            if logical_id == &row.refused {
                assert!(
                    actual_reason.contains(row.diagnosis),
                    "case {:?}: expected {logical_id}'s own reason to carry {:?}, got \
                     {actual_reason:?}",
                    row.case,
                    row.diagnosis
                );
            } else {
                assert!(
                    actual_reason.is_empty(),
                    "case {:?}: expected {logical_id} to report no failure of its own, got \
                     {actual_reason:?}",
                    row.case
                );
            }
        }

        // The same diagnosis is on the failing resource's own event, which is where a
        // client reads why ONE resource failed.
        let on_the_event = resource_event_reason(
            &router,
            row.stack,
            row.refused,
            resource_status::DELETE_FAILED,
        )
        .await;
        assert!(
            on_the_event.contains(row.diagnosis),
            "case {:?}: expected the failed resource's event to carry {:?}, got \
             {on_the_event:?}",
            row.case,
            row.diagnosis
        );

        // What the owning services really hold, which is what those records claim.
        if let Some(identifier) = row.surviving_instance {
            let (code, instances) = rds(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier)],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {:?}: expected the protected instance to survive the refused \
                 deletion, got {code} {instances}",
                row.case
            );
            assert_eq!(
                xml_text(&instances, "DeletionProtection").as_deref(),
                Some("true"),
                "case {:?}: expected the refused deletion to leave deletion protection on, \
                 got {instances}",
                row.case
            );
        }
        for name in row.deleted_topics {
            let (code, attributes) = sns(
                &router,
                "GetTopicAttributes",
                &[("TopicArn", &sns_arn(name))],
            )
            .await;
            assert_ne!(
                code, 200,
                "case {:?}: a DELETE_COMPLETE record must mean the topic really went, got \
                 {code} {attributes}",
                row.case
            );
        }
        if let Some(name) = row.untouched_topic {
            let (code, attributes) = sns(
                &router,
                "GetTopicAttributes",
                &[("TopicArn", &sns_arn(name))],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {:?}: a resource the refused one needs must still be there, got \
                 {code} {attributes}",
                row.case
            );
        }
    }
}
