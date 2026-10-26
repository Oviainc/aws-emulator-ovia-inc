// SPDX-License-Identifier: CC0-1.0

//! What a stack's resource views report once an operation has DISPOSED of its resources,
//! end to end over the awsQuery wire.
//!
//! The rule under test: **a stack operation that disposes of a stack's resources — the
//! rollback of a create whose resource an owning service refused, and the deletion that
//! `OnFailure=DELETE` performs — KEEPS one record per resource it acted on, at the
//! terminal `DELETE_COMPLETE` it disposed of that resource at, rather than dropping the
//! records.** `ListStackResources` declares no errors and reports
//! `StackResourceSummaries`, so a stack that exists answers 200 with those rows, and they
//! are the only per-resource view left of a stack whose create failed: they name the
//! logical id and the resource TYPE the template declared, which is what says which
//! declared resource the rollback disposed of. A declaration the deployment never reached
//! is the other half of the rule: the work on it never started, so there is nothing to
//! report for it.
//!
//! `DescribeStackResources` reads the SAME records, so every row is asserted through both
//! readers.
//!
//! The rows differ along what the rule generalises over: how many resources the create
//! declared, whether the resource that failed was the first one or came after resources
//! the create had already made, whether the type is one an owning service creates or one
//! the stack records from the template alone, which service refused, and how the create
//! was unwound (`ROLLBACK`, the default, and `OnFailure=DELETE`, which disposes of the
//! stack itself and leaves it addressable by its stack id).
//!
//! The refusals here come from an owning service this crate's harness does not mount —
//! which is exactly the shape of "the call did not happen" for the type it owns
//! (`unprovisioned_resources.rs` pins that), and `AWS::S3Vectors::VectorBucket` is the
//! declared type of the recorded unit this suite belongs to.

use serde_json::json;

use crate::list_stack_resources::{wire_summaries, WireSummary};
use crate::stack_lifecycle::{call, create_stack, describe_stack, member, xml_text};
use crate::stack_resources::{is_iso8601_instant, wire_resources};
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

/// How the failed create was unwound, which decides how the stack is addressed afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unwind {
    /// `ROLLBACK`, the default: the stack stays, reporting `ROLLBACK_COMPLETE`.
    Rollback,
    /// `OnFailure=DELETE`: the stack itself is disposed of, and a disposed stack is named
    /// by its unique stack id only.
    Delete,
}

/// One row: a template whose create cannot complete, and the records the stack must keep
/// once the unwind has run.
struct Row<'a> {
    case: &'a str,
    stack: &'a str,
    template: String,
    unwind: Unwind,
    /// `(logical id, resource type)` for every resource the deployment ACTED ON, in any
    /// order: the rows are compared as a set, sorted by logical id.
    expected: &'a [(&'a str, &'a str)],
    /// The logical ids the deployment never reached, which no row may name.
    unreached: &'a [&'a str],
}

/// The type of the recorded unit's resource, whose owning service this crate's harness
/// does not mount — so its create cannot be carried out and the stack rolls back.
const VECTOR_BUCKET: &str = "AWS::S3Vectors::VectorBucket";
/// A type Amazon S3 really creates through the harness's own S3 service.
const BUCKET: &str = "AWS::S3::Bucket";
/// A type this emulator provisions nowhere: the stack records it from the template alone,
/// which is still a resource the deployment acted on.
const VPC: &str = "AWS::EC2::VPC";
/// A type whose owning service (Amazon Data Firehose) the harness does not mount either.
const DELIVERY_STREAM: &str = "AWS::KinesisFirehose::DeliveryStream";
/// A type the walk never reaches in the row that declares it behind a failure.
const TOPIC: &str = "AWS::SNS::Topic";

/// A delivery stream's declaration, which Amazon Data Firehose would have to create.
fn stream_properties(name: &str) -> serde_json::Value {
    json!({
        "DeliveryStreamName": name,
        "ExtendedS3DestinationConfiguration": {
            "BucketARN": "arn:aws:s3:::a-bucket",
            "RoleARN": "arn:aws:iam::000000000000:role/a-role"
        }
    })
}

