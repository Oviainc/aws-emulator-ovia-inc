// SPDX-License-Identifier: CC0-1.0

//! A refusal this service can only ground in ANOTHER service's answer is never
//! assumed from that service's silence.
//!
//! RULE: the IAM roles and the EC2 subnets a request names are resources of IAM and
//! EC2, so this service resolves them against those services and refuses what they
//! report as missing (`crate::state`'s `unresolved_subnet_refusal` and
//! `service_principal_trust_violation`, driven over the in-process callback). An
//! emulator with no IAM and no EC2 mounted beside this service knows NOTHING about
//! the role or the subnet a request names, and nothing is what it may refuse on: the
//! request is served on its own merits and the resource is stored.
//!
//! This is the half of those rules that the cross-service suite
//! (`core/router/tests/all/eks_cluster_inputs_resolve.rs`, which mounts EKS beside EC2
//! and IAM) cannot reach, and it is load-bearing in two directions: it is what makes
//! every other suite in this crate — a router with one service on it — see a cluster
//! at all, and it is what keeps a client that never created the subnet or the role in
//! this emulator from being refused a resource AWS would have given it. A resolution
//! that refused on silence instead would pass the cross-service suite and break both.
//!
//! The rows span every member whose resolution needs a peer: `CreateCluster`'s
//! `resourcesVpcConfig.subnetIds` and `CreateNodegroup`'s `subnets` (EC2's to resolve),
//! `RegisterCluster`'s `connectorConfig.roleArn` and `CreateCapability`'s `roleArn`
//! (IAM's, for the SSM and the Amazon EKS service principal respectively) — each named
//! with values no account in this process holds, in the forms EC2 and IAM publish for
//! them. The `roleArn`/`nodeRole` every row carries beside them is a role of this
//! account that no IAM holds either, so each row also shows that the pass-role
//! authorization (which needs no peer, and has its own suite in
//! `pass_role_authorization`) is not what answers.
//!
//! The wire harness (`crate::wire`) and the per-operation helpers the other suites own
//! are reused as they are.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::state::service_state;

use crate::cluster_lifecycle::{create, describe, REGION};
use crate::wire::{call, emulator, emulator_with_engine, encode_label};

/// A role ARN of the account this emulator serves — passable, so the pass-role
/// authorization (which needs no peer, and is pinned by `pass_role_authorization`)
/// is never what answers here. No IAM in this process holds any of these roles.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// Subnet ids in the two forms EC2 publishes (`^subnet-[0-9a-f]{8,}$`, 8 and 17 hex
/// digits). No EC2 in this process holds a subnet for any of them.
fn unheld_subnets() -> Vec<Vec<String>> {
    vec![
        vec!["subnet-0badf00d".to_string(), "subnet-0badbeef".to_string()],
        vec![
            "subnet-00000000deadbeef1".to_string(),
            "subnet-00000000deadbeef2".to_string(),
        ],
        vec![
            "subnet-0badf00d".to_string(),
            "subnet-00000000deadbeef1".to_string(),
            "subnet-0abcdef01234567ff".to_string(),
        ],
    ]
}

/// A `CreateCluster` body naming `subnets`, and nothing else beyond the required
/// members.
fn cluster_body(name: &str, subnets: &[String]) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": role_arn("eksClusterRole"),
        "resourcesVpcConfig": { "subnetIds": subnets },
    })
}

// ---------------------------------------------------------------------------
// RULE (the EC2 half): with no EC2 to ask, a `subnetIds` list nothing resolved
// is served, and the cluster or node group it names is stored.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subnets_no_ec2_was_asked_about_are_not_refused() {
    let router = emulator();

    for (index, subnets) in unheld_subnets().into_iter().enumerate() {
        let name = format!("ungrounded-subnets-{index}");
        let answer = create(&router, &cluster_body(&name, &subnets)).await;
        assert_eq!(
            answer.status, 200,
            "case {subnets:?}: with no EC2 to ask, a subnet refusal has no grounds, got \
             {answer:?} ({})",
            answer.message()
        );
        let described = describe(&router, &name).await;
        assert_eq!(
            described.status, 200,
            "case {subnets:?}: the cluster must be stored, got {described:?}"
        );
        assert_eq!(
            described.body["cluster"]["resourcesVpcConfig"]["subnetIds"],
            serde_json::json!(subnets),
            "case {subnets:?}: the stored cluster must carry the subnets it was given, got {}",
            described.body["cluster"]["resourcesVpcConfig"]
        );
    }
}

