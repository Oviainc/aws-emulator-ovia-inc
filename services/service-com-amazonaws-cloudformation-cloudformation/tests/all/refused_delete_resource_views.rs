// SPDX-License-Identifier: CC0-1.0

//! The per-resource STATUS every resource view reports after a deletion that FAILED, end to
//! end over the awsQuery wire.
//!
//! RULE under test: **`StackResource$ResourceStatus` is that resource's "Current status", so
//! for every resource of a stack each view of it — `DescribeStackResources`,
//! `DescribeStackResource` and `ListStackResources` — reports the status (and the reason) of
//! the LAST transition the stack recorded for that resource, which is the newest event the
//! stack's own event stream carries for its logical id: the resource view and the event
//! stream cannot disagree about what became of one resource.**
//!
//! Why it matters: a stack stuck in `DELETE_FAILED` is diagnosed from its resource list —
//! which logical ids to pass to `DeleteStack`'s `RetainResources`, which to repair by hand —
//! so a view still reporting the status a resource's CREATE left tells the client nothing
//! failed and forces tooling to parse the event stream instead.
//!
//! The rows differ along the per-resource OUTCOME the one deletion walk recorded, which is
//! what the rule generalises over: a resource the owning service REFUSED to delete
//! (`DELETE_FAILED`, carrying that service's refusal), one the same walk deleted
//! (`DELETE_COMPLETE`), one whose `DeletionPolicy` kept it so nothing was asked of its owning
//! service (`DELETE_SKIPPED`), and one the refused resource DEPENDS ON, which the walk could
//! not reach and which therefore still reports the status its create left
//! (`CREATE_COMPLETE`). Each row reads the owning service back, so a `DELETE_COMPLETE` record
//! means the resource really went and a `DELETE_SKIPPED` record means it really stayed.
//!
//! `resource_status::*` names the members of the model's `ResourceStatus` enum
//! (`smithy_query(query="shape",shape="ResourceStatus")`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, deploy, describe_stack, member, rds, router, sns, wait_for_the_transition, xml_blocks,
    xml_text,
};
use crate::stack_resources::sns_arn;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

/// The declaration of a DB instance whose deletion protection is ON: Amazon RDS refuses
/// `DeleteDBInstance` for one, which is what makes the stack's deletion fail.
fn protected_instance(identifier: &str, depends_on: Option<&str>) -> Value {
    let mut declared = json!({
        "Type": "AWS::RDS::DBInstance",
        "Properties": {
            "DBInstanceIdentifier": identifier,
            "Engine": "mysql",
            "DBInstanceClass": "db.t3.micro",
            "DeletionProtection": true
        }
    });
    if let Some(needed) = depends_on {
        declared["DependsOn"] = json!(needed);
    }
    declared
}

/// The declaration of a topic Amazon SNS deletes without complaint, under whatever
/// `DeletionPolicy` the row wants on it.
fn topic(name: &str, policy: Option<&str>) -> Value {
    let mut declared = json!({"Type": "AWS::SNS::Topic", "Properties": {"TopicName": name}});
    if let Some(policy) = policy {
        declared["DeletionPolicy"] = json!(policy);
    }
    declared
}

/// What one resource of the row's stack must report once the deletion has failed.
struct Expected {
    logical_id: &'static str,
    resource_status: &'static str,
    /// The topic this resource is, and whether Amazon SNS must still hold it — `None` for a
    /// resource that is not a topic.
    topic: Option<(&'static str, bool)>,
}

/// One row: a stack whose deletion one owning service refuses, and what every resource view
/// of it must report afterwards.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The logical id whose deletion Amazon RDS refuses.
    refused: &'static str,
    /// The DB instance that must have survived the refused deletion.
    surviving_instance: &'static str,
    expected: Vec<Expected>,
}

