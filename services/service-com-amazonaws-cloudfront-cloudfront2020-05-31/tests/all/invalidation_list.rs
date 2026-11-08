// SPDX-License-Identifier: CC0-1.0

//! `ListInvalidations`: the invalidations persisted under ONE distribution, paged, end to
//! end over HTTP.
//!
//! The rules under test are the ones CloudFront states for this operation:
//!
//! * a list reports exactly the invalidations the stored distribution holds — every
//!   invalidation a `CreateInvalidation` landed under it, and nothing created under
//!   another distribution — each summary carrying the id, the creation time and the
//!   status the SAME record answers a `GetInvalidation` with;
//! * the order is "decreasing order from most recent to oldest", so the most recent
//!   invalidation is the first item of the first page;
//! * `MaxItems` cuts the page (bounded above by the server's own ceiling), `NextMarker`
//!   is present exactly when invalidation batches remain and names the id the page ended
//!   at, and a request resumes after the invalidation its `Marker` names — so walking the
//!   pages at ANY admissible page size enumerates every invalidation of the distribution
//!   exactly once, in that order;
//! * a page size that names no page, a `Marker` that is no resume point this service
//!   handed out, and a distribution that does not exist are refused with the errors the
//!   operation models (`InvalidArgument` 400, `NoSuchDistribution` 404);
//! * a list is a READ: the status it reports is the one the clock owes the batch at that
//!   instant, and listing twice reports the same records.
//!
//! Every row below differs along what those rules generalise over: how many
//! invalidations one distribution holds (none, one, a few, more than a whole page), how
//! many paths each batch names and in which form, the page size requested (absent, one,
//! several, exactly the collection size, past it, past the ceiling), and whether a
//! `Marker` is carried at all.
//!
//! The harness (the router, the request helpers and the element scanner) is the lifecycle
//! suite's, reused rather than copied.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{MAX_ITEMS, PROPAGATION_DELAY};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    batch_xml, call, config, create_distribution, create_invalidation, emulator, get_invalidation,
    is_invalidation_id, Answer, Node, API_VERSION,
};

/// `GET /2020-05-31/distribution/{DistributionId}/invalidation`, with `query` appended
/// verbatim (`?MaxItems=2&Marker=I…`), from the operation's `http` binding.
async fn list_invalidations(
    router: &MultiServiceRouter,
    distribution_id: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{distribution_id}/invalidation{query}"),
        None,
        None,
    )
    .await
}

/// One distribution, created through the API, reported by its id.
async fn distribution(router: &MultiServiceRouter, reference: &str) -> String {
    let row = config("list-invalidations", reference);
    let created = create_distribution(router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {reference}: expected HTTP 201 from CreateDistribution, got {} with {}",
        created.status, created.raw
    );
    created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string()
}

/// Create one invalidation per entry of `batches` under `distribution_id`, in order, and
/// report their ids in CREATION order.
async fn create_batches(
    router: &MultiServiceRouter,
    distribution_id: &str,
    case: &str,
    batches: &[Vec<&str>],
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for (index, paths) in batches.iter().enumerate() {
        let reference = format!("{case}-batch-{index}");
        let answer =
            create_invalidation(router, distribution_id, &batch_xml(&reference, paths)).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201 creating invalidation {index}, got {} with {}",
            answer.status, answer.raw
        );
        let id = answer
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_invalidation_id(&id),
            "case {case}: expected an `I`+13 invalidation id for batch {index}, got {id:?}"
        );
        ids.push(id);
        // Distinct creation instants, so "most recent first" is an order each case can
        // state exactly rather than up to invalidations sharing one instant.
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    ids
}

/// The `InvalidationSummary` elements of an `InvalidationList`.
fn summaries(list: &Node) -> Vec<&Node> {
    list.child("Items")
        .map(|items| items.children_named("InvalidationSummary"))
        .unwrap_or_default()
}

