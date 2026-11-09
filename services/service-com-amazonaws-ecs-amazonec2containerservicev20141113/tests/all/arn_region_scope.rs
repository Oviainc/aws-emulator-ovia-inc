// SPDX-License-Identifier: CC0-1.0

//! Which Region's resources an ARN a REQUEST supplies may name, and what a batch answer
//! echoes back for an identifier it could not resolve. Every case goes through the shared
//! wire harness in [`crate::ecs_wire`], the same front door a client uses, so nothing
//! reaches into the state struct.
//!
//! The rules under test, each over a TABLE of inputs rather than one request:
//!
//! 1. Amazon ECS is REGIONAL ("Amazon ECS clusters are Region specific"), so an ARN a
//!    request supplies as a resource identifier names a resource of the Region the request
//!    was addressed to: one whose Region segment is some other Region names nothing this
//!    endpoint can resolve, and is refused HTTP 400 with the fixed sentence "Invalid Region
//!    in ARN" BEFORE the identifier is resolved — so the refusal does not depend on what
//!    this store happens to hold, nor on the FORM of the id the ARN carries. The rows span
//!    the operations that resolve such an identifier, the member kinds an ARN reaches them
//!    through (`cluster`, `task`, a list of tasks or clusters, `containerInstance`, an
//!    `attachmentArn`, a tagging `resourceArn`) and the Region the ARN carries.
//! 2. The refusal is about the ARN's REGION and not about ARNs: the same request carrying
//!    the same identifier in THIS Region's spelling resolves as the identifier it is, and an
//!    ARN with no Region segment at all carries no Region to disagree with.
//! 3. A batch answer reports an identifier it could not resolve AS THE CALLER SENT IT:
//!    `MISSING` under the ARN itself where the entry was an ARN, and under the id alone
//!    where it was an id — never under a cluster-qualified ARN the caller never named,
//!    which only a STORED task has. A task the call resolves but may not act on keeps its
//!    own stored ARN.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    answered, cluster_with_capacity, list, ok, refusal, register_definition, service, string,
    ACCOUNT, SIGNED_REGION,
};

/// The sentence an ARN of another Region earns, wherever one reaches this service.
const INVALID_REGION_IN_ARN: &str = "Invalid Region in ARN";

/// `Failure$reason` for an identifier that names nothing.
const MISSING: &str = "MISSING";

/// `Failure$reason` for a task a protection call cannot act on.
const TASK_NOT_VALID: &str = "TASK_NOT_VALID";

/// The cluster every row acts in.
const CLUSTER: &str = "regionscope";

/// The task-definition family the live task runs.
const FAMILY: &str = "regionscopefamily";

/// A task id of the form `RunTask` mints: 32 hexadecimal digits.
const TASK_ID: &str = "0123456789abcdef0123456789abcdef";

/// A container-instance id of the form a registration mints.
const INSTANCE_ID: &str = "fedcba9876543210fedcba9876543210";

/// An attachment id of the form an attachment carries.
const ATTACHMENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// Region segments that are NOT the Region these requests are addressed to
/// ([`SIGNED_REGION`]), each with the partition it belongs to. The first five are Region
/// codes of three partitions, from the Regions table of the AWS General Reference
/// (`us-east-1`, `eu-west-1`, `ap-southeast-4`, `us-gov-west-1`, `cn-north-1`); the last two
/// are values that are no Region code at all. What makes an ARN another Region's is only
/// that its Region segment is not this endpoint's.
const OTHER_REGIONS: [(&str, &str); 7] = [
    ("aws", "us-east-1"),
    ("aws", "eu-west-1"),
    ("aws", "ap-southeast-4"),
    ("aws-us-gov", "us-gov-west-1"),
    ("aws-cn", "cn-north-1"),
    ("aws", "not-a-region"),
    ("aws", "US-WEST-2"),
];

