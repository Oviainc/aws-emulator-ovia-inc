// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what an add-on INSTALL settles in: the terminal
//! `AddonStatus` and the `AddonHealth$issues` `DescribeAddon` reports once the
//! clock has moved a new add-on out of `CREATING`, driven over the wire through
//! the same `MultiServiceRouter` the production binary mounts.
//!
//! The rule every suite here asserts is that the state an install settles in is
//! computed from the cluster it ran against — the add-ons whose Kubernetes
//! resources it is applied against, and whether the cluster has compute to
//! schedule its pods onto — rather than being the same state for every install.
//!
//! Each suite drives a TABLE whose rows differ along what the rule generalises
//! over: the add-on's workload (one pod per node against a replicated one),
//! whether the cluster has compute and in which FORM (a managed node group, a
//! Fargate profile selecting the namespace, a Fargate profile selecting another
//! one, none at all), whether the add-ons it is installed against are present and
//! running, and whether the catalog publishes the add-on at all. Each assertion
//! names its case and prints the actual value.
//!
//! The request harness (`Answer`, `emulator`, `call`, `encode_label`) is the one
//! every suite in this crate shares, from `tests/all/wire.rs`; the add-on install
//! and read-back are the helpers `tests/all/addon_lifecycle.rs` already exposes.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ADDON_ACTIVATION_DELAY, ADDON_DELETION_DELAY, ADDON_ISSUE_INSUFFICIENT_REPLICAS_MESSAGE,
    CLUSTER_ACTIVATION_DELAY, NODEGROUP_ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::addon_lifecycle::{create_addon, describe_addon};
use crate::nodegroup_lifecycle::{create_nodegroup_in, minimal_request as nodegroup_request};
use crate::wire::{call, emulator, encode_label, Answer};

/// The region every request in this suite is signed for — the one the shared
/// add-on helpers use.
const REGION: &str = "us-west-2";

/// The add-on every add-on below is installed against: the one whose
/// certificates and admission webhook the operator add-ons of the catalog need.
const PREREQUISITE: &str = "cert-manager";

/// The add-ons the catalog publishes as installed against [`PREREQUISITE`].
/// Source: `AddonCatalogSeed::required_addons`, the prerequisites the Amazon EKS
/// User Guide lists for each of them.
const ADDONS_INSTALLED_AGAINST_THE_PREREQUISITE: &[&str] = &[
    "adot",
    "amazon-sagemaker-hyperpod-taskgovernance",
    "amazon-sagemaker-hyperpod-training-operator",
];

fn cluster_request(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    })
}