/// The ids an `InvalidationList` page carries, in the order it serves them.
fn summary_ids(list: &Node) -> Vec<String> {
    summaries(list)
        .iter()
        .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
        .collect()
}

/// Rows differing in how many invalidations one distribution holds and in the batches
/// they name (one wildcard, one object, several objects, a deep path).
fn rows() -> Vec<(&'static str, Vec<Vec<&'static str>>)> {
    vec![
        ("no-invalidations", vec![]),
        ("one-wildcard-batch", vec![vec!["/*"]]),
        (
            "two-batches",
            vec![vec!["/index.html"], vec!["/a.png", "/b.png"]],
        ),
        (
            "five-batches",
            vec![
                vec!["/*"],
                vec!["/index.html"],
                vec!["/css/site.css", "/js/app.js"],
                vec!["/images/a.png", "/images/b.png", "/images/c.png"],
                vec!["/deep/nested/path/to/object.json"],
            ],
        ),
    ]
}

/// A list reports exactly the invalidations the distribution holds, most recent first,
/// and each summary carries what the same record answers a `GetInvalidation` with.
#[tokio::test]
async fn a_list_reports_the_stored_invalidations_most_recent_first() {
    let router = emulator();

    for (case, batches) in rows() {
        let id = distribution(&router, &format!("ref-{case}")).await;
        let created = create_batches(&router, &id, case, &batches).await;
        // "the results are returned in decreasing order from most recent to oldest"
        let expected: Vec<String> = created.iter().rev().cloned().collect();

        let answer = list_invalidations(&router, &id, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        assert!(
            answer.raw.contains("<InvalidationList"),
            "case {case}: expected an <InvalidationList> payload, got {}",
            answer.raw
        );
        let list = answer.root();
        let listed = summary_ids(list);
        assert_eq!(
            listed, expected,
            "case {case}: expected the invalidations most recent first ({expected:?}), \
             got {listed:?}"
        );
        assert_eq!(
            list.text_at("Quantity"),
            Some(listed.len().to_string().as_str()),
            "case {case}: expected Quantity to equal the {} items served, got {:?}",
            listed.len(),
            list.text_at("Quantity")
        );
        // "The value that you provided for the Marker request parameter" — none here.
        assert_eq!(
            list.text_at("Marker"),
            Some(""),
            "case {case}: expected an empty Marker, got {:?}",
            list.text_at("Marker")
        );
        // A request naming no `MaxItems` is served at the service's own page size.
        assert_eq!(
            list.text_at("MaxItems"),
            Some(MAX_ITEMS.to_string().as_str()),
            "case {case}: expected MaxItems {MAX_ITEMS}, got {:?}",
            list.text_at("MaxItems")
        );
        assert_eq!(
            list.text_at("IsTruncated"),
            Some("false"),
            "case {case}: a page holding the whole collection is not truncated, got {:?}",
            list.text_at("IsTruncated")
        );
        assert_eq!(
            list.text_at("NextMarker"),
            None,
            "case {case}: expected no NextMarker on the last page, got {:?}",
            list.text_at("NextMarker")
        );

        // Every summary is a projection of the record `GetInvalidation` answers from.
        for summary in summaries(list) {
            let invalidation_id = summary.text_at("Id").unwrap_or_default().to_string();
            let fetched = get_invalidation(&router, &id, &invalidation_id).await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: expected HTTP 200 reading {invalidation_id}, got {} with {}",
                fetched.status, fetched.raw
            );
            let read = fetched.root();
            assert_eq!(
                summary.text_at("Status"),
                read.text_at("Status"),
                "case {case}: summary {invalidation_id} must report the status the record \
                 reports ({:?}), got {:?}",
                read.text_at("Status"),
                summary.text_at("Status")
            );
            assert_eq!(
                summary.text_at("CreateTime"),
                read.text_at("CreateTime"),
                "case {case}: summary {invalidation_id} must report the creation time the \
                 record reports ({:?}), got {:?}",
                read.text_at("CreateTime"),
                summary.text_at("CreateTime")
            );
        }
    }
}

