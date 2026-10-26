// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the per-cluster concurrency gate every operation that
//! starts a cluster update is refused by, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The RULE under test: a cluster runs at most ONE update at a time, so an
//! operation that would start another while one is in flight is refused on the
//! CLUSTER — before anything about whatever the call named inside it is judged —
//! with 409 `ResourceInUseException` whose message is "Cannot <UpdateType> because
//! cluster <clusterName> currently has update <updateId> in progress": the update
//! type the refused call would have minted, the cluster name the request carried
//! and the id of the update already running, which is the id a caller polls
//! `DescribeUpdate` on before retrying. The refusal starts nothing, and the same
//! call is accepted once that update is done — minting an update of the very type
//! the refusal named.
//!
//! The rows differ along what the rule generalises over: the operation called (each
//! of the ones that start a cluster update), the update type it attempts — a body
//! spanning two configuration categories reports the generic configuration update,
//! and a version move reports a different type in each direction — the body that
//! cannot be read as a change at all, and which update is the one in flight (the
//! append a rotation's own next call follows, or an unrelated configuration
//! update). The `UpdateType` tokens the rows name are the model's, from
//! `smithy_query query="shape" shape="UpdateType"`; the Kubernetes versions are
//! read from the calendar `DescribeClusterVersions` publishes rather than written
//! out here.
//!
//! The wire harness is this test target's one copy in `tests/all/wire.rs` and the
//! cluster/certificate-authority helpers are `certificate_authority`'s; only the
//! request builder for the operations under test is local to this suite.

use std::time::SystemTime;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ComAmazonawsEksAwswesleyfrontendState, CLUSTER_ACTIVATION_DELAY, UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::{
    append_authority, create_cluster, describe_cluster, describe_update, settle, str_member, REGION,
};
use crate::wire::{call, emulator_with_engine, encode_label, error_code, Answer};

/// Which published neighbour of the cluster's current Kubernetes version a version
/// row moves to: the next one up is an upgrade, the one below the rollback of one.
#[derive(Debug, Clone, Copy)]
enum Neighbour {
    Newer,
    Older,
}

/// The operation a row calls, with whatever it carries in its body.
#[derive(Debug, Clone)]
enum Operation {
    /// `ActivateCertificateAuthority` on the successor the cluster's own pending
    /// append put there: the rotation sequence, where the append's own update is
    /// what the cluster is busy with.
    ActivateSuccessor,
    /// `CreateCertificateAuthority`.
    AppendAuthority,
    /// `UpdateClusterConfig` with this body.
    UpdateConfig(serde_json::Value),
    /// `UpdateClusterVersion` to a published neighbour of the cluster's version.
    UpdateVersion(Neighbour),
    /// `AssociateEncryptionConfig`.
    AssociateEncryption,
    /// `AssociateIdentityProviderConfig`.
    AssociateIdentityProvider,
}

/// One row: the call that is refused while the cluster is busy, the `UpdateType`
/// that call attempts, and whether the same call is accepted once the cluster is
/// idle again.
struct Row {
    case: &'static str,
    cluster: &'static str,
    operation: Operation,
    update_type: &'static str,
    accepted_when_idle: bool,
}

