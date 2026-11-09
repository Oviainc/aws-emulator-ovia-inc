// SPDX-License-Identifier: CC0-1.0

//! CLUSTER-SCOPED capacity providers: the create Amazon ECS has to stand capacity up for,
//! and what a cluster still carrying one makes of `DeleteCluster`.
//!
//! The rules under test:
//!
//! * a capacity provider whose capacity Amazon ECS stands up on the caller's behalf (an
//!   Amazon ECS Managed Instances provider) is answered under the NON-TERMINAL
//!   `PROVISIONING` status with its create `CREATE_IN_PROGRESS`, and the clock settles it
//!   to `ACTIVE` with the create `CREATE_COMPLETE` — a terminal pair a caller can wait
//!   for — while a provider over an Auto Scaling group the caller already has is `ACTIVE`
//!   at once and reports no update in flight;
//! * such a provider is a resource OF its cluster ("Managed instances capacity providers
//!   are cluster-scoped"), so `DeleteCluster` refuses while one is still there — HTTP 400
//!   `ClientException` — and the refused delete changes nothing: the cluster keeps its
//!   status, its `capacityProviders` association and its resources. A cluster carrying
//!   only account-scoped associations (an Auto Scaling group provider, the predefined
//!   Fargate providers) deletes and settles to `INACTIVE`, and a cluster that an ACTIVE
//!   SERVICE holds open too earns the service's own modelled refusal, which is the hold
//!   the caller has to clear first.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::capacity_providers::{group_arn, managed_instances};
use crate::ecs_wire::{
    create_auto_scaling_group, create_cluster, list, ok, refusal, register_definition, service,
    string,
};

/// The whole of AWS's sentence for a delete a cluster-scoped capacity provider holds
/// open. It has no variable part: it names neither the cluster nor the provider.
const CLUSTER_SCOPED_REFUSAL: &str =
    "The Cluster cannot be deleted while Cluster Scoped Capacity Providers are attached \
     to the cluster.";

/// One row of the delete table: what the cluster carries, and what `DeleteCluster` must
/// answer for it — `None` for a delete that lands.
struct DeleteCase {
    case: &'static str,
    cluster: &'static str,
    /// The providers `PutClusterCapacityProviders` attaches to it.
    providers: &'static [&'static str],
    /// Whether an ACTIVE service of the cluster holds it open too.
    with_service: bool,
    /// The `(code, message)` the delete is refused with.
    refusal: Option<(&'static str, &'static str)>,
}

/// Long enough for the create of a provider Amazon ECS stands capacity up for to fall
/// due (`state::TRANSITION_SPAN`), with margin for a loaded test machine.
const PROVISIONED: Duration = Duration::from_millis(
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::TRANSITION_SPAN
        .as_millis() as u64
        + 400,
);

/// Describe one capacity provider by name and return it.
async fn describe_provider(
    router: &MultiServiceRouter,
    provider_name: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeCapacityProviders",
        json!({ "capacityProviders": [provider_name] }),
        case,
    )
    .await;
    let providers = list(&body, "capacityProviders", case);
    assert_eq!(
        providers.len(),
        1,
        "case {case:?}: expected exactly one described provider, got {body}"
    );
    providers[0].clone()
}

/// Describe one cluster by name and return it.
async fn describe_cluster(router: &MultiServiceRouter, cluster: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeClusters",
        json!({ "clusters": [cluster] }),
        case,
    )
    .await;
    let clusters = list(&body, "clusters", case);
    assert_eq!(
        clusters.len(),
        1,
        "case {case:?}: expected exactly one described cluster, got {body}"
    );
    clusters[0].clone()
}

/// One row of the settling table: how the provider is created, the `(status,
/// updateStatus)` pair the create answers under, and the pair the clock settles it into.
struct SettleCase {
    case: &'static str,
    provider_name: &'static str,
    /// The members the request carries beside `name`.
    request: fn() -> Value,
    created: (&'static str, Option<&'static str>),
    settled: (&'static str, Option<&'static str>),
}

