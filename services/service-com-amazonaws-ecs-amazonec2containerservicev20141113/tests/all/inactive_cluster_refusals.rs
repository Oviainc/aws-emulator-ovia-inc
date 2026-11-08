// SPDX-License-Identifier: CC0-1.0

//! Which sentence a `ClusterNotFoundException` carries, driven end to end over HTTP.
//!
//! The rule under test: Amazon ECS answers ONE error code for a `cluster` an operation
//! cannot act in, and the cluster's own stored state picks the sentence that code carries.
//! A record this account still holds, deleted and so left `INACTIVE` (`Cluster$status`:
//! "The cluster has been deleted. Clusters with an INACTIVE status may remain discoverable
//! in your account for a period of time"), is reported as the inactive cluster it is; a
//! reference that resolves to no record at all is reported as not found. Both sentences
//! are fixed text with no variable part, so the same request earns different wording only
//! because the cluster behind it is in a different state — which is the only signal a
//! client has for telling "you deleted this, re-create it or wait" from "you named a
//! cluster this account and Region never held".
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    create_cluster, list, ok, refusal, register_definition, service, string, ACCOUNT,
    SIGNED_REGION,
};

/// The sentence the refusal carries for a cluster record that is still there, in the
/// status a delete leaves it in.
const INACTIVE: &str = "The referenced cluster was inactive.";

/// The sentence the same error code carries for a reference that resolves to no cluster
/// record at all.
const NOT_FOUND: &str = "Cluster not found.";

/// The family every row that has to name a task definition runs. Registered once: a
/// revision belongs to the account, not to a cluster.
const FAMILY: &str = "inactiveclusterfamily";

