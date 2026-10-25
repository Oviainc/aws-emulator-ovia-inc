// SPDX-License-Identifier: CC0-1.0

//! The tag namespace Amazon Web Services keeps for ITSELF, across every ECS operation
//! that takes a caller-supplied `tags` list.
//!
//! The rule under test, from the `Tag` shape's own doc (`smithy_query(query="doc",
//! shape="Tag")`): "Do not use `aws:`, `AWS:`, or any upper or lowercase combination of
//! such as a prefix for either keys or values as it is reserved for Amazon Web Services
//! use. You cannot edit or delete tag keys or values with this prefix." The `TagKey`
//! pattern admits ':', so this is the SERVICE's restriction and not the model's: a request
//! whose `tags` carry that prefix on a key or a value is refused with HTTP 400 and
//! `InvalidParameterException`, and nothing it asked for is created or written. A value
//! that merely contains those letters — without the colon, or not at the start — is the
//! caller's own to write and is stored as given.
//!
//! The rows span what the rule generalises over: which operation carries the list, the
//! CASE of the prefix, whether the key or the value carries it, where the offending entry
//! sits in the list, and how long the list is.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{identity_document, list, ok, refusal};
use crate::resource_tags::{create, listed, pairs, setup, Kind, CLUSTER, RUNNABLE_FAMILY};
use crate::task_lifecycle::container;

/// A container image, as a container definition names one. Irrelevant to the rule, so one
/// value for every row.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// Every operation in this service whose input models a `tags` list and whose handler is
/// implemented — the set the rule has to hold over, not just the one the report named.
/// (The remaining `tags`-carrying operations — `CreateTaskSet`, `StartTask`,
/// `CreateGateway`, `CreateDaemon`, `RegisterDaemonTaskDefinition` — still answer 501.)
const TAGGING_OPERATIONS: [&str; 7] = [
    "CreateCapacityProvider",
    "CreateCluster",
    "CreateService",
    "RegisterContainerInstance",
    "RegisterTaskDefinition",
    "RunTask",
    "TagResource",
];

/// A request body for `operation` carrying `tags`, identifying whatever resource that
/// operation needs by `name` (and, for `TagResource`, by the ARN of a cluster that is
/// already there).
fn request_with_tags(operation: &str, name: &str, cluster_arn: &str, tags: &Value) -> Value {
    match operation {
        "CreateCapacityProvider" => json!({
            "name": name,
            "autoScalingGroupProvider": { "autoScalingGroupArn": format!("asg-{name}") },
            "tags": tags,
        }),
        "CreateCluster" => json!({ "clusterName": name, "tags": tags }),
        "CreateService" => json!({
            "cluster": CLUSTER,
            "serviceName": name,
            "taskDefinition": RUNNABLE_FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
            "tags": tags,
        }),
        "RegisterContainerInstance" => json!({
            "cluster": CLUSTER,
            "instanceIdentityDocument": identity_document(name),
            "tags": tags,
        }),
        "RegisterTaskDefinition" => json!({
            "family": name,
            "containerDefinitions": [container("app", IMAGE)],
            "tags": tags,
        }),
        "RunTask" => json!({
            "cluster": CLUSTER,
            "taskDefinition": RUNNABLE_FAMILY,
            "count": 1,
            "tags": tags,
        }),
        "TagResource" => json!({ "resourceArn": cluster_arn, "tags": tags }),
        other => panic!("unexpected operation {other:?}"),
    }
}

/// The records `operation` would have created under `name`, read back through the
/// operation's own reader — empty when the refusal created nothing. `RunTask`,
/// `RegisterContainerInstance` and `TagResource` name no new record of their own, so they
/// are checked by the cluster-wide counts in the test body instead.
async fn created(
    router: &MultiServiceRouter,
    operation: &str,
    name: &str,
    case: &str,
) -> Vec<Value> {
    match operation {
        "CreateCapacityProvider" => {
            let body = ok(
                router,
                "DescribeCapacityProviders",
                json!({ "capacityProviders": [name] }),
                case,
            )
            .await;
            list(&body, "capacityProviders", case)
        }
        "CreateCluster" => {
            let body = ok(router, "DescribeClusters", json!({ "clusters": [name] }), case).await;
            list(&body, "clusters", case)
        }
        "CreateService" => {
            let body = ok(
                router,
                "DescribeServices",
                json!({ "cluster": CLUSTER, "services": [name] }),
                case,
            )
            .await;
            list(&body, "services", case)
        }
        "RegisterTaskDefinition" => {
            let body = ok(
                router,
                "ListTaskDefinitions",
                json!({ "familyPrefix": name }),
                case,
            )
            .await;
            list(&body, "taskDefinitionArns", case)
        }
        _ => Vec::new(),
    }
}

