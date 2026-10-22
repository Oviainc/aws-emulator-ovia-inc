// SPDX-License-Identifier: CC0-1.0

//! `UpdateCluster`, end to end over HTTP.
//!
//! The rule under test: `UpdateCluster` writes the network type the request names onto
//! the stored cluster the ARN identifies — together with the Regional data-plane
//! endpoints that network type decides — leaves every other member of the record alone,
//! and answers with the record a later read answers with; an ARN no cluster is stored
//! under is refused with the modelled not-found error.
//!
//! The tables span every ordered pair of the `NetworkType` values the model declares
//! (`smithy_query(query="shape",shape="NetworkType")` -> DUALSTACK | IPV4), including
//! the rows where the requested network is the one the cluster is already on and the
//! rows whose cluster was created with the member omitted, and every resource level of
//! the hierarchy for the not-found rule.

use serde_json::{json, Value};

// The HTTP harness every suite in this target shares, and the facts about this
// service's resources the lifecycle suite already states.
use super::common::{call, create_cluster, enc, error_code, router};
use super::recovery_control_lifecycle::{
    ACCOUNT, ARN_PREFIX, ENDPOINT_REGIONS, PAST_DEPLOYMENT, STATUS_DEPLOYED, STATUS_PENDING,
};

/// The `NetworkType` values the model declares, via
/// `smithy_query(query="shape",shape="NetworkType")`: `DUALSTACK | IPV4`.
const NETWORK_TYPES: &[&str] = &["DUALSTACK", "IPV4"];

/// Call `UpdateCluster` for `cluster_arn` and `network_type`.
async fn update_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    cluster_arn: &str,
    network_type: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        "/cluster",
        Some(json!({ "ClusterArn": cluster_arn, "NetworkType": network_type })),
    )
    .await
}

/// A cluster's endpoints as `Region|URL` strings with the cluster's own id replaced by
/// `{id}`, so two clusters' endpoint shapes are comparable: this is the part of an
/// endpoint the network type decides.
fn endpoint_templates(cluster: &Value) -> Vec<String> {
    let arn = cluster["ClusterArn"].as_str().unwrap_or_default();
    let id = arn.rsplit_once('/').map_or("", |(_, id)| id);
    cluster["ClusterEndpoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|endpoint| {
            let region = endpoint["Region"].as_str().unwrap_or_default();
            let url = endpoint["Endpoint"].as_str().unwrap_or_default();
            format!("{region}|{}", url.replace(id, "{id}"))
        })
        .collect()
}

/// `DescribeCluster` for `cluster_arn`, as the stored `Cluster` body.
async fn describe_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    cluster_arn: &str,
) -> Value {
    let (status, body) = call(router, "GET", &format!("/cluster/{}", enc(cluster_arn)), None).await;
    assert_eq!(
        status, 200,
        "case describe {cluster_arn}: expected DescribeCluster 200, got {status} {body:?}"
    );
    body["Cluster"].clone()
}