/// The `(status, updateStatus)` pair a described provider reports.
fn lifecycle(provider: &Value, case: &str) -> (String, Option<String>) {
    (
        string(provider, "status", case),
        provider["updateStatus"].as_str().map(str::to_string),
    )
}

#[tokio::test]
async fn a_provider_whose_capacity_is_stood_up_settles_to_a_terminal_status() {
    let router = service();
    create_cluster(&router, "settling", "setup").await;
    create_auto_scaling_group(&router, "asg-settle", "setup").await;

    // The rows differ along what backs the provider, which is what decides whether
    // anything has to be stood up for it before it can be used.
    let rows = [
        SettleCase {
            case: "capacity Amazon ECS stands up for the cluster",
            provider_name: "standsup",
            request: || {
                json!({
                    "cluster": "settling",
                    "managedInstancesProvider": managed_instances(),
                })
            },
            created: ("PROVISIONING", Some("CREATE_IN_PROGRESS")),
            settled: ("ACTIVE", Some("CREATE_COMPLETE")),
        },
        SettleCase {
            case: "an Auto Scaling group the caller already has",
            provider_name: "alreadythere",
            request: || {
                json!({
                    "autoScalingGroupProvider": {
                        "autoScalingGroupArn": group_arn("asg-settle"),
                    },
                })
            },
            created: ("ACTIVE", None),
            settled: ("ACTIVE", None),
        },
    ];

    for row in &rows {
        let (case, provider_name) = (row.case, row.provider_name);
        let mut request = (row.request)();
        request["name"] = json!(provider_name);
        let answer = ok(&router, "CreateCapacityProvider", request, case).await;
        let answer = answer["capacityProvider"].clone();
        let expected_created = (row.created.0.to_string(), row.created.1.map(str::to_string));
        assert_eq!(
            lifecycle(&answer, case),
            expected_created,
            "case {case:?}: unexpected (status, updateStatus) from the create, got {answer}"
        );
        // The same pair reads back through the describe, so the create reported the
        // STORED record rather than a status of its own.
        let read = describe_provider(&router, provider_name, case).await;
        assert_eq!(
            lifecycle(&read, case),
            expected_created,
            "case {case:?}: the describe must report what the create stored, got {read}"
        );

        // Nothing sleeps in the emulator: the transition is owed by the clock, so a
        // caller that waits for it sees it without having asked for it.
        tokio::time::sleep(PROVISIONED).await;
        let settled_read = describe_provider(&router, provider_name, case).await;
        assert_eq!(
            lifecycle(&settled_read, case),
            (row.settled.0.to_string(), row.settled.1.map(str::to_string)),
            "case {case:?}: unexpected settled (status, updateStatus), got {settled_read}"
        );
        // A settled provider stays where it settled: a second read answers the same.
        let again = describe_provider(&router, provider_name, case).await;
        assert_eq!(
            lifecycle(&again, case),
            lifecycle(&settled_read, case),
            "case {case:?}: two reads at the same instant must agree, got {again}"
        );
    }
}

