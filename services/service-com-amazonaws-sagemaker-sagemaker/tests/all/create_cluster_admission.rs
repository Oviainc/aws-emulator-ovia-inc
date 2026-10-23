// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for what SageMaker HyperPod's `CreateCluster` ADMITS: the
//! bounds of its own input, the account quota that bounds the store, and which modelled
//! error each refusal carries.
//!
//! `CreateCluster` itself (the minted `ClusterArn`, the create/describe round trip, the
//! clock-owed `Creating` -> `InService` move) is pinned by `hyperpod_cluster_lifecycle`
//! and `hyperpod_cluster_listing`; this suite pins the admission rules those two leave
//! open, so the operation cannot regress into accepting what the model forbids or
//! growing the store without end.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses. The
//! harness is the one `inference_deployment_lifecycle` declares.
//!
//! The rules, each asserted over a TABLE of inputs rather than one call:
//!
//! * a request inside every published bound of `CreateCluster`'s input is accepted and
//!   read back by `DescribeCluster`; one past any of those bounds is refused with HTTP
//!   400 and persists nothing;
//! * the store is bounded by the account quota: the create past it is refused with the
//!   operation's modelled `ResourceLimitExceeded` and mints no ARN;
//! * a `ClusterName` already in use is refused with the modelled `ResourceInUse`
//!   whatever the store's occupancy — a create that cannot add a record is a conflict,
//!   never a limit failure.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// A minimal instance group. `InstanceGroupName`, `InstanceCount`, `InstanceType` and
/// `ExecutionRole` are the group's required members (smithy_query
/// shape=ClusterInstanceGroupSpecification).
fn instance_group(name: &str, instance_count: i64) -> Value {
    json!({
        "InstanceGroupName": name,
        "InstanceCount": instance_count,
        "InstanceType": "ml.c5.xlarge",
        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-{name}"),
    })
}

/// A `CreateCluster` body naming `name`, with one instance group.
fn cluster_body(name: &str) -> Value {
    json!({
        "ClusterName": name,
        "InstanceGroups": [instance_group("workers", 1)],
    })
}

/// A `CreateCluster` body naming `name`, with `groups` instance groups whose names are
/// `g000`, `g001`, … in request order.
fn cluster_body_with_groups(name: &str, groups: usize) -> Value {
    let groups: Vec<Value> = (0..groups)
        .map(|index| instance_group(&format!("g{index:03}"), 1))
        .collect();
    json!({ "ClusterName": name, "InstanceGroups": groups })
}

/// The `InstanceGroupName`s a `DescribeCluster` response reports, in the order it
/// reported them.
fn described_group_names(described: &Value) -> Vec<String> {
    described
        .get("InstanceGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected InstanceGroups in {described}"))
        .iter()
        .map(|group| str_field(group, "InstanceGroupName"))
        .collect()
}

