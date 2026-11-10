// SPDX-License-Identifier: CC0-1.0

//! `DescribeCertificateAuthority` refuses a certificate authority the named cluster
//! does not hold the way Amazon EKS does: with the refusal the certificate-authority
//! plane answered, carried through WRAPPED.
//!
//! The RULE under test: a read Amazon EKS serves out of an internal plane reports
//! that plane's own refusal with the tail that names the call behind it — the
//! service that answered, the status it answered, the id of that call and how many
//! attempts were made — so the message is
//! `certificate authority not found (Service: EksCertificateAuthority, Status Code:
//! 404, Request ID: <request id>) (SDK Attempt Count: 1)`. Everything but the
//! request id is fixed text: the certificate authority and the cluster are the
//! request's own `@httpLabel`s, so neither is echoed. The request id identifies the
//! call that was made, so it is drawn afresh per call and two refusals never carry
//! the same one.
//!
//! The rows differ along what the rule generalises over: a certificate authority id
//! that was never minted (in and out of the UUID shape this service mints), one that
//! belongs to another cluster, one whose cluster is in another region, and — the case
//! a rotation client actually hits — the id of a certificate authority this cluster
//! HELD and a delete has taken out of its trust bundle.
//!
//! `ActivateCertificateAuthority` and `DeleteCertificateAuthority` word the same
//! absence as their own recordings show, so they are not asserted here: this is the
//! READ's refusal.
//!
//! The wire harness and the certificate-authority drivers are the shared ones
//! (`tests/all/wire.rs`, `tests/all/certificate_authority.rs`).

use std::collections::BTreeSet;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CA_DISTRIBUTION_DELAY, CERTIFICATE_AUTHORITY_MANAGER, CERTIFICATE_AUTHORITY_NOT_FOUND_DETAIL,
    CLUSTER_ACTIVATION_DELAY, UPDATE_COMPLETION_DELAY,
};

use crate::certificate_authority::{
    append_authority, create_cluster, describe_authority, is_uuid, settle, str_member, REGION,
};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The second region a row is signed for.
const OTHER_REGION: &str = "eu-central-1";

/// The head of the wrapped refusal: the plane's own sentence, and the tail's
/// opening up to the request id.
fn expected_head() -> String {
    format!(
        "{CERTIFICATE_AUTHORITY_NOT_FOUND_DETAIL} (Service: {CERTIFICATE_AUTHORITY_MANAGER}, \
         Status Code: 404, Request ID: "
    )
}

/// The tail that closes it: how many attempts were made to reach that plane.
const EXPECTED_TAIL: &str = ") (SDK Attempt Count: 1)";

async fn delete_authority(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    cluster: &str,
    authority_id: &str,
) -> Answer {
    call(
        router,
        region,
        "DELETE",
        &format!(
            "/clusters/{}/certificate-authorities/{}",
            encode_label(cluster),
            encode_label(authority_id)
        ),
        "",
    )
    .await
}

/// The request id the wrapped refusal carries, from a message that matches the
/// fixed text around it.
fn request_id(case: &str, message: &str) -> String {
    let head = expected_head();
    assert!(
        message.starts_with(&head),
        "case {case:?}: the refusal must begin {head:?}, got {message:?}"
    );
    assert!(
        message.ends_with(EXPECTED_TAIL),
        "case {case:?}: the refusal must end {EXPECTED_TAIL:?}, got {message:?}"
    );
    let id = &message[head.len()..message.len() - EXPECTED_TAIL.len()];
    assert!(
        is_uuid(id),
        "case {case:?}: the tail must carry the id of the call that was made, got {id:?}"
    );
    id.to_string()
}

