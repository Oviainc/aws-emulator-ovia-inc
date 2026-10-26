// SPDX-License-Identifier: CC0-1.0

//! How `ActivateCertificateAuthority` words its refusal of a certificate-authority
//! id the named cluster's trust bundle does not hold, driven over the wire through
//! the same `MultiServiceRouter` the production binary mounts.
//!
//! The RULE under test: an activation promotes a certificate authority that is
//! already present on the cluster, so a `certificateAuthorityId` the named cluster's
//! trust bundle does not hold is refused with HTTP 404
//! `ResourceNotFoundException` whose message states that non-membership —
//! `"Certificate authority <certificateAuthorityId> is not in the trusted list for
//! cluster <clusterName>"`, with no trailing stop — where both values are the
//! request's own two `@httpLabel`s and the payload carries no identity member
//! (`tests/all/not_found_payload.rs` has that half of the contract).
//!
//! The rows differ along what the rule generalises over: WHY the bundle does not
//! hold the id (never minted, minted for another cluster, minted for the same-named
//! cluster in another region, or trusted once and then deleted out of the bundle —
//! the rotation case), the FORM of the id (a version-4 UUID, a string that is no
//! UUID at all, one character, a value whose bytes have to be percent-encoded into
//! the path), and the form of the cluster name the second variable is taken from
//! (including the documented `length 1..100` ceiling). Each assertion names its case
//! and prints the actual value.
//!
//! The wire harness is this test target's one copy in `tests/all/wire.rs`, the
//! cluster and certificate-authority helpers are `certificate_authority`'s, the
//! activation and deletion calls are the two lifecycle suites': no suite grows a
//! second client.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CLUSTER_ACTIVATION_DELAY, UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::{
    append_authority, create_cluster, describe_authority, describe_cluster, settle, str_member,
    REGION,
};
use crate::certificate_authority_activation::activate_authority;
use crate::certificate_authority_deletion::delete_authority;
use crate::not_found_payload::IDENTITY_MEMBERS;
use crate::wire::{emulator, error_code};

/// A second region, for the row whose id belongs to the same-named cluster
/// elsewhere: a cluster name identifies a cluster only within one region, so each
/// region's cluster has its own trust bundle.
const OTHER_REGION: &str = "eu-central-1";

/// The refusal AWS words for a certificate authority the cluster does not trust,
/// spelled out here rather than taken from the emulator, so this suite pins the
/// wording instead of agreeing with whatever the handler happens to build: the fixed
/// text and the two request-derived variables, with no trailing stop.
fn expected_message(authority_id: &str, cluster: &str) -> String {
    format!("Certificate authority {authority_id} is not in the trusted list for cluster {cluster}")
}

/// A cluster, waited out to `ACTIVE`, with one successor certificate authority
/// appended and its trust distribution finished — the state a rotation reaches
/// before its activation. Returns `(the signer's id, the successor's id)`.
async fn cluster_with_a_distributed_successor(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    cluster: &str,
) -> (String, String) {
    let created = create_cluster(router, region, cluster).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateCluster, got {created:?}"
    );
    settle(CLUSTER_ACTIVATION_DELAY).await;

    let described = describe_cluster(router, region, cluster).await;
    let signer = str_member(
        case,
        "cluster.certificateAuthority.active.id",
        &described.body["cluster"]["certificateAuthority"]["active"]["id"],
    );

    let appended = append_authority(router, region, cluster, None).await;
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

/// One probe: the case it covers, the region it is signed for, and the two
/// `@httpLabel` values it sends.
struct Probe {
    case: &'static str,
    region: &'static str,
    cluster: String,
    authority_id: String,
}

