// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two asynchronous node group updates:
//! `UpdateNodegroupConfig` and `UpdateNodegroupVersion`, driven over the wire through
//! the same `MultiServiceRouter` the production binary mounts.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along which member the change carries, how a label or a
//! taint is added, replaced or removed, which half of the addressed pair is missing,
//! and which documented rule a bad request breaks. Each assertion names its case and
//! prints the actual value.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one
//! in `tests/all/wire.rs`, and the node group setup is `nodegroup_lifecycle`'s.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    NODEGROUP_ACTIVATION_DELAY, UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::REGION;
use crate::nodegroup_lifecycle::{create_cluster, create_nodegroup_in, minimal_request};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// A little longer than the span the clock owes a transition, so a read after it is
/// certainly past the due instant.
fn past(delay: Duration) -> Duration {
    delay + Duration::from_millis(250)
}

async fn update_config(
    router: &MultiServiceRouter,
    cluster: &str,
    nodegroup: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/node-groups/{}/update-config",
            encode_label(cluster),
            encode_label(nodegroup)
        ),
        &body.to_string(),
    )
    .await
}

async fn update_version(
    router: &MultiServiceRouter,
    cluster: &str,
    nodegroup: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/node-groups/{}/update-version",
            encode_label(cluster),
            encode_label(nodegroup)
        ),
        &body.to_string(),
    )
    .await
}

async fn describe_nodegroup(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/node-groups/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

/// Stand up one `ACTIVE` node group named `ng` on a cluster of its own, with the
/// labels and taints a change can then add to, replace or remove, and answer the
/// Kubernetes version both the cluster and the node group run.
async fn active_nodegroup(router: &MultiServiceRouter, cluster: &str) -> String {
    let cluster_version = create_cluster(router, cluster).await;
    let mut body = minimal_request("ng");
    body["labels"] = serde_json::json!({ "tier": "spot", "team": "platform" });
    body["taints"] = serde_json::json!([
        { "key": "dedicated", "value": "gpu", "effect": "NO_SCHEDULE" },
    ]);
    body["scalingConfig"] = serde_json::json!({ "minSize": 1, "maxSize": 4, "desiredSize": 2 });
    let created = create_nodegroup_in(router, REGION, cluster, &body).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateNodegroup on {cluster:?}, got {created:?}"
    );
    tokio::time::sleep(past(NODEGROUP_ACTIVATION_DELAY)).await;
    cluster_version
}