/// One row of the ARN table: the operation, the ARN it is given (built in the partition and
/// Region the row is run with), the request built around that ARN, and the error code this
/// operation answers an ARN of another Region with.
struct ArnRow {
    case: &'static str,
    operation: &'static str,
    arn: fn(&str, &str) -> String,
    request: fn(&str) -> Value,
    code: &'static str,
}

/// An ECS ARN in `partition` and `region` for the resource path `resource`.
fn ecs_arn(partition: &str, region: &str, resource: &str) -> String {
    format!("arn:{partition}:ecs:{region}:{ACCOUNT}:{resource}")
}

/// The cluster's ARN in `partition` and `region`.
fn cluster_arn(partition: &str, region: &str) -> String {
    ecs_arn(partition, region, &format!("cluster/{CLUSTER}"))
}

/// A task ARN in `partition` and `region`, cluster-qualified the way a stored task's is.
fn task_arn(partition: &str, region: &str) -> String {
    ecs_arn(partition, region, &format!("task/{CLUSTER}/{TASK_ID}"))
}

/// A container-instance ARN in `partition` and `region`.
fn instance_arn(partition: &str, region: &str) -> String {
    ecs_arn(
        partition,
        region,
        &format!("container-instance/{CLUSTER}/{INSTANCE_ID}"),
    )
}

/// A container-instance ARN whose ID SEGMENT is no container-instance id, so a request
/// carrying it would earn the id-form refusal if the ARN were read at all.
fn instance_arn_without_an_id(partition: &str, region: &str) -> String {
    ecs_arn(
        partition,
        region,
        &format!("container-instance/{CLUSTER}/not an id"),
    )
}

/// An attachment ARN in `partition` and `region`.
fn attachment_arn(partition: &str, region: &str) -> String {
    ecs_arn(partition, region, &format!("attachment/{ATTACHMENT_ID}"))
}