// ---------------------------------------------------------------------------
// RULE: every certificate authority this read cannot resolve is refused 404
// `ResourceNotFoundException` with the wrapped refusal, naming nothing from the
// request and a fresh request id per call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unresolvable_certificate_authority_read_carries_the_wrapped_refusal() {
    let router = emulator();

    // Two clusters of the same name in two regions, and a second cluster in the
    // first region, so an id can be addressed on a cluster that does not hold it.
    for region in [REGION, OTHER_REGION] {
        let created = create_cluster(&router, region, "ca-gone").await;
        assert_eq!(
            created.status, 200,
            "setup: expected 200 from CreateCluster in {region:?}, got {created:?}"
        );
    }
    let neighbour = create_cluster(&router, REGION, "ca-neighbour").await;
    assert_eq!(
        neighbour.status, 200,
        "setup: expected 200 from CreateCluster for the neighbour, got {neighbour:?}"
    );
    settle(CLUSTER_ACTIVATION_DELAY).await;

    // A successor on the neighbour: an id that exists, but not on the cluster the
    // rows address.
    let elsewhere = append_authority(&router, REGION, "ca-neighbour", None).await;
    assert_eq!(
        elsewhere.status, 200,
        "setup: expected 200 from CreateCertificateAuthority, got {elsewhere:?}"
    );
    let elsewhere_id = str_member(
        "setup",
        "certificateAuthority.id",
        &elsewhere.body["certificateAuthority"]["id"],
    );

    // A successor the cluster HELD and then had deleted: the id a rotation client
    // reads back after its teardown.
    let appended = append_authority(&router, REGION, "ca-gone", None).await;
    assert_eq!(
        appended.status, 200,
        "setup: expected 200 from CreateCertificateAuthority, got {appended:?}"
    );
    let deleted_id = str_member(
        "setup",
        "certificateAuthority.id",
        &appended.body["certificateAuthority"]["id"],
    );
    settle(CA_DISTRIBUTION_DELAY).await;
    let removed = delete_authority(&router, REGION, "ca-gone", &deleted_id).await;
    assert_eq!(
        removed.status, 200,
        "setup: expected 200 from DeleteCertificateAuthority, got {removed:?}"
    );
    settle(UPDATE_COMPLETION_DELAY).await;

    let cases: Vec<(&str, &str, &str, String)> = vec![
        (
            "an id in the UUID shape that was never minted",
            REGION,
            "ca-gone",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "an id that is not a UUID at all",
            REGION,
            "ca-gone",
            "not-an-id".to_string(),
        ),
        (
            "a certificate authority of another cluster",
            REGION,
            "ca-gone",
            elsewhere_id.clone(),
        ),
        (
            "a certificate authority of the same-named cluster in another region",
            OTHER_REGION,
            "ca-gone",
            deleted_id.clone(),
        ),
        (
            "a certificate authority a delete took out of the trust bundle",
            REGION,
            "ca-gone",
            deleted_id.clone(),
        ),
    ];

    let mut request_ids: BTreeSet<String> = BTreeSet::new();
    for (case, region, cluster, authority_id) in &cases {
        let answer = describe_authority(&router, region, cluster, authority_id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 from DescribeCertificateAuthority, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        let message = answer.message();
        let id = request_id(case, &message);
        assert!(
            !message.contains(authority_id.as_str()) && !message.contains(cluster),
            "case {case:?}: the refusal echoes neither label of the request, got {message:?}"
        );
        assert!(
            request_ids.insert(id.clone()),
            "case {case:?}: each call carries its own request id, but {id:?} appeared twice"
        );
        // The payload names no resource of its own: see `child_resource_not_found`.
        assert!(
            answer.body.get("clusterName").is_none(),
            "case {case:?}: the payload names no cluster, got {:?}",
            answer.body
        );
    }

    // Two reads of the SAME unresolvable id carry two request ids: the id belongs to
    // the call, not to the resource that was asked for.
    let repeat = describe_authority(&router, REGION, "ca-gone", &deleted_id).await;
    let repeated_id = request_id("a repeated read", &repeat.message());
    assert!(
        !request_ids.contains(&repeated_id),
        "a repeated read draws its own request id, got {repeated_id:?} again"
    );
}