/// The `tags` lists that write into the reserved namespace, each with the case it names.
///
/// They differ along the CASE of the prefix, whether the KEY or the VALUE carries it,
/// where the offending entry sits (alone, second, in the middle of a longer list) and
/// what follows the prefix (nothing, a word, a whole AWS-managed key).
fn reserved_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the reserved prefix as the whole key",
            json!([{ "key": "aws:", "value": "x" }]),
        ),
        (
            "a lowercase reserved key",
            json!([{ "key": "aws:reserved", "value": "x" }]),
        ),
        (
            "an uppercase reserved key",
            json!([{ "key": "AWS:cloudformation:stack-name", "value": "x" }]),
        ),
        (
            "a capitalised reserved key",
            json!([{ "key": "Aws:billing", "value": "x" }]),
        ),
        (
            "a reserved key in a case no one would type",
            json!([{ "key": "aWs:team", "value": "x" }]),
        ),
        (
            "a reserved key whose value is empty",
            json!([{ "key": "aws:empty", "value": "" }]),
        ),
        (
            "a reserved key as the second entry of two",
            json!([
                { "key": "env", "value": "prod" },
                { "key": "aws:second", "value": "x" },
            ]),
        ),
        (
            "a reserved key in the middle of a longer list",
            json!([
                { "key": "env", "value": "prod" },
                { "key": "aws:middle", "value": "x" },
                { "key": "owner", "value": "team-a" },
                { "key": "tier", "value": "web" },
            ]),
        ),
        (
            "a reserved VALUE under an ordinary key",
            json!([{ "key": "owner", "value": "aws:internal" }]),
        ),
        (
            "a mixed-case reserved value",
            json!([{ "key": "owner", "value": "AwS:internal" }]),
        ),
        (
            "a reserved value as the last entry of three",
            json!([
                { "key": "env", "value": "prod" },
                { "key": "owner", "value": "team-a" },
                { "key": "managed", "value": "AWS:true" },
            ]),
        ),
    ]
}

/// The `tags` lists that carry those letters WITHOUT the reserved prefix, so the caller
/// owns them: the prefix without its colon, the letters not at the start, the colon
/// somewhere else, and an ARN as a value.
fn allowed_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the prefix without its colon",
            json!([{ "key": "aws", "value": "AWS" }]),
        ),
        (
            "a key that only begins with the same letters",
            json!([{ "key": "awsome:thing", "value": "y" }]),
        ),
        (
            "the reserved text not at the start of the key",
            json!([{ "key": "my:aws:thing", "value": "z" }]),
        ),
        (
            "a colon elsewhere in the key",
            json!([{ "key": "cost:center", "value": "cc-42" }]),
        ),
        (
            "an ARN as the value",
            json!([{ "key": "role", "value": "arn:aws:iam::000000000000:role/demo" }]),
        ),
        (
            "two ordinary tags",
            json!([
                { "key": "env", "value": "prod" },
                { "key": "owner", "value": "team-a" },
            ]),
        ),
    ]
}