/// Every operation that resolves a caller-supplied ARN identifier on this surface, one row
/// per member kind the ARN reaches it through.
fn arn_rows() -> Vec<ArnRow> {
    vec![
        ArnRow {
            case: "SubmitContainerStateChange, the task",
            operation: "SubmitContainerStateChange",
            arn: task_arn,
            request: |arn| json!({ "cluster": CLUSTER, "task": arn, "status": "RUNNING" }),
            code: "ClientException",
        },
        ArnRow {
            case: "SubmitTaskStateChange, the task",
            operation: "SubmitTaskStateChange",
            arn: task_arn,
            request: |arn| json!({ "cluster": CLUSTER, "task": arn, "status": "RUNNING" }),
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "SubmitTaskStateChange, an attachment of the report",
            operation: "SubmitTaskStateChange",
            arn: attachment_arn,
            request: |arn| {
                json!({
                    "cluster": CLUSTER,
                    "task": TASK_ID,
                    "attachments": [{ "attachmentArn": arn, "status": "ATTACHED" }],
                })
            },
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "SubmitAttachmentStateChanges, the attachment",
            operation: "SubmitAttachmentStateChanges",
            arn: attachment_arn,
            request: |arn| {
                json!({
                    "cluster": CLUSTER,
                    "attachments": [{ "attachmentArn": arn, "status": "ATTACHED" }],
                })
            },
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "DiscoverPollEndpoint, the container instance",
            operation: "DiscoverPollEndpoint",
            arn: instance_arn,
            request: |arn| json!({ "cluster": CLUSTER, "containerInstance": arn }),
            code: "ClientException",
        },
        ArnRow {
            case: "DiscoverPollEndpoint, a container instance ARN carrying no id",
            operation: "DiscoverPollEndpoint",
            arn: instance_arn_without_an_id,
            request: |arn| json!({ "cluster": CLUSTER, "containerInstance": arn }),
            code: "ClientException",
        },
        ArnRow {
            case: "DiscoverPollEndpoint, the cluster",
            operation: "DiscoverPollEndpoint",
            arn: cluster_arn,
            request: |arn| json!({ "cluster": arn }),
            code: "ClientException",
        },
        ArnRow {
            case: "GetTaskProtection, a task of the list",
            operation: "GetTaskProtection",
            arn: task_arn,
            request: |arn| json!({ "cluster": CLUSTER, "tasks": [arn] }),
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "UpdateTaskProtection, a task of the list",
            operation: "UpdateTaskProtection",
            arn: task_arn,
            request: |arn| {
                json!({ "cluster": CLUSTER, "tasks": [arn], "protectionEnabled": true })
            },
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "DescribeTasks, a task of the list",
            operation: "DescribeTasks",
            arn: task_arn,
            request: |arn| json!({ "cluster": CLUSTER, "tasks": [arn] }),
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "DescribeClusters, a cluster of the list",
            operation: "DescribeClusters",
            arn: cluster_arn,
            request: |arn| json!({ "clusters": [arn] }),
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "StopTask, the task",
            operation: "StopTask",
            arn: task_arn,
            request: |arn| json!({ "cluster": CLUSTER, "task": arn }),
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "ListTasks, the cluster",
            operation: "ListTasks",
            arn: cluster_arn,
            request: |arn| json!({ "cluster": arn }),
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "DescribeContainerInstances, the cluster",
            operation: "DescribeContainerInstances",
            arn: cluster_arn,
            request: |arn| json!({ "cluster": arn, "containerInstances": [INSTANCE_ID] }),
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "UpdateContainerInstancesState, an instance of the list",
            operation: "UpdateContainerInstancesState",
            arn: instance_arn,
            request: |arn| {
                json!({
                    "cluster": CLUSTER,
                    "containerInstances": [arn],
                    "status": "DRAINING",
                })
            },
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "TagResource, the resourceArn",
            operation: "TagResource",
            arn: cluster_arn,
            request: |arn| {
                json!({ "resourceArn": arn, "tags": [{ "key": "team", "value": "platform" }] })
            },
            code: "InvalidParameterException",
        },
        ArnRow {
            case: "ListTagsForResource, the resourceArn",
            operation: "ListTagsForResource",
            arn: cluster_arn,
            request: |arn| json!({ "resourceArn": arn }),
            code: "InvalidParameterException",
        },
    ]
}

/// A router holding the cluster the rows act in, with capacity, and one revision of
/// [`FAMILY`].
async fn fixture() -> MultiServiceRouter {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "fixture").await;
    register_definition(&router, FAMILY, "fixture").await;
    router
}