fn rows() -> Vec<Row<'static>> {
    vec![
        Row {
            // The recorded shape: one declared vector bucket, and the stack rolls back.
            case: "one declared resource, the only one, whose create is refused",
            stack: "disposed-one",
            template: json!({
                "Resources": {
                    "Vectors": {
                        "Type": VECTOR_BUCKET,
                        "Properties": {"VectorBucketName": "disposed.one.vectors"}
                    }
                }
            })
            .to_string(),
            unwind: Unwind::Rollback,
            expected: &[("Vectors", VECTOR_BUCKET)],
            unreached: &[],
        },
        Row {
            // The failure comes AFTER a resource the create really made in Amazon S3, so
            // the rollback had something to remove as well as something it never created.
            case: "a resource the create had made before the one that failed",
            stack: "disposed-after-made",
            template: json!({
                "Resources": {
                    "ABucket": {
                        "Type": BUCKET,
                        "Properties": {"BucketName": "disposed-after-made-bucket"}
                    },
                    "ZVectors": {
                        "Type": VECTOR_BUCKET,
                        "Properties": {"VectorBucketName": "disposed.after.made"}
                    }
                }
            })
            .to_string(),
            unwind: Unwind::Rollback,
            expected: &[("ABucket", BUCKET), ("ZVectors", VECTOR_BUCKET)],
            unreached: &[],
        },
        Row {
            // Three resources: one the stack records from the template alone, one whose
            // owning service refuses the create, and one declared BEHIND that failure,
            // which the deployment never reaches.
            case: "a declared-only resource, a refused one, and one the walk never reaches",
            stack: "disposed-mixed",
            template: json!({
                "Resources": {
                    "AVpc": {"Type": VPC, "Properties": {"CidrBlock": "10.0.0.0/16"}},
                    "MStream": {
                        "Type": DELIVERY_STREAM,
                        "Properties": stream_properties("disposed-mixed-stream")
                    },
                    "ZTopic": {
                        "Type": TOPIC,
                        "DependsOn": "MStream",
                        "Properties": {"TopicName": "disposed-mixed-topic"}
                    }
                }
            })
            .to_string(),
            unwind: Unwind::Rollback,
            expected: &[("AVpc", VPC), ("MStream", DELIVERY_STREAM)],
            unreached: &["ZTopic"],
        },
        Row {
            // The other documented unwind: the stack itself is disposed of, and its
            // records go on being reported for it — by its stack id, since a disposed
            // stack's name no longer names it.
            case: "OnFailure=DELETE disposes of the stack and keeps its records",
            stack: "disposed-deleted",
            template: json!({
                "Resources": {
                    "AVpc": {"Type": VPC, "Properties": {"CidrBlock": "10.1.0.0/16"}},
                    "ZVectors": {
                        "Type": VECTOR_BUCKET,
                        "Properties": {"VectorBucketName": "disposed.deleted.vectors"}
                    }
                }
            })
            .to_string(),
            unwind: Unwind::Delete,
            expected: &[("AVpc", VPC), ("ZVectors", VECTOR_BUCKET)],
            unreached: &[],
        },
    ]
}

/// Every page of `ListStackResources` for one stack, drained through its `NextToken` —
/// which is how a client inventories a stack's resources.
async fn every_summary(
    router: &MultiServiceRouter,
    case: &str,
    name_or_id: &str,
) -> Vec<WireSummary> {
    let mut summaries = Vec::new();
    let mut next_token: Option<String> = None;
    // Bounded, so a token the service failed to retire cannot loop forever.
    for _ in 0..16 {
        let mut members = vec![member("StackName", name_or_id)];
        if let Some(token) = &next_token {
            members.push(member("NextToken", token));
        }
        let (status_code, body) =
            crate::list_stack_resources::list_stack_resources(router, &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: ListStackResources declares no errors, so an existing stack \
             answers 200, got {status_code} {body}"
        );
        summaries.extend(wire_summaries(&body));
        next_token = xml_text(&body, "NextToken");
        if next_token.is_none() {
            break;
        }
    }
    summaries
}