#[tokio::test]
async fn a_reserved_prefix_tag_is_refused_by_every_operation_that_takes_tags() {
    let router = service_with_setup().await;
    let cluster_arn = create(
        &router,
        Kind::Cluster,
        "tagged",
        &json!([{ "key": "env", "value": "prod" }]),
        "setup",
    )
    .await;

    // What the setup left in the cluster, so the operations that mint a record with no
    // caller-chosen name (`RunTask`, `RegisterContainerInstance`) can be shown to have
    // minted nothing: one container instance for the capacity, and no task at all.
    let instances_before = cluster_listing(&router, "ListContainerInstances", "setup").await;
    let tasks_before = cluster_listing(&router, "ListTasks", "setup").await;

    for operation in TAGGING_OPERATIONS {
        for (index, (row, tags)) in reserved_rows().iter().enumerate() {
            let case = format!("{operation}: {row}");
            // A name per row, so a row cannot pass by colliding with an earlier one. The
            // container-instance row needs an EC2 instance id, which is the same shape of
            // name with the id's own prefix.
            let name = if operation == "RegisterContainerInstance" {
                format!("i-{index:016x}")
            } else {
                format!("reserved{index}")
            };

            let (code, message) = refusal(
                &router,
                operation,
                request_with_tags(operation, &name, &cluster_arn, tags),
                &case,
            )
            .await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException for {tags}, got {code} \
                 ({message:?})"
            );
            assert!(
                !message.is_empty(),
                "case {case:?}: every refusal carries a message, got {message:?}"
            );

            // A refused request creates nothing: the record it would have minted is not
            // there afterwards.
            let records = created(&router, operation, &name, &case).await;
            assert!(
                records.is_empty(),
                "case {case:?}: a refused {operation} must create nothing, but {name} now \
                 reads back as {records:?}"
            );
        }
    }

    // Nor did any refused `RunTask` place a task, or any refused
    // `RegisterContainerInstance` register an instance.
    let instances_after = cluster_listing(&router, "ListContainerInstances", "after").await;
    let tasks_after = cluster_listing(&router, "ListTasks", "after").await;
    assert_eq!(
        instances_after, instances_before,
        "case \"after\": the refused registrations must leave the cluster's container \
         instances at {instances_before:?}, got {instances_after:?}"
    );
    assert_eq!(
        tasks_after, tasks_before,
        "case \"after\": the refused runs must place no task, expected {tasks_before:?}, \
         got {tasks_after:?}"
    );

    // And the cluster the writes were aimed at still carries only the tag it was created
    // with: a refused `TagResource` writes nothing.
    let listed_tags = listed(&router, &cluster_arn, "after").await;
    let want = pairs(
        json!([{ "key": "env", "value": "prod" }])
            .as_array()
            .expect("the literal is a list"),
        "after",
    );
    assert_eq!(
        listed_tags, want,
        "case \"after\": a refused TagResource must write nothing, expected {want:?}, got \
         {listed_tags:?}"
    );
}

#[tokio::test]
async fn tags_that_only_resemble_the_reserved_prefix_are_the_callers_own() {
    let router = service_with_setup().await;

    // The same rows written two ways — at CREATE time and through `TagResource` — and read
    // back through `ListTagsForResource`: the rule refuses the reserved prefix, nothing
    // else.
    for (index, (row, tags)) in allowed_rows().iter().enumerate() {
        let case = format!("created with {row}");
        let arn = create(
            &router,
            Kind::Cluster,
            &format!("allowed{index}"),
            tags,
            &case,
        )
        .await;
        let want = pairs(
            tags.as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the row's tags are a list")),
            &case,
        );
        let actual = listed(&router, &arn, &case).await;
        assert_eq!(
            actual, want,
            "case {case:?}: expected {arn} to carry {want:?}, got {actual:?}"
        );

        let case = format!("tagged with {row}");
        let written = create(
            &router,
            Kind::TaskDefinition,
            &format!("allowed{index}"),
            &json!([]),
            &case,
        )
        .await;
        ok(
            &router,
            "TagResource",
            json!({ "resourceArn": written, "tags": tags }),
            &case,
        )
        .await;
        let actual = listed(&router, &written, &case).await;
        assert_eq!(
            actual, want,
            "case {case:?}: expected {written} to carry {want:?} after TagResource, got \
             {actual:?}"
        );
    }
}

/// The ARNs one of the cluster-scoped list operations reports for [`CLUSTER`], sorted —
/// what the test compares a refused write against.
async fn cluster_listing(router: &MultiServiceRouter, operation: &str, case: &str) -> Vec<String> {
    let body = ok(router, operation, json!({ "cluster": CLUSTER }), case).await;
    let member = match operation {
        "ListContainerInstances" => "containerInstanceArns",
        "ListTasks" => "taskArns",
        other => panic!("unexpected operation {other:?}"),
    };
    let mut arns: Vec<String> = list(&body, member, case)
        .iter()
        .map(|arn| {
            arn.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an ARN, got {arn}"))
                .to_string()
        })
        .collect();
    arns.sort();
    arns
}

/// A router with this service mounted, the cluster the in-cluster resources live in, the
/// capacity a placed task needs, and one task-definition revision to run.
async fn service_with_setup() -> MultiServiceRouter {
    let router = crate::ecs_wire::service();
    setup(&router).await;
    router
}