#[tokio::test]
async fn an_arn_of_another_region_is_refused_before_the_identifier_is_resolved() {
    let router = fixture().await;

    for row in arn_rows() {
        for (partition, region) in OTHER_REGIONS {
            let arn = (row.arn)(partition, region);
            let case = format!("{}: {arn}", row.case);
            let (code, message) = refusal(&router, row.operation, (row.request)(&arn), &case).await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (row.code, INVALID_REGION_IN_ARN),
                "case {case:?}: expected the ARN-Region refusal, got {code} with {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_same_identifier_in_this_region_is_resolved_as_the_identifier_it_is() {
    let router = fixture().await;

    // THIS endpoint's Region: every row is answered as the identifier it carries — either
    // served, or refused for the resource it names rather than for the ARN.
    for row in arn_rows() {
        let arn = (row.arn)("aws", SIGNED_REGION);
        let case = format!("{}: {arn}", row.case);
        let (status, body) = answered(&router, row.operation, (row.request)(&arn)).await;
        assert_ne!(
            body["message"].as_str(),
            Some(INVALID_REGION_IN_ARN),
            "case {case:?}: this Region's own ARN is not refused for its Region, got \
             {status} with {body}"
        );
    }

    // An ARN with NO Region segment names a resource no Region owns, so it carries no
    // Region to disagree with and resolves as the name it holds.
    let case = "a cluster ARN with an empty Region segment";
    let global = format!("arn:aws:ecs::{ACCOUNT}:cluster/{CLUSTER}");
    let body = ok(&router, "ListTasks", json!({ "cluster": global }), case).await;
    assert!(
        list(&body, "taskArns", case).is_empty(),
        "case {case:?}: expected the cluster's (empty) task list, got {body}"
    );
}

/// The failure entry a protection call reports for ONE reference, with the reference's own
/// spelling and the ARN the entry is expected to carry.
struct FailureRow {
    case: &'static str,
    reference: String,
    expected_arn: String,
    expected_reason: &'static str,
}

#[tokio::test]
async fn a_task_protection_failure_names_the_identifier_the_request_sent() {
    let router = fixture().await;

    // A live STANDALONE task, so one row is a task the call resolves and may not act on.
    let body = ok(
        &router,
        "RunTask",
        json!({ "cluster": CLUSTER, "taskDefinition": FAMILY, "count": 1 }),
        "fixture",
    )
    .await;
    let running = string(&list(&body, "tasks", "fixture")[0], "taskArn", "fixture");

    let rows = [
        FailureRow {
            case: "a task id nothing holds",
            reference: TASK_ID.to_string(),
            expected_arn: format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{TASK_ID}"),
            expected_reason: MISSING,
        },
        FailureRow {
            case: "an unqualified task ARN nothing holds",
            reference: format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{TASK_ID}"),
            expected_arn: format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{TASK_ID}"),
            expected_reason: MISSING,
        },
        FailureRow {
            case: "a cluster-qualified task ARN nothing holds",
            reference: format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{CLUSTER}/{TASK_ID}"
            ),
            expected_arn: format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{CLUSTER}/{TASK_ID}"
            ),
            expected_reason: MISSING,
        },
        FailureRow {
            case: "a standalone task this cluster holds",
            reference: running.clone(),
            expected_arn: running.clone(),
            expected_reason: TASK_NOT_VALID,
        },
    ];

    for row in &rows {
        for operation in ["GetTaskProtection", "UpdateTaskProtection"] {
            let case = format!("{operation}: {}", row.case);
            let mut request = json!({ "cluster": CLUSTER, "tasks": [row.reference.clone()] });
            if operation == "UpdateTaskProtection" {
                request["protectionEnabled"] = json!(true);
            }
            let body = ok(&router, operation, request, &case).await;
            let failures = list(&body, "failures", &case);
            assert_eq!(
                failures.len(),
                1,
                "case {case:?}: expected one failure for {:?}, got {body}",
                row.reference
            );
            assert_eq!(
                (
                    string(&failures[0], "arn", &case).as_str(),
                    string(&failures[0], "reason", &case).as_str()
                ),
                (row.expected_arn.as_str(), row.expected_reason),
                "case {case:?}: expected the failure to name {:?}, got {body}",
                row.expected_arn
            );
            assert!(
                list(&body, "protectedTasks", &case).is_empty(),
                "case {case:?}: a task the call cannot act on protects nothing, got {body}"
            );
        }
    }

    // A task ARN names the cluster that holds the task, so an entry naming ANOTHER cluster
    // than the request's `cluster` is not a failure of that entry: it names two scopes at
    // once, which is no task at all, and the call is refused.
    let elsewhere = format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/elsewhere/{TASK_ID}");
    for operation in ["GetTaskProtection", "UpdateTaskProtection"] {
        let case = format!("{operation}: a task ARN of another cluster of this endpoint");
        let mut request = json!({ "cluster": CLUSTER, "tasks": [elsewhere.clone()] });
        if operation == "UpdateTaskProtection" {
            request["protectionEnabled"] = json!(true);
        }
        let (code, message) = refusal(&router, operation, request, &case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                "Invalid identifier: cluster identifiers mismatch"
            ),
            "case {case:?}: expected the cross-cluster refusal, got {code} with {message:?}"
        );
    }
}