/// Walking the pages at any admissible page size enumerates every invalidation of the
/// distribution exactly once, most recent first.
#[tokio::test]
async fn walking_the_pages_enumerates_every_invalidation_once() {
    let router = emulator();
    let id = distribution(&router, "ref-pagination").await;
    let batches: Vec<Vec<&str>> = (0..7)
        .map(|index| match index % 3 {
            0 => vec!["/*"],
            1 => vec!["/index.html", "/about.html"],
            _ => vec!["/images/one.png", "/images/two.png", "/images/three.png"],
        })
        .collect();
    let created = create_batches(&router, &id, "pagination", &batches).await;
    let expected: Vec<String> = created.iter().rev().cloned().collect();

    // Page sizes spanning what the rule generalises over: none at all, one at a time,
    // two, exactly the collection size, one past it, and the server's own ceiling.
    for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(8), Some(100)] {
        let label = page_size
            .map(|size| format!("MaxItems={size}"))
            .unwrap_or_else(|| "no-MaxItems".to_string());
        let mut marker: Option<String> = None;
        let mut collected: Vec<String> = Vec::new();

        for page in 0.. {
            let mut query = String::new();
            if let Some(size) = page_size {
                query.push_str(&format!("?MaxItems={size}"));
            }
            if let Some(resume) = marker.as_deref() {
                query.push_str(if query.is_empty() { "?" } else { "&" });
                query.push_str(&format!("Marker={resume}"));
            }
            let answer = list_invalidations(&router, &id, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {label} page {page}: expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let list = answer.root();
            assert_eq!(
                list.text_at("Marker"),
                Some(marker.as_deref().unwrap_or_default()),
                "case {label} page {page}: expected the Marker the request carried, got {:?}",
                list.text_at("Marker")
            );
            assert_eq!(
                list.text_at("MaxItems"),
                Some(page_size.unwrap_or(MAX_ITEMS).to_string().as_str()),
                "case {label} page {page}: expected MaxItems to echo the page size, got {:?}",
                list.text_at("MaxItems")
            );
            let ids = summary_ids(list);
            assert_eq!(
                list.text_at("Quantity"),
                Some(ids.len().to_string().as_str()),
                "case {label} page {page}: expected Quantity to equal the {} items served, \
                 got {:?}",
                ids.len(),
                list.text_at("Quantity")
            );
            if let Some(size) = page_size {
                assert!(
                    i32::try_from(ids.len()).unwrap_or(i32::MAX) <= size,
                    "case {label} page {page}: a page holds at most {size} summaries, got {}",
                    ids.len()
                );
            }
            collected.extend(ids);

            let next_marker = list.text_at("NextMarker").map(str::to_string);
            let is_truncated = list.text_at("IsTruncated").unwrap_or_default();
            match &next_marker {
                Some(next) => {
                    assert_eq!(
                        is_truncated, "true",
                        "case {label} page {page}: a NextMarker must come with IsTruncated \
                         true, got {is_truncated:?}"
                    );
                    assert_eq!(
                        Some(next.as_str()),
                        collected.last().map(String::as_str),
                        "case {label} page {page}: NextMarker must name the id this page \
                         ended at ({:?}), got {next:?}",
                        collected.last()
                    );
                    marker = next_marker;
                }
                None => {
                    assert_eq!(
                        is_truncated, "false",
                        "case {label} page {page}: no NextMarker means IsTruncated false, \
                         got {is_truncated:?}"
                    );
                    break;
                }
            }
        }

        assert_eq!(
            collected, expected,
            "case {label}: walking the pages must enumerate every invalidation exactly \
             once, most recent first ({expected:?}), got {collected:?}"
        );
    }
}

