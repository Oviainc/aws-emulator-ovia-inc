// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListCertificateAuthorities`: the listing of one cluster's
//! certificate authorities, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The write and the single read it pages over (`CreateCertificateAuthority`,
//! `DescribeCertificateAuthority`) are covered by `certificate_authority.rs`, whose
//! helpers this suite reuses rather than growing a second client; this suite is
//! about what the LISTING adds: which certificate authorities it reports, in which
//! order, that each summary agrees with the read of the same CA, how it pages them,
//! and what it answers for a cluster that does not exist.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (how many
//! certificate authorities the cluster holds, how far their trust distribution has
//! got, the cluster name's shape, the region the request is signed for, the page
//! size, how a token is foreign), and every assertion names its case and prints the
//! actual value.

use std::collections::BTreeSet;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, encode_page_token, encode_scoped_page_token, CAPABILITIES_LISTING,
    CA_DISTRIBUTION_DELAY, CLUSTER_ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::{
    append_authority, create_cluster, describe_authority, describe_cluster, settle, str_member,
    REGION,
};
use crate::wire::{call, emulator, encode_label, Answer};

/// The members a `CertificateAuthoritySummary` carries, which is what the listing
/// reports and what the corresponding `DescribeCertificateAuthority` read must
/// agree with member for member.
const SUMMARY_MEMBERS: &[&str] = &[
    "id",
    "createdAt",
    "createdBy",
    "activatedAt",
    "activatedBy",
    "signingStatus",
    "distributionStatus",
];

/// `ListCertificateAuthorities`: `GET /clusters/{clusterName}/certificate-authorities`.
async fn list_authorities_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    query: &str,
) -> Answer {
    let path = format!("/clusters/{}/certificate-authorities", encode_label(cluster));
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, region, "GET", &uri, "").await
}

async fn list_authorities(router: &MultiServiceRouter, cluster: &str, query: &str) -> Answer {
    list_authorities_in(router, REGION, cluster, query).await
}

/// The `certificateAuthorities` of a listing answer, as the ids it returned, in the
/// order it returned them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["certificateAuthorities"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// One summary of a listing answer, by the id it carries.
fn summary_of<'a>(answer: &'a Answer, id: &str) -> &'a serde_json::Value {
    answer.body["certificateAuthorities"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"].as_str() == Some(id)))
        .unwrap_or_else(|| panic!("expected the listing to carry {id:?}, got {answer:?}"))
}

/// The id of the certificate authority the cluster reports as its signer, which is
/// how a caller learns the id of the CA Amazon EKS provisioned with the cluster.
async fn signer_id(router: &MultiServiceRouter, case: &str, cluster: &str) -> String {
    let answer = describe_cluster(router, REGION, cluster).await;
    str_member(
        case,
        "cluster.certificateAuthority.active.id",
        &answer.body["cluster"]["certificateAuthority"]["active"]["id"],
    )
}

/// An `ACTIVE` cluster, which is the precondition every certificate-authority call
/// in this suite needs.
async fn active_cluster(router: &MultiServiceRouter, case: &str, cluster: &str) {
    let created = create_cluster(router, REGION, cluster).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: setup CreateCluster expected 200, got {created:?}"
    );
    settle(CLUSTER_ACTIVATION_DELAY).await;
}

