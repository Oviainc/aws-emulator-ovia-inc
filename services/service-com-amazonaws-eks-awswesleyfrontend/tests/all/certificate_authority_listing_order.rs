// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ORDER `ListCertificateAuthorities` reports a cluster's
//! certificate authorities in, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The RULE under test: the listing reports a cluster's certificate authorities
//! most recently created FIRST — ordered by `createdAt` descending, with the
//! certificate-authority id as the tie-break so two created within one clock tick
//! still have one stable order — and `nextToken` continues that same order, so the
//! concatenation of the pages at any page size is the unpaginated listing. A caller
//! rotating a cluster CA takes the successor it must distribute off the head of the
//! listing and the outgoing signer off its tail, so the head cannot be whichever
//! certificate authority happens to sort first by id.
//!
//! The rows differ along what the rule generalises over: how many certificate
//! authorities the cluster holds, and how their creation order relates to the byte
//! order of their ids — the newest being the id that sorts LAST, the newest being
//! the id that sorts FIRST, and the two created at the very same instant. The two
//! arrangements that fix a creation order against the ids park `createdAt` on the
//! same `ClusterRecord::certificate_authorities` map `CreateCertificateAuthority`
//! writes and `DescribeCertificateAuthority` reads, through the crate's public
//! `state` module, because an id is derived from the cluster and a counter and no
//! sequence of calls can choose how it sorts; the call under test is always a real
//! HTTP request. Every assertion names its case and prints the actual value.
//!
//! The wire harness is this test target's one copy in `tests/all/wire.rs`, the
//! cluster and certificate-authority calls are `certificate_authority`'s and the
//! listing client is `certificate_authority_listing`'s: no suite grows a second
//! client.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ComAmazonawsEksAwswesleyfrontendState, CA_DISTRIBUTION_DELAY, CLUSTER_ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::{
    append_authority, create_cluster, epoch, settle, str_member, REGION,
};
use crate::certificate_authority_listing::{list_authorities, listed_ids, signer_id};
use crate::wire::{emulator_with_engine, Answer};

/// How the creation instants of a cluster's two certificate authorities are
/// arranged against the byte order of their ids.
#[derive(Debug, Clone, Copy)]
enum Arrangement {
    /// Whatever the calls produced: the appended successor is the newer CA.
    AsCreated,
    /// The newest CA is the one whose id sorts LAST, which is the arrangement an
    /// id-ordered listing reports backwards.
    NewestIsTheGreaterId,
    /// The newest CA is the one whose id sorts FIRST, so newest-first and ascending
    /// id agree — the arrangement that catches a listing that merely reverses the
    /// ids.
    NewestIsTheSmallerId,
    /// Both certificate authorities were created at the very same instant, so only
    /// the tie-break orders them: ascending id.
    SameInstant,
}

/// Rewrite the instant `cluster`'s certificate authority `authority` was created
/// at.
///
/// It is written into the same `ClusterRecord::certificate_authorities` map
/// `CreateCertificateAuthority` writes and `DescribeCertificateAuthority` reads,
/// which is what lets a row fix a creation order against ids no call can choose.
fn set_created_at(
    engine: &engine::InMemoryEngine,
    cluster: &str,
    authority: &str,
    created_at: SystemTime,
) {
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let mut guard = store.lock();
    let record = guard
        .clusters
        .get_mut(&(REGION.to_string(), cluster.to_string()))
        .expect("setup: the cluster was created above")
        .certificate_authority_mut(authority)
        .expect("setup: the certificate authority was created above");
    record.created_at = created_at;
}

