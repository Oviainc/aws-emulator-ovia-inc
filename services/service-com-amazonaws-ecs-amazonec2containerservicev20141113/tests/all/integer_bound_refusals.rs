// SPDX-License-Identifier: CC0-1.0

//! Every integer member whose own documentation states an inclusive range, driven end to
//! end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: a value outside that range is refused with the operation's
//! `InvalidParameterException`, whose message states the rule the value broke — the member
//! and the ONE bound it crossed for a member bounding a period or a count, while a
//! `maxResults` carries the page-size sentence its own operation words — and every value
//! inside the range, each published bound included, is accepted and acted on. The refusal
//! is a property of the BOUND, not of the operation or of how far past it the value is, so
//! the table below varies the member, the operation, the bound crossed and the distance
//! past it.
//!
//! The page-size sentences are driven over every paging operation and both ends of every
//! documented range in [`crate::page_size_range_refusals`]; the rows here are the ones
//! that sit beside the other bounded integers.
//!
//! The bounds and their sentences come from the members' own docs
//! (`smithy_query(query="doc",shape="<Request>",member="<member>")`):
//!
//! * `UpdateTaskProtection$expiresInMinutes` — "You can specify a value from 1 minute to up
//!   to 2,880 minutes (48 hours)."
//! * `RunTask$count` — "You can specify up to 10 tasks for each call."
//! * `ListClusters$maxResults`, `ListTasks$maxResults`, `ListDaemonTaskDefinitions$maxResults`,
//!   `ListTaskDefinitionFamilies$maxResults` — "This value can be between 1 and 100."
//! * `ListAccountSettings$maxResults`, `DescribeCapacityProviders$maxResults` — "This value
//!   can be between 1 and 10."
//!
//! Nothing here reaches into the state struct: what a write left is read back through an
//! implemented read.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, list, ok, refusal, service, string};
use crate::task_lifecycle::container;

/// The cluster every row below acts in.
const CLUSTER: &str = "boundedplane";

/// The task-definition family its service and its standalone task run.
const FAMILY: &str = "boundedfamily";

/// The service whose task scale-in protection is set on.
const SERVICE: &str = "bounded-service";

/// A router with one cluster that has capacity and one revision of [`FAMILY`].
async fn fixture() -> MultiServiceRouter {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "fixture").await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "containerDefinitions": [
                container("app", "public.ecr.aws/nginx/nginx:latest"),
            ],
        }),
        "fixture",
    )
    .await;
    router
}

/// The ARN of the one task the service scheduler placed — the kind of task a
/// task-protection call acts on.
async fn service_task(router: &MultiServiceRouter) -> String {
    ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": SERVICE,
            "taskDefinition": FAMILY,
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        "fixture",
    )
    .await;
    let listed = ok(
        router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "serviceName": SERVICE }),
        "fixture",
    )
    .await;
    let arns = list(&listed, "taskArns", "fixture");
    arns.first()
        .and_then(|arn| arn.as_str())
        .unwrap_or_else(|| panic!("case \"fixture\": expected the service's task, got {listed}"))
        .to_string()
}

/// Whether the task is protected right now, read back through `GetTaskProtection`.
async fn protection_of(router: &MultiServiceRouter, task_arn: &str, case: &str) -> bool {
    let body = ok(
        router,
        "GetTaskProtection",
        json!({ "cluster": CLUSTER, "tasks": [task_arn] }),
        case,
    )
    .await;
    let protected = list(&body, "protectedTasks", case);
    protected
        .first()
        .and_then(|task| task["protectionEnabled"].as_bool())
        .unwrap_or_else(|| panic!("case {case:?}: expected the stored protection, got {body}"))
}