// ---------------------------------------------------------------------------
// RULE: the listing reports exactly the certificate authorities the store holds
// for the named cluster — every one of them, nothing else — in certificate
// authority id order, each as the same summary the read of that CA answers with,
// including the trust-distribution status the clock has reached by the time of the
// call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_reports_every_stored_authority_and_agrees_with_the_read() {
    let router = emulator();

    // Rows differ along how many certificate authorities the cluster holds, how far
    // the successor's trust distribution has got by the time of the listing, and the
    // shape of the cluster name.
    struct Case {
        case: &'static str,
        cluster: &'static str,
        append: bool,
        settle_distribution: bool,
        expected_distribution: &'static str,
    }
    let cases = vec![
        Case {
            case: "only the CA Amazon EKS provisions with the cluster",
            cluster: "ca-list-initial",
            append: false,
            settle_distribution: false,
            expected_distribution: "COMPLETE",
        },
        Case {
            case: "a successor whose distribution is still in flight",
            cluster: "ca-list-appending",
            append: true,
            settle_distribution: false,
            expected_distribution: "IN_PROGRESS",
        },
        Case {
            case: "a successor the clock has finished distributing",
            cluster: "ca-list-distributed",
            append: true,
            settle_distribution: true,
            expected_distribution: "COMPLETE",
        },
        Case {
            case: "another cluster name shape",
            cluster: "Ca9_list-Mixed",
            append: true,
            settle_distribution: true,
            expected_distribution: "COMPLETE",
        },
    ];

    for row in &cases {
        let case = row.case;
        active_cluster(&router, case, row.cluster).await;
        let initial = signer_id(&router, case, row.cluster).await;
        let mut expected: BTreeSet<String> = BTreeSet::new();
        expected.insert(initial.clone());

        let mut successor: Option<String> = None;
        if row.append {
            let appended = append_authority(&router, REGION, row.cluster, None).await;
            assert_eq!(
                appended.status, 200,
                "case {case:?}: setup CreateCertificateAuthority expected 200, got {appended:?}"
            );
            let id = str_member(
                case,
                "certificateAuthority.id",
                &appended.body["certificateAuthority"]["id"],
            );
            expected.insert(id.clone());
            successor = Some(id);
        }
        if row.settle_distribution {
            settle(CA_DISTRIBUTION_DELAY).await;
        }

        let answer = list_authorities(&router, row.cluster, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from ListCertificateAuthorities, got {answer:?}"
        );
        let ids = listed_ids(&answer);
        let expected_ids: Vec<String> = expected.iter().cloned().collect();
        assert_eq!(
            ids, expected_ids,
            "case {case:?}: expected exactly the stored certificate authorities in id \
             order {expected_ids:?}, got {answer:?}"
        );
        assert!(
            answer.body["nextToken"].is_null(),
            "case {case:?}: a single page offers no nextToken, got {answer:?}"
        );

        // The CA Amazon EKS provisions with the cluster is the signer and is already
        // distributed; a CA the caller appended is not signing, and its distribution
        // is wherever the clock has got to.
        let signer = summary_of(&answer, &initial);
        assert_eq!(
            signer["signingStatus"],
            serde_json::json!("IN_USE"),
            "case {case:?}: the cluster's own CA signs, got {:?}",
            signer["signingStatus"]
        );
        assert_eq!(
            signer["createdBy"],
            serde_json::json!("EKS"),
            "case {case:?}: the CA provisioned with the cluster is created by EKS, got {:?}",
            signer["createdBy"]
        );
        if let Some(id) = &successor {
            let summary = summary_of(&answer, id);
            assert_eq!(
                summary["signingStatus"],
                serde_json::json!("NOT_USED"),
                "case {case:?}: an appended successor does not sign, got {:?}",
                summary["signingStatus"]
            );
            assert_eq!(
                summary["createdBy"],
                serde_json::json!("CUSTOMER"),
                "case {case:?}: a CA the caller appends is created by CUSTOMER, got {:?}",
                summary["createdBy"]
            );
            assert_eq!(
                summary["distributionStatus"],
                serde_json::json!(row.expected_distribution),
                "case {case:?}: expected distributionStatus {:?}, got {:?}",
                row.expected_distribution,
                summary["distributionStatus"]
            );
        }

        // The listing and the read of the same CA are two projections of ONE stored
        // record, so they agree member for member.
        for id in &ids {
            let described = describe_authority(&router, REGION, row.cluster, id).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: every listed CA is readable, got {described:?}"
            );
            let read = &described.body["certificateAuthority"];
            let summary = summary_of(&answer, id);
            for member in SUMMARY_MEMBERS {
                assert_eq!(
                    summary[member], read[member],
                    "case {case:?}: the listing and the read of {id:?} must agree on \
                     {member}, got {:?} and {:?}",
                    summary[member], read[member]
                );
            }
        }

        // A listing is a READ: calling it again reports exactly what it reported
        // first, and no cluster's listing leaks another's certificate authorities.
        let again = list_authorities(&router, row.cluster, "").await;
        assert_eq!(
            listed_ids(&again),
            ids,
            "case {case:?}: a listing must not change what the next caller sees, got {again:?}"
        );
    }

    // Across the whole table no two clusters reported the same certificate
    // authority, and every cluster reported exactly what it holds.
    let mut everything: Vec<String> = Vec::new();
    for row in &cases {
        everything.extend(listed_ids(&list_authorities(&router, row.cluster, "").await));
    }
    let unique: BTreeSet<&String> = everything.iter().collect();
    assert_eq!(
        unique.len(),
        everything.len(),
        "a certificate authority belongs to exactly one cluster, but one was listed \
         twice across clusters: {everything:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: whatever page size the listing is driven at, following `nextToken` to
// exhaustion yields each certificate authority exactly once in id order and the
// last page carries no token; a page size outside the modelled range is refused,
// and so is a token this listing did not issue.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_at_any_size_yields_every_authority_exactly_once() {
    let router = emulator();
    let cluster = "ca-list-paged";
    active_cluster(&router, "paging", cluster).await;
    let initial = signer_id(&router, "paging", cluster).await;
    let appended = append_authority(&router, REGION, cluster, Some("tok-list-paged")).await;
    assert_eq!(
        appended.status, 200,
        "setup CreateCertificateAuthority expected 200, got {appended:?}"
    );
    let successor = str_member(
        "paging",
        "certificateAuthority.id",
        &appended.body["certificateAuthority"]["id"],
    );
    settle(CA_DISTRIBUTION_DELAY).await;
    let stored: Vec<String> = [initial, successor.clone()]
        .into_iter()
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();

    // Rows differ along page size: the modelled bounds (1 and 100), the size that is
    // exactly the number stored, and one above it.
    for page_size in [1, 2, 3, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(value) => format!("maxResults={page_size}&nextToken={value}"),
            };
            let answer = list_authorities(&router, cluster, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size}: expected 200, got {answer:?}"
            );
            let page = listed_ids(&answer);
            assert!(
                page.len() <= page_size as usize,
                "case maxResults={page_size}: a page must not exceed the limit, got {page:?}"
            );
            seen.extend(page.clone());
            pages += 1;
            assert!(
                pages <= stored.len() + 1,
                "case maxResults={page_size}: paging did not terminate after {pages} pages"
            );
            match answer.body["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        !page.is_empty(),
                        "case maxResults={page_size}: a token was returned for an empty page"
                    );
                    token = Some(next.to_owned());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, stored,
            "case maxResults={page_size}: paging must yield every certificate authority \
             once, in id order; got {seen:?}"
        );
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case maxResults={page_size}: a certificate authority was repeated across \
             pages: {seen:?}"
        );
    }

    // One past each published bound of `maxResults` (1..100) is refused by the
    // modelled range, before the handler runs.
    for case in ["maxResults=0", "maxResults=101"] {
        let answer = list_authorities(&router, cluster, case).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: outside the modelled range expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }

    // A token this listing did not issue is a bad parameter, not an empty page. The
    // rows differ along HOW the token is foreign: not this encoding's alphabet, an
    // odd length, a well-formed encoding carrying no provenance tag, a hex encoding
    // of a REAL certificate authority id, and the two a scope-less check would wave
    // through — tokens other listings of this same service mint.
    let untagged: String = successor.bytes().map(|byte| format!("{byte:02x}")).collect();
    let clusters_listing = encode_page_token(cluster);
    let capabilities_listing = encode_scoped_page_token(CAPABILITIES_LISTING, &successor);
    for case in [
        "not-hex",
        "zz",
        "abc",
        "ffffffffff",
        untagged.as_str(),
        clusters_listing.as_str(),
        capabilities_listing.as_str(),
    ] {
        let answer = list_authorities(&router, cluster, &format!("nextToken={case}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a foreign nextToken, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster this store does not hold for the region the request is signed
// for has no listing at all — `ResourceNotFoundException` (404) naming the missing
// cluster, not an empty page.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_a_cluster_that_does_not_exist_is_not_found() {
    let router = emulator();
    active_cluster(&router, "setup", "ca-list-present").await;

    // Rows differ along WHY the cluster is not in the store for this request: it was
    // never created, its name differs only in case, and it lives in another region.
    let cases: Vec<(&str, &str, Answer)> = vec![
        (
            "a cluster that was never created",
            "ca-list-absent",
            list_authorities(&router, "ca-list-absent", "").await,
        ),
        (
            "a cluster name differing only in case",
            "CA-LIST-PRESENT",
            list_authorities(&router, "CA-LIST-PRESENT", "").await,
        ),
        (
            "the right name in another region",
            "ca-list-present",
            list_authorities_in(&router, "eu-west-1", "ca-list-present", "").await,
        ),
    ];

    for (case, cluster, answer) in &cases {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(cluster_not_found_message(cluster).as_str()),
            "case {case:?}: expected the message to name the missing cluster, got {answer:?}"
        );
        assert_eq!(
            answer.body["clusterName"].as_str(),
            Some(*cluster),
            "case {case:?}: expected clusterName to name the missing cluster, got {answer:?}"
        );
    }

    // The cluster that DOES exist in the signed region still lists its own
    // certificate authority, so the rows above fail for the cluster and not for the
    // listing.
    let present = list_authorities(&router, "ca-list-present", "").await;
    assert_eq!(
        listed_ids(&present).len(),
        1,
        "the cluster that exists lists the CA Amazon EKS provisioned with it, got {present:?}"
    );
}