/// A page is bounded by the SERVER's own ceiling, whatever `MaxItems` a caller asks for:
/// one request can never serve every invalidation of a distribution, and the rest is
/// paged with `Marker`.
#[tokio::test]
async fn a_page_is_bounded_by_the_servers_own_ceiling() {
    let router = emulator();
    let id = distribution(&router, "ref-ceiling").await;
    // One more invalidation than the ceiling, so a request asking for far more than the
    // ceiling still has to truncate.
    let total = usize::try_from(MAX_ITEMS).expect("a positive ceiling") + 1;
    let mut created: BTreeSet<String> = BTreeSet::new();
    for index in 0..total {
        let reference = format!("ceiling-{index:04}");
        let answer = create_invalidation(
            &router,
            &id,
            &batch_xml(&reference, &[&format!("/{index:04}/object.html")]),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case ceiling-{index}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        let invalidation_id = answer
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            created.insert(invalidation_id.clone()),
            "case ceiling-{index}: invalidation id {invalidation_id} was already allocated"
        );
    }

    for asked in [MAX_ITEMS + 1, 1_000, i32::MAX] {
        let answer = list_invalidations(&router, &id, &format!("?MaxItems={asked}")).await;
        assert_eq!(
            answer.status, 200,
            "case MaxItems={asked}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let list = answer.root();
        let ids = summary_ids(list);
        assert_eq!(
            i32::try_from(ids.len()).unwrap_or(i32::MAX),
            MAX_ITEMS,
            "case MaxItems={asked}: expected a page bounded at {MAX_ITEMS}, got {}",
            ids.len()
        );
        // "The value that you provided for the MaxItems request parameter."
        assert_eq!(
            list.text_at("MaxItems"),
            Some(asked.to_string().as_str()),
            "case MaxItems={asked}: expected MaxItems to echo the request, got {:?}",
            list.text_at("MaxItems")
        );
        assert_eq!(
            list.text_at("IsTruncated"),
            Some("true"),
            "case MaxItems={asked}: more invalidations remain, so the page is truncated, \
             got {:?}",
            list.text_at("IsTruncated")
        );
        let next = list
            .text_at("NextMarker")
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            Some(next.as_str()),
            ids.last().map(String::as_str),
            "case MaxItems={asked}: NextMarker must name the id the page ended at ({:?}), \
             got {next:?}",
            ids.last()
        );

        // The rest of the collection, resumed after that id: the two pages together are
        // every invalidation created, each exactly once.
        let rest = list_invalidations(&router, &id, &format!("?Marker={next}")).await;
        assert_eq!(
            rest.status, 200,
            "case MaxItems={asked}: expected HTTP 200 resuming at {next}, got {} with {}",
            rest.status, rest.raw
        );
        let rest_ids = summary_ids(rest.root());
        assert_eq!(
            rest_ids.len(),
            total - ids.len(),
            "case MaxItems={asked}: expected the remaining {} invalidations, got {}",
            total - ids.len(),
            rest_ids.len()
        );
        let walked: BTreeSet<String> = ids.into_iter().chain(rest_ids).collect();
        assert_eq!(
            walked, created,
            "case MaxItems={asked}: the pages together must enumerate every invalidation \
             exactly once"
        );
    }
}

