// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListAssetRevisions`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response body as evidence, so a handler that
//! answered from a hardcoded value instead of from the store would fail here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. The listing reports EVERY revision the store holds for the addressed asset, newest
//!    first, each carrying the asset's own identity (`id`, `domainId`), that revision's
//!    number and the principal and instant that recorded it — the same `createdBy` and
//!    `createdAt` `GetAsset` reads back for that revision.
//! 2. A page holds at most `maxResults` revisions, and following the `nextToken` a page
//!    ends with walks the whole history exactly once, in that one order.
//! 3. Reporting changes nothing: the same call answered twice answers the same thing, and
//!    the asset is still at the revision it was at.
//! 4. The prerequisites are refused with the operation's own modelled errors: an absent
//!    domain or asset, or a domain on its way out, with `ResourceNotFoundException`; a
//!    `nextToken` this service never issued, or a `maxResults` outside the modelled
//!    range, with `ValidationException`.

use crate::harness::{Answer, Emulator};
use serde_json::{json, Value};

/// The principal the harness's SigV4 scope names, which is what every `createdBy` this
/// service writes must be.
const CALLER: &str = "arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY";

/// A stored asset of a service-defined type with `extra` revisions recorded on top of
/// the one `CreateAsset` records, so it has `extra + 1` revisions in all. Answers its id.
async fn asset_with_revisions(
    dz: &Emulator,
    domain: &str,
    project: &str,
    name: &str,
    extra: usize,
) -> String {
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets"),
            Some(json!({
                "name": name,
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating the asset {name:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    let id = created.s("id");
    for number in 0..extra {
        let revised = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/assets/{id}/revisions"),
                Some(json!({ "name": format!("{name} r{number}") })),
            )
            .await;
        assert_eq!(
            revised.status, 200,
            "revising the asset {name:?}: expected 200, got {} {:?}",
            revised.status, revised.body
        );
    }
    id
}

/// Every revision the answer reported, in the order it reported them: the `revision`,
/// `id`, `domainId` and `createdBy` as strings plus the raw `createdAt`.
fn reported(answer: &Answer) -> Vec<(String, String, String, String, Option<Value>)> {
    let member = |item: &Value, name: &str| {
        item.get(name)
            .and_then(Value::as_str)
            .unwrap_or("<absent>")
            .to_string()
    };
    answer
        .body
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            (
                member(item, "revision"),
                member(item, "id"),
                member(item, "domainId"),
                member(item, "createdBy"),
                item.get("createdAt").cloned(),
            )
        })
        .collect()
}

/// Just the revision numbers the answer reported, in order.
fn numbers(answer: &Answer) -> Vec<String> {
    reported(answer)
        .into_iter()
        .map(|(revision, ..)| revision)
        .collect()
}

#[tokio::test]
async fn the_listing_reports_every_recorded_revision_newest_first() {
    let dz = Emulator::new();
    let domain = dz.available_domain("revision-listing-domain").await;
    let project = dz.project(&domain, "revision owner").await;

    // The rows differ along what the rule generalises over: HOW MANY revisions the asset
    // has, from the single revision a create records up to a history several revisions
    // deep.
    for extra in [0usize, 1, 2, 7] {
        let label = format!("an asset with {} revision(s)", extra + 1);
        let id = asset_with_revisions(&dz, &domain, &project, &format!("asset-{extra}"), extra)
            .await;
        let answer = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/assets/{id}/revisions"),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        // Newest first, every recorded number exactly once, and nothing invented.
        let expected: Vec<String> = (1..=extra + 1).rev().map(|n| n.to_string()).collect();
        assert_eq!(
            numbers(&answer),
            expected,
            "case {label:?}: expected the revisions {expected:?} newest first, got {:?}",
            answer.body
        );
        // The whole history fits one page, so there is nothing to resume from.
        assert_eq!(
            answer.body.get("nextToken"),
            None,
            "case {label:?}: the whole history fits one page, so no token is offered, got {:?}",
            answer.body.get("nextToken")
        );
        // Each reported revision carries the asset's identity and the creation the store
        // recorded for THAT revision, as the asset's own reader reads it back.
        for (revision, reported_id, reported_domain, created_by, created_at) in reported(&answer) {
            assert_eq!(
                (reported_id.as_str(), reported_domain.as_str(), created_by.as_str()),
                (id.as_str(), domain.as_str(), CALLER),
                "case {label:?} revision {revision}: expected the asset's identity and the \
                 calling principal, got {:?}",
                answer.body
            );
            let read = dz
                .call(
                    "GET",
                    &format!("/v2/domains/{domain}/assets/{id}?revision={revision}"),
                    None,
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {label:?} revision {revision}: the listed revision must be readable, got \
                 {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                created_at,
                read.body.get("createdAt").cloned(),
                "case {label:?} revision {revision}: the listed createdAt must be the one the \
                 asset reads back, got {:?}",
                read.body.get("createdAt")
            );
        }
    }
}

#[tokio::test]
async fn following_the_token_walks_the_whole_history_once_per_page_size() {
    let dz = Emulator::new();
    let domain = dz.available_domain("revision-paging-domain").await;
    let project = dz.project(&domain, "revision owner").await;
    // Eight revisions: more than the small page sizes below, fewer than the modelled
    // `maxResults` ceiling of 50, so one unpaged call still reports them all.
    const EXTRA: usize = 7;
    let id = asset_with_revisions(&dz, &domain, &project, "paged asset", EXTRA).await;
    let whole: Vec<String> = (1..=EXTRA + 1).rev().map(|n| n.to_string()).collect();

    // Each row is a page size: inside the history, exactly its length, past it, and each
    // published bound of the modelled 1..50 range.
    for page_size in [1usize, 2, 3, 5, 8, 9, 50] {
        let mut token: Option<String> = None;
        let mut seen: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case maxResults={page_size}: paging did not terminate after {pages} pages, saw \
                 {seen:?}"
            );
            let query = match &token {
                Some(t) => format!("?maxResults={page_size}&nextToken={t}"),
                None => format!("?maxResults={page_size}"),
            };
            let answer = dz
                .call(
                    "GET",
                    &format!("/v2/domains/{domain}/assets/{id}/revisions{query}"),
                    None,
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size} page {pages}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            let page = numbers(&answer);
            assert!(
                page.len() <= page_size,
                "case maxResults={page_size} page {pages}: a page must not exceed maxResults, \
                 got {page:?}"
            );
            seen.extend(page);
            token = answer
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, whole,
            "case maxResults={page_size}: paging must report the whole history once, newest \
             first, got {seen:?}"
        );
    }

    // A request naming no maxResults gets the whole history, which is smaller than the
    // modelled ceiling.
    let unpaged = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/assets/{id}/revisions"),
            None,
        )
        .await;
    assert_eq!(
        numbers(&unpaged),
        whole,
        "case \"no maxResults\": expected the whole history, got {:?}",
        unpaged.body
    );
}