/// A fragment of Amazon RDS's own refusal for an instance its deletion protection holds,
/// which belongs on the refused resource and not on the stack.
const DIAGNOSIS: &str = "Cannot delete protected DB Instance";

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a refused DB instance beside a topic the walk deleted",
            stack: "refview-deleted",
            template: json!({
                "Resources": {
                    "Db": protected_instance("refview-deleted-db", None),
                    "ZTopic": topic("refview-deleted-z", None)
                }
            })
            .to_string(),
            refused: "Db",
            surviving_instance: "refview-deleted-db",
            expected: vec![
                Expected {
                    logical_id: "Db",
                    resource_status: resource_status::DELETE_FAILED,
                    topic: None,
                },
                Expected {
                    logical_id: "ZTopic",
                    resource_status: resource_status::DELETE_COMPLETE,
                    topic: Some(("refview-deleted-z", false)),
                },
            ],
        },
        Row {
            case: "a refused DB instance beside a topic its DeletionPolicy kept",
            stack: "refview-skipped",
            template: json!({
                "Resources": {
                    "Db": protected_instance("refview-skipped-db", None),
                    "Keep": topic("refview-skipped-keep", Some("Retain")),
                    "ZTopic": topic("refview-skipped-z", None)
                }
            })
            .to_string(),
            refused: "Db",
            surviving_instance: "refview-skipped-db",
            expected: vec![
                Expected {
                    logical_id: "Db",
                    resource_status: resource_status::DELETE_FAILED,
                    topic: None,
                },
                Expected {
                    logical_id: "Keep",
                    resource_status: resource_status::DELETE_SKIPPED,
                    topic: Some(("refview-skipped-keep", true)),
                },
                Expected {
                    logical_id: "ZTopic",
                    resource_status: resource_status::DELETE_COMPLETE,
                    topic: Some(("refview-skipped-z", false)),
                },
            ],
        },
        Row {
            case: "a refused DB instance that depends on a topic, beside one kept by policy",
            stack: "refview-needed",
            template: json!({
                "Resources": {
                    "ADep": topic("refview-needed-dep", None),
                    "Db": protected_instance("refview-needed-db", Some("ADep")),
                    "Keep": topic("refview-needed-keep", Some("Retain"))
                }
            })
            .to_string(),
            refused: "Db",
            surviving_instance: "refview-needed-db",
            expected: vec![
                // The topic the database NEEDS cannot be deleted while the database is still
                // there, so the walk never reached it: it reports what its create left.
                Expected {
                    logical_id: "ADep",
                    resource_status: resource_status::CREATE_COMPLETE,
                    topic: Some(("refview-needed-dep", true)),
                },
                Expected {
                    logical_id: "Db",
                    resource_status: resource_status::DELETE_FAILED,
                    topic: None,
                },
                Expected {
                    logical_id: "Keep",
                    resource_status: resource_status::DELETE_SKIPPED,
                    topic: Some(("refview-needed-keep", true)),
                },
            ],
        },
    ]
}

/// The `(ResourceStatus, ResourceStatusReason)` of the NEWEST event the stack recorded for
/// `logical_id` — `DescribeStackEvents` answers newest first — or `None` when it recorded
/// none at all.
async fn newest_event(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> Option<(String, String)> {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackEvents must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .find(|entry| xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id))
        .map(|entry| {
            (
                xml_text(entry, "ResourceStatus").unwrap_or_default(),
                xml_text(entry, "ResourceStatusReason").unwrap_or_default(),
            )
        })
}

/// The `(ResourceStatus, ResourceStatusReason)` `DescribeStackResources` reports for
/// `logical_id`.
async fn described_resources(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> (String, String) {
    let (code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackResources must answer 200, got {code} {body}"
    );
    let entry = xml_blocks(&body, "member")
        .into_iter()
        .find(|entry| xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: no DescribeStackResources record for {logical_id}: {body}")
        });
    (
        xml_text(&entry, "ResourceStatus").unwrap_or_default(),
        xml_text(&entry, "ResourceStatusReason").unwrap_or_default(),
    )
}

