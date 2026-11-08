// SPDX-License-Identifier: CC0-1.0

//! The invalidations of a distribution TENANT, end to end over HTTP:
//! `CreateInvalidationForDistributionTenant`, `GetInvalidationForDistributionTenant` and
//! `ListInvalidationsForDistributionTenant`.
//!
//! The rules under test are the ones CloudFront states for an invalidation listing, read
//! against the tenant the request names:
//!
//! * a list reports exactly the invalidations the stored TENANT holds — every invalidation
//!   a create landed under it, and nothing created under another tenant — each summary
//!   carrying the id, the creation time and the status the SAME record answers a read with;
//! * the order is "decreasing order from most recent to oldest", so the most recent
//!   invalidation is the first item of the first page;
//! * `MaxItems` cuts the page, `NextMarker` is present exactly when invalidation batches
//!   remain and names the id the page ended at, and a request resumes after the
//!   invalidation its `Marker` names — so walking the pages at ANY admissible page size
//!   enumerates every invalidation of the tenant exactly once, in that order;
//! * a page size that names no page, a `Marker` that is no resume point this service handed
//!   out, and a tenant or an invalidation that does not exist are refused with the errors the
//!   operations model (`InvalidArgument` 400, `EntityNotFound` 404);
//! * a batch over the published per-request object quota is refused with
//!   `TooManyInvalidationsInProgress` (400) and persists nothing, so the listing after it
//!   is the listing before it;
//! * a repeated `CallerReference` carrying the same batch returns the invalidation already
//!   created rather than a second one;
//! * a list and a read are READS: the status they report is the one the clock owes the
//!   batch at that instant, and listing twice reports the same records.
//!
//! Every row below differs along what those rules generalise over: how many invalidations
//! one tenant holds (none, one, a few, more than a whole page), how many paths each batch
//! names and in which form, the page size requested (absent, one, several, exactly the
//! collection size, past it), whether a `Marker` is carried at all, and — for the quota —
//! the published bound itself and one past it.
//!
//! The harness (the router, the request helpers and the element scanner) is the
//! distribution lifecycle suite's, reused rather than copied; the multi-tenant
//! distribution and the tenant create go through the tenant lifecycle suite's helpers.

use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    id_format_invalid_message, MAX_INVALIDATION_PATHS, MAX_ITEMS, PROPAGATION_DELAY,
    TOO_MANY_INVALIDATIONS_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    batch_xml, call, emulator, is_invalidation_id, Answer, Node, API_VERSION,
};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, managed_certificate_request, multi_tenant_distribution,
};

// ---------------------------------------------------------------------------
// Calling the tenant invalidation surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/distribution-tenant/{Id}/invalidation`, from the operation's `http`
/// binding.
///
/// `pub(crate)` so a sibling suite calls the tenant invalidation surface through these
/// helpers rather than through a second copy of them.
pub(crate) async fn create_invalidation_for_tenant(
    router: &MultiServiceRouter,
    tenant_id: &str,
    batch: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution-tenant/{tenant_id}/invalidation"),
        Some(batch),
        None,
    )
    .await
}

