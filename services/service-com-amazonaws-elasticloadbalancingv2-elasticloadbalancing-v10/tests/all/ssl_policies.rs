// SPDX-License-Identifier: CC0-1.0

//! Paging the `DescribeSSLPolicies` catalogue, and what a read of it may not change.
//!
//! The predefined security policies are a catalogue the SERVICE publishes: no operation
//! of this model creates, changes or deletes one, so the rule under test here is that
//! `DescribeSSLPolicies` is a REPORT over one stable order —
//!
//! * pages of ANY page size concatenate to the unpaged order, each `NextMarker`
//!   resuming exactly where the previous page ended;
//! * paging a FILTERED read pages that filtered collection, in the order the filter
//!   produced, not the whole catalogue;
//! * a `Marker` that is not a position in the collection is refused rather than
//!   silently treated as the first page;
//! * and the catalogue is the service's own, so it is identical on a second emulator
//!   over a different engine and after any number of reads.
//!
//! This suite complements `all/catalog_reads.rs`, which asserts the CONTENT of the
//! catalogue (every member of every policy, the filters and the modelled refusal) and
//! walks it at page size 1; the readers and the request helper are that suite's, reused
//! here rather than written twice. Every request goes through the router exactly as a
//! client's would — awsQuery form body in, XML out.

use crate::catalog_reads::{describe, reported_policy_names};
use crate::provisioning_loop::{emulator, p, Reply};
use smithy_server_runtime::MultiServiceRouter;

/// The policy names one `DescribeSSLPolicies` call with `fields` reports, asserting it
/// was served.
async fn policies(router: &MultiServiceRouter, case: &str, fields: &[(String, String)]) -> Vec<String> {
    let reply: Reply = describe(router, "DescribeSSLPolicies", fields).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reported_policy_names(&reply)
}

/// Walk `fields` with `PageSize` pages, answering the names in the order the pages
/// reported them.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    fields: &[(String, String)],
    page_size: usize,
    bound: usize,
) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut paged = fields.to_vec();
        paged.push(p("PageSize", &page_size.to_string()));
        if let Some(marker) = marker.as_deref() {
            paged.push(p("Marker", marker));
        }
        let page = describe(router, "DescribeSSLPolicies", &paged).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            page.status, page.body
        );
        let reported = reported_policy_names(&page);
        assert!(
            reported.len() <= page_size,
            "case {case:?}: a page holds at most {page_size} policies, got {reported:?}"
        );
        walked.extend(reported);
        pages += 1;
        assert!(
            pages <= bound + 1,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        match page.text("NextMarker") {
            Some(next) => marker = Some(next),
            None => break,
        }
    }
    walked
}

#[tokio::test]
async fn pages_of_any_size_concatenate_to_the_unpaged_catalogue_order() {
    let (router, _engine) = emulator();
    let whole = policies(&router, "the whole catalogue", &[]).await;
    assert!(
        whole.len() > 1,
        "case the whole catalogue: expected the predefined policies, got {whole:?}"
    );

    // The page sizes differ in how they divide the catalogue: one per page, several
    // whole pages with a remainder, exactly one page, and the model's published
    // maximum — the rule holds for each.
    let sizes = [
        2usize,
        3,
        5,
        whole.len() - 1,
        whole.len(),
        whole.len() + 1,
        400,
    ];
    for page_size in sizes {
        let case = format!("page size {page_size}");
        let walked = walk(&router, &case, &[], page_size, whole.len()).await;
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the unpaged order, got {walked:?}"
        );
    }

    // A page size that holds the whole catalogue leaves nothing to resume from.
    let one_page = describe(
        &router,
        "DescribeSSLPolicies",
        &[p("PageSize", &whole.len().to_string())],
    )
    .await;
    assert_eq!(
        one_page.text("NextMarker"),
        None,
        "case one full page: nothing follows the last page, got {:?}",
        one_page.text("NextMarker")
    );
}