/// An update writes the requested network type and the endpoints it decides onto the
/// stored cluster, leaves the rest of the record alone, and a later read answers the
/// same body.
#[tokio::test]
async fn update_writes_the_network_type_and_describe_answers_it() {
    let router = router();

    // The endpoint shape a cluster on each network type publishes, taken from clusters
    // created directly on it: the update must land the SAME endpoints, so no caller can
    // tell an updated cluster from one created that way.
    let mut reference = std::collections::BTreeMap::new();
    for network_type in NETWORK_TYPES {
        let created = create_cluster(
            &router,
            json!({ "ClusterName": format!("reference-{network_type}"), "NetworkType": network_type }),
        )
        .await;
        reference.insert((*network_type).to_string(), endpoint_templates(&created));
    }

    // Rows: every ordered pair of the declared values (including the pair that requests
    // the network the cluster is already on), plus the cluster created with the member
    // omitted, which the model gives no default.
    let mut cases: Vec<(String, Option<&str>, &str)> = Vec::new();
    for from in NETWORK_TYPES {
        for to in NETWORK_TYPES {
            cases.push((format!("{from}-to-{to}"), Some(*from), *to));
        }
        cases.push((format!("omitted-to-{from}"), None, *from));
    }

    for (case, created_with, target) in cases {
        let mut body = json!({ "ClusterName": format!("cluster-{case}") });
        if let Some(created_with) = created_with {
            body["NetworkType"] = json!(created_with);
        }
        let created = create_cluster(&router, body).await;
        let arn = created["ClusterArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, answer) = update_cluster(&router, &arn, target).await;
        assert_eq!(
            status, 200,
            "case {case}: expected UpdateCluster 200, got {status} {answer:?}"
        );
        let updated = answer["Cluster"].clone();

        assert_eq!(
            updated["NetworkType"], json!(target),
            "case {case}: expected the requested NetworkType {target}, got {:?}",
            updated["NetworkType"]
        );
        for member in ["ClusterArn", "Name", "Owner"] {
            assert_eq!(
                updated[member], created[member],
                "case {case}: UpdateCluster must not change {member}, got {:?} after {:?}",
                updated[member], created[member]
            );
        }
        assert_eq!(
            updated["Owner"], json!(ACCOUNT),
            "case {case}: expected owner {ACCOUNT}, got {:?}",
            updated["Owner"]
        );

        let regions: Vec<&str> = updated["ClusterEndpoints"]
            .as_array()
            .map(|endpoints| {
                endpoints
                    .iter()
                    .map(|e| e["Region"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            regions, ENDPOINT_REGIONS,
            "case {case}: expected one endpoint per supported Region, got {regions:?}"
        );
        let templates = endpoint_templates(&updated);
        assert_eq!(
            templates.as_slice(),
            reference[target].as_slice(),
            "case {case}: an updated cluster must publish the endpoints its network type \
             decides, got {templates:?} instead of {:?}",
            reference[target]
        );

        // The response came from the stored record, so the read answers the same body.
        let read = describe_cluster(&router, &arn).await;
        for member in ["ClusterArn", "Name", "Owner", "NetworkType", "ClusterEndpoints"] {
            assert_eq!(
                read[member], updated[member],
                "case {case}: DescribeCluster must answer the written {member}, got {:?} \
                 after {:?}",
                read[member], updated[member]
            );
        }
    }
}

/// An update that changes the network type re-deploys the cluster: it reads `PENDING`
/// straight after the call and reaches `DEPLOYED` on its own. An update that requests
/// the network the cluster is already on changes nothing, so it does not put a deployed
/// cluster back into `PENDING`.
#[tokio::test]
async fn a_changing_update_redeploys_the_cluster_and_a_no_op_does_not() {
    let router = router();

    for from in NETWORK_TYPES {
        for to in NETWORK_TYPES {
            let case = format!("{from}-to-{to}");
            let created = create_cluster(
                &router,
                json!({ "ClusterName": format!("redeploy-{case}"), "NetworkType": from }),
            )
            .await;
            let arn = created["ClusterArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            // Wait the cluster's first deployment out, so the status this update leaves
            // is the only thing the next read can be reporting.
            std::thread::sleep(PAST_DEPLOYMENT);
            let before = describe_cluster(&router, &arn).await;
            assert_eq!(
                before["Status"], json!(STATUS_DEPLOYED),
                "case {case}: the created cluster must be {STATUS_DEPLOYED} before the update, \
                 got {:?}",
                before["Status"]
            );

            let (status, answer) = update_cluster(&router, &arn, to).await;
            assert_eq!(
                status, 200,
                "case {case}: expected UpdateCluster 200, got {status} {answer:?}"
            );
            let expected_status = if from == to {
                STATUS_DEPLOYED
            } else {
                STATUS_PENDING
            };
            assert_eq!(
                answer["Cluster"]["Status"], json!(expected_status),
                "case {case}: expected {expected_status} right after the update, got {:?}",
                answer["Cluster"]["Status"]
            );

            // Nothing writes the transition: waiting the span out is what makes it due.
            std::thread::sleep(PAST_DEPLOYMENT);
            let after = describe_cluster(&router, &arn).await;
            assert_eq!(
                after["Status"], json!(STATUS_DEPLOYED),
                "case {case}: expected {STATUS_DEPLOYED} once the deployment is due, got {:?}",
                after["Status"]
            );
            assert_eq!(
                after["NetworkType"], json!(to),
                "case {case}: the written NetworkType must survive the deployment, got {:?}",
                after["NetworkType"]
            );
        }
    }
}

/// An update names exactly one cluster: every other stored cluster is untouched.
#[tokio::test]
async fn an_update_touches_only_the_cluster_it_names() {
    let router = router();

    let mut clusters = Vec::new();
    for (n, network_type) in ["IPV4", "DUALSTACK", "IPV4"].iter().enumerate() {
        let created = create_cluster(
            &router,
            json!({ "ClusterName": format!("neighbour-{n}"), "NetworkType": network_type }),
        )
        .await;
        clusters.push(created);
    }

    let target = clusters[1]["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, answer) = update_cluster(&router, &target, "IPV4").await;
    assert_eq!(
        status, 200,
        "case update the middle cluster: expected 200, got {status} {answer:?}"
    );

    for (n, cluster) in clusters.iter().enumerate() {
        let arn = cluster["ClusterArn"].as_str().unwrap_or_default();
        let read = describe_cluster(&router, arn).await;
        let expected = if n == 1 {
            json!("IPV4")
        } else {
            cluster["NetworkType"].clone()
        };
        assert_eq!(
            read["NetworkType"], expected,
            "case cluster {n} after the update: expected NetworkType {expected:?}, got {:?}",
            read["NetworkType"]
        );
        if n != 1 {
            assert_eq!(
                endpoint_templates(&read),
                endpoint_templates(cluster),
                "case cluster {n} after the update: its endpoints must be unchanged"
            );
        }
    }
}

/// An ARN no cluster is stored under is `ResourceNotFoundException` (404), whatever
/// resource the ARN names.
#[tokio::test]
async fn updating_an_unknown_cluster_is_not_found() {
    let router = router();

    // A real cluster exists, so a refusal is about the ARN the request names and not
    // about an empty store.
    let existing = create_cluster(&router, json!({ "ClusterName": "present" })).await;
    let existing_arn = existing["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let missing_panel = format!("{ARN_PREFIX}controlpanel/00000000-0000-0000-0000-000000000000");
    let cases: Vec<(&str, String)> = vec![
        (
            "a cluster ARN nothing is stored under",
            format!("{ARN_PREFIX}cluster/00000000-0000-0000-0000-000000000000"),
        ),
        ("a control-panel ARN", missing_panel.clone()),
        (
            "a routing-control ARN",
            format!("{missing_panel}/routingcontrol/11111111-1111-1111-1111-111111111111"),
        ),
        (
            "a deleted cluster's ARN",
            {
                let doomed = create_cluster(&router, json!({ "ClusterName": "doomed" })).await;
                let arn = doomed["ClusterArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let (status, body) =
                    call(&router, "DELETE", &format!("/cluster/{}", enc(&arn)), None).await;
                assert_eq!(
                    status, 200,
                    "case prepare a deleted cluster: expected DeleteCluster 200, got {status} \
                     {body:?}"
                );
                arn
            },
        ),
    ];

    for (case, arn) in cases {
        let (status, answer) = update_cluster(&router, &arn, "DUALSTACK").await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    // The cluster that does exist is still on the network type it was created with.
    let read = describe_cluster(&router, &existing_arn).await;
    assert_eq!(
        read["NetworkType"], existing["NetworkType"],
        "case the existing cluster: a refused update must change nothing, got {:?} after {:?}",
        read["NetworkType"], existing["NetworkType"]
    );
}

/// A `NetworkType` the model does not declare is refused with `ValidationException`
/// (400), and the cluster keeps the network type it had.
#[tokio::test]
async fn a_network_type_outside_the_modelled_values_is_refused() {
    let router = router();

    let created = create_cluster(
        &router,
        json!({ "ClusterName": "refusing", "NetworkType": "IPV4" }),
    )
    .await;
    let arn = created["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows: a plausible value the model omits, the declared values in the wrong case,
    // and the empty string.
    for case in ["IPV6", "ipv4", "DualStack", ""] {
        let (status, answer) = update_cluster(&router, &arn, case).await;
        assert_eq!(
            status, 400,
            "case NetworkType {case:?}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case NetworkType {case:?}: expected ValidationException, got {answer:?}"
        );
        let read = describe_cluster(&router, &arn).await;
        assert_eq!(
            read["NetworkType"], json!("IPV4"),
            "case NetworkType {case:?}: a refused update must not write, got {:?}",
            read["NetworkType"]
        );
    }
}