/// One row of the operation table: the operation, and the request it builds around the
/// `cluster` reference the row is testing.
type Row = (&'static str, fn(&str) -> Value);

/// The implemented operations that resolve a `cluster` to WORK IN, spanning both the
/// mutating calls (a create, an update, a delete, a launch, a cluster-level write) and the
/// listings — the rule belongs to the cluster-resolution step, so it is the same answer
/// whichever of them asked. A describe that answers per NAME is NOT one of them: it has a
/// `failures` channel for the names it cannot resolve, so it resolves the tombstone and
/// reports through that instead (`crate::cluster_tombstone`).
const ROWS: [Row; 7] = [
    ("CreateService", |cluster| {
        json!({
            "cluster": cluster,
            "serviceName": "web",
            "taskDefinition": FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
        })
    }),
    ("UpdateService", |cluster| {
        json!({ "cluster": cluster, "service": "web", "desiredCount": 1 })
    }),
    ("DeleteService", |cluster| {
        json!({ "cluster": cluster, "service": "web" })
    }),
    ("RunTask", |cluster| {
        json!({ "cluster": cluster, "taskDefinition": FAMILY })
    }),
    ("PutClusterCapacityProviders", |cluster| {
        json!({
            "cluster": cluster,
            "capacityProviders": ["FARGATE"],
            "defaultCapacityProviderStrategy": [],
        })
    }),
    ("ListTasks", |cluster| json!({ "cluster": cluster })),
    ("ListContainerInstances", |cluster| {
        json!({ "cluster": cluster })
    }),
];

/// The `status` `DescribeClusters` reports for `reference`, or `None` when it answers a
/// `MISSING` failure instead of a cluster — the observable state the rule reads.
async fn described_status(
    router: &MultiServiceRouter,
    reference: &str,
    case: &str,
) -> Option<String> {
    let body = ok(
        router,
        "DescribeClusters",
        json!({ "clusters": [reference] }),
        case,
    )
    .await;
    let clusters = list(&body, "clusters", case);
    match clusters.first() {
        Some(cluster) => Some(string(cluster, "status", case)),
        None => {
            assert_eq!(
                list(&body, "failures", case).len(),
                1,
                "case {case:?}: expected a MISSING failure for a cluster nothing holds, \
                 got {body}"
            );
            None
        }
    }
}

/// An ARN of a cluster that was never created, so a row can name an absent cluster in
/// the OTHER documented form ("The short name or full Amazon Resource Name (ARN) of the
/// cluster").
fn cluster_arn(name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{name}")
}

#[tokio::test]
async fn the_cluster_refusal_sentence_is_selected_by_the_clusters_stored_state() {
    let router = service();
    register_definition(&router, FAMILY, "setup").await;

    // Rows differ along the cluster NAME's shape as well as the operation and the
    // identifier form, because both sentences are fixed text: nothing from the request
    // may reach either of them.
    let names = ["alpha-cluster", "b_2", "a-very-long-cluster-name-for-one-row"];
    for name in names {
        let arn = create_cluster(&router, name, "setup").await;
        ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": name }),
            "setup",
        )
        .await;
        assert_eq!(
            described_status(&router, name, name).await.as_deref(),
            Some("INACTIVE"),
            "case {name:?}: expected the deleted cluster to still describe as INACTIVE"
        );

        let absent = format!("{name}-never-created");
        assert_eq!(
            described_status(&router, &absent, &absent).await,
            None,
            "case {absent:?}: expected a cluster nothing holds to describe as missing"
        );

        for (operation, build) in ROWS {
            // Each state is named in both documented identifier forms, so the selector
            // is shown to be the resolved cluster rather than the spelling.
            let cases = [
                ("inactive by name", name.to_string(), INACTIVE),
                ("inactive by ARN", arn.clone(), INACTIVE),
                ("absent by name", absent.clone(), NOT_FOUND),
                ("absent by ARN", cluster_arn(&absent), NOT_FOUND),
            ];
            for (state, reference, expected) in cases {
                let case = format!("{operation}, {state}, cluster {name:?}");
                let (code, message) = refusal(&router, operation, build(&reference), &case).await;
                assert_eq!(
                    code, "ClusterNotFoundException",
                    "case {case:?}: expected a ClusterNotFoundException, got {code:?} with \
                     {message:?}"
                );
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
                assert!(
                    !message.contains(name),
                    "case {case:?}: the sentence is fixed text and must name no cluster, \
                     got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_request_that_names_no_cluster_is_answered_for_the_default_cluster_it_resolves() {
    let router = service();
    register_definition(&router, FAMILY, "setup").await;

    // "If you do not specify a cluster, the default cluster is assumed": the request
    // carries no `cluster` member at all, so whichever sentence comes back was selected
    // by the state of the cluster the reference RESOLVED to, not by anything sent.
    let request = json!({
        "serviceName": "web",
        "taskDefinition": FAMILY,
        "desiredCount": 0,
        "launchType": "EC2",
    });

    let case = "no default cluster has ever been created";
    let (code, message) = refusal(&router, "CreateService", request.clone(), case).await;
    assert_eq!(
        code, "ClusterNotFoundException",
        "case {case:?}: expected a ClusterNotFoundException, got {code:?} with {message:?}"
    );
    assert_eq!(
        message, NOT_FOUND,
        "case {case:?}: expected {NOT_FOUND:?}, got {message:?}"
    );

    create_cluster(&router, "default", "setup").await;
    ok(
        &router,
        "DeleteCluster",
        json!({ "cluster": "default" }),
        "setup",
    )
    .await;

    let case = "the default cluster was created and then deleted";
    let (code, message) = refusal(&router, "CreateService", request, case).await;
    assert_eq!(
        code, "ClusterNotFoundException",
        "case {case:?}: expected a ClusterNotFoundException, got {code:?} with {message:?}"
    );
    assert_eq!(
        message, INACTIVE,
        "case {case:?}: expected {INACTIVE:?}, got {message:?}"
    );
}

#[tokio::test]
async fn a_cluster_created_over_a_tombstone_is_a_cluster_an_operation_can_work_in_again() {
    let router = service();
    register_definition(&router, FAMILY, "setup").await;
    create_cluster(&router, "revived", "setup").await;
    ok(
        &router,
        "DeleteCluster",
        json!({ "cluster": "revived" }),
        "setup",
    )
    .await;

    let case = "while the tombstone stands";
    let (_, message) = refusal(
        &router,
        "CreateService",
        json!({
            "cluster": "revived",
            "serviceName": "web",
            "taskDefinition": FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    assert_eq!(
        message, INACTIVE,
        "case {case:?}: expected {INACTIVE:?}, got {message:?}"
    );

    // Creating the cluster again over the tombstone makes it a cluster to act in: the
    // sentence was read off the stored record, so a new record is a new answer.
    let case = "after the cluster is created again";
    let created = ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "revived" }),
        case,
    )
    .await;
    assert_eq!(
        string(&created["cluster"], "status", case),
        "ACTIVE",
        "case {case:?}: expected the re-created cluster to be ACTIVE, got {created}"
    );

    let service_body = ok(
        &router,
        "CreateService",
        json!({
            "cluster": "revived",
            "serviceName": "web",
            "taskDefinition": FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    assert_eq!(
        string(&service_body["service"], "serviceName", case),
        "web",
        "case {case:?}: expected the service to be created, got {service_body}"
    );

    // And the created service reads back through its own describe, so the success is
    // persisted state rather than one response.
    let read = ok(
        &router,
        "DescribeServices",
        json!({ "cluster": "revived", "services": ["web"] }),
        case,
    )
    .await;
    let services = list(&read, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected the service to be described, got {read}"
    );
    assert_eq!(
        string(&services[0], "clusterArn", case),
        string(&created["cluster"], "clusterArn", case),
        "case {case:?}: expected the service to sit in the re-created cluster, got {read}"
    );
}
