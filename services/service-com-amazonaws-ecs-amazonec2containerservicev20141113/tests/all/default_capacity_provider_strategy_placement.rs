// SPDX-License-Identifier: CC0-1.0

//! Which capacity provider PLACED a task, when the run named none.
//!
//! The rule under test (`RunTask$capacityProviderStrategy`: "If no
//! `capacityProviderStrategy` or `launchType` is specified, the
//! `defaultCapacityProviderStrategy` for the cluster is used"): a placement that names
//! neither takes the cluster's own default strategy, and the task it creates reports the
//! provider that strategy named in `capacityProviderName` and no `launchType` at all; a
//! placement that names a strategy of its own uses THAT one instead, and one that names a
//! launch type carries the launch type and no provider. The cluster's default strategy is
//! itself stored as set, entry for entry, so `DescribeClusters` echoes the `base` and
//! `weight` each entry was given.
//!
//! The table spans what the run names (nothing, a strategy, a launch type) and what the
//! cluster has to fall back on (a default strategy, or none at all), over the predefined
//! Fargate providers every account holds.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, create_cluster, list, ok, register_capacity, service, string,
};

/// A Fargate-compatible revision: Fargate validates the task definition against the
/// launch type, so it has to be registered for it — `awsvpc`, with one of the published
/// task sizes.
async fn register_fargate_definition(router: &MultiServiceRouter, family: &str, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "requiresCompatibilities": ["FARGATE"],
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": "web",
                "image": "public.ecr.aws/nginx/nginx:latest",
                "cpu": 0,
                "memory": 128,
            }],
        }),
        case,
    )
    .await;
}

/// What one row's `RunTask` names for its placement.
#[derive(Debug, Clone, Copy)]
enum Names {
    /// Neither a launch type nor a strategy, so the cluster's default decides.
    Nothing,
    /// A strategy of its own, naming this provider.
    Strategy(&'static str),
    /// A launch type.
    LaunchType(&'static str),
}

/// One row: what the run names, and the `(capacityProviderName, launchType)` pair the
/// task it creates must report.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    names: Names,
    expect_provider: Option<&'static str>,
    expect_launch_type: Option<&'static str>,
}

#[tokio::test]
async fn a_run_that_names_no_placement_is_placed_by_the_clusters_default_strategy() {
    let router = service();
    let cluster = "defaulted";
    create_cluster(&router, cluster, "setup").await;
    register_fargate_definition(&router, "defaulted", "setup").await;

    // The cluster's default strategy, with a `base` and a `weight` the echo below is
    // checked against.
    let strategy = json!([{ "capacityProvider": "FARGATE_SPOT", "weight": 1, "base": 0 }]);
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": cluster,
            "capacityProviders": ["FARGATE", "FARGATE_SPOT"],
            "defaultCapacityProviderStrategy": strategy.clone(),
        }),
        "setup",
    )
    .await;

    // "DescribeClusters echoes defaultCapacityProviderStrategy exactly as set": the
    // strategy a placement falls back on is the stored one, so it is read back first.
    let case = "the cluster echoes the default strategy it was given";
    let body = ok(
        &router,
        "DescribeClusters",
        json!({ "clusters": [cluster] }),
        case,
    )
    .await;
    let described = list(&body, "clusters", case);
    assert_eq!(
        described[0]["defaultCapacityProviderStrategy"],
        strategy,
        "case {case:?}: the stored default strategy must read back as set, got {body}"
    );

    let rows = [
        Row {
            case: "the run names neither, so the cluster's default places it",
            names: Names::Nothing,
            expect_provider: Some("FARGATE_SPOT"),
            expect_launch_type: None,
        },
        Row {
            case: "the run names a strategy of its own, which overrides the default",
            names: Names::Strategy("FARGATE"),
            expect_provider: Some("FARGATE"),
            expect_launch_type: None,
        },
        Row {
            case: "the run names a launch type, which is not a provider at all",
            names: Names::LaunchType("FARGATE"),
            expect_provider: None,
            expect_launch_type: Some("FARGATE"),
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut request = json!({
            "cluster": cluster,
            "taskDefinition": "defaulted",
            "networkConfiguration": awsvpc_network_configuration(),
        });
        match row.names {
            Names::Nothing => {}
            Names::Strategy(provider) => {
                request["capacityProviderStrategy"] =
                    json!([{ "capacityProvider": provider, "weight": 1 }]);
            }
            Names::LaunchType(launch_type) => request["launchType"] = json!(launch_type),
        }

        let body = ok(&router, "RunTask", request, case).await;
        let tasks = list(&body, "tasks", case);
        assert_eq!(
            tasks.len(),
            1,
            "case {case:?}: expected one launched task, got {body}"
        );
        // The attribution is STORED with the task, so the describe reports it too: a
        // client that comes back later still learns which provider placed the task.
        let arn = string(&tasks[0], "taskArn", case);
        let read = ok(
            &router,
            "DescribeTasks",
            json!({ "cluster": cluster, "tasks": [arn] }),
            case,
        )
        .await;
        let read = list(&read, "tasks", case);
        for reported in [&tasks[0], &read[0]] {
            assert_eq!(
                reported["capacityProviderName"].as_str(),
                row.expect_provider,
                "case {case:?}: unexpected capacityProviderName in {reported}"
            );
            assert_eq!(
                reported["launchType"].as_str(),
                row.expect_launch_type,
                "case {case:?}: unexpected launchType in {reported}"
            );
        }
    }
}

#[tokio::test]
async fn a_cluster_with_no_default_strategy_places_the_run_on_its_own_capacity() {
    // The other half of the same sentence: with no strategy anywhere, there is no
    // provider to attribute the placement to, and the task runs on the capacity
    // registered to the cluster under the default launch type.
    let router = service();
    let cluster = "undefaulted";
    let case = "no strategy anywhere";
    create_cluster(&router, cluster, case).await;
    register_capacity(&router, cluster, case).await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "undefaulted",
            "containerDefinitions": [{
                "name": "web",
                "image": "public.ecr.aws/nginx/nginx:latest",
                "cpu": 0,
                "memory": 128,
            }],
        }),
        case,
    )
    .await;

    let body = ok(
        &router,
        "RunTask",
        json!({ "cluster": cluster, "taskDefinition": "undefaulted" }),
        case,
    )
    .await;
    let task: &Value = &list(&body, "tasks", case)[0];
    assert_eq!(
        (
            task["capacityProviderName"].as_str(),
            task["launchType"].as_str()
        ),
        (None, Some("EC2")),
        "case {case:?}: expected the run to carry the default launch type and no \
         provider, got {task}"
    );
}