#[tokio::test]
async fn a_cluster_carrying_a_cluster_scoped_provider_cannot_be_deleted() {
    let router = service();

    // The rows differ along what the cluster carries: a cluster-scoped provider, which is
    // a resource of the cluster itself; an account-scoped Auto Scaling group provider
    // associated with it; the predefined Fargate providers; nothing at all; and a cluster
    // that carries BOTH a cluster-scoped provider and an active service, where the
    // service's own modelled refusal is the one the caller has to act on first.
    let rows: [DeleteCase; 5] = [
        DeleteCase {
            case: "a cluster-scoped provider is attached",
            cluster: "scoped",
            providers: &["clusterscoped"],
            with_service: false,
            refusal: Some(("ClientException", CLUSTER_SCOPED_REFUSAL)),
        },
        DeleteCase {
            case: "an account-scoped Auto Scaling group provider is attached",
            cluster: "accountscoped",
            providers: &["accountwide"],
            with_service: false,
            refusal: None,
        },
        DeleteCase {
            case: "the predefined Fargate providers are attached",
            cluster: "fargateonly",
            providers: &["FARGATE", "FARGATE_SPOT"],
            with_service: false,
            refusal: None,
        },
        DeleteCase {
            case: "no provider is attached",
            cluster: "bare",
            providers: &[],
            with_service: false,
            refusal: None,
        },
        DeleteCase {
            case: "a cluster-scoped provider is attached AND a service is active",
            cluster: "scopedwithservice",
            providers: &["servicescoped"],
            with_service: true,
            refusal: Some((
                "ClusterContainsServicesException",
                "The Cluster cannot be deleted while Services are active.",
            )),
        },
    ];

    // The cluster-scoped and the account-scoped providers, created once for the rows that
    // attach them. A cluster-scoped one is created for its row's own cluster, since that
    // is the only cluster it can be used in.
    for (cluster, provider) in [
        ("scoped", "clusterscoped"),
        ("scopedwithservice", "servicescoped"),
    ] {
        create_cluster(&router, cluster, "setup").await;
        ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": provider,
                "cluster": cluster,
                "managedInstancesProvider": managed_instances(),
            }),
            "setup",
        )
        .await;
    }
    create_auto_scaling_group(&router, "asg-delete", "setup").await;
    ok(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "accountwide",
            "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-delete") },
        }),
        "setup",
    )
    .await;
    register_definition(&router, "deletehold", "setup").await;
    // The cluster-scoped providers have to be usable before they can be attached, which
    // is one clock span after their create.
    tokio::time::sleep(PROVISIONED).await;

    for row in &rows {
        let (case, cluster, providers) = (row.case, row.cluster, row.providers);
        if !matches!(cluster, "scoped" | "scopedwithservice") {
            create_cluster(&router, cluster, case).await;
        }
        if row.with_service {
            // An ACTIVE service of the cluster, scaled to nothing: what holds the cluster
            // open is the service itself, not the tasks it runs.
            ok(
                &router,
                "CreateService",
                json!({
                    "cluster": cluster,
                    "serviceName": "holder",
                    "taskDefinition": "deletehold",
                    "desiredCount": 0,
                }),
                case,
            )
            .await;
        }
        if !providers.is_empty() {
            ok(
                &router,
                "PutClusterCapacityProviders",
                json!({
                    "cluster": cluster,
                    "capacityProviders": providers,
                    "defaultCapacityProviderStrategy": [],
                }),
                case,
            )
            .await;
            // The association an attachment update is still landing puts the cluster in a
            // busy state of its own, which is a refusal about the UPDATE rather than
            // about what is attached: the delete below is asked once that update has
            // landed, so what it answers is decided by the attachment itself.
            tokio::time::sleep(PROVISIONED).await;
        }

        let Some((expected_code, expected_message)) = row.refusal else {
            let deleted = ok(
                &router,
                "DeleteCluster",
                json!({ "cluster": cluster }),
                case,
            )
            .await;
            assert_eq!(
                string(&deleted["cluster"], "status", case),
                "INACTIVE",
                "case {case:?}: a deletable cluster settles to INACTIVE, got {deleted}"
            );
            continue;
        };

        let (code, message) = refusal(
            &router,
            "DeleteCluster",
            json!({ "cluster": cluster }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (expected_code, expected_message),
            "case {case:?}: unexpected refusal, got {code:?} {message:?}"
        );
        // The refused delete had no effect: the cluster is still there, still not
        // INACTIVE, and still carries the provider it was refused for.
        let read = describe_cluster(&router, cluster, case).await;
        assert_eq!(
            string(&read, "status", case),
            "ACTIVE",
            "case {case:?}: a refused delete leaves the cluster as it was, got {read}"
        );
        assert_eq!(
            list(&read, "capacityProviders", case),
            providers.iter().map(|name| json!(name)).collect::<Vec<_>>(),
            "case {case:?}: a refused delete leaves the association as it was, got {read}"
        );
        // And the same call is refused again, so the refusal is a property of the state
        // rather than of the first call.
        let (again, _) = refusal(
            &router,
            "DeleteCluster",
            json!({ "cluster": cluster }),
            case,
        )
        .await;
        assert_eq!(
            again, expected_code,
            "case {case:?}: the refusal is answered for every call, got {again:?}"
        );
    }
}