/// Start one cluster. Nothing waits here, so a suite can start every cluster it
/// needs and then wait out ONE activation span for all of them.
async fn start_cluster(router: &MultiServiceRouter, name: &str) {
    let answer = call(
        router,
        REGION,
        "POST",
        "/clusters",
        &cluster_request(name).to_string(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup CreateCluster {name:?}: got {answer:?}"
    );
}

async fn wait_for_cluster_active() {
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// Wait out the span the clock owes a new add-on, after which it reports the
/// state its install settled in.
async fn wait_for_addon_settled() {
    tokio::time::sleep(ADDON_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// Wait out the span the clock owes a new node group, after which its nodes are
/// running and a pod can be scheduled onto them.
async fn wait_for_nodes_running() {
    tokio::time::sleep(NODEGROUP_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// Give one cluster a managed node group at the default scaling size: compute a
/// pod of any namespace can be scheduled onto.
async fn add_nodes(router: &MultiServiceRouter, cluster: &str) {
    let answer = create_nodegroup_in(router, REGION, cluster, &nodegroup_request("nodes")).await;
    assert_eq!(
        answer.status, 200,
        "setup CreateNodegroup on {cluster:?}: got {answer:?}"
    );
}

/// Give one cluster a Fargate profile that selects `namespace`: compute only the
/// pods of that namespace can be scheduled onto.
async fn add_fargate_profile(router: &MultiServiceRouter, cluster: &str, namespace: &str) {
    let body = serde_json::json!({
        "fargateProfileName": format!("fp-{namespace}"),
        "podExecutionRoleArn": format!(
            "arn:aws:iam::{}:role/eksFargatePodExecutionRole",
            engine::emulator_account_id()
        ),
        "selectors": [{ "namespace": namespace }],
    });
    let answer = call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/fargate-profiles", encode_label(cluster)),
        &body.to_string(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup CreateFargateProfile on {cluster:?} for {namespace:?}: got {answer:?}"
    );
}

/// The `AddonIssueCode`s an add-on reports, in the order it reports them.
fn health_codes(addon: &serde_json::Value) -> Vec<String> {
    addon["health"]["issues"]
        .as_array()
        .map(|issues| {
            issues
                .iter()
                .filter_map(|issue| issue["code"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `(status, issue codes)` of one installed add-on, read through
/// `DescribeAddon`.
async fn settled(router: &MultiServiceRouter, cluster: &str, addon: &str) -> (String, Vec<String>) {
    let described = describe_addon(router, cluster, addon).await;
    assert_eq!(
        described.status, 200,
        "DescribeAddon {addon:?} on {cluster:?}: got {described:?}"
    );
    (
        described.addon()["status"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        health_codes(described.addon()),
    )
}

async fn install(router: &MultiServiceRouter, cluster: &str, body: &serde_json::Value) -> Answer {
    let answer = create_addon(router, cluster, body).await;
    assert_eq!(
        answer.status, 200,
        "CreateAddon {body} on {cluster:?}: got {answer:?}"
    );
    answer
}

async fn list_addons(router: &MultiServiceRouter, cluster: &str) -> Vec<String> {
    let answer = call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}/addons", encode_label(cluster)),
        "",
    )
    .await;
    assert_eq!(answer.status, 200, "ListAddons {cluster:?}: got {answer:?}");
    answer.body["addons"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// RULE: an install settles in the state the cluster it ran against could give
// it — running when its pods have somewhere to run, running degraded with
// `InsufficientNumberOfReplicas` when a replica it needs has nowhere to be
// scheduled, and failed with `K8sResourceNotFound` when the add-ons its
// resources are applied against are not on the cluster.
// ---------------------------------------------------------------------------

/// Which compute the cluster of a row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compute {
    /// No node group and no Fargate profile: nothing to schedule a pod onto.
    None,
    /// One managed node group, which runs a pod of any namespace.
    Nodes,
    /// One Fargate profile selecting this namespace, which runs only the pods of
    /// that namespace.
    FargateFor(&'static str),
}

impl Compute {
    /// The cluster name a row with this compute installs onto, so rows sharing
    /// compute share one cluster and each add-on is installed once on it.
    fn cluster(self) -> String {
        match self {
            Compute::None => "outcome-no-compute".to_string(),
            Compute::Nodes => "outcome-nodes".to_string(),
            Compute::FargateFor(namespace) => format!("outcome-fargate-{namespace}"),
        }
    }
}

struct Row {
    case: &'static str,
    compute: Compute,
    addon: &'static str,
    /// The namespace the install asks for, when it asks for one: the namespace
    /// the add-on's pods must be placed in is the one it is installed into.
    namespace: Option<&'static str>,
    expected_status: &'static str,
    expected_codes: Vec<&'static str>,
}

#[tokio::test]
async fn an_install_settles_in_the_state_the_cluster_could_give_it() {
    let router = emulator();

    // The rows differ along the add-on's workload, the form the cluster's
    // compute takes (or its absence), and the namespace the add-on is installed
    // into. Every row names an add-on the catalog publishes, because an install is
    // the only way a record is written and a name the catalog does not publish for
    // the cluster's Kubernetes version is refused before any install runs.
    let rows = vec![
        Row {
            case: "a per-node add-on has no replica to place, so no nodes is no problem",
            compute: Compute::None,
            addon: "vpc-cni",
            namespace: None,
            expected_status: "ACTIVE",
            expected_codes: vec![],
        },
        Row {
            case: "another per-node add-on, on the same cluster with no compute",
            compute: Compute::None,
            addon: "kube-proxy",
            namespace: None,
            expected_status: "ACTIVE",
            expected_codes: vec![],
        },
        Row {
            case: "a replicated add-on with nowhere to schedule its replica",
            compute: Compute::None,
            addon: "coredns",
            namespace: None,
            expected_status: "DEGRADED",
            expected_codes: vec!["InsufficientNumberOfReplicas"],
        },
        Row {
            // A row the issue did not cite, to show the rule is not keyed to one
            // add-on: `metrics-server` is published by the same catalog as a
            // replicated workload.
            case: "another replicated add-on with nowhere to schedule its replica",
            compute: Compute::None,
            addon: "metrics-server",
            namespace: None,
            expected_status: "DEGRADED",
            expected_codes: vec!["InsufficientNumberOfReplicas"],
        },
        Row {
            case: "a replicated add-on on a cluster with a node group",
            compute: Compute::Nodes,
            addon: "coredns",
            namespace: None,
            expected_status: "ACTIVE",
            expected_codes: vec![],
        },
        Row {
            case: "a per-node add-on on a cluster with a node group",
            compute: Compute::Nodes,
            addon: "vpc-cni",
            namespace: None,
            expected_status: "ACTIVE",
            expected_codes: vec![],
        },
        Row {
            case: "a replicated add-on whose namespace a Fargate profile selects",
            compute: Compute::FargateFor("kube-system"),
            addon: "coredns",
            namespace: None,
            expected_status: "ACTIVE",
            expected_codes: vec![],
        },
        Row {
            case: "a replicated add-on installed into the namespace a Fargate profile selects",
            compute: Compute::FargateFor("tenant-apps"),
            addon: "metrics-server",
            namespace: Some("tenant-apps"),
            expected_status: "ACTIVE",
            expected_codes: vec![],
        },
        Row {
            case: "a replicated add-on whose namespace no Fargate profile selects",
            compute: Compute::FargateFor("tenant-apps"),
            addon: "coredns",
            namespace: None,
            expected_status: "DEGRADED",
            expected_codes: vec!["InsufficientNumberOfReplicas"],
        },
        Row {
            // A row the issue did not cite, to show the rule is not keyed to one
            // add-on: `eks-node-monitoring-agent` is published by the same catalog
            // as a per-node workload, so it has no replica to place either.
            case: "a third per-node add-on, on the same cluster with no compute",
            compute: Compute::None,
            addon: "eks-node-monitoring-agent",
            namespace: None,
            expected_status: "ACTIVE",
            expected_codes: vec![],
        },
        Row {
            case: "an add-on whose prerequisite add-on is not on the cluster",
            compute: Compute::Nodes,
            addon: "amazon-sagemaker-hyperpod-training-operator",
            namespace: None,
            expected_status: "CREATE_FAILED",
            expected_codes: vec!["K8sResourceNotFound"],
        },
        Row {
            case: "another add-on installed against the same absent prerequisite",
            compute: Compute::Nodes,
            addon: "adot",
            namespace: None,
            expected_status: "CREATE_FAILED",
            expected_codes: vec!["K8sResourceNotFound"],
        },
        Row {
            case: "a third add-on installed against the same absent prerequisite",
            compute: Compute::Nodes,
            addon: "amazon-sagemaker-hyperpod-taskgovernance",
            namespace: None,
            expected_status: "CREATE_FAILED",
            expected_codes: vec!["K8sResourceNotFound"],
        },
    ];

    // Every cluster the rows need is started first and waited into ACTIVE
    // together, so the suite waits one activation span rather than one per row.
    let mut clusters: Vec<Compute> = Vec::new();
    for row in &rows {
        if !clusters.contains(&row.compute) {
            clusters.push(row.compute);
            start_cluster(&router, &row.compute.cluster()).await;
        }
    }
    wait_for_cluster_active().await;
    for compute in &clusters {
        match compute {
            Compute::None => {}
            Compute::Nodes => add_nodes(&router, &compute.cluster()).await,
            Compute::FargateFor(namespace) => {
                add_fargate_profile(&router, &compute.cluster(), namespace).await;
            }
        }
    }

    for row in &rows {
        let mut body = serde_json::json!({ "addonName": row.addon });
        if let Some(namespace) = row.namespace {
            body["namespaceConfig"] = serde_json::json!({ "namespace": namespace });
        }
        let created = install(&router, &row.compute.cluster(), &body).await;
        // The install is ASYNCHRONOUS whatever it settles in: a caller acting
        // straight after the create still sees the transient state, and nothing
        // is reported wrong with the add-on yet.
        assert_eq!(
            created.addon()["status"],
            serde_json::json!("CREATING"),
            "case {:?}: CreateAddon reports CREATING, got {:?}",
            row.case,
            created.addon()["status"]
        );
        assert!(
            health_codes(created.addon()).is_empty(),
            "case {:?}: a create reports no issue yet, got {:?}",
            row.case,
            created.addon()["health"]
        );
    }

    wait_for_addon_settled().await;

    for row in &rows {
        let (status, codes) = settled(&router, &row.compute.cluster(), row.addon).await;
        assert_eq!(
            (status.as_str(), codes.as_slice()),
            (
                row.expected_status,
                row.expected_codes
                    .iter()
                    .map(|code| (*code).to_string())
                    .collect::<Vec<String>>()
                    .as_slice()
            ),
            "case {:?}: expected ({:?}, {:?}), got ({status:?}, {codes:?})",
            row.case,
            row.expected_status,
            row.expected_codes
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: an add-on installed against another add-on's resources settles by what
// that add-on is doing — running when it is running, and failed with
// `AdmissionRequestDenied` when the webhook it is admitted through has no
// running replica to answer the admission review.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_addon_installed_against_another_settles_by_what_that_one_is_doing() {
    let router = emulator();
    // Two clusters whose ONLY difference is whether they have compute, so the
    // prerequisite installed on each settles differently: running on the one
    // with a node group, degraded with no replica on the other.
    let healthy = "prerequisite-running";
    let replica_less = "prerequisite-degraded";
    start_cluster(&router, healthy).await;
    start_cluster(&router, replica_less).await;
    wait_for_cluster_active().await;
    add_nodes(&router, healthy).await;

    for cluster in [healthy, replica_less] {
        install(
            &router,
            cluster,
            &serde_json::json!({ "addonName": PREREQUISITE }),
        )
        .await;
    }
    wait_for_addon_settled().await;

    let (healthy_status, healthy_codes) = settled(&router, healthy, PREREQUISITE).await;
    assert_eq!(
        (healthy_status.as_str(), healthy_codes.as_slice()),
        ("ACTIVE", &[] as &[String]),
        "the prerequisite on a cluster with compute runs, got \
         ({healthy_status:?}, {healthy_codes:?})"
    );
    let (degraded_status, degraded_codes) = settled(&router, replica_less, PREREQUISITE).await;
    assert_eq!(
        (degraded_status.as_str(), degraded_codes.as_slice()),
        (
            "DEGRADED",
            ["InsufficientNumberOfReplicas".to_string()].as_slice()
        ),
        "the prerequisite on a cluster with no compute is degraded, got \
         ({degraded_status:?}, {degraded_codes:?})"
    );

    // The rows are every add-on the catalog publishes as installed against that
    // prerequisite: each is installed on BOTH clusters, so the only thing that
    // differs between its two outcomes is what the prerequisite is doing.
    for addon in ADDONS_INSTALLED_AGAINST_THE_PREREQUISITE {
        for cluster in [healthy, replica_less] {
            install(&router, cluster, &serde_json::json!({ "addonName": addon })).await;
        }
    }
    wait_for_addon_settled().await;

    for addon in ADDONS_INSTALLED_AGAINST_THE_PREREQUISITE {
        let (status, codes) = settled(&router, healthy, addon).await;
        assert_eq!(
            (status.as_str(), codes.as_slice()),
            ("ACTIVE", &[] as &[String]),
            "case {addon:?}: with the prerequisite running the install runs, got \
             ({status:?}, {codes:?})"
        );
        let (status, codes) = settled(&router, replica_less, addon).await;
        assert_eq!(
            (status.as_str(), codes.as_slice()),
            (
                "CREATE_FAILED",
                ["AdmissionRequestDenied".to_string()].as_slice()
            ),
            "case {addon:?}: with the prerequisite's webhook unable to answer, the install \
             is denied at admission, got ({status:?}, {codes:?})"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the state an install settled in is what the record CARRIES — every read
// answers it identically, the issue says what went wrong and names the resource
// it is about, a failed add-on is still an add-on of its cluster, and deleting
// it takes it off.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_state_an_install_settled_in_is_stored_and_read_back() {
    let router = emulator();
    let cluster = "settled-outcome-readback";
    start_cluster(&router, cluster).await;
    wait_for_cluster_active().await;

    // One add-on per outcome the install can settle in, on one cluster: the
    // per-node add-on runs, the replicated one is degraded for lack of a
    // replica, and the one installed against an absent add-on fails.
    let failed = "amazon-sagemaker-hyperpod-training-operator";
    for addon in ["vpc-cni", "coredns", failed] {
        install(&router, cluster, &serde_json::json!({ "addonName": addon })).await;
    }
    wait_for_addon_settled().await;

    // The sentence each settled state's issue carries: the replica shortage is
    // one fixed sentence Amazon EKS words for every add-on alike, while the
    // missing prerequisite names the add-on and the cluster it is about.
    for (addon, expected, fixed_message) in [
        ("vpc-cni", "ACTIVE", None),
        (
            "coredns",
            "DEGRADED",
            Some(ADDON_ISSUE_INSUFFICIENT_REPLICAS_MESSAGE),
        ),
        (failed, "CREATE_FAILED", None),
    ] {
        let first = describe_addon(&router, cluster, addon).await;
        let again = describe_addon(&router, cluster, addon).await;
        assert_eq!(
            first.addon()["status"],
            serde_json::json!(expected),
            "case {addon:?}: expected {expected:?}, got {:?}",
            first.addon()["status"]
        );
        assert_eq!(
            again.addon(),
            first.addon(),
            "case {addon:?}: a second read must answer identically"
        );
        // An add-on that settled in a state with an issue reports an issue that
        // says what is wrong and names the resource it is about; one that runs
        // reports none at all.
        let issues = first.addon()["health"]["issues"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            issues.len(),
            usize::from(expected != "ACTIVE"),
            "case {addon:?}: expected an issue only where something is wrong, got {:?}",
            first.addon()["health"]
        );
        for issue in &issues {
            match fixed_message {
                // The replica shortage is one fixed sentence about the add-on's own
                // pods, which are no resource this emulator holds, so it names
                // none.
                Some(fixed) => {
                    assert_eq!(
                        issue["message"].as_str(),
                        Some(fixed),
                        "case {addon:?}: expected the fixed sentence {fixed:?}, got {:?}",
                        issue["message"]
                    );
                    assert_eq!(
                        issue["resourceIds"],
                        serde_json::Value::Null,
                        "case {addon:?}: this issue names no resource, got {:?}",
                        issue["resourceIds"]
                    );
                }
                // An issue about the resources of OTHER add-ons names them.
                None => {
                    assert!(
                        issue["message"].as_str().is_some_and(|message| {
                            message.contains(addon) && message.contains(cluster)
                        }),
                        "case {addon:?}: the issue must say what is wrong of this add-on on \
                         this cluster, got {:?}",
                        issue["message"]
                    );
                    assert!(
                        issue["resourceIds"]
                            .as_array()
                            .is_some_and(|ids| !ids.is_empty()),
                        "case {addon:?}: the issue must name the resources it is about, got \
                         {:?}",
                        issue["resourceIds"]
                    );
                }
            }
        }
    }

    // A failed install is still an add-on of its cluster until it is removed.
    let listed = list_addons(&router, cluster).await;
    assert!(
        listed.contains(&failed.to_string()),
        "a failed add-on is still installed on its cluster, got {listed:?}"
    );
    let deleted = call(
        &router,
        REGION,
        "DELETE",
        &format!(
            "/clusters/{}/addons/{}",
            encode_label(cluster),
            encode_label(failed)
        ),
        "",
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "a failed add-on can be deleted, got {deleted:?}"
    );
    // The removal is asynchronous, so the add-on is off the cluster once the clock
    // has finished it.
    tokio::time::sleep(ADDON_DELETION_DELAY + Duration::from_millis(250)).await;
    let listed = list_addons(&router, cluster).await;
    assert!(
        !listed.contains(&failed.to_string()),
        "the deleted add-on must be gone from ListAddons, got {listed:?}"
    );
    let gone = describe_addon(&router, cluster, failed).await;
    assert_eq!(
        gone.status, 404,
        "a deleted add-on reads back as a 404, got {gone:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: whether an add-on's replicas are running is derived from the cluster's
// compute, so a replica-less add-on recovers once the cluster has somewhere to
// schedule its pod — while an install that never came up stays failed, because
// such an add-on is removed and installed again rather than recovering.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replica_less_addon_recovers_once_the_cluster_can_schedule_it() {
    let router = emulator();
    let cluster = "recovering-addons";
    start_cluster(&router, cluster).await;
    wait_for_cluster_active().await;

    // The rows differ along what the add-on needs from the cluster: a replica to
    // schedule, nothing to schedule, and an add-on whose prerequisite is absent.
    let failed = "amazon-sagemaker-hyperpod-training-operator";
    let rows: Vec<(&str, &str, &str)> = vec![
        ("a replicated add-on", "coredns", "DEGRADED"),
        ("a per-node add-on", "vpc-cni", "ACTIVE"),
        ("an add-on whose prerequisite is absent", failed, "CREATE_FAILED"),
    ];
    for (_, addon, _) in &rows {
        install(&router, cluster, &serde_json::json!({ "addonName": addon })).await;
    }
    wait_for_addon_settled().await;
    for (case, addon, before) in &rows {
        let (status, _) = settled(&router, cluster, addon).await;
        assert_eq!(
            status.as_str(),
            *before,
            "case {case:?}: with no compute the install settles {before:?}, got {status:?}"
        );
    }

    // The cluster gains compute. Nothing is installed or updated, so the only
    // thing that can change is what the cluster can run.
    add_nodes(&router, cluster).await;
    wait_for_nodes_running().await;

    for (case, addon, before) in &rows {
        let (status, codes) = settled(&router, cluster, addon).await;
        // An add-on short of replicas recovers; one that never came up does not,
        // and one that was already running is untouched.
        let expected = if *before == "CREATE_FAILED" {
            "CREATE_FAILED"
        } else {
            "ACTIVE"
        };
        assert_eq!(
            status.as_str(),
            expected,
            "case {case:?}: once the cluster has nodes, expected {expected:?}, got {status:?}"
        );
        assert_eq!(
            codes.is_empty(),
            expected == "ACTIVE",
            "case {case:?}: a running add-on reports no issue and a failed one does, got \
             {codes:?}"
        );
    }
}