/// RULE: the number of clusters one account holds is bounded — every create up to the
/// quota is accepted, and the one past it is refused with the operation's modelled
/// `ResourceLimitExceeded`, minting no ARN and storing nothing. A conflicting name is
/// still a conflict at that bound: `ResourceInUse` is decided by the name, not by how
/// full the store is.
#[tokio::test]
async fn the_account_quota_bounds_the_cluster_store_and_a_conflict_stays_a_conflict() {
    // The bound the handler enforces, read from the code rather than restated, so this
    // test keeps exercising the boundary if the bound moves.
    let quota = service_com_amazonaws_sagemaker_sagemaker::state::CLUSTER_QUOTA;
    let emulator = Emulator::new();

    let mut first_arn = String::new();
    for index in 0..quota {
        let name = format!("q{index:05}");
        let created = emulator.ok("CreateCluster", &cluster_body(&name)).await;
        let arn = str_field(&created, "ClusterArn");
        if index == 0 {
            first_arn = arn;
        }
    }

    // One past the bound: refused with the modelled limit failure.
    let (status, parsed) = emulator
        .call("CreateCluster", &cluster_body("one-past-the-quota"))
        .await;
    assert_eq!(
        status, 400,
        "case one-past-the-quota: a create past the quota must be refused, got {status} \
         with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceLimitExceeded",
        "case one-past-the-quota: the refusal must use the operation's modelled limit \
         error, got {parsed:?}"
    );
    assert!(
        parsed.get("ClusterArn").is_none(),
        "case one-past-the-quota: a refused create mints no ARN, got {parsed:?}"
    );

    // Nothing was stored for it: the reader does not know the name.
    let (status, parsed) = emulator
        .call(
            "DescribeCluster",
            &json!({ "ClusterName": "one-past-the-quota" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one-past-the-quota: the refused name must be unknown to the reader, got \
         {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFound",
        "case one-past-the-quota: the reader's refusal for an unknown cluster, got {parsed:?}"
    );

    // A name already in use is refused for the conflict, with the ARN of the cluster
    // that holds the name — not for a limit the request cannot consume.
    let (status, parsed) = emulator.call("CreateCluster", &cluster_body("q00000")).await;
    assert_eq!(
        status, 400,
        "case conflict-at-the-quota: a name already in use must be refused, got {status} \
         with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceInUse",
        "case conflict-at-the-quota: a conflicting name must be refused as a conflict \
         whatever the store's occupancy, got {parsed:?}"
    );
    assert!(
        message(&parsed).contains(&first_arn),
        "case conflict-at-the-quota: the refusal must name the existing cluster \
         {first_arn:?}, got {parsed:?}"
    );

    // The cluster that holds the name is untouched by either refusal.
    let described = emulator
        .ok("DescribeCluster", &json!({ "ClusterName": "q00000" }))
        .await;
    assert_eq!(
        str_field(&described, "ClusterArn"),
        first_arn,
        "case conflict-at-the-quota: the stored cluster must keep its ARN, got {described:?}"
    );
}

/// RULE: `InstanceGroups` is a list `len 1..100` (smithy_query operation=CreateCluster) —
/// a request at either bound is accepted and read back with its groups in request order;
/// one past either bound is refused with HTTP 400 and persists nothing.
#[tokio::test]
async fn the_instance_group_list_is_accepted_at_its_bounds_and_refused_past_them() {
    let emulator = Emulator::new();

    // Accepted: the lower bound, an ordinary multi-group cluster, and the upper bound.
    for groups in [1_usize, 7, 100] {
        let name = format!("groups-{groups}");
        let created = emulator
            .ok("CreateCluster", &cluster_body_with_groups(&name, groups))
            .await;
        let arn = str_field(&created, "ClusterArn");
        assert!(
            arn.starts_with("arn:aws:sagemaker:"),
            "case {groups} groups: expected a cluster ARN, got {arn:?}"
        );

        let described = emulator
            .ok("DescribeCluster", &json!({ "ClusterName": name }))
            .await;
        let reported = described_group_names(&described);
        let expected: Vec<String> = (0..groups).map(|index| format!("g{index:03}")).collect();
        assert_eq!(
            reported, expected,
            "case {groups} groups: DescribeCluster must replay the groups in request \
             order, got {reported:?}"
        );
    }

    // Refused: one past each bound of the list, and one past `ClusterInstanceCount`'s
    // own range (range 0..6758, smithy_query shape=ClusterInstanceCount).
    let too_many = cluster_body_with_groups("groups-101", 101);
    let empty = json!({ "ClusterName": "groups-0", "InstanceGroups": [] });
    let count_past_range = json!({
        "ClusterName": "count-past-range",
        "InstanceGroups": [instance_group("workers", 6_759)],
    });
    let count_below_range = json!({
        "ClusterName": "count-below-range",
        "InstanceGroups": [instance_group("workers", -1)],
    });
    let rows: Vec<(&str, Value)> = vec![
        ("one-past-100-groups", too_many),
        ("below-1-group", empty),
        ("instance-count-above-6758", count_past_range),
        ("instance-count-below-0", count_below_range),
    ];
    for (case, body) in rows {
        let (status, parsed) = emulator.call("CreateCluster", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside the modelled bounds must be refused, got \
             {status} with {parsed}"
        );
        assert!(
            parsed.get("ClusterArn").is_none(),
            "case {case:?}: a refused create mints no ARN, got {parsed:?}"
        );
    }

    // Only the three accepted clusters above reached the store.
    let listed = emulator.ok("ListClusters", &json!({})).await;
    let names: Vec<String> = listed
        .get("ClusterSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ClusterSummaries in {listed}"))
        .iter()
        .map(|summary| str_field(summary, "ClusterName"))
        .collect();
    assert_eq!(
        names,
        vec![
            "groups-1".to_string(),
            "groups-7".to_string(),
            "groups-100".to_string()
        ],
        "a refused create must persist nothing, got {listed:?}"
    );
}

/// RULE: `ClusterName` is `len 1..63`, pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`
/// (smithy_query operation=CreateCluster) — a name of that shape is accepted and names
/// the cluster the reader answers; any other is refused with HTTP 400 and persists
/// nothing.
#[tokio::test]
async fn cluster_names_outside_the_modelled_shape_are_refused() {
    let emulator = Emulator::new();

    // Accepted: both length bounds and the shapes the pattern admits between them.
    let longest = format!("c{}", "x".repeat(62));
    let accepted = [
        ("shortest", "a".to_string()),
        ("longest", longest.clone()),
        ("digits-only", "42".to_string()),
        ("hyphen-runs", "a-b--c9".to_string()),
        ("mixed-case", "MixedCase".to_string()),
    ];
    for (case, name) in &accepted {
        let created = emulator.ok("CreateCluster", &cluster_body(name)).await;
        let arn = str_field(&created, "ClusterArn");
        let described = emulator
            .ok("DescribeCluster", &json!({ "ClusterName": name }))
            .await;
        assert_eq!(
            str_field(&described, "ClusterName"),
            *name,
            "case {case:?}: the reader must answer the name that was created, got \
             {described:?}"
        );
        assert_eq!(
            str_field(&described, "ClusterArn"),
            arn,
            "case {case:?}: the reader must answer the ARN that was minted, got {described:?}"
        );
    }

    // Refused: one past each length bound, and every shape the pattern excludes.
    let too_long = "a".repeat(64);
    let refused = [
        ("empty", String::new()),
        ("one-past-63", too_long),
        ("leading-hyphen", "-leading".to_string()),
        ("trailing-hyphen", "trailing-".to_string()),
        ("underscore", "under_score".to_string()),
        ("dot", "dotted.name".to_string()),
        ("space", "two words".to_string()),
        ("slash", "a/b".to_string()),
    ];
    for (case, name) in &refused {
        let (status, parsed) = emulator.call("CreateCluster", &cluster_body(name)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a name outside the modelled shape must be refused, got \
             {status} with {parsed}"
        );
        assert!(
            parsed.get("ClusterArn").is_none(),
            "case {case:?}: a refused create mints no ARN, got {parsed:?}"
        );
    }

    // The store holds exactly the accepted names: nothing refused reached it.
    let listed = emulator.ok("ListClusters", &json!({})).await;
    let stored: Vec<String> = listed
        .get("ClusterSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ClusterSummaries in {listed}"))
        .iter()
        .map(|summary| str_field(summary, "ClusterName"))
        .collect();
    assert_eq!(
        stored.len(),
        accepted.len(),
        "the store must hold exactly the accepted names, got {stored:?}"
    );
    for (case, name) in &accepted {
        assert!(
            stored.contains(name),
            "case {case:?}: {name:?} must be stored, got {stored:?}"
        );
    }
}
