// SPDX-License-Identifier: CC0-1.0

//! The baseline catalog as `ListBaselines` serves it over HTTP.
//!
//! Drives `ListBaselines` (and `GetBaseline`, to read a listed entry back) through the
//! same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees — status codes, error codes and JSON bodies — not the handler's Rust signature.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a paginated listing reports every entry of the collection exactly once, in one
//!   stable order, in pages of at most `maxResults`, carrying a `nextToken` exactly while
//!   entries remain — so walking the pages for ANY page size yields the same sequence as
//!   one unpaginated call;
//! * each listed entry is the catalog entry's `name` and `description` plus the ARN this
//!   endpoint serves that baseline under — the request's signed region and an EMPTY
//!   account segment, because a baseline is service-managed data no account owns — so
//!   every listed ARN resolves through `GetBaseline` to the entry listed beside it;
//! * a `nextToken` the service never handed out is `ValidationException` (400), and a
//!   `maxResults` outside the modelled range is refused by the front door (400
//!   `BadRequestException`);
//! * the catalog is service-managed: a listing does not depend on what the account holds,
//!   and no call adds, changes or removes an entry.
//!
//! The catalog itself is read from the service (`state::BASELINE_CATALOG`), so a retune of
//! its entries retunes these tests rather than leaving them asserting a stale copy. The
//! shapes come from `smithy_query(query="shape", shape="ListBaselinesInput,
//! ListBaselinesOutput,BaselineSummary,ListBaselinesMaxResults")` on this crate's
//! `smithy-model/`: `maxResults` is `@range(min: 4, max: 100)`, `nextToken` is a plain
//! `String`, and a `BaselineSummary` is `arn` (required), `name` (required) and
//! `description`.

use service_com_amazonaws_controltower_awscontroltowerapis::state::BASELINE_CATALOG;
use smithy_server_runtime::MultiServiceRouter;

// `served_arn_in` is the sibling catalog suite's definition of the ARN this endpoint
// serves a baseline under, reused rather than re-formatted here so the two suites cannot
// disagree about that form.
use crate::baseline_catalog::served_arn_in;
use crate::landing_zone_lifecycle::{
    call, call_in_region, emulator, simple_manifest, Answer, SIGNED_REGION,
};

/// The region these tests sign for, and therefore the region of every ARN the service
/// mints for them: the harness's own signing region rather than a second literal.
const REQUEST_REGION: &str = SIGNED_REGION;

/// One listed baseline, as a client reads it off the response body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    arn: String,
    name: String,
    description: String,
}

/// POST `/list-baselines` with whatever pagination members `body` carries.
async fn list_baselines(router: &MultiServiceRouter, body: serde_json::Value) -> Answer {
    call(router, "/list-baselines", body).await
}

/// The `baselines` member of a successful answer, as [`Listed`] rows.
fn page_entries(case: &str, answer: &Answer) -> Vec<Listed> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    let baselines = answer
        .get("baselines")
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a baselines list, got {:?}", answer.body))
        .clone();
    baselines
        .iter()
        .map(|entry| {
            let member = |name: &str| {
                entry
                    .get(name)
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string()
            };
            assert!(
                !member("arn").is_empty() && !member("name").is_empty(),
                "case {case:?}: both required members must be reported, got {entry:?}"
            );
            Listed {
                arn: member("arn"),
                name: member("name"),
                description: member("description"),
            }
        })
        .collect()
}

/// The whole listing walked page by page with `max_results`, plus the number of pages it
/// took. `None` for `max_results` means the member is omitted entirely.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<i64>,
) -> (Vec<Listed>, usize) {
    let mut collected: Vec<Listed> = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;

    loop {
        let mut body = serde_json::Map::new();
        if let Some(max) = max_results {
            body.insert("maxResults".to_string(), serde_json::json!(max));
        }
        if let Some(token) = &token {
            body.insert("nextToken".to_string(), serde_json::json!(token));
        }

        let answer = list_baselines(router, serde_json::Value::Object(body)).await;
        let entries = page_entries(case, &answer);
        pages += 1;

        if let Some(max) = max_results {
            assert!(
                entries.len() as i64 <= max,
                "case {case:?}: a page must hold at most maxResults {max} entries, got {}",
                entries.len()
            );
        }

        let next = answer.get("nextToken").as_str().map(str::to_string);
        assert!(
            next.is_none() || !entries.is_empty(),
            "case {case:?}: a page that carries a nextToken must not be empty, got {:?}",
            answer.body
        );
        collected.extend(entries);

        match next {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            pages <= BASELINE_CATALOG.len() + 2,
            "case {case:?}: the walk must terminate, got {pages} pages for {} entries",
            BASELINE_CATALOG.len()
        );
    }

    (collected, pages)
}