/// `GET /2020-05-31/distribution-tenant/{DistributionTenantId}/invalidation/{Id}`.
pub(crate) async fn get_invalidation_for_tenant(
    router: &MultiServiceRouter,
    tenant_id: &str,
    invalidation_id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/{API_VERSION}/distribution-tenant/{tenant_id}/invalidation/{invalidation_id}"
        ),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/distribution-tenant/{Id}/invalidation`, with `query` appended verbatim
/// (`?MaxItems=2&Marker=I…`).
pub(crate) async fn list_invalidations_for_tenant(
    router: &MultiServiceRouter,
    tenant_id: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution-tenant/{tenant_id}/invalidation{query}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The tenants the invalidations are created under
// ---------------------------------------------------------------------------

/// One distribution tenant, created through the API under a multi-tenant distribution, and
/// reported by its id.
///
/// `tenantName` is the parameter the multi-tenant distribution under test marks required,
/// so every create supplies a value for it.
async fn tenant(router: &MultiServiceRouter, distribution_id: &str, name: &str) -> String {
    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{distribution_id}</DistributionId><Name>{name}</Name>\
         <Domains><member><Domain>{name}.example.com</Domain></member></Domains>\
         <Parameters><member><Name>tenantName</Name><Value>{name}</Value></member>\
         </Parameters>{}</CreateDistributionTenantRequest>",
        managed_certificate_request()
    );
    let created = create_distribution_tenant(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {name}: expected HTTP 201 from CreateDistributionTenant, got {} with {}",
        created.status, created.raw
    );
    created
        .root()
        .find_text("Id")
        .unwrap_or_else(|| panic!("case {name}: the create carries no Id: {}", created.raw))
        .to_string()
}

/// One multi-tenant distribution with one tenant under it, reported as the tenant's id.
pub(crate) async fn distribution_with_tenant(router: &MultiServiceRouter, case: &str) -> String {
    let distribution = multi_tenant_distribution(router, &format!("ref-{case}")).await;
    tenant(router, &distribution, case).await
}

/// Create one invalidation per entry of `batches` under `tenant_id`, in order, and report
/// their ids in CREATION order.
async fn create_batches(
    router: &MultiServiceRouter,
    tenant_id: &str,
    case: &str,
    batches: &[Vec<&str>],
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for (index, paths) in batches.iter().enumerate() {
        let reference = format!("{case}-batch-{index}");
        let answer =
            create_invalidation_for_tenant(router, tenant_id, &batch_xml(&reference, paths)).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201 creating invalidation {index}, got {} with {}",
            answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
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
pub(crate) fn summaries(list: &Node) -> Vec<&Node> {
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

/// The paths an `Invalidation` payload reports its batch naming, in document order.
pub(crate) fn batch_paths(invalidation: &Node) -> Vec<String> {
    invalidation
        .at("InvalidationBatch/Paths/Items")
        .map(|items| {
            items
                .children_named("Path")
                .iter()
                .map(|path| path.text().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Rows differing in how many invalidations one tenant holds and in the batches they name
/// (one wildcard, one object, several objects, a deep path).
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

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create persists the batch under the tenant and mints the identity the answer carries,
/// and a read answers from that record.
#[tokio::test]
async fn a_tenant_invalidation_is_persisted_and_read_back() {
    let router = emulator();

    for (case, batches) in rows() {
        let tenant_id = distribution_with_tenant(&router, case).await;
        for (index, paths) in batches.iter().enumerate() {
            let reference = format!("{case}-roundtrip-{index}");
            let created =
                create_invalidation_for_tenant(&router, &tenant_id, &batch_xml(&reference, paths))
                    .await;
            assert_eq!(
                created.status, 201,
                "case {case}/{index}: expected HTTP 201, got {} with {}",
                created.status, created.raw
            );
            let answer = created.root();
            let id = answer.text_at("Id").unwrap_or_default().to_string();
            assert!(
                is_invalidation_id(&id),
                "case {case}/{index}: expected an `I`+13 invalidation id, got {id:?}"
            );
            // "The location for the invalidation": the URI the tenant's invalidation is
            // addressed by.
            let location = created.header("location");
            assert!(
                location.ends_with(&format!(
                    "/{API_VERSION}/distribution-tenant/{tenant_id}/invalidation/{id}"
                )),
                "case {case}/{index}: expected the Location to address the new invalidation, \
                 got {location:?}"
            );
            // A batch CloudFront has just accepted has not finished yet.
            assert_eq!(
                answer.text_at("Status"),
                Some("InProgress"),
                "case {case}/{index}: expected InProgress, got {:?}",
                answer.text_at("Status")
            );
            let submitted: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
            assert_eq!(
                batch_paths(answer),
                submitted,
                "case {case}/{index}: expected the submitted paths echoed from storage"
            );

            // The record a read answers from is the record the create stored.
            let read = get_invalidation_for_tenant(&router, &tenant_id, &id).await;
            assert_eq!(
                read.status, 200,
                "case {case}/{index}: expected HTTP 200 reading {id}, got {} with {}",
                read.status, read.raw
            );
            let stored = read.root();
            assert_eq!(
                stored.text_at("Id"),
                Some(id.as_str()),
                "case {case}/{index}: expected the read to answer about {id}, got {:?}",
                stored.text_at("Id")
            );
            assert_eq!(
                batch_paths(stored),
                submitted,
                "case {case}/{index}: expected the read to report the stored batch"
            );
            assert_eq!(
                stored.text_at("InvalidationBatch/CallerReference"),
                Some(reference.as_str()),
                "case {case}/{index}: expected the submitted CallerReference, got {:?}",
                stored.text_at("InvalidationBatch/CallerReference")
            );
            assert_eq!(
                stored.text_at("CreateTime"),
                answer.text_at("CreateTime"),
                "case {case}/{index}: expected the creation time the create reported ({:?}), \
                 got {:?}",
                answer.text_at("CreateTime"),
                stored.text_at("CreateTime")
            );
        }
    }
}

/// A list reports exactly the invalidations the tenant holds, most recent first, and each
/// summary carries what the same record answers a read with.
#[tokio::test]
async fn a_list_reports_the_stored_invalidations_most_recent_first() {
    let router = emulator();

    for (case, batches) in rows() {
        let tenant_id = distribution_with_tenant(&router, case).await;
        let created = create_batches(&router, &tenant_id, case, &batches).await;
        // "the results are returned in decreasing order from most recent to oldest"
        let expected: Vec<String> = created.iter().rev().cloned().collect();

        let answer = list_invalidations_for_tenant(&router, &tenant_id, "").await;
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

        // Every summary is a projection of the record a read answers from.
        for summary in summaries(list) {
            let invalidation_id = summary.text_at("Id").unwrap_or_default().to_string();
            let fetched = get_invalidation_for_tenant(&router, &tenant_id, &invalidation_id).await;
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
/// tenant exactly once, most recent first.
#[tokio::test]
async fn walking_the_pages_enumerates_every_invalidation_once() {
    let router = emulator();
    let tenant_id = distribution_with_tenant(&router, "pagination").await;
    let batches: Vec<Vec<&str>> = (0..7)
        .map(|index| match index % 3 {
            0 => vec!["/*"],
            1 => vec!["/index.html", "/about.html"],
            _ => vec!["/images/one.png", "/images/two.png", "/images/three.png"],
        })
        .collect();
    let created = create_batches(&router, &tenant_id, "pagination", &batches).await;
    let expected: Vec<String> = created.iter().rev().cloned().collect();

    // Page sizes spanning what the rule generalises over: none at all, one at a time, two,
    // exactly the collection size, one past it, and the server's own ceiling.
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
            let answer = list_invalidations_for_tenant(&router, &tenant_id, &query).await;
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
            "case {label}: walking the pages must enumerate every invalidation exactly once, \
             most recent first ({expected:?}), got {collected:?}"
        );
    }
}

/// A list and a read are scoped to the tenant the request names: an invalidation created
/// under another tenant of the SAME distribution is not listed, is no resume point for this
/// one, and is no invalidation of it.
#[tokio::test]
async fn a_list_is_scoped_to_one_tenant() {
    let router = emulator();
    let distribution = multi_tenant_distribution(&router, "ref-scope").await;
    let first = tenant(&router, &distribution, "scope-first").await;
    let second = tenant(&router, &distribution, "scope-second").await;
    let first_ids =
        create_batches(&router, &first, "scope-first", &[vec!["/*"], vec!["/index.html"]]).await;
    let second_ids = create_batches(&router, &second, "scope-second", &[vec!["/only.html"]]).await;

    for (case, tenant_id, own, foreign) in [
        ("first", &first, &first_ids, &second_ids),
        ("second", &second, &second_ids, &first_ids),
    ] {
        let listed = summary_ids(
            list_invalidations_for_tenant(&router, tenant_id, "")
                .await
                .root(),
        );
        let expected: Vec<String> = own.iter().rev().cloned().collect();
        assert_eq!(
            listed, expected,
            "case {case}: expected only this tenant's invalidations ({expected:?}), \
             got {listed:?}"
        );

        let marker = foreign.first().expect("each tenant has an invalidation");
        // A marker naming another tenant's invalidation was never a resume point for this
        // list.
        let answer = list_invalidations_for_tenant(&router, tenant_id, &format!("?Marker={marker}"))
            .await;
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

        // Nor is it an invalidation of this tenant.
        let read = get_invalidation_for_tenant(&router, tenant_id, marker).await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected HTTP 404 reading a foreign invalidation, got {} with {}",
            read.status, read.raw
        );
        assert_eq!(
            read.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {:?}",
            read.error_code()
        );
    }
}

/// The refusals these operations model: a page size that names no page, a `Marker` that is
/// no resume point, and a distribution tenant that does not exist.
#[tokio::test]
async fn unusable_arguments_and_unknown_tenants_are_refused() {
    let router = emulator();
    let tenant_id = distribution_with_tenant(&router, "refusals").await;
    create_batches(&router, &tenant_id, "refusals", &[vec!["/*"]]).await;

    // Each row: the query, and what it is.
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
        let answer = list_invalidations_for_tenant(&router, &tenant_id, query).await;
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

    // A tenant id of the right shape that no create ever handed out, and a value that is
    // no tenant identifier at all. The first names no tenant. The second was never a tenant
    // id: the two operations whose model serves `InvalidArgument` report it as the argument
    // failure it is, while the READ — whose model serves none — reports it under its own
    // not-found code with the sentence that names the member the identifier arrived in.
    let tenant_id_format_invalid = id_format_invalid_message("DistributionTenantId");
    for (case, unknown, well_formed) in [
        ("never-created", "dt_1111111111111", true),
        ("not-an-id-at-all", "nope", false),
    ] {
        for (operation, answer) in [
            ("list", list_invalidations_for_tenant(&router, unknown, "").await),
            (
                "create",
                create_invalidation_for_tenant(&router, unknown, &batch_xml("unknown", &["/*"]))
                    .await,
            ),
            (
                "read",
                get_invalidation_for_tenant(&router, unknown, "I1111111111111").await,
            ),
        ] {
            let (status, code) = if well_formed || operation == "read" {
                (404, "EntityNotFound")
            } else {
                (400, "InvalidArgument")
            };
            assert_eq!(
                answer.status, status,
                "case {case}/{operation}: expected HTTP {status}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case}/{operation}: expected {code}, got {:?}",
                answer.error_code()
            );
            if !well_formed && operation == "read" {
                let message = answer.root().find_text("Message").unwrap_or_default();
                assert_eq!(
                    message, tenant_id_format_invalid,
                    "case {case}/{operation}: expected {tenant_id_format_invalid:?}, got \
                     {message:?} in {}",
                    answer.raw
                );
            }
        }
    }
}

/// The published per-request object quota: a batch AT the bound is accepted, one PAST it is
/// refused with `TooManyInvalidationsInProgress` (400) and persists nothing — the code this
/// surface answers the per-batch object quota under, where the distribution-scoped create
/// answers `BatchTooLarge`.
#[tokio::test]
async fn a_batch_past_the_object_quota_is_refused_and_persists_nothing() {
    let router = emulator();
    let tenant_id = distribution_with_tenant(&router, "quota").await;

    // Each row: how many paths the batch names, and whether the service accepts it.
    // `MAX_INVALIDATION_PATHS` is the service's published bound, so the rows sit at it,
    // just under it and just past it.
    let rows: &[(&str, usize, bool)] = &[
        ("one path", 1, true),
        ("one under the bound", MAX_INVALIDATION_PATHS - 1, true),
        ("exactly the bound", MAX_INVALIDATION_PATHS, true),
        ("one past the bound", MAX_INVALIDATION_PATHS + 1, false),
        ("far past the bound", MAX_INVALIDATION_PATHS * 2, false),
    ];

    let mut accepted = 0usize;
    for (case, count, is_accepted) in rows {
        let owned: Vec<String> = (0..*count).map(|index| format!("/{index:05}.html")).collect();
        let paths: Vec<&str> = owned.iter().map(String::as_str).collect();
        let before = summary_ids(
            list_invalidations_for_tenant(&router, &tenant_id, "")
                .await
                .root(),
        );
        let answer = create_invalidation_for_tenant(
            &router,
            &tenant_id,
            &batch_xml(&format!("quota-{count}"), &paths),
        )
        .await;
        let after = summary_ids(
            list_invalidations_for_tenant(&router, &tenant_id, "")
                .await
                .root(),
        );

        if *is_accepted {
            accepted += 1;
            assert_eq!(
                answer.status, 201,
                "case {case}: expected HTTP 201 for {count} paths, got {} with {}",
                answer.status,
                answer.raw.chars().take(400).collect::<String>()
            );
            assert_eq!(
                after.len(),
                before.len() + 1,
                "case {case}: an accepted batch is one more invalidation, got {} after {}",
                after.len(),
                before.len()
            );
        } else {
            // "You have exceeded the maximum number of allowable InProgress invalidation
            // batch requests, or invalidation objects."
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400 for {count} paths, got {} with {}",
                answer.status,
                answer.raw.chars().take(400).collect::<String>()
            );
            assert_eq!(
                answer.error_code(),
                "TooManyInvalidationsInProgress",
                "case {case}: expected TooManyInvalidationsInProgress, got {:?}",
                answer.error_code()
            );
            // The sentence CloudFront states for this refusal: fixed service text, naming
            // nothing the request carried.
            assert_eq!(
                answer.root().find_text("Message"),
                Some(TOO_MANY_INVALIDATIONS_MESSAGE),
                "case {case}: expected the quota sentence the service states, got {:?}",
                answer.root().find_text("Message")
            );
            assert_eq!(
                after, before,
                "case {case}: a refused batch persists nothing, so the listing is unchanged \
                 ({before:?}), got {after:?}"
            );
        }
    }

    let listed = summary_ids(
        list_invalidations_for_tenant(&router, &tenant_id, "")
            .await
            .root(),
    );
    assert_eq!(
        listed.len(),
        accepted,
        "expected the {accepted} accepted batches to be the whole collection, got {}",
        listed.len()
    );
}

/// A repeated `CallerReference` carrying the same batch returns the invalidation already
/// created; carrying a different batch is refused, and neither stores a second record.
#[tokio::test]
async fn a_repeated_caller_reference_creates_no_second_invalidation() {
    let router = emulator();
    let tenant_id = distribution_with_tenant(&router, "caller-reference").await;

    // Each row: the batch a reference is first used for, and the batch it is repeated
    // with — the same set of paths, the same set in another order, and a different set.
    let rows: &[(&str, &[&str], &[&str], bool)] = &[
        ("the same single path", &["/*"], &["/*"], true),
        (
            "the same paths in the same order",
            &["/a.html", "/b.html"],
            &["/a.html", "/b.html"],
            true,
        ),
        (
            "a different set of paths",
            &["/c.html"],
            &["/c.html", "/d.html"],
            false,
        ),
    ];

    for (case, first, repeated, is_same) in rows {
        let reference = format!("repeat-{}", case.replace(' ', "-"));
        let created =
            create_invalidation_for_tenant(&router, &tenant_id, &batch_xml(&reference, first))
                .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 for the first batch, got {} with {}",
            created.status, created.raw
        );
        let id = created.root().text_at("Id").unwrap_or_default().to_string();

        let again =
            create_invalidation_for_tenant(&router, &tenant_id, &batch_xml(&reference, repeated))
                .await;
        if *is_same {
            assert_eq!(
                again.status, 201,
                "case {case}: expected the invalidation already created, got {} with {}",
                again.status, again.raw
            );
            assert_eq!(
                again.root().text_at("Id"),
                Some(id.as_str()),
                "case {case}: expected the id of the invalidation already created ({id}), \
                 got {:?}",
                again.root().text_at("Id")
            );
        } else {
            assert_eq!(
                again.status, 400,
                "case {case}: expected HTTP 400 for a reference reused with other paths, \
                 got {} with {}",
                again.status, again.raw
            );
            assert_eq!(
                again.error_code(),
                "InvalidArgument",
                "case {case}: expected InvalidArgument, got {:?}",
                again.error_code()
            );
        }

        let listed = summary_ids(
            list_invalidations_for_tenant(&router, &tenant_id, "")
                .await
                .root(),
        );
        assert_eq!(
            listed.iter().filter(|listed_id| *listed_id == &id).count(),
            1,
            "case {case}: the reference must name exactly one invalidation, got {listed:?}"
        );
    }
}

/// A list and a read REPORT: the status they carry is the one the clock owes the batch at
/// the instant of the call, and listing twice reports the same records.
#[tokio::test]
async fn a_list_reports_the_status_the_clock_owes_and_changes_nothing() {
    let router = emulator();
    let tenant_id = distribution_with_tenant(&router, "status").await;
    let created = create_batches(
        &router,
        &tenant_id,
        "status",
        &[vec!["/*"], vec!["/a.html", "/b.html"]],
    )
    .await;

    // A batch CloudFront has just accepted has not finished yet.
    let fresh = list_invalidations_for_tenant(&router, &tenant_id, "").await;
    for summary in summaries(fresh.root()) {
        assert_eq!(
            summary.text_at("Status"),
            Some("InProgress"),
            "case just-created: expected a just-created invalidation to be InProgress, got {:?}",
            summary.text_at("Status")
        );
    }

    // Past the batch's deadline the clock owes it the terminal status, and the read is what
    // materialises it — for the list and for the record alike.
    tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(250)).await;
    let settled = list_invalidations_for_tenant(&router, &tenant_id, "").await;
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
        let read = get_invalidation_for_tenant(&router, &tenant_id, invalidation_id).await;
        assert_eq!(
            read.root().text_at("Status"),
            Some("Completed"),
            "case settled: expected {invalidation_id} to read back Completed, got {:?}",
            read.root().text_at("Status")
        );
    }

    // The list changed nothing a later caller sees.
    let again = list_invalidations_for_tenant(&router, &tenant_id, "").await;
    let second_pass = summary_ids(again.root());
    assert_eq!(
        second_pass, first_pass,
        "case repeated: listing twice must report the same invalidations in the same order \
         ({first_pass:?}), got {second_pass:?}"
    );
}