#[tokio::test]
async fn an_integer_outside_its_documented_range_is_refused_naming_the_bound_it_crossed() {
    let router = fixture().await;
    let task_arn = service_task(&router).await;

    // One row per (member, bound, distance past it): the value is below the minimum or
    // above the maximum, just past the bound or far past it, on five members of five
    // operations.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "expiresInMinutes one below the period",
            "UpdateTaskProtection",
            json!({
                "cluster": CLUSTER,
                "tasks": [task_arn.clone()],
                "protectionEnabled": true,
                "expiresInMinutes": 0,
            }),
            "expiresInMinutes must be at least 1",
        ),
        (
            "expiresInMinutes far below the period",
            "UpdateTaskProtection",
            json!({
                "cluster": CLUSTER,
                "tasks": [task_arn.clone()],
                "protectionEnabled": true,
                "expiresInMinutes": -1_440,
            }),
            "expiresInMinutes must be at least 1",
        ),
        (
            "expiresInMinutes one above the period",
            "UpdateTaskProtection",
            json!({
                "cluster": CLUSTER,
                "tasks": [task_arn.clone()],
                "protectionEnabled": true,
                "expiresInMinutes": 2_881,
            }),
            "expiresInMinutes must be at most 2880",
        ),
        (
            "expiresInMinutes far above the period",
            "UpdateTaskProtection",
            json!({
                "cluster": CLUSTER,
                "tasks": [task_arn.clone()],
                "protectionEnabled": true,
                "expiresInMinutes": 100_000,
            }),
            "expiresInMinutes must be at most 2880",
        ),
        (
            // A `RunTask` count is a COUNT, which this API words as it words every other
            // count of it (`DesiredCount can not be lesser than 0`).
            "count below the range",
            "RunTask",
            json!({ "cluster": CLUSTER, "taskDefinition": FAMILY, "count": 0 }),
            "Task count can not be lesser than 1",
        ),
        (
            "count one above the range",
            "RunTask",
            json!({ "cluster": CLUSTER, "taskDefinition": FAMILY, "count": 11 }),
            "Task count can not be greater than 10",
        ),
        (
            "a list page below the range",
            "ListClusters",
            json!({ "maxResults": 0 }),
            "Max results can not be lesser than 1",
        ),
        (
            "a list page above the range",
            "ListTasks",
            json!({ "cluster": CLUSTER, "maxResults": 101 }),
            "MaxResults must be a positive number between 1-100.",
        ),
        (
            "a narrower page above its own range",
            "ListAccountSettings",
            json!({ "maxResults": 11 }),
            "MaxResults must be a positive number between 1-10.",
        ),
        (
            "a narrower page below its own range",
            "DescribeCapacityProviders",
            json!({ "maxResults": -2 }),
            "Max results can not be lesser than 1",
        ),
        (
            "a daemon list page below the range",
            "ListDaemonTaskDefinitions",
            json!({ "maxResults": 0 }),
            "MaxResults must be a positive number between 1-100.",
        ),
        (
            "a daemon list page far above the range",
            "ListDaemonTaskDefinitions",
            json!({ "maxResults": 1_000 }),
            "MaxResults must be a positive number between 1-100.",
        ),
        (
            "a family list page one above the range",
            "ListTaskDefinitionFamilies",
            json!({ "maxResults": 101 }),
            "MaxResults must be a positive number between 1-100.",
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (code, message) = refusal(&router, operation, request, case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", expected),
            "case {case:?}: expected {operation} to name the bound it crossed, got {code} \
             with {message:?}"
        );
    }

    // A refusal changes nothing: none of the refused protection periods protected the
    // task.
    assert!(
        !protection_of(&router, &task_arn, "after the refusals").await,
        "case \"after the refusals\": a refused period must protect nothing"
    );

    // Each published bound is INSIDE the range, so each is accepted and acted on.
    for (case, minutes) in [("the shortest period", 1), ("the longest period", 2_880)] {
        let body = ok(
            &router,
            "UpdateTaskProtection",
            json!({
                "cluster": CLUSTER,
                "tasks": [task_arn.clone()],
                "protectionEnabled": true,
                "expiresInMinutes": minutes,
            }),
            case,
        )
        .await;
        let protected = list(&body, "protectedTasks", case);
        assert_eq!(
            protected.first().and_then(|task| task["protectionEnabled"].as_bool()),
            Some(true),
            "case {case:?}: a period at a published bound protects the task, got {body}"
        );
        assert!(
            protection_of(&router, &task_arn, case).await,
            "case {case:?}: the protection the call recorded must be what a read reports"
        );
    }

    // And the same for the other members' bounds: a page size at either end, and the
    // smallest accepted task count, are answered rather than refused.
    for (case, operation, request) in [
        (
            "the smallest list page",
            "ListClusters",
            json!({ "maxResults": 1 }),
        ),
        (
            "the largest list page",
            "ListClusters",
            json!({ "maxResults": 100 }),
        ),
        (
            "the largest narrow page",
            "ListAccountSettings",
            json!({ "maxResults": 10 }),
        ),
        (
            "the smallest narrow page",
            "DescribeCapacityProviders",
            json!({ "maxResults": 1 }),
        ),
    ] {
        let body = ok(&router, operation, request, case).await;
        assert!(
            body.is_object(),
            "case {case:?}: expected {operation} to answer a body, got {body}"
        );
    }

    let placed = ok(
        &router,
        "RunTask",
        json!({ "cluster": CLUSTER, "taskDefinition": FAMILY, "count": 1 }),
        "the smallest task count",
    )
    .await;
    let tasks = list(&placed, "tasks", "the smallest task count");
    assert_eq!(
        tasks.len(),
        1,
        "case \"the smallest task count\": expected one task placed, got {placed}"
    );
    assert!(
        !string(&tasks[0], "taskArn", "the smallest task count").is_empty(),
        "case \"the smallest task count\": expected the placed task's ARN, got {placed}"
    );
}