/// The same pair, as `DescribeStackResource` reports it for the one named resource.
async fn described_resource(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> (String, String) {
    let (code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackResource({logical_id}) must answer 200, got {code} {body}"
    );
    (
        xml_text(&body, "ResourceStatus").unwrap_or_default(),
        xml_text(&body, "ResourceStatusReason").unwrap_or_default(),
    )
}

/// The same pair, as `ListStackResources` reports it in its summary of that resource.
async fn listed_resource(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> (String, String) {
    let (code, body) = call(
        router,
        "ListStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: ListStackResources must answer 200, got {code} {body}"
    );
    let entry = xml_blocks(&body, "member")
        .into_iter()
        .find(|entry| xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: no ListStackResources summary for {logical_id}: {body}")
        });
    (
        xml_text(&entry, "ResourceStatus").unwrap_or_default(),
        xml_text(&entry, "ResourceStatusReason").unwrap_or_default(),
    )
}

#[tokio::test]
async fn every_resource_view_reports_what_the_failed_deletion_did_to_that_resource() {
    for row in rows() {
        let router = router();
        deploy(&router, row.stack, &row.template).await;

        let (code, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            code, 200,
            "case {:?}: DeleteStack must be accepted — the stack reports the outcome — got \
             {code} {body}",
            row.case
        );
        wait_for_the_transition().await;

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

        for expected in &row.expected {
            let logical_id = expected.logical_id;
            // What the EVENT STREAM last recorded about this resource, which is the status
            // every view of it has to agree with.
            let event = newest_event(&router, row.case, row.stack, logical_id)
                .await
                .unwrap_or_else(|| {
                    panic!("case {:?}: no event at all for {logical_id}", row.case)
                });
            assert_eq!(
                event.0, expected.resource_status,
                "case {:?}: expected the newest event for {logical_id} to be {}, got {:?}",
                row.case, expected.resource_status, event
            );

            for (reader, reported) in [
                (
                    "DescribeStackResources",
                    described_resources(&router, row.case, row.stack, logical_id).await,
                ),
                (
                    "DescribeStackResource",
                    described_resource(&router, row.case, row.stack, logical_id).await,
                ),
                (
                    "ListStackResources",
                    listed_resource(&router, row.case, row.stack, logical_id).await,
                ),
            ] {
                assert_eq!(
                    reported.0, expected.resource_status,
                    "case {:?}: expected {reader} to report {logical_id} at {}, got {:?}",
                    row.case, expected.resource_status, reported
                );
                assert_eq!(
                    reported, event,
                    "case {:?}: expected {reader} and the event stream to agree about \
                     {logical_id}, got {reported:?} against {event:?}",
                    row.case
                );
                if logical_id == row.refused {
                    assert!(
                        reported.1.contains(DIAGNOSIS),
                        "case {:?}: expected {reader} to carry the owning service's refusal \
                         {DIAGNOSIS:?} for {logical_id}, got {:?}",
                        row.case,
                        reported.1
                    );
                } else {
                    assert!(
                        reported.1.is_empty(),
                        "case {:?}: expected {reader} to report no failure of its own for \
                         {logical_id}, got {:?}",
                        row.case,
                        reported.1
                    );
                }
            }

            // ...and the record is BACKED: a topic reported deleted really went from Amazon
            // SNS, one reported skipped or untouched is really still there.
            if let Some((name, still_there)) = expected.topic {
                let (code, attributes) =
                    sns(&router, "GetTopicAttributes", &[("TopicArn", &sns_arn(name))]).await;
                assert_eq!(
                    code == 200,
                    still_there,
                    "case {:?}: expected Amazon SNS to still hold {name} = {still_there} for a \
                     {} record, got {code} {attributes}",
                    row.case,
                    expected.resource_status
                );
            }
        }

        // The refused database is still there, with its protection on: the refusal changed
        // nothing in the owning service.
        let (code, instances) = rds(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", row.surviving_instance)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: expected the protected instance to survive the refused deletion, got \
             {code} {instances}",
            row.case
        );
        assert_eq!(
            xml_text(&instances, "DeletionProtection").as_deref(),
            Some("true"),
            "case {:?}: expected the refused deletion to leave deletion protection on, got \
             {instances}",
            row.case
        );
    }
}