/// A list is scoped to the distribution the request names: an invalidation created under
/// another distribution is not listed, and is no resume point for this one.
#[tokio::test]
async fn a_list_is_scoped_to_one_distribution() {
    let router = emulator();
    let first = distribution(&router, "ref-scope-first").await;
    let second = distribution(&router, "ref-scope-second").await;
    let first_ids = create_batches(
        &router,
        &first,
        "scope-first",
        &[vec!["/*"], vec!["/index.html"]],
    )
    .await;
    let second_ids =
        create_batches(&router, &second, "scope-second", &[vec!["/only.html"]]).await;

    for (case, id, own, foreign) in [
        ("first", &first, &first_ids, &second_ids),
        ("second", &second, &second_ids, &first_ids),
    ] {
        let listed = summary_ids(list_invalidations(&router, id, "").await.root());
        let expected: Vec<String> = own.iter().rev().cloned().collect();
        assert_eq!(
            listed, expected,
            "case {case}: expected only this distribution's invalidations ({expected:?}), \
             got {listed:?}"
        );
        // A marker naming another distribution's invalidation was never a resume point
        // for this list.
        let marker = foreign.first().expect("each distribution has an invalidation");
        let answer = list_invalidations(&router, id, &format!("?Marker={marker}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for a foreign marker, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for a foreign marker, got {:?}",
            answer.error_code()
        );
    }
}

/// The refusals the operation models: a page size that names no page, a `Marker` that is
/// no resume point, and a distribution that does not exist.
#[tokio::test]
async fn unusable_arguments_and_unknown_distributions_are_refused() {
    let router = emulator();
    let id = distribution(&router, "ref-refusals").await;
    create_batches(&router, &id, "refusals", &[vec!["/*"]]).await;

    // Each row: the query, what it is, and the modelled refusal it earns.
    let refusals: &[(&str, &str)] = &[
        ("?MaxItems=-3", "a negative page size"),
        ("?MaxItems=not-a-number", "a page size that is no integer"),
        ("?Marker=not-an-id", "a marker that is no invalidation id"),
        (
            "?Marker=IZZZZZZZZZZZZZ",
            "a marker shaped like an invalidation id this service never issued",
        ),
        (
            "?MaxItems=2&Marker=not-an-id",
            "an unknown marker beside a usable page size",
        ),
    ];
    for (query, description) in refusals {
        let answer = list_invalidations(&router, &id, query).await;
        assert_eq!(
            answer.status, 400,
            "case {description}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {description}: expected InvalidArgument, got {:?}",
            answer.error_code()
        );
    }

    // A distribution id of the right shape that no create ever handed out.
    for (case, unknown) in [
        ("never-created", "E1111111111111"),
        ("not-an-id-at-all", "nope"),
    ] {
        let answer = list_invalidations(&router, unknown, "").await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchDistribution",
            "case {case}: expected NoSuchDistribution, got {:?}",
            answer.error_code()
        );
    }
}

/// A list REPORTS: the status each summary carries is the one the clock owes that batch
/// at the instant of the call, and listing twice reports the same records.
#[tokio::test]
async fn a_list_reports_the_status_the_clock_owes_and_changes_nothing() {
    let router = emulator();
    let id = distribution(&router, "ref-status").await;
    let created = create_batches(
        &router,
        &id,
        "status",
        &[vec!["/*"], vec!["/a.html", "/b.html"]],
    )
    .await;

    // A batch CloudFront has just accepted has not finished yet.
    let fresh = list_invalidations(&router, &id, "").await;
    for summary in summaries(fresh.root()) {
        assert_eq!(
            summary.text_at("Status"),
            Some("InProgress"),
            "case just-created: expected a just-created invalidation to be InProgress, got {:?}",
            summary.text_at("Status")
        );
    }

    // Past the batch's deadline the clock owes it the terminal status, and the read is
    // what materialises it — for the list and for the record alike.
    tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(250)).await;
    let settled = list_invalidations(&router, &id, "").await;
    let first_pass = summary_ids(settled.root());
    for summary in summaries(settled.root()) {
        assert_eq!(
            summary.text_at("Status"),
            Some("Completed"),
            "case settled: expected a finished invalidation to be Completed, got {:?}",
            summary.text_at("Status")
        );
    }
    for invalidation_id in &created {
        let read = get_invalidation(&router, &id, invalidation_id).await;
        assert_eq!(
            read.root().text_at("Status"),
            Some("Completed"),
            "case settled: expected {invalidation_id} to read back Completed, got {:?}",
            read.root().text_at("Status")
        );
    }

    // The list changed nothing a later caller sees.
    let again = list_invalidations(&router, &id, "").await;
    let second_pass = summary_ids(again.root());
    assert_eq!(
        second_pass, first_pass,
        "case repeated: listing twice must report the same invalidations in the same \
         order ({first_pass:?}), got {second_pass:?}"
    );
}