/// The row's call on the wire, against the cluster it names.
///
/// `successor` is the certificate authority an activation addresses and `version`
/// the Kubernetes version a version update asks for — each resolved from the
/// cluster the row stood up, never written out.
async fn perform(
    router: &MultiServiceRouter,
    cluster: &str,
    operation: &Operation,
    successor: Option<&str>,
    version: Option<&str>,
) -> Answer {
    match operation {
        Operation::ActivateSuccessor => {
            let authority_id = successor.expect("an activation row resolves its successor");
            crate::certificate_authority_activation::activate_authority(
                router,
                REGION,
                cluster,
                authority_id,
                None,
            )
            .await
        }
        Operation::AppendAuthority => append_authority(router, REGION, cluster, None).await,
        Operation::UpdateConfig(body) => {
            call(
                router,
                REGION,
                "POST",
                &format!("/clusters/{}/update-config", encode_label(cluster)),
                &body.to_string(),
            )
            .await
        }
        Operation::UpdateVersion(_) => {
            let version = version.expect("a version row resolves its target version");
            call(
                router,
                REGION,
                "POST",
                &format!("/clusters/{}/updates", encode_label(cluster)),
                &serde_json::json!({ "version": version }).to_string(),
            )
            .await
        }
        Operation::AssociateEncryption => {
            let body = serde_json::json!({
                "encryptionConfig": [{
                    "resources": ["secrets"],
                    "provider": {
                        "keyArn": format!("arn:aws:kms:{REGION}:000000000000:key/abcd-1234"),
                    },
                }],
            });
            call(
                router,
                REGION,
                "POST",
                &format!(
                    "/clusters/{}/encryption-config/associate",
                    encode_label(cluster)
                ),
                &body.to_string(),
            )
            .await
        }
        Operation::AssociateIdentityProvider => {
            let body = serde_json::json!({
                "oidc": {
                    "identityProviderConfigName": "concurrency",
                    "issuerUrl": "https://oidc.example.com",
                    "clientId": "kubernetes",
                },
            });
            call(
                router,
                REGION,
                "POST",
                &format!(
                    "/clusters/{}/identity-provider-configs/associate",
                    encode_label(cluster)
                ),
                &body.to_string(),
            )
            .await
        }
    }
}

/// The published Kubernetes version one minor step from `current` in the direction
/// `neighbour` names.
///
/// Read from the calendar the handlers read — the rows `DescribeClusterVersions`
/// reports as released — so the table spans whatever that calendar carries rather
/// than a snapshot of it. The calendar is published newest version first, so the
/// newer neighbour is the row before `current` and the older one the row after it.
fn published_neighbour(
    engine: &engine::InMemoryEngine,
    case: &str,
    current: &str,
    neighbour: Neighbour,
) -> String {
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let guard = store.lock();
    let published: Vec<String> = guard
        .released_cluster_versions(SystemTime::now())
        .map(|row| row.cluster_version.clone())
        .collect();
    let index = published
        .iter()
        .position(|version| version == current)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {current} is not a published version, got {published:?}")
        });
    let wanted = match neighbour {
        Neighbour::Newer => index.checked_sub(1),
        Neighbour::Older => Some(index + 1),
    };
    wanted
        .and_then(|at| published.get(at))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the calendar offers no {neighbour:?} neighbour of {current}, got \
                 {published:?}"
            )
        })
}

/// `UpdateClusterConfig`, which is what puts an UNRELATED update in flight for
/// every row whose own call is not the second step of a rotation.
async fn start_an_unrelated_update(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/update-config", encode_label(cluster)),
        &serde_json::json!({ "deletionProtection": true }).to_string(),
    )
    .await
}