/// The catalog as this endpoint serves it when the request is signed for `region`: the
/// published order, and for each entry the ARN that region serves it under.
fn expected_listing_in(region: &str) -> Vec<Listed> {
    BASELINE_CATALOG
        .iter()
        .map(|baseline| Listed {
            arn: served_arn_in(region, baseline),
            name: baseline.name.to_string(),
            description: baseline.description.to_string(),
        })
        .collect()
}

/// [`expected_listing_in`] for the region these tests sign for by default.
fn expected_listing() -> Vec<Listed> {
    expected_listing_in(REQUEST_REGION)
}

/// RULE: an unpaginated listing reports every catalog entry exactly once, each with the
/// catalog's name and description and the ARN this endpoint serves that baseline under.
#[tokio::test]
async fn an_unpaginated_listing_reports_the_whole_catalog() {
    let router = emulator();
    let answer = list_baselines(&router, serde_json::json!({})).await;
    let entries = page_entries("no pagination members at all", &answer);

    assert_eq!(
        entries,
        expected_listing(),
        "case {:?}: expected the catalog as this endpoint serves it, got {:?}",
        "no pagination members at all",
        answer.body
    );
    assert!(
        answer.body.get("nextToken").is_none(),
        "case {:?}: a listing that returned everything must carry no nextToken, got {:?}",
        "no pagination members at all",
        answer.body
    );
}

/// RULE (the pagination side of it): walking the pages for ANY page size yields the same
/// sequence as one unpaginated call, each page holding at most `maxResults` entries.
///
/// The sizes span what the rule generalises over: each published bound of
/// `ListBaselinesMaxResults` (`@range(min: 4, max: 100)`), a size that divides the catalog
/// evenly, one that does not, one equal to its length and one past it.
#[tokio::test]
async fn every_page_size_walks_the_same_listing() {
    let router = emulator();
    let total = BASELINE_CATALOG.len() as i64;
    let cases: Vec<(String, Option<i64>)> = vec![
        ("maxResults omitted".to_string(), None),
        (
            "maxResults exactly the range min of 4".to_string(),
            Some(4),
        ),
        ("maxResults 5".to_string(), Some(5)),
        (
            format!("maxResults exactly the catalog's length of {total}"),
            Some(total),
        ),
        (
            format!("maxResults one past the catalog's length ({})", total + 1),
            Some(total + 1),
        ),
        (
            "maxResults exactly the range max of 100".to_string(),
            Some(100),
        ),
    ];

    let expected = expected_listing();
    for (case, max_results) in cases {
        let (listed, pages) = walk(&router, &case, max_results).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: a paginated walk must yield the whole catalog in its published \
             order, got {listed:?}"
        );

        // A page size below the catalog's length must take more than one page, and one at
        // or above it exactly one: the token is present exactly while entries remain.
        let expected_pages = match max_results {
            None => 1,
            Some(max) => (total as usize).div_ceil((max as usize).max(1)).max(1),
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) for {total} entries, got {pages}"
        );
    }
}

/// RULE: every ARN the listing reports resolves through `GetBaseline` to the very entry
/// the listing showed beside it — the two reporting operations read one catalog.
#[tokio::test]
async fn every_listed_arn_resolves_through_get_baseline() {
    let router = emulator();
    let (listed, _) = walk(&router, "a 4-per-page walk", Some(4)).await;
    assert!(
        !listed.is_empty(),
        "the catalog must publish at least one baseline, got {listed:?}"
    );

    for entry in listed {
        let answer = call(
            &router,
            "/get-baseline",
            serde_json::json!({ "baselineIdentifier": entry.arn }),
        )
        .await;
        assert_eq!(
            (
                answer.status,
                answer.string("arn"),
                answer.string("name"),
                answer.string("description")
            ),
            (200, entry.arn.clone(), entry.name.clone(), entry.description.clone()),
            "case {:?}: a listed ARN must resolve to the entry the listing reported, got {:?}",
            entry.name,
            answer.body
        );
    }
}

