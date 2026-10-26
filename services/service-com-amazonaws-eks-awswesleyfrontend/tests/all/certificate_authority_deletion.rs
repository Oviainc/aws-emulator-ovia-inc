// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteCertificateAuthority`, driven over the wire through
//! the same `MultiServiceRouter` the production binary mounts.
//!
//! The RULE under test: deleting a certificate authority the named cluster holds,
//! and that is not the one signing for it, starts ONE asynchronous update that
//! removes the CA's public certificate from the cluster's trust bundle — the CA
//! reports `distributionStatus` `DELETING` and the cluster `UPDATING` while that
//! update runs, and when the clock completes it the record has left the cluster
//! (`DescribeCertificateAuthority` no longer resolves it) while whichever CA signs
//! keeps signing. A call whose cluster or certificate authority cannot take the
//! removal changes nothing and is refused with the class that names what is wrong: a
//! cluster or an id the region does not hold is 404 `ResourceNotFoundException`, and
//! a cluster running another update or a CA that is signing is 409
//! `ResourceInUseException`.
//!
//! The rows differ along what the rule generalises over: which certificate authority
//! is removed (a successor the caller appended, or the outgoing CA an activation
//! retired), whether an idempotency token is supplied and whether a second call
//! replays it, and which half of the `(cluster, certificate authority)` pair the
//! request gets wrong. Every assertion names its case and prints the actual value.
//!
//! The wire harness is this test target's one copy in `tests/all/wire.rs`, the
//! cluster and certificate-authority helpers are `certificate_authority`'s and the
//! activation call is the activation suite's: no suite grows a second client.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CLUSTER_ACTIVATION_DELAY, UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::{
    append_authority, create_cluster, describe_authority, describe_cluster, describe_update, is_uuid,
    settle, str_member, REGION,
};
use crate::certificate_authority_activation::activate_authority;
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// `DeleteCertificateAuthority`:
/// `DELETE /clusters/{clusterName}/certificate-authorities/{certificateAuthorityId}`,
/// whose `clientRequestToken` is an `@httpQuery` parameter rather than a body
/// member.
pub(crate) async fn delete_authority(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    authority_id: &str,
    client_request_token: Option<&str>,
) -> Answer {
    let mut uri = format!(
        "/clusters/{}/certificate-authorities/{}",
        encode_label(cluster),
        encode_label(authority_id)
    );
    if let Some(token) = client_request_token {
        uri.push_str(&format!("?clientRequestToken={}", encode_label(token)));
    }
    call(router, region, "DELETE", &uri, "").await
}

/// The id of the certificate authority the cluster reports as its signer
/// (`Cluster$certificateAuthority.active`), which is how a caller learns which CA
/// signs without knowing any id in advance.
async fn signer_of(router: &MultiServiceRouter, cluster: &str) -> String {
    let described = describe_cluster(router, REGION, cluster).await;
    str_member(
        cluster,
        "cluster.certificateAuthority.active.id",
        &described.body["cluster"]["certificateAuthority"]["active"]["id"],
    )
}

/// A cluster with a successor certificate authority appended and fully
/// distributed — the state a rotation reaches before its activation — returning
/// `(the signer's id, the successor's id)`.
async fn cluster_with_a_distributed_successor(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
) -> (String, String) {
    let created = create_cluster(router, REGION, cluster).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateCluster, got {created:?}"
    );
    settle(CLUSTER_ACTIVATION_DELAY).await;
    let signer = signer_of(router, cluster).await;

    let appended = append_authority(router, REGION, cluster, None).await;
    assert_eq!(
        appended.status, 200,
        "case {case:?}: expected 200 from CreateCertificateAuthority, got {appended:?}"
    );
    let successor = str_member(
        case,
        "certificateAuthority.id",
        &appended.body["certificateAuthority"]["id"],
    );
    settle(UPDATE_COMPLETION_DELAY).await;
    (signer, successor)
}

// ---------------------------------------------------------------------------
// RULE: a deletion starts one asynchronous update that takes the certificate
// authority out of the cluster's trust bundle when the clock completes it, and the
// cluster's signer is untouched throughout.
// ---------------------------------------------------------------------------

/// Which of a cluster's certificate authorities a row deletes.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// The successor the caller appended, which has never signed.
    Successor,
    /// The outgoing CA an activation retired, which signed until then: "to remove
    /// the outgoing CA, first activate the successor CA"
    /// (`DeleteCertificateAuthority`).
    RetiredSigner,
}

struct Row {
    case: &'static str,
    cluster: &'static str,
    target: Target,
    token: Option<&'static str>,
    /// Whether a second call, made while the removal runs, replays that token.
    retry_replays_the_token: bool,
}