// ---------------------------------------------------------------------------
// RULE: the change is asynchronous — the node group keeps every value it had until
// the clock completes the update, and then reports exactly what was asked for,
// leaving every member the request did not name alone.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which member carries the change
/// (labels added, labels removed, taints added, taints replaced, taints removed, a
/// partial scaling triple, the update config, the node repair config, the warm pool)
/// and, for the collection members, whether the entry was already there.
#[tokio::test]
async fn a_node_group_reports_a_change_only_once_the_clock_completes_the_update() {
    struct Case {
        name: &'static str,
        request: serde_json::Value,
        /// The JSON pointer into the node group document the change shows up at, and
        /// the value it must report once the update completes.
        expected: Vec<(&'static str, serde_json::Value)>,
    }

    let cases = [
        Case {
            name: "a label added beside the ones the node group carries",
            request: serde_json::json!({
                "labels": { "addOrUpdateLabels": { "zone": "a" } },
            }),
            expected: vec![(
                "/labels",
                serde_json::json!({ "team": "platform", "tier": "spot", "zone": "a" }),
            )],
        },
        Case {
            name: "a label that REPLACES the value of a key already there",
            request: serde_json::json!({
                "labels": { "addOrUpdateLabels": { "tier": "on-demand" } },
            }),
            expected: vec![(
                "/labels",
                serde_json::json!({ "team": "platform", "tier": "on-demand" }),
            )],
        },
        Case {
            name: "a label removed, and another added in the same request",
            request: serde_json::json!({
                "labels": {
                    "addOrUpdateLabels": { "zone": "b" },
                    "removeLabels": ["team"],
                },
            }),
            expected: vec![(
                "/labels",
                serde_json::json!({ "tier": "spot", "zone": "b" }),
            )],
        },
        Case {
            name: "a taint added for a key the node group does not taint",
            request: serde_json::json!({
                "taints": {
                    "addOrUpdateTaints": [
                        { "key": "spot", "value": "true", "effect": "PREFER_NO_SCHEDULE" },
                    ],
                },
            }),
            expected: vec![(
                "/taints",
                serde_json::json!([
                    { "key": "dedicated", "value": "gpu", "effect": "NO_SCHEDULE" },
                    { "key": "spot", "value": "true", "effect": "PREFER_NO_SCHEDULE" },
                ]),
            )],
        },
        Case {
            name: "a taint that REPLACES the value of the (key, effect) pair already tainted",
            request: serde_json::json!({
                "taints": {
                    "addOrUpdateTaints": [
                        { "key": "dedicated", "value": "inference", "effect": "NO_SCHEDULE" },
                    ],
                },
            }),
            expected: vec![(
                "/taints",
                serde_json::json!([
                    { "key": "dedicated", "value": "inference", "effect": "NO_SCHEDULE" },
                ]),
            )],
        },
        Case {
            name: "the one taint removed, leaving the node group untainted",
            request: serde_json::json!({
                "taints": {
                    "removeTaints": [
                        { "key": "dedicated", "value": "gpu", "effect": "NO_SCHEDULE" },
                    ],
                },
            }),
            expected: vec![("/taints", serde_json::json!([]))],
        },
        Case {
            name: "one member of the scaling triple, leaving the other two as stored",
            request: serde_json::json!({ "scalingConfig": { "desiredSize": 3 } }),
            expected: vec![(
                "/scalingConfig",
                serde_json::json!({ "minSize": 1, "maxSize": 4, "desiredSize": 3 }),
            )],
        },
        Case {
            name: "the whole scaling triple at once",
            request: serde_json::json!({
                "scalingConfig": { "minSize": 2, "maxSize": 6, "desiredSize": 5 },
            }),
            expected: vec![(
                "/scalingConfig",
                serde_json::json!({ "minSize": 2, "maxSize": 6, "desiredSize": 5 }),
            )],
        },
        Case {
            name: "the update config, by percentage",
            request: serde_json::json!({
                "updateConfig": { "maxUnavailablePercentage": 50, "updateStrategy": "MINIMAL" },
            }),
            expected: vec![(
                "/updateConfig",
                serde_json::json!({ "maxUnavailablePercentage": 50, "updateStrategy": "MINIMAL" }),
            )],
        },
        Case {
            name: "the node repair config",
            request: serde_json::json!({
                "nodeRepairConfig": { "enabled": true, "maxUnhealthyNodeThresholdCount": 3 },
            }),
            expected: vec![(
                "/nodeRepairConfig",
                serde_json::json!({ "enabled": true, "maxUnhealthyNodeThresholdCount": 3 }),
            )],
        },
        Case {
            name: "the warm pool",
            request: serde_json::json!({
                "warmPoolConfig": {
                    "enabled": true,
                    "minSize": 1,
                    "maxGroupPreparedCapacity": 8,
                    "poolState": "STOPPED",
                    "reuseOnScaleIn": true,
                },
            }),
            expected: vec![(
                "/warmPoolConfig",
                serde_json::json!({
                    "enabled": true,
                    "minSize": 1,
                    "maxGroupPreparedCapacity": 8,
                    "poolState": "STOPPED",
                    "reuseOnScaleIn": true,
                }),
            )],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let router = emulator();
        let cluster = format!("ng-config-{index}");
        active_nodegroup(&router, &cluster).await;

        let before = describe_nodegroup(&router, &cluster, "ng").await;
        let started = update_config(&router, &cluster, "ng", &case.request).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected 200 from UpdateNodegroupConfig, got {started:?}",
            case.name
        );
        assert_eq!(
            started.body["update"]["type"],
            serde_json::json!("ConfigUpdate"),
            "case {:?}: expected update type ConfigUpdate, got {:?}",
            case.name,
            started.body["update"]["type"]
        );
        assert!(
            started.body["update"]["params"]
                .as_array()
                .is_some_and(|params| !params.is_empty()),
            "case {:?}: the update must report the parameters it carries, got {:?}",
            case.name,
            started.body["update"]["params"]
        );

        // The change is NOT visible while the update is in flight, and the node group
        // says so: it is `UPDATING`.
        let during = describe_nodegroup(&router, &cluster, "ng").await;
        assert_eq!(
            during.body["nodegroup"]["status"],
            serde_json::json!("UPDATING"),
            "case {:?}: expected status UPDATING while the update runs, got {:?}",
            case.name,
            during.body["nodegroup"]["status"]
        );
        for (pointer, _) in &case.expected {
            assert_eq!(
                during.body["nodegroup"].pointer(pointer),
                before.body["nodegroup"].pointer(pointer),
                "case {:?}: {pointer} must not change before the update completes, got {:?}",
                case.name,
                during.body["nodegroup"].pointer(pointer)
            );
        }

        tokio::time::sleep(past(UPDATE_COMPLETION_DELAY)).await;

        let after = describe_nodegroup(&router, &cluster, "ng").await;
        assert_eq!(
            after.body["nodegroup"]["status"],
            serde_json::json!("ACTIVE"),
            "case {:?}: expected status ACTIVE once the update completes, got {:?}",
            case.name,
            after.body["nodegroup"]["status"]
        );
        for (pointer, expected) in &case.expected {
            let actual = after.body["nodegroup"].pointer(pointer);
            assert_eq!(
                actual,
                Some(expected),
                "case {:?}: expected {pointer} {expected:?}, got {actual:?}",
                case.name
            );
        }
        // Every member the request did not name is the one the create wrote.
        for untouched in ["nodegroupName", "nodeRole", "amiType", "diskSize", "subnets"] {
            assert_eq!(
                after.body["nodegroup"][untouched], before.body["nodegroup"][untouched],
                "case {:?}: {untouched} must be left alone, got {:?}",
                case.name, after.body["nodegroup"][untouched]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the pair the request addresses is resolved before the body is judged, and a
// body that breaks a documented rule is refused with nothing recorded.
// ---------------------------------------------------------------------------

/// The table spans the refusals the operation can give, by what the row breaks:
/// which half of the addressed pair is missing, which documented rule of which
/// member is broken, and a request that asks for nothing at all.
#[tokio::test]
async fn an_update_is_refused_before_anything_is_recorded() {
    let router = emulator();
    active_nodegroup(&router, "ng-refusals").await;

    let cases: Vec<(&str, &str, &str, serde_json::Value, u16, &str)> = vec![
        (
            "a cluster that was never created",
            "no-such-cluster",
            "ng",
            serde_json::json!({ "scalingConfig": { "desiredSize": 2 } }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a node group the cluster does not carry",
            "ng-refusals",
            "no-such-nodegroup",
            serde_json::json!({ "scalingConfig": { "desiredSize": 2 } }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a desired size above the stored maximum",
            "ng-refusals",
            "ng",
            serde_json::json!({ "scalingConfig": { "desiredSize": 9 } }),
            400,
            "InvalidParameterException",
        ),
        (
            "a minimum above the stored maximum",
            "ng-refusals",
            "ng",
            serde_json::json!({ "scalingConfig": { "minSize": 7 } }),
            400,
            "InvalidParameterException",
        ),
        (
            "an updateConfig carrying both maxUnavailable and maxUnavailablePercentage",
            "ng-refusals",
            "ng",
            serde_json::json!({
                "updateConfig": { "maxUnavailable": 1, "maxUnavailablePercentage": 10 },
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "an updateConfig carrying neither of them",
            "ng-refusals",
            "ng",
            serde_json::json!({ "updateConfig": { "updateStrategy": "DEFAULT" } }),
            400,
            "InvalidParameterException",
        ),
        (
            "a nodeRepairConfig threshold given as both a count and a percentage",
            "ng-refusals",
            "ng",
            serde_json::json!({
                "nodeRepairConfig": {
                    "enabled": true,
                    "maxParallelNodesRepairedCount": 2,
                    "maxParallelNodesRepairedPercentage": 20,
                },
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "one label key both added and removed",
            "ng-refusals",
            "ng",
            serde_json::json!({
                "labels": {
                    "addOrUpdateLabels": { "tier": "spot" },
                    "removeLabels": ["tier"],
                },
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "one taint both added and removed",
            "ng-refusals",
            "ng",
            serde_json::json!({
                "taints": {
                    "addOrUpdateTaints": [
                        { "key": "dedicated", "value": "gpu", "effect": "NO_SCHEDULE" },
                    ],
                    "removeTaints": [
                        { "key": "dedicated", "value": "gpu", "effect": "NO_SCHEDULE" },
                    ],
                },
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "a warm pool whose maximum prepared capacity is below its minimum",
            "ng-refusals",
            "ng",
            serde_json::json!({
                "warmPoolConfig": { "enabled": true, "minSize": 4, "maxGroupPreparedCapacity": 2 },
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "a request that asks for no change at all",
            "ng-refusals",
            "ng",
            serde_json::json!({}),
            400,
            "InvalidParameterException",
        ),
    ];

    for (case, cluster, nodegroup, request, status, expected_code) in &cases {
        let answer = update_config(&router, cluster, nodegroup, request).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {answer:?}"
        );
        let actual = error_code(&answer);
        assert_eq!(
            &actual, expected_code,
            "case {case:?}: expected {expected_code}, got {actual:?}"
        );
        // Nothing was recorded: the node group is still settled, with the labels,
        // taints and scaling the create wrote.
        let described = describe_nodegroup(&router, "ng-refusals", "ng").await;
        assert_eq!(
            described.body["nodegroup"]["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: a refused update must leave the node group settled, got {described:?}"
        );
        assert_eq!(
            described.body["nodegroup"]["scalingConfig"],
            serde_json::json!({ "minSize": 1, "maxSize": 4, "desiredSize": 2 }),
            "case {case:?}: a refused update must change nothing, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: one node group takes one update at a time, and a retry of the call that
// started one is answered with that same update.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_update_conflicts_and_a_retry_is_absorbed() {
    let router = emulator();
    active_nodegroup(&router, "ng-one-at-a-time").await;

    let request = serde_json::json!({
        "clientRequestToken": "token-one",
        "scalingConfig": { "desiredSize": 3 },
    });
    let first = update_config(&router, "ng-one-at-a-time", "ng", &request).await;
    assert_eq!(
        first.status, 200,
        "case first update: expected 200, got {first:?}"
    );
    let update_id = first.body["update"]["id"].clone();

    let retried = update_config(&router, "ng-one-at-a-time", "ng", &request).await;
    assert_eq!(
        retried.status, 200,
        "case retry: expected 200, got {retried:?}"
    );
    assert_eq!(
        retried.body["update"]["id"], update_id,
        "case retry: expected the first update's id {update_id:?}, got {:?}",
        retried.body["update"]["id"]
    );

    let conflicting = update_config(
        &router,
        "ng-one-at-a-time",
        "ng",
        &serde_json::json!({
            "clientRequestToken": "token-two",
            "scalingConfig": { "desiredSize": 4 },
        }),
    )
    .await;
    assert_eq!(
        conflicting.status, 409,
        "case second update: expected HTTP 409, got {conflicting:?}"
    );
    let actual = error_code(&conflicting);
    assert_eq!(
        actual, "ResourceInUseException",
        "case second update: expected ResourceInUseException, got {actual:?}"
    );

    tokio::time::sleep(past(UPDATE_COMPLETION_DELAY)).await;
    let next = update_config(
        &router,
        "ng-one-at-a-time",
        "ng",
        &serde_json::json!({
            "clientRequestToken": "token-three",
            "scalingConfig": { "desiredSize": 4 },
        }),
    )
    .await;
    assert_eq!(
        next.status, 200,
        "case update after the first completed: expected 200, got {next:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a version update moves the node group to the Kubernetes version the request
// asks for — the cluster's own when it asks for none — and only once the clock
// completes it; a version it may not move to is refused.
// ---------------------------------------------------------------------------

/// How one row of the version-update table builds its request, given the
/// Kubernetes version the cluster and the node group both run.
type BuildVersionRequest = fn(&str) -> serde_json::Value;

/// The table spans what the rule generalises over: whether the request names a
/// version at all, whether it names an AMI version beside it, and whether the force
/// flag is set.
#[tokio::test]
async fn a_version_update_moves_the_node_group_only_once_the_clock_completes_it() {
    let cases: [(&str, BuildVersionRequest); 3] = [
        (
            "no version at all, which is the cluster's own",
            |_version| serde_json::json!({}),
        ),
        (
            "the cluster's own version, named",
            |version| serde_json::json!({ "version": version }),
        ),
        (
            "the cluster's version with an AMI version and the force flag",
            |version| {
                serde_json::json!({
                    "version": version,
                    "releaseVersion": format!("{version}.20240101"),
                    "force": true,
                })
            },
        ),
    ];

    for (index, (case, build)) in cases.iter().enumerate() {
        let router = emulator();
        let cluster = format!("ng-version-{index}");
        let cluster_version = active_nodegroup(&router, &cluster).await;
        let request = build(&cluster_version);

        let started = update_version(&router, &cluster, "ng", &request).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected 200 from UpdateNodegroupVersion, got {started:?}"
        );
        assert_eq!(
            started.body["update"]["type"],
            serde_json::json!("VersionUpdate"),
            "case {case:?}: expected update type VersionUpdate, got {:?}",
            started.body["update"]["type"]
        );

        let during = describe_nodegroup(&router, &cluster, "ng").await;
        assert_eq!(
            during.body["nodegroup"]["status"],
            serde_json::json!("UPDATING"),
            "case {case:?}: expected status UPDATING while the update runs, got {:?}",
            during.body["nodegroup"]["status"]
        );

        tokio::time::sleep(past(UPDATE_COMPLETION_DELAY)).await;

        let after = describe_nodegroup(&router, &cluster, "ng").await;
        assert_eq!(
            after.body["nodegroup"]["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: expected status ACTIVE once the update completes, got {:?}",
            after.body["nodegroup"]["status"]
        );
        assert_eq!(
            after.body["nodegroup"]["version"],
            serde_json::json!(cluster_version),
            "case {case:?}: expected version {cluster_version:?}, got {:?}",
            after.body["nodegroup"]["version"]
        );
        let expected_release = request.get("releaseVersion").cloned();
        if let Some(expected_release) = expected_release {
            assert_eq!(
                after.body["nodegroup"]["releaseVersion"], expected_release,
                "case {case:?}: expected releaseVersion {expected_release:?}, got {:?}",
                after.body["nodegroup"]["releaseVersion"]
            );
        }
    }
}

/// The table spans the refusals a version update can give, by what the row breaks:
/// which half of the addressed pair is missing, which direction the version moves,
/// and a launch template on a node group that was deployed without one.
#[tokio::test]
async fn a_version_update_is_refused_before_anything_is_recorded() {
    let router = emulator();
    let cluster_version = active_nodegroup(&router, "ng-version-refusals").await;
    // The node group runs the cluster's version, so an earlier minor version is a
    // rollback and a later one is ahead of the cluster.
    let (major, minor) = cluster_version
        .split_once('.')
        .expect("an EKS Kubernetes version is <major>.<minor>");
    let minor: u32 = minor.parse().expect("the minor version is a number");
    let earlier = format!("{major}.{}", minor - 1);
    let ahead = format!("{major}.{}", minor + 1);

    let cases: [(&str, &str, &str, serde_json::Value, u16, &str); 6] = [
        (
            "a cluster that was never created",
            "no-such-cluster",
            "ng",
            serde_json::json!({}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a node group the cluster does not carry",
            "ng-version-refusals",
            "no-such-nodegroup",
            serde_json::json!({}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an earlier Kubernetes version, which is a rollback",
            "ng-version-refusals",
            "ng",
            serde_json::json!({ "version": earlier.clone() }),
            400,
            "InvalidParameterException",
        ),
        (
            "a version ahead of the cluster's own",
            "ng-version-refusals",
            "ng",
            serde_json::json!({ "version": ahead.clone() }),
            400,
            "InvalidParameterException",
        ),
        (
            "a value that is not a Kubernetes version at all",
            "ng-version-refusals",
            "ng",
            serde_json::json!({ "version": "latest" }),
            400,
            "InvalidParameterException",
        ),
        (
            "a launch template on a node group deployed without one",
            "ng-version-refusals",
            "ng",
            serde_json::json!({ "launchTemplate": { "name": "lt", "version": "2" } }),
            400,
            "InvalidParameterException",
        ),
    ];

    for (case, cluster, nodegroup, request, status, expected_code) in &cases {
        let answer = update_version(&router, cluster, nodegroup, request).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {answer:?}"
        );
        let actual = error_code(&answer);
        assert_eq!(
            &actual, expected_code,
            "case {case:?}: expected {expected_code}, got {actual:?}"
        );
        let described = describe_nodegroup(&router, "ng-version-refusals", "ng").await;
        assert_eq!(
            described.body["nodegroup"]["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: a refused update must leave the node group settled, got {described:?}"
        );
        assert_eq!(
            described.body["nodegroup"]["version"],
            serde_json::json!(cluster_version),
            "case {case:?}: a refused update must change nothing, got {described:?}"
        );
    }
}