/// RULE (the region side of the ARN rule): the ARNs a listing reports are the ones THIS
/// endpoint serves the baselines under, so they carry the region the request was signed
/// for — not a region baked into the catalog.
#[tokio::test]
async fn the_listed_arns_carry_the_requests_region() {
    let router = emulator();
    // Regions spanning the partitions `BaselineArn`'s `[a-z0-9-]*` region segment admits.
    for region in ["us-west-2", "eu-central-1", "ap-southeast-4", "us-gov-west-1"] {
        let answer = call_in_region(&router, region, "/list-baselines", serde_json::json!({})).await;
        assert_eq!(
            page_entries(region, &answer),
            expected_listing_in(region),
            "case {region:?}: expected the catalog served in the signed region, got {:?}",
            answer.body
        );
    }
}

/// RULE: a `nextToken` the service never handed out is the operation's own modeled
/// `ValidationException` (400), and the refusal reports no baselines.
#[tokio::test]
async fn a_token_the_service_never_handed_out_is_refused() {
    let router = emulator();
    let past_the_end = (BASELINE_CATALOG.len() + 1).to_string();
    let cases: Vec<(&str, String)> = vec![
        ("a token that is not a number", "not-a-token".to_string()),
        ("a negative index", "-1".to_string()),
        ("an index past the end of the catalog", past_the_end),
        ("a fractional index", "1.5".to_string()),
        (
            "a token from another operation's listing",
            "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2".to_string(),
        ),
    ];

    for (case, token) in cases {
        let refused = list_baselines(&router, serde_json::json!({ "nextToken": token })).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException for {token:?}, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("baselines").is_none(),
            "case {case:?}: a refusal must report no baselines, got {:?}",
            refused.body
        );
    }
}

/// RULE (the other side of the token rule): the empty token, which `nextToken`'s plain
/// `String` type admits through the front door, lists from the start — as an absent token
/// does; and an index the caller wrote itself is not a token, so the index of the end of
/// the catalog resumes nothing.
#[tokio::test]
async fn an_empty_token_lists_from_the_start_and_an_index_is_not_a_token() {
    let router = emulator();

    let from_empty = list_baselines(&router, serde_json::json!({ "nextToken": "" })).await;
    assert_eq!(
        page_entries("an empty nextToken", &from_empty),
        expected_listing(),
        "case {:?}: expected the listing from its start, got {:?}",
        "an empty nextToken",
        from_empty.body
    );

    let at_end = BASELINE_CATALOG.len().to_string();
    let refused = list_baselines(&router, serde_json::json!({ "nextToken": at_end })).await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {:?}: expected 400 ValidationException for {at_end:?}, got {:?} body {:?}",
        "the index just past the last entry",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
    assert!(
        refused.body.get("baselines").is_none(),
        "case {:?}: a refusal must report no baselines, got {:?}",
        "the index just past the last entry",
        refused.body
    );
}

/// RULE: a `maxResults` outside the modelled range never reaches the handler — the front
/// door refuses it with 400 `BadRequestException`, on each side of
/// `ListBaselinesMaxResults`'s published bounds.
#[tokio::test]
async fn a_max_results_outside_the_modeled_range_is_a_front_door_bad_request() {
    let router = emulator();
    let cases: Vec<(&str, i64)> = vec![
        ("maxResults one below the range min of 4", 3),
        ("maxResults one above the range max of 100", 101),
        ("maxResults zero", 0),
        ("maxResults negative", -7),
    ];

    for (case, max_results) in cases {
        let refused = list_baselines(&router, serde_json::json!({ "maxResults": max_results })).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException for {max_results}, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
    }
}

/// RULE (the service-managed side of it): the catalog does not depend on what the account
/// holds, so a repeated listing is identical and no call — not even one that enables a
/// baseline on a target — adds, changes or removes an entry.
#[tokio::test]
async fn no_call_changes_the_listing() {
    let router = emulator();
    let before = list_baselines(&router, serde_json::json!({})).await;
    let expected = page_entries("before any mutating call", &before);

    // Mutating calls of each kind this service serves: a landing zone, and an enablement
    // of a baseline the catalog publishes.
    let created = call(
        &router,
        "/create-landingzone",
        serde_json::json!({
            "version": "3.3",
            "manifest": simple_manifest()
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "creating a landing zone: expected 200, got {} body {:?}",
        created.status, created.body
    );

    let enabled = call(
        &router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2",
            "baselineVersion": "4.0",
            "targetIdentifier": "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-ct11-1c",
        }),
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "enabling a catalogued baseline: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );

    let after = list_baselines(&router, serde_json::json!({})).await;
    assert_eq!(
        page_entries("after the mutating calls", &after),
        expected,
        "case {:?}: the listing must be unchanged by any call, got {:?}",
        "after the mutating calls",
        after.body
    );
}