/// The `(logical id, type, status)` triples a list of summaries reports, sorted by logical
/// id.
fn reported(summaries: &[WireSummary]) -> Vec<(String, String, String)> {
    let mut rows: Vec<(String, String, String)> = summaries
        .iter()
        .map(|summary| {
            (
                summary.logical_id.clone().unwrap_or_default(),
                summary.resource_type.clone().unwrap_or_default(),
                summary.status.clone().unwrap_or_default(),
            )
        })
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn a_disposed_of_stack_keeps_a_record_per_resource_it_acted_on() {
    for row in rows() {
        let router = crate::stack_lifecycle::router();
        let case = row.case;
        let extra = match row.unwind {
            Unwind::Rollback => vec![],
            Unwind::Delete => vec![member("OnFailure", "DELETE")],
        };
        let (status_code, created) =
            create_stack(&router, row.stack, &row.template, &extra).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: CreateStack must answer 200 — the creation starts and the stack \
             reports the outcome — got {status_code} {created}"
        );
        let stack_id = xml_text(&created, "StackId").unwrap_or_default();
        // Twice: the create's own failure is recorded at once, and the unwind it owes has
        // its own clock-owed terminal status.
        crate::stack_lifecycle::wait_for_the_transition().await;
        crate::stack_lifecycle::wait_for_the_transition().await;

        // How the stack is addressed once the unwind has run, and what it reports.
        let address = match row.unwind {
            Unwind::Rollback => {
                let (status_code, described) = describe_stack(&router, row.stack).await;
                assert_eq!(
                    status_code, 200,
                    "case {case:?}: the rolled-back stack is still there, got {status_code} \
                     {described}"
                );
                assert_eq!(
                    xml_text(&described, "StackStatus").as_deref(),
                    Some(status::ROLLBACK_COMPLETE),
                    "case {case:?}: expected the failed create to be rolled back, got \
                     {described}"
                );
                row.stack.to_string()
            }
            Unwind::Delete => {
                let (status_code, described) = describe_stack(&router, row.stack).await;
                assert_eq!(
                    status_code, 400,
                    "case {case:?}: a disposed-of stack is no longer named by its name, got \
                     {status_code} {described}"
                );
                stack_id.clone()
            }
        };

        // Every page of the summaries: one row per resource the deployment acted on, at
        // the terminal status it was disposed of at.
        let summaries = every_summary(&router, case, &address).await;
        let expected: Vec<(String, String, String)> = {
            let mut rows: Vec<(String, String, String)> = row
                .expected
                .iter()
                .map(|(logical_id, resource_type)| {
                    (
                        (*logical_id).to_string(),
                        (*resource_type).to_string(),
                        resource_status::DELETE_COMPLETE.to_string(),
                    )
                })
                .collect();
            rows.sort();
            rows
        };
        let actual = reported(&summaries);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        for summary in &summaries {
            let last_updated = summary.last_updated.clone().unwrap_or_default();
            assert!(
                is_iso8601_instant(&last_updated),
                "case {case:?}: every summary carries the instant the status was updated, \
                 got {last_updated:?} in {summary:?}"
            );
            assert_eq!(
                summary.status_reason, None,
                "case {case:?}: the terminal delete status speaks for itself, so the summary \
                 states no reason, got {summary:?}"
            );
        }
        for logical_id in row.unreached {
            assert!(
                !summaries
                    .iter()
                    .any(|summary| summary.logical_id.as_deref() == Some(*logical_id)),
                "case {case:?}: the deployment never reached {logical_id}, so nothing is \
                 reported for it, got {summaries:?}"
            );
        }

        // `DescribeStackResources` reads the SAME records, so it reports the same rows.
        let (status_code, body) = call(
            &router,
            "DescribeStackResources",
            &[member("StackName", &address)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: DescribeStackResources must answer 200, got {status_code} {body}"
        );
        let mut described: Vec<(String, String, String)> = wire_resources(&body)
            .iter()
            .map(|resource| {
                (
                    resource.logical_id.clone().unwrap_or_default(),
                    resource.resource_type.clone().unwrap_or_default(),
                    resource.status.clone().unwrap_or_default(),
                )
            })
            .collect();
        described.sort();
        assert_eq!(
            described, expected,
            "case {case:?}: the two readers of one record must agree, expected {expected:?}, \
             got {described:?}"
        );
    }
}