#[tokio::test]
async fn a_deletion_removes_the_authority_from_the_trust_bundle_when_its_update_completes() {
    let router = emulator();

    let rows = [
        Row {
            case: "a successor the caller appended, no token",
            cluster: "del-successor",
            target: Target::Successor,
            token: None,
            retry_replays_the_token: false,
        },
        Row {
            case: "a successor the caller appended, with a token the retry replays",
            cluster: "del-successor-token",
            target: Target::Successor,
            token: Some("tok-delete"),
            retry_replays_the_token: true,
        },
        Row {
            case: "the outgoing certificate authority an activation retired",
            cluster: "del-retired",
            target: Target::RetiredSigner,
            token: None,
            retry_replays_the_token: false,
        },
    ];

    for row in &rows {
        let (case, cluster) = (row.case, row.cluster);
        let (signer, successor) =
            cluster_with_a_distributed_successor(&router, case, cluster).await;

        // The row that deletes a RETIRED signer first completes the rotation, which
        // is what retires one: the successor becomes the signer and the outgoing CA
        // stays in the trust bundle, no longer signing.
        let (target, expected_signer) = match row.target {
            Target::Successor => (successor.clone(), signer.clone()),
            Target::RetiredSigner => {
                let activated = activate_authority(&router, REGION, cluster, &successor, None).await;
                assert_eq!(
                    activated.status, 200,
                    "case {case:?}: expected 200 from ActivateCertificateAuthority, got \
                     {activated:?}"
                );
                settle(UPDATE_COMPLETION_DELAY).await;
                (signer.clone(), successor.clone())
            }
        };

        let deleted = delete_authority(&router, REGION, cluster, &target, row.token).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from DeleteCertificateAuthority, got {deleted:?}"
        );
        let update_id = str_member(case, "update.id", &deleted.body["update"]["id"]);
        assert!(
            is_uuid(&update_id),
            "case {case:?}: the update carries a UUID id, got {update_id:?}"
        );
        assert_eq!(
            deleted.body["update"]["type"],
            serde_json::json!("CertificateAuthorityUpdate"),
            "case {case:?}: expected a CertificateAuthorityUpdate, got {:?}",
            deleted.body["update"]["type"]
        );
        assert_eq!(
            deleted.body["update"]["status"],
            serde_json::json!("InProgress"),
            "case {case:?}: the removal is asynchronous, got {:?}",
            deleted.body["update"]["status"]
        );
        assert_eq!(
            deleted.body["certificateAuthority"]["id"],
            serde_json::json!(target),
            "case {case:?}: the answer is about the CA the request named, got {:?}",
            deleted.body["certificateAuthority"]
        );
        assert_eq!(
            deleted.body["certificateAuthority"]["distributionStatus"],
            serde_json::json!("DELETING"),
            "case {case:?}: the CA reports DELETING while the removal runs, got {:?}",
            deleted.body["certificateAuthority"]["distributionStatus"]
        );

        // While that update runs the record is still readable, reporting the
        // distribution status of a CA on its way out, and the cluster is under the
        // update.
        let during = describe_authority(&router, REGION, cluster, &target).await;
        assert_eq!(
            during.body["certificateAuthority"]["distributionStatus"],
            serde_json::json!("DELETING"),
            "case {case:?}: a read reports the same DELETING status, got {during:?}"
        );
        let cluster_during = describe_cluster(&router, REGION, cluster).await;
        assert_eq!(
            cluster_during.body["cluster"]["status"],
            serde_json::json!("UPDATING"),
            "case {case:?}: the cluster is under the removal update, got {:?}",
            cluster_during.body["cluster"]["status"]
        );

        // A second call while the removal runs: a replayed token is absorbed onto
        // the update already running, and a call that replays nothing meets the
        // cluster's concurrency gate.
        let again = delete_authority(&router, REGION, cluster, &target, row.token).await;
        if row.retry_replays_the_token {
            assert_eq!(
                again.body["update"]["id"],
                serde_json::json!(update_id),
                "case {case:?}: a replayed token reports the original update, got {again:?}"
            );
        } else {
            assert_eq!(
                (again.status, error_code(&again).as_str()),
                (409, "ResourceInUseException"),
                "case {case:?}: a second call is refused while the removal runs, got {again:?}"
            );
        }

        settle(UPDATE_COMPLETION_DELAY).await;

        // The update completed, so the record has left the trust bundle and the
        // cluster is back to running nothing.
        let after = describe_authority(&router, REGION, cluster, &target).await;
        assert_eq!(
            (after.status, error_code(&after).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the deleted CA is gone from the trust bundle, got {after:?}"
        );
        let completed = describe_update(&router, REGION, cluster, &update_id).await;
        assert_eq!(
            completed.body["update"]["status"],
            serde_json::json!("Successful"),
            "case {case:?}: the removal update completed, got {:?}",
            completed.body["update"]["status"]
        );
        let cluster_after = describe_cluster(&router, REGION, cluster).await;
        assert_eq!(
            cluster_after.body["cluster"]["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: the cluster is idle again, got {:?}",
            cluster_after.body["cluster"]["status"]
        );
        assert_eq!(
            cluster_after.body["cluster"]["certificateAuthority"]["active"]["id"],
            serde_json::json!(expected_signer),
            "case {case:?}: the signer is untouched by a deletion, got {:?}",
            cluster_after.body["cluster"]["certificateAuthority"]["active"]
        );
        let signer_still_there =
            describe_authority(&router, REGION, cluster, &expected_signer).await;
        assert_eq!(
            signer_still_there.status, 200,
            "case {case:?}: the signing CA is still in the trust bundle, got \
             {signer_still_there:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a call whose cluster or certificate authority cannot take the removal
// writes nothing and is refused with the class that names what is wrong.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deletion_the_cluster_or_the_authority_cannot_take_is_refused_and_writes_nothing() {
    let router = emulator();
    const OTHER_REGION: &str = "eu-central-1";

    let (signer, successor) =
        cluster_with_a_distributed_successor(&router, "refusals", "del-refused").await;

    // The rows that are about the (cluster, id) pair, and the one about the CA that
    // signs: the cluster is idle for all of them.
    let rows: Vec<(&str, &str, String, &str, u16, &str)> = vec![
        (
            "an unknown cluster",
            REGION,
            successor.clone(),
            "no-such-cluster",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a well-formed but unused CA id",
            REGION,
            "00000000-0000-4000-8000-000000000000".to_string(),
            "del-refused",
            404,
            "ResourceNotFoundException",
        ),
        (
            "an id that is not a UUID at all",
            REGION,
            "not-an-id".to_string(),
            "del-refused",
            404,
            "ResourceNotFoundException",
        ),
        (
            "the same-named cluster in another region",
            OTHER_REGION,
            successor.clone(),
            "del-refused",
            404,
            "ResourceNotFoundException",
        ),
        (
            "the certificate authority that signs for the cluster",
            REGION,
            signer.clone(),
            "del-refused",
            409,
            "ResourceInUseException",
        ),
    ];

    for (case, region, authority_id, cluster, status, code) in &rows {
        let refused = delete_authority(&router, region, cluster, authority_id, None).await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (*status, *code),
            "case {case:?}: expected {status} {code}, got {refused:?}"
        );
    }

    // A cluster already running another update is refused on the cluster's
    // concurrency, worded as every refused cluster update of this service words it.
    let busy = call(
        &router,
        REGION,
        "POST",
        &format!("/clusters/{}/update-config", encode_label("del-refused")),
        &serde_json::json!({ "deletionProtection": true }).to_string(),
    )
    .await;
    assert_eq!(
        busy.status, 200,
        "setup: expected 200 from UpdateClusterConfig, got {busy:?}"
    );
    let busy_update = str_member("busy", "update.id", &busy.body["update"]["id"]);
    let refused = delete_authority(&router, REGION, "del-refused", &successor, None).await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (409, "ResourceInUseException"),
        "case \"a cluster running another update\": expected 409 ResourceInUseException, got \
         {refused:?}"
    );
    let expected = format!(
        "Cannot CertificateAuthorityUpdate because cluster del-refused currently has update \
         {busy_update} in progress"
    );
    assert_eq!(
        refused.body["message"].as_str(),
        Some(expected.as_str()),
        "case \"a cluster running another update\": expected the message {expected:?}, got {:?}",
        refused.body["message"]
    );

    settle(UPDATE_COMPLETION_DELAY).await;

    // None of the refusals wrote anything: both certificate authorities are still in
    // the trust bundle, fully distributed, and the signer still signs.
    for (case, authority_id) in [("the signer", &signer), ("the successor", &successor)] {
        let described = describe_authority(&router, REGION, "del-refused", authority_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: a refused deletion leaves the CA in the trust bundle, got {described:?}"
        );
        assert_eq!(
            described.body["certificateAuthority"]["distributionStatus"],
            serde_json::json!("COMPLETE"),
            "case {case:?}: a refused deletion does not move the distribution status, got {:?}",
            described.body["certificateAuthority"]["distributionStatus"]
        );
    }
    assert_eq!(
        signer_of(&router, "del-refused").await,
        signer,
        "a refused deletion must not change the signer"
    );

    // And the still-present successor is deleted once it is asked for properly,
    // which is what makes the refusals above about state and not about the call.
    let deleted = delete_authority(&router, REGION, "del-refused", &successor, None).await;
    assert_eq!(
        deleted.status, 200,
        "the idle cluster takes the deletion, got {deleted:?}"
    );
}