/// A cluster holding the certificate authority Amazon EKS provisions with it and,
/// unless `only_the_initial`, one appended successor — with their creation instants
/// arranged as `arrangement` asks — and the ids the listing must report, in the
/// order it must report them.
async fn cluster_arranged(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    cluster: &str,
    only_the_initial: bool,
    arrangement: Arrangement,
) -> Vec<String> {
    let created = create_cluster(router, REGION, cluster).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: setup CreateCluster expected 200, got {created:?}"
    );
    settle(CLUSTER_ACTIVATION_DELAY).await;
    let initial = signer_id(router, case, cluster).await;
    if only_the_initial {
        return vec![initial];
    }

    let appended = append_authority(router, REGION, cluster, None).await;
    assert_eq!(
        appended.status, 200,
        "case {case:?}: setup CreateCertificateAuthority expected 200, got {appended:?}"
    );
    let successor = str_member(
        case,
        "certificateAuthority.id",
        &appended.body["certificateAuthority"]["id"],
    );
    settle(CA_DISTRIBUTION_DELAY).await;

    // Which of the two ids sorts first is a property of the ids, not of the order
    // they were created in, so the rows below are stated in terms of it.
    let (smaller, greater) = if successor < initial {
        (successor.clone(), initial.clone())
    } else {
        (initial.clone(), successor.clone())
    };
    // Instants well in the past, so no row depends on how long the setup above took.
    let older = SystemTime::now() - Duration::from_secs(3600);
    let newer = older + Duration::from_secs(600);
    match arrangement {
        Arrangement::AsCreated => vec![successor, initial],
        Arrangement::NewestIsTheGreaterId => {
            set_created_at(engine, cluster, &smaller, older);
            set_created_at(engine, cluster, &greater, newer);
            vec![greater, smaller]
        }
        Arrangement::NewestIsTheSmallerId => {
            set_created_at(engine, cluster, &greater, older);
            set_created_at(engine, cluster, &smaller, newer);
            vec![smaller, greater]
        }
        Arrangement::SameInstant => {
            set_created_at(engine, cluster, &smaller, older);
            set_created_at(engine, cluster, &greater, older);
            vec![smaller, greater]
        }
    }
}

/// The `createdAt` of each summary a listing answer carries, in the order it
/// reported them.
fn listed_creation_instants(case: &str, answer: &Answer) -> Vec<f64> {
    answer.body["certificateAuthorities"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| epoch(case, "certificateAuthorities[].createdAt", &item["createdAt"]))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// RULE: the listing reports a cluster's certificate authorities newest `createdAt`
// first, ties broken by id — never in the id order the records are keyed under.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_reports_the_newest_certificate_authority_first() {
    let (router, engine) = emulator_with_engine();

    struct Case {
        case: &'static str,
        cluster: &'static str,
        only_the_initial: bool,
        arrangement: Arrangement,
    }
    let cases = vec![
        Case {
            case: "only the CA Amazon EKS provisions with the cluster",
            cluster: "ca-order-initial",
            only_the_initial: true,
            arrangement: Arrangement::AsCreated,
        },
        Case {
            case: "an appended successor is the newest CA",
            cluster: "ca-order-appended",
            only_the_initial: false,
            arrangement: Arrangement::AsCreated,
        },
        Case {
            case: "the newest CA is the one whose id sorts last",
            cluster: "ca-order-greater-id-newest",
            only_the_initial: false,
            arrangement: Arrangement::NewestIsTheGreaterId,
        },
        Case {
            case: "the newest CA is the one whose id sorts first",
            cluster: "Ca9_order-Smaller-Id",
            only_the_initial: false,
            arrangement: Arrangement::NewestIsTheSmallerId,
        },
        Case {
            case: "both CAs created at the same instant",
            cluster: "ca-order-tied",
            only_the_initial: false,
            arrangement: Arrangement::SameInstant,
        },
    ];

    // At least one row must put the id that sorts LAST at the head of the listing:
    // without such a row the table could not tell newest-first apart from id order.
    let mut rows_contradicting_id_order = 0;

    for row in &cases {
        let case = row.case;
        let expected = cluster_arranged(
            &router,
            &engine,
            case,
            row.cluster,
            row.only_the_initial,
            row.arrangement,
        )
        .await;

        let answer = list_authorities(&router, row.cluster, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from ListCertificateAuthorities, got {answer:?}"
        );
        let ids = listed_ids(&answer);
        assert_eq!(
            ids, expected,
            "case {case:?}: expected the certificate authorities newest created first \
             {expected:?}, got {ids:?}"
        );
        if ids.len() > 1 && ids.first() > ids.last() {
            rows_contradicting_id_order += 1;
        }

        // The order is the one the reported `createdAt` members themselves state, so
        // a caller reading the head of the listing cannot find an older instant
        // further down it.
        let instants = listed_creation_instants(case, &answer);
        assert!(
            instants.windows(2).all(|pair| pair[0] >= pair[1]),
            "case {case:?}: createdAt must not increase down the listing, got {instants:?}"
        );

        // A listing is a READ: the next caller sees the same order.
        let again = list_authorities(&router, row.cluster, "").await;
        assert_eq!(
            listed_ids(&again),
            ids,
            "case {case:?}: a listing must not change the order the next caller sees, \
             got {again:?}"
        );
    }

    assert!(
        rows_contradicting_id_order > 0,
        "the table must cover a cluster whose newest certificate authority is the id \
         that sorts last, otherwise newest-first and id order are indistinguishable; \
         covered {rows_contradicting_id_order} such rows"
    );
}