// ---------------------------------------------------------------------------
// RULE: a `certificateAuthorityId` the named cluster's trust bundle does not hold
// is a 404 `ResourceNotFoundException` that says the id is not in that cluster's
// trusted list, built from the request's own two labels.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_untrusted_certificate_authority_is_refused_as_not_in_the_trusted_list() {
    let router = emulator();

    // The cluster most rows probe: real, ACTIVE, holding its own signer and one
    // distributed successor — so a refusal below is about the id, not about an empty
    // store.
    let (ready_signer, ready_successor) =
        cluster_with_a_distributed_successor(&router, "ready", REGION, "trust-ready").await;

    // A second cluster in the same region: its successor's id is a real CA id that
    // this region holds, but not in the first cluster's bundle.
    let (_, other_cluster_successor) =
        cluster_with_a_distributed_successor(&router, "other cluster", REGION, "trust-other").await;

    // The same name in another region is a different cluster, with its own bundle,
    // so the first cluster's successor id means nothing there.
    let elsewhere = create_cluster(&router, OTHER_REGION, "trust-ready").await;
    assert_eq!(
        elsewhere.status, 200,
        "setup: expected 200 creating the other region's cluster, got {elsewhere:?}"
    );

    // A cluster name at the documented ceiling (`clusterName` is `length 1..100`),
    // so the sentence's second variable is exercised at that bound too.
    let longest = "t".repeat(100);
    let longest_cluster = create_cluster(&router, REGION, &longest).await;
    assert_eq!(
        longest_cluster.status, 200,
        "setup: expected 200 creating the longest-named cluster, got {longest_cluster:?}"
    );

    // The recorded rotation case: an id the cluster DID trust, deleted out of the
    // bundle by `DeleteCertificateAuthority` and its update completed.
    let (_, deleted) =
        cluster_with_a_distributed_successor(&router, "rotated", REGION, "trust-rotated").await;
    let deletion = delete_authority(&router, REGION, "trust-rotated", &deleted, None).await;
    assert_eq!(
        deletion.status, 200,
        "setup: expected 200 from DeleteCertificateAuthority, got {deletion:?}"
    );
    settle(UPDATE_COMPLETION_DELAY).await;
    let gone = describe_authority(&router, REGION, "trust-rotated", &deleted).await;
    assert_eq!(
        gone.status, 404,
        "setup: the deleted CA must have left the bundle, got {gone:?}"
    );

    let probes = vec![
        Probe {
            case: "a version-4 UUID the cluster never held",
            region: REGION,
            cluster: "trust-ready".to_string(),
            authority_id: "00000000-0000-4000-8000-000000000000".to_string(),
        },
        Probe {
            case: "an id that is not a UUID at all",
            region: REGION,
            cluster: "trust-ready".to_string(),
            authority_id: "not-an-id".to_string(),
        },
        Probe {
            case: "a one-character id",
            region: REGION,
            cluster: "trust-ready".to_string(),
            authority_id: "x".to_string(),
        },
        Probe {
            case: "an id whose bytes must be percent-encoded into the path",
            region: REGION,
            cluster: "trust-ready".to_string(),
            authority_id: "ca id/with a slash+plus".to_string(),
        },
        Probe {
            case: "a CA of another cluster in the same region",
            region: REGION,
            cluster: "trust-ready".to_string(),
            authority_id: other_cluster_successor,
        },
        Probe {
            case: "a CA of the same-named cluster in another region",
            region: OTHER_REGION,
            cluster: "trust-ready".to_string(),
            authority_id: ready_successor.clone(),
        },
        Probe {
            case: "a cluster name at the documented length ceiling",
            region: REGION,
            cluster: longest.clone(),
            authority_id: ready_successor.clone(),
        },
        Probe {
            case: "a CA deleted out of the cluster's trust bundle",
            region: REGION,
            cluster: "trust-rotated".to_string(),
            authority_id: deleted.clone(),
        },
    ];

    for probe in &probes {
        let case = probe.case;
        let refused = activate_authority(
            &router,
            probe.region,
            &probe.cluster,
            &probe.authority_id,
            Some("tok-untrusted"),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        let expected = expected_message(&probe.authority_id, &probe.cluster);
        assert_eq!(
            refused.body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.body["message"]
        );
        // The sentence states a membership fact and stops there: no full stop, and
        // none of the exception's identity members is set (the certificate authority
        // is the resource reported missing, not the cluster).
        assert!(
            !expected.ends_with('.'),
            "case {case:?}: the refusal carries no trailing stop, got {expected:?}"
        );
        for member in IDENTITY_MEMBERS {
            assert_eq!(
                refused.body[member],
                serde_json::Value::Null,
                "case {case:?}: the payload must not carry {member}, got {:?}",
                refused.body[member]
            );
        }
    }

    // The refusals changed nothing, and the SAME cluster takes the activation of the
    // successor its bundle does hold: the wording above reports trust-bundle
    // membership read out of the store, not a fixed answer to this shape of request.
    let still = describe_cluster(&router, REGION, "trust-ready").await;
    assert_eq!(
        still.body["cluster"]["certificateAuthority"]["active"]["id"].as_str(),
        Some(ready_signer.as_str()),
        "a refused activation must not change the signer, got {:?}",
        still.body["cluster"]["certificateAuthority"]["active"]
    );
    assert_eq!(
        still.body["cluster"]["status"].as_str(),
        Some("ACTIVE"),
        "a refused activation must not take the cluster into an update, got {:?}",
        still.body["cluster"]["status"]
    );
    let activated =
        activate_authority(&router, REGION, "trust-ready", &ready_successor, None).await;
    assert_eq!(
        activated.status, 200,
        "the trusted successor is activatable, got {activated:?}"
    );

    // And once that successor is promoted and the outgoing CA retired, the ids the
    // bundle still holds are not refused by this path at all — the refusal tracks
    // membership, not identity.
    settle(UPDATE_COMPLETION_DELAY).await;
    let retired = describe_authority(&router, REGION, "trust-ready", &ready_signer).await;
    assert_eq!(
        retired.status, 200,
        "the retired CA stays in the trust bundle, got {retired:?}"
    );
}