/// `CreateNodegroup` resolves its own `subnets` the same way, so the same silence
/// leaves a node group created too — and its `nodeRole`, which a mounted IAM would
/// have been asked about, is not refused either.
#[tokio::test]
async fn a_nodegroup_in_subnets_no_ec2_was_asked_about_is_not_refused() {
    let router = emulator();
    let cluster = "ungrounded-nodegroup-host";
    let created = create(&router, &cluster_body(cluster, &unheld_subnets()[0])).await;
    assert_eq!(
        created.status, 200,
        "setup: the cluster a node group hangs off must be created, got {created:?}"
    );

    for (index, subnets) in unheld_subnets().into_iter().enumerate() {
        let nodegroup = format!("ungrounded-nodes-{index}");
        let body = serde_json::json!({
            "nodegroupName": nodegroup,
            "nodeRole": role_arn("eksNodeRole"),
            "subnets": subnets,
        });
        let answer = call(
            &router,
            REGION,
            "POST",
            &format!("/clusters/{}/node-groups", encode_label(cluster)),
            &body.to_string(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {subnets:?}: with no EC2 to ask, a node group's subnets are not refused, \
             got {answer:?} ({})",
            answer.message()
        );
        let described = call(
            &router,
            REGION,
            "GET",
            &format!(
                "/clusters/{}/node-groups/{}",
                encode_label(cluster),
                encode_label(&nodegroup)
            ),
            "",
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {subnets:?}: the node group must be stored, got {described:?}"
        );
        assert_eq!(
            described.body["nodegroup"]["subnets"],
            serde_json::json!(subnets),
            "case {subnets:?}: the stored node group must carry the subnets it was given, \
             got {}",
            described.body["nodegroup"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE (the IAM half): with no IAM to ask, a role whose trust policy nothing
// could read is not refused, and the resource it was named for is stored.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_connector_role_no_iam_was_asked_about_is_not_refused() {
    let router = emulator();

    // The rows differ along the path the role sits under and the name it carries —
    // including the role the recorded refusal names, which IAM holds in the recorded
    // account but nothing holds here.
    let roles = [
        role_arn("AmazonEKSConnectorRole"),
        role_arn("service-role/never-created-connector"),
        role_arn("sim2real-infra-lambda-exec-role"),
    ];
    for (index, role) in roles.iter().enumerate() {
        let name = format!("ungrounded-connector-{index}");
        let body = serde_json::json!({
            "name": name,
            "connectorConfig": { "roleArn": role, "provider": "OTHER" },
        });
        let answer = call(
            &router,
            REGION,
            "POST",
            "/cluster-registrations",
            &body.to_string(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {role:?}: with no IAM to ask, a connector-role refusal has no grounds, got \
             {answer:?} ({})",
            answer.message()
        );
        let described = describe(&router, &name).await;
        assert_eq!(
            described.status, 200,
            "case {role:?}: the registration must be stored, got {described:?}"
        );
        assert_eq!(
            described.body["cluster"]["connectorConfig"]["roleArn"],
            serde_json::json!(role),
            "case {role:?}: the stored registration must carry the role it was given, got {}",
            described.body["cluster"]["connectorConfig"]
        );
    }
}

/// `CreateCapability` asks IAM about its own `roleArn` (the Amazon EKS service
/// principal, not the connector's), so the same silence leaves a capability created.
#[tokio::test]
async fn a_capability_role_no_iam_was_asked_about_is_not_refused() {
    let (router, engine) = emulator_with_engine();
    let cluster = "ungrounded-capability-host";
    let created = create(&router, &cluster_body(cluster, &unheld_subnets()[0])).await;
    assert_eq!(
        created.status, 200,
        "setup: the cluster a capability hangs off must be created, got {created:?}"
    );
    // A capability is refused on a cluster that is not ACTIVE, and a new cluster is
    // CREATING until the clock owes it ACTIVE: the stored due instant is moved into the
    // past so the next read materialises the transition, which is what a caller a
    // moment later sees.
    {
        let store = service_state(&*engine).expect("the eks store");
        let mut guard = store.lock();
        let record = guard
            .clusters
            .get_mut(&(REGION.to_string(), cluster.to_string()))
            .expect("the cluster just created");
        record.active_at = Some(SystemTime::now() - Duration::from_secs(1));
    }
    let described = describe(&router, cluster).await;
    assert_eq!(
        described.body["cluster"]["status"],
        serde_json::json!("ACTIVE"),
        "setup: the cluster must be ACTIVE before a capability is created, got {described:?}"
    );

    // "You can only create one Capability of each type on a given Amazon EKS cluster",
    // so each row names its own type (the two members of `CapabilityType`).
    for (index, (role, capability_type)) in [
        (role_arn("eksCapabilityRole"), "ARGOCD"),
        (role_arn("service-role/never-created-capability"), "ACK"),
    ]
    .iter()
    .enumerate()
    {
        let capability = format!("ungrounded-capability-{index}");
        let body = serde_json::json!({
            "capabilityName": capability,
            "type": capability_type,
            "roleArn": role,
            "deletePropagationPolicy": "RETAIN",
        });
        let answer = call(
            &router,
            REGION,
            "POST",
            &format!("/clusters/{}/capabilities", encode_label(cluster)),
            &body.to_string(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {role:?}: with no IAM to ask, a capability-role refusal has no grounds, got \
             {answer:?} ({})",
            answer.message()
        );
        let described = call(
            &router,
            REGION,
            "GET",
            &format!(
                "/clusters/{}/capabilities/{}",
                encode_label(cluster),
                encode_label(&capability)
            ),
            "",
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {role:?}: the capability must be stored, got {described:?}"
        );
        assert_eq!(
            described.body["capability"]["roleArn"],
            serde_json::json!(role),
            "case {role:?}: the stored capability must carry the role it was given, got {}",
            described.body["capability"]
        );
    }
}