// ---------------------------------------------------------------------------
// RULE: `nextToken` continues the newest-first order, so at any page size the
// pages concatenate to the unpaginated listing and only the last page omits the
// token.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_continues_the_newest_first_order() {
    let (router, engine) = emulator_with_engine();

    // Rows differ along the arrangement of creation instants against the ids, so a
    // token that resumed on an id rather than on the order key is caught.
    struct Case {
        case: &'static str,
        cluster: &'static str,
        arrangement: Arrangement,
    }
    let cases = vec![
        Case {
            case: "paging with the greater id newest",
            cluster: "ca-page-greater-id-newest",
            arrangement: Arrangement::NewestIsTheGreaterId,
        },
        Case {
            case: "paging with the smaller id newest",
            cluster: "ca-page-smaller-id-newest",
            arrangement: Arrangement::NewestIsTheSmallerId,
        },
        Case {
            case: "paging two CAs created at the same instant",
            cluster: "ca-page-tied",
            arrangement: Arrangement::SameInstant,
        },
    ];

    for row in &cases {
        let case = row.case;
        let expected =
            cluster_arranged(&router, &engine, case, row.cluster, false, row.arrangement).await;

        // Page sizes: the modelled floor, exactly what the cluster holds, one above
        // it, and the modelled ceiling.
        for page_size in [1, 2, 3, 100] {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let query = match &token {
                    None => format!("maxResults={page_size}"),
                    Some(value) => format!("maxResults={page_size}&nextToken={value}"),
                };
                let answer = list_authorities(&router, row.cluster, &query).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} maxResults={page_size}: expected 200, got {answer:?}"
                );
                let page = listed_ids(&answer);
                assert!(
                    page.len() <= page_size as usize,
                    "case {case:?} maxResults={page_size}: a page must not exceed the \
                     limit, got {page:?}"
                );
                seen.extend(page.clone());
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?} maxResults={page_size}: paging did not terminate after \
                     {pages} pages, seen {seen:?}"
                );
                match answer.body["nextToken"].as_str() {
                    Some(next) => {
                        assert!(
                            !page.is_empty(),
                            "case {case:?} maxResults={page_size}: a token was returned \
                             for an empty page, got {answer:?}"
                        );
                        token = Some(next.to_owned());
                    }
                    None => break,
                }
            }
            assert_eq!(
                seen, expected,
                "case {case:?} maxResults={page_size}: the pages must concatenate to the \
                 newest-first listing {expected:?}, got {seen:?}"
            );
        }
    }
}