#[tokio::test]
async fn listing_the_revisions_changes_nothing_a_later_caller_sees() {
    let dz = Emulator::new();
    let domain = dz.available_domain("revision-read-only-domain").await;
    let project = dz.project(&domain, "revision owner").await;
    let id = asset_with_revisions(&dz, &domain, &project, "stable asset", 2).await;
    let revisions_uri = format!("/v2/domains/{domain}/assets/{id}/revisions");

    let first = dz.call("GET", &revisions_uri, None).await;
    let second = dz.call("GET", &revisions_uri, None).await;
    assert_eq!(
        reported(&first),
        reported(&second),
        "case \"two reads in a row\": a reporting call must answer the same thing twice, got {:?} \
         then {:?}",
        first.body,
        second.body
    );
    // And the asset is still at the revision it was at: no read moved it on.
    let read = dz
        .call("GET", &format!("/v2/domains/{domain}/assets/{id}"), None)
        .await;
    assert_eq!(
        read.s("revision"),
        "3",
        "case \"the asset after two listings\": expected revision 3, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn the_preconditions_the_listing_needs_are_refused_with_the_modelled_errors() {
    let dz = Emulator::new();
    let domain = dz.available_domain("revision-listing-refusal-domain").await;
    let project = dz.project(&domain, "revision owner").await;
    let id = asset_with_revisions(&dz, &domain, &project, "refusal asset", 1).await;

    // Each row: the request, and the refusal it earns.
    let rows = vec![
        (
            // A permission is held ON a domain, so the caller holds none on an
            // identifier the store serves no domain for: authorization decides the
            // listing before any lookup.
            "a domain nothing created",
            format!("/v2/domains/dzd_0000000000/assets/{id}/revisions"),
            403,
            "AccessDeniedException",
        ),
        (
            "an asset the domain does not hold",
            format!("/v2/domains/{domain}/assets/nosuchasset/revisions"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a nextToken this service never issued",
            format!("/v2/domains/{domain}/assets/{id}/revisions?nextToken=notatoken"),
            400,
            "ValidationException",
        ),
        // One past each published bound of the modelled `maxResults` range (1..50).
        (
            "maxResults below the modelled range",
            format!("/v2/domains/{domain}/assets/{id}/revisions?maxResults=0"),
            400,
            "ValidationException",
        ),
        (
            "maxResults above the modelled range",
            format!("/v2/domains/{domain}/assets/{id}/revisions?maxResults=51"),
            400,
            "ValidationException",
        ),
    ];
    for (label, uri, status, code) in rows {
        let answer = dz.call("GET", &uri, None).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // The refusals left the history exactly as it was.
    let listed = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/assets/{id}/revisions"),
            None,
        )
        .await;
    assert_eq!(
        numbers(&listed),
        vec!["2".to_string(), "1".to_string()],
        "the refused calls must not have changed the stored history, got {:?}",
        listed.body
    );

    // A domain on its way out: deleted, and listed on the very next call, before the
    // deletion the clock owes has settled, so the domain is still stored and DELETING.
    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{domain}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "deleting {domain:?}: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    let too_late = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/assets/{id}/revisions"),
            None,
        )
        .await;
    assert_eq!(
        (too_late.status, too_late.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a domain that is being deleted\": expected 404 ResourceNotFoundException, got {} \
         {:?}",
        too_late.status,
        too_late.body
    );
}