// ---------------------------------------------------------------------------
// RULE: an operation that starts a cluster update, called while that cluster
// already runs one, is refused on the cluster's concurrency — 409
// `ResourceInUseException`, worded "Cannot <UpdateType> because cluster
// <clusterName> currently has update <updateId> in progress" — writes nothing, and
// is accepted once that update is done.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_operation_that_starts_an_update_is_refused_on_the_clusters_in_flight_update() {
    let (router, engine) = emulator_with_engine();

    let rows = vec![
        Row {
            case: "activating the successor the cluster's own pending append added",
            cluster: "conc-activate",
            operation: Operation::ActivateSuccessor,
            update_type: "CertificateAuthorityUpdate",
            accepted_when_idle: true,
        },
        Row {
            case: "appending a certificate authority",
            cluster: "conc-append",
            operation: Operation::AppendAuthority,
            update_type: "CertificateAuthorityUpdate",
            accepted_when_idle: true,
        },
        Row {
            case: "a configuration change in one category",
            cluster: "conc-logging",
            operation: Operation::UpdateConfig(serde_json::json!({
                "logging": { "clusterLogging": [{ "types": ["api"], "enabled": true }] },
            })),
            update_type: "LoggingUpdate",
            accepted_when_idle: true,
        },
        Row {
            case: "a configuration change spanning two categories",
            cluster: "conc-spanning",
            operation: Operation::UpdateConfig(serde_json::json!({
                "logging": { "clusterLogging": [{ "types": ["audit"], "enabled": true }] },
                "upgradePolicy": { "supportType": "STANDARD" },
            })),
            update_type: "ConfigUpdate",
            accepted_when_idle: true,
        },
        Row {
            case: "a body that asks for no configuration change at all",
            cluster: "conc-empty",
            operation: Operation::UpdateConfig(serde_json::json!({})),
            update_type: "ConfigUpdate",
            accepted_when_idle: false,
        },
        Row {
            case: "a version move one minor version up",
            cluster: "conc-upgrade",
            operation: Operation::UpdateVersion(Neighbour::Newer),
            update_type: "VersionUpdate",
            accepted_when_idle: true,
        },
        Row {
            case: "a version move one minor version down",
            cluster: "conc-rollback",
            operation: Operation::UpdateVersion(Neighbour::Older),
            update_type: "VersionRollback",
            accepted_when_idle: true,
        },
        Row {
            case: "associating an encryption configuration",
            cluster: "conc-encryption",
            operation: Operation::AssociateEncryption,
            update_type: "AssociateEncryptionConfig",
            accepted_when_idle: true,
        },
        Row {
            case: "associating an identity provider configuration",
            cluster: "conc-idp",
            operation: Operation::AssociateIdentityProvider,
            update_type: "AssociateIdentityProviderConfig",
            accepted_when_idle: true,
        },
    ];

    for row in &rows {
        let (case, cluster) = (row.case, row.cluster);

        // A cluster that is ACTIVE and running nothing.
        let created = create_cluster(&router, REGION, cluster).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
        settle(CLUSTER_ACTIVATION_DELAY).await;

        // The version a version row moves from is the cluster's own, read back
        // through `DescribeCluster`.
        let version = match row.operation {
            Operation::UpdateVersion(neighbour) => {
                let described = describe_cluster(&router, REGION, cluster).await;
                let current = str_member(case, "cluster.version", &described.body["cluster"]["version"]);
                Some(published_neighbour(&engine, case, &current, neighbour))
            }
            _ => None,
        };

        // ONE update in flight, and the id of it: the append the rotation row's own
        // call follows, or an unrelated configuration update.
        let (blocking_id, successor) = match row.operation {
            Operation::ActivateSuccessor => {
                let appended = append_authority(&router, REGION, cluster, None).await;
                assert_eq!(
                    appended.status, 200,
                    "case {case:?}: expected 200 from CreateCertificateAuthority, got {appended:?}"
                );
                (
                    str_member(case, "update.id", &appended.body["update"]["id"]),
                    Some(str_member(
                        case,
                        "certificateAuthority.id",
                        &appended.body["certificateAuthority"]["id"],
                    )),
                )
            }
            _ => {
                let started = start_an_unrelated_update(&router, cluster).await;
                assert_eq!(
                    started.status, 200,
                    "case {case:?}: expected 200 from UpdateClusterConfig, got {started:?}"
                );
                (
                    str_member(case, "update.id", &started.body["update"]["id"]),
                    None,
                )
            }
        };

        // The call under test, while that update runs.
        let refused = perform(
            &router,
            cluster,
            &row.operation,
            successor.as_deref(),
            version.as_deref(),
        )
        .await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (409, "ResourceInUseException"),
            "case {case:?}: expected 409 ResourceInUseException, got {refused:?}"
        );
        let expected = format!(
            "Cannot {} because cluster {cluster} currently has update {blocking_id} in progress",
            row.update_type
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {:?}",
            refused.body["message"]
        );
        assert_eq!(
            refused.body["clusterName"],
            serde_json::json!(cluster),
            "case {case:?}: the refusal names the cluster that is busy, got {:?}",
            refused.body["clusterName"]
        );

        // The refusal started nothing: the cluster is still under the ONE update it
        // was under, and that update is still the one the message named.
        let described = describe_cluster(&router, REGION, cluster).await;
        assert_eq!(
            described.body["cluster"]["status"],
            serde_json::json!("UPDATING"),
            "case {case:?}: the cluster is still under its one update, got {:?}",
            described.body["cluster"]["status"]
        );
        let blocking = describe_update(&router, REGION, cluster, &blocking_id).await;
        assert_eq!(
            blocking.body["update"]["status"],
            serde_json::json!("InProgress"),
            "case {case:?}: the named update is the one in flight, got {:?}",
            blocking.body["update"]["status"]
        );

        settle(UPDATE_COMPLETION_DELAY).await;

        // And the same call, with the cluster idle, is judged on its own merits: the
        // update it mints carries the very type the refusal named, and a body that
        // asks for no change is the argument-level refusal it always was.
        let again = perform(
            &router,
            cluster,
            &row.operation,
            successor.as_deref(),
            version.as_deref(),
        )
        .await;
        if row.accepted_when_idle {
            assert_eq!(
                again.status, 200,
                "case {case:?}: the idle cluster takes the call, got {again:?}"
            );
            assert_eq!(
                again.body["update"]["type"],
                serde_json::json!(row.update_type),
                "case {case:?}: expected update.type {:?}, got {:?}",
                row.update_type,
                again.body["update"]["type"]
            );
        } else {
            assert_eq!(
                (again.status, error_code(&again).as_str()),
                (400, "InvalidParameterException"),
                "case {case:?}: a body that asks for nothing is refused on its own merits, got \
                 {again:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE (the same one, from the caller's side): the refusal of a rotation's second
// step names the CLUSTER and the update it is waiting on, not the certificate
// authority it addressed — so a caller can poll `DescribeUpdate` on the id it was
// given and retry. The certificate authority's own state is reported only once the
// cluster is idle.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_in_flight_refusal_names_the_cluster_and_its_update_not_the_authority() {
    let (router, _engine) = emulator_with_engine();
    const CLUSTER: &str = "conc-rotation";

    let created = create_cluster(&router, REGION, CLUSTER).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster, got {created:?}"
    );
    settle(CLUSTER_ACTIVATION_DELAY).await;

    let appended = append_authority(&router, REGION, CLUSTER, Some("tok-rotation")).await;
    assert_eq!(
        appended.status, 200,
        "setup: expected 200 from CreateCertificateAuthority, got {appended:?}"
    );
    let append_update = str_member("rotation", "update.id", &appended.body["update"]["id"]);
    let successor = str_member(
        "rotation",
        "certificateAuthority.id",
        &appended.body["certificateAuthority"]["id"],
    );

    let refused = perform(
        &router,
        CLUSTER,
        &Operation::ActivateSuccessor,
        Some(&successor),
        None,
    )
    .await;
    let message = refused.body["message"].as_str().unwrap_or_default();
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (409, "ResourceInUseException"),
        "expected 409 ResourceInUseException, got {refused:?}"
    );
    assert!(
        message.contains(&append_update),
        "case \"rotation\": the refusal names the update to wait on, got {message:?}"
    );
    assert!(
        !message.contains(&successor),
        "case \"rotation\": the blocking resource is the cluster, not the certificate \
         authority, got {message:?}"
    );

    // The id the refusal named is readable, and it is the append's own update: that
    // is what a caller polls before retrying.
    let waited_on = describe_update(&router, REGION, CLUSTER, &append_update).await;
    assert_eq!(
        waited_on.status, 200,
        "case \"rotation\": the id the refusal named is readable, got {waited_on:?}"
    );
    assert_eq!(
        waited_on.body["update"]["type"],
        serde_json::json!("CertificateAuthorityUpdate"),
        "case \"rotation\": the update being waited on is the append's, got {:?}",
        waited_on.body["update"]["type"]
    );

    // Once that update is done the cluster is idle, and the activation the gate was
    // holding back is accepted.
    settle(UPDATE_COMPLETION_DELAY).await;
    let activated = perform(
        &router,
        CLUSTER,
        &Operation::ActivateSuccessor,
        Some(&successor),
        None,
    )
    .await;
    assert_eq!(
        activated.status, 200,
        "case \"rotation\": the idle cluster takes the activation, got {activated:?}"
    );
    assert_eq!(
        activated.body["certificateAuthority"]["id"],
        serde_json::json!(successor),
        "case \"rotation\": the activation is about the successor, got {:?}",
        activated.body["certificateAuthority"]
    );
}