#[tokio::test]
async fn paging_a_filtered_read_pages_that_collection_in_the_order_it_was_filtered() {
    let (router, _engine) = emulator();
    let whole = policies(&router, "the whole catalogue", &[]).await;

    // `Names` reports the policies in the order the request listed them, so a filtered
    // walk must reproduce THAT order rather than the catalogue's. The rows differ in how
    // many names they list and in how the list is ordered against the catalogue.
    let reversed: Vec<String> = whole.iter().rev().cloned().collect();
    let every_other: Vec<String> = whole.iter().step_by(2).cloned().collect();
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("two names", whole.iter().take(2).cloned().collect()),
        ("five names", whole.iter().take(5).cloned().collect()),
        ("every other name", every_other),
        ("the whole catalogue reversed", reversed),
    ];

    for (case, names) in rows {
        let fields: Vec<(String, String)> = names
            .iter()
            .enumerate()
            .map(|(index, name)| p(&format!("Names.member.{}", index + 1), name))
            .collect();
        let unpaged = policies(&router, case, &fields).await;
        assert_eq!(
            unpaged, names,
            "case {case:?}: expected the names as listed, got {unpaged:?}"
        );
        for page_size in [1usize, 2, 3] {
            let paged_case = format!("{case} at page size {page_size}");
            let walked = walk(&router, &paged_case, &fields, page_size, names.len()).await;
            assert_eq!(
                walked, unpaged,
                "case {paged_case:?}: expected the unpaged filtered order, got {walked:?}"
            );
        }
    }

    // An empty collection — no predefined policy has a Gateway Load Balancer listener to
    // be set on — pages to nothing rather than to the whole catalogue.
    let gateway = vec![p("LoadBalancerType", "gateway")];
    let walked = walk(&router, "gateway at page size 1", &gateway, 1, whole.len()).await;
    assert_eq!(
        walked,
        Vec::<String>::new(),
        "case gateway: expected no policy at all, got {walked:?}"
    );
}

#[tokio::test]
async fn a_marker_that_is_not_a_position_in_the_catalogue_is_refused() {
    let (router, _engine) = emulator();
    let whole = policies(&router, "the whole catalogue", &[]).await;

    for (case, marker) in [
        ("a marker that is not a number", "not-a-marker".to_string()),
        ("a negative marker", "-1".to_string()),
        ("a fractional marker", "1.5".to_string()),
        ("a marker past the end", (whole.len() + 1).to_string()),
        ("a marker far past the end", "9999".to_string()),
    ] {
        let refused = describe(&router, "DescribeSSLPolicies", &[p("Marker", &marker)]).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} body {}",
            refused.status,
            refused.body
        );
        assert!(
            !refused.body.contains("<SslPolicies"),
            "case {case:?}: a refusal reports no policy, got {}",
            refused.body
        );
    }

    // The marker that ends the collection is a position in it, so it is served — with
    // the empty tail after the last policy.
    let at_the_end = describe(
        &router,
        "DescribeSSLPolicies",
        &[p("Marker", &whole.len().to_string())],
    )
    .await;
    assert_eq!(
        at_the_end.status, 200,
        "case the marker at the end: expected 200, got {} body {}",
        at_the_end.status, at_the_end.body
    );
    assert_eq!(
        reported_policy_names(&at_the_end),
        Vec::<String>::new(),
        "case the marker at the end: nothing follows the last policy, got {}",
        at_the_end.body
    );
}

#[tokio::test]
async fn the_catalogue_is_the_services_own_and_a_read_never_changes_it() {
    let (router, _engine) = emulator();
    let whole = policies(&router, "the whole catalogue", &[]).await;

    // Reads of every shape — unfiltered, by name, by type, paged — and then the whole
    // catalogue again: a report may not change what a later caller sees.
    let probes: Vec<(&str, Vec<(String, String)>)> = vec![
        ("unfiltered", vec![]),
        ("by name", vec![p("Names.member.1", &whole[0])]),
        ("by type", vec![p("LoadBalancerType", "network")]),
        ("paged", vec![p("PageSize", "1")]),
        ("at a marker", vec![p("Marker", "1")]),
    ];
    for (case, fields) in &probes {
        let _ = policies(&router, case, fields).await;
    }
    let after = policies(&router, "after every probe", &[]).await;
    assert_eq!(
        after, whole,
        "case after every probe: expected the same catalogue, got {after:?}"
    );

    // A second emulator over its OWN engine publishes the same catalogue: it is the
    // service's, not something an engine accumulates.
    let (other, _other_engine) = emulator();
    let elsewhere = policies(&other, "a second emulator", &[]).await;
    assert_eq!(
        elsewhere, whole,
        "case a second emulator: expected the same catalogue, got {elsewhere:?}"
    );
}
