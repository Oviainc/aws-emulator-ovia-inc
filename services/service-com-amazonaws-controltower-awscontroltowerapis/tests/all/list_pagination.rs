// SPDX-License-Identifier: CC0-1.0

//! The ONE pagination rule this service's listings cut their pages by, driven end to end
//! over HTTP through every listing that shares it.
//!
//! `ListLandingZones`, `ListLandingZoneOperations`, `ListControlOperations`,
//! `ListEnabledControls`, `ListEnabledBaselines` and `ListBaselines` all answer
//! `nextToken`/`maxResults` the same way, so the rule is written once, in `crate::state`,
//! and each handler reaches it there. This suite drives that single path through all six
//! listings, so a change to it that paged one listing differently from its siblings fails
//! here.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees.
//!
//! The rule, asserted over a TABLE of page sizes and tokens rather than one case:
//!
//! * a call that sends no pagination member reports the whole listing and no `nextToken`;
//! * a token marks the first record the previous page did not return, so walking the pages
//!   for ANY page size yields the listing exactly once, in the same order the unpaginated
//!   call reports, each page holding at most `maxResults` records;
//! * the response carries a `nextToken` exactly while records remain after the page;
//! * a token no page of the listing handed out — an index the caller wrote itself included,
//!   since a token is not a bare index — is the operation's own modeled
//!   `ValidationException` (400), in the one wording every listing refuses it with;
//! * the empty token lists from the start wherever the model's `nextToken` type admits one.
//!
//! Which listing a token resumes — and the parameters a narrowed one binds its token to —
//! is the sibling suite `page_token_listing_scope`.
//!
//! The bounds written out by hand name their source: each listing's `maxResults` range and
//! the `nextToken` type that decides whether an empty token reaches the handler come from
//! `smithy_query(query="operation", operation="ListLandingZones,ListLandingZoneOperations,
//! ListControlOperations,ListEnabledControls,ListEnabledBaselines,ListBaselines")` on this
//! crate's `smithy-model/`. The controls, baselines, versions and manifests the fixture
//! seeds are the tables the sibling suites drive those calls with, so no identifier or
//! version is written out here.

use smithy_server_runtime::MultiServiceRouter;

use crate::baseline_operations::{enable as enable_baseline, enablements};
use crate::enabled_control_lifecycle::{control, enable as enable_control, entries, ou};
use crate::front_door_validation::FRONT_DOOR_CODE;
use crate::landing_zone_lifecycle::{
    call, emulator, manifest_governing, past_the_settle_span, Answer,
};

/// One listing that pages by the shared rule: the operation its refusal names, the path its
/// `@http` trait binds it to, the output member carrying its records, and the record member
/// that identifies one record across pages.
#[derive(Clone, Copy)]
pub(crate) struct Listing {
    pub(crate) operation: &'static str,
    pub(crate) path: &'static str,
    pub(crate) records: &'static str,
    pub(crate) identity: &'static str,
    /// The published bounds of this operation's `maxResults`, which the page sizes a case
    /// drives it with are built from.
    pub(crate) max_results_range: (i64, i64),
    /// Whether this listing is narrowed from the request, so its token only resumes for a
    /// call that repeats those members — which is what its refusal says.
    pub(crate) token_scoped_to_parameters: bool,
    /// Whether an empty `nextToken` reaches the handler at all: `true` where the member is
    /// a plain `String`, `false` where its modelled pattern admits no empty value, so the
    /// front door refuses one.
    pub(crate) empty_token_reaches_the_handler: bool,
}

/// `ListLandingZones`, whose handler held one copy of the rule.
const LANDING_ZONES: Listing = Listing {
    operation: "ListLandingZones",
    path: "/list-landingzones",
    records: "landingZones",
    identity: "arn",
    max_results_range: (1, 1),
    token_scoped_to_parameters: false,
    empty_token_reaches_the_handler: true,
};

/// `ListLandingZoneOperations`, whose handler held another.
const LANDING_ZONE_OPERATIONS: Listing = Listing {
    operation: "ListLandingZoneOperations",
    path: "/list-landingzone-operations",
    records: "landingZoneOperations",
    identity: "operationIdentifier",
    max_results_range: (1, 100),
    token_scoped_to_parameters: true,
    empty_token_reaches_the_handler: true,
};

/// `ListControlOperations`, whose handler held another.
const CONTROL_OPERATIONS: Listing = Listing {
    operation: "ListControlOperations",
    path: "/list-control-operations",
    records: "controlOperations",
    identity: "operationIdentifier",
    max_results_range: (1, 100),
    token_scoped_to_parameters: true,
    // `ListControlOperationsNextToken` is modelled `pattern \S+`.
    empty_token_reaches_the_handler: false,
};

/// `ListEnabledControls`, whose handler held another.
const ENABLED_CONTROLS: Listing = Listing {
    operation: "ListEnabledControls",
    path: "/list-enabled-controls",
    records: "enabledControls",
    identity: "arn",
    max_results_range: (1, 200),
    token_scoped_to_parameters: true,
    empty_token_reaches_the_handler: true,
};

/// `ListEnabledBaselines`, whose handler held another.
const ENABLED_BASELINES: Listing = Listing {
    operation: "ListEnabledBaselines",
    path: "/list-enabled-baselines",
    records: "enabledBaselines",
    identity: "arn",
    max_results_range: (5, 100),
    token_scoped_to_parameters: true,
    // `ListEnabledBaselinesNextToken` is modelled `pattern \S+`.
    empty_token_reaches_the_handler: false,
};

/// `ListBaselines`, whose handler held the sixth copy.
const BASELINES: Listing = Listing {
    operation: "ListBaselines",
    path: "/list-baselines",
    records: "baselines",
    identity: "arn",
    max_results_range: (4, 100),
    token_scoped_to_parameters: false,
    empty_token_reaches_the_handler: true,
};

/// Every listing that reaches the shared rule.
pub(crate) fn listings() -> Vec<Listing> {
    vec![
        LANDING_ZONES,
        LANDING_ZONE_OPERATIONS,
        CONTROL_OPERATIONS,
        ENABLED_CONTROLS,
        ENABLED_BASELINES,
        BASELINES,
    ]
}

/// POST one listing's path with `body`, asserting the call succeeded.
pub(crate) async fn list(
    router: &MultiServiceRouter,
    listing: Listing,
    body: serde_json::Value,
) -> Answer {
    let answer = call(router, listing.path, body.clone()).await;
    assert_eq!(
        answer.status, 200,
        "{} {body}: expected 200, got {} body {:?}",
        listing.path, answer.status, answer.body
    );
    answer
}

/// The identity of every record an answer reports, in the order reported — a page is
/// compared as a SEQUENCE, because a token resumes an order rather than a set.
pub(crate) fn identities(listing: Listing, answer: &Answer) -> Vec<String> {
    entries(answer.get(listing.records))
        .iter()
        .map(|record| {
            record
                .get(listing.identity)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `nextToken` an answer carries, or `None` when it reported the end of the listing.
pub(crate) fn reported_token(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("nextToken")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// The request body a page asks for: `maxResults` and `nextToken` only where the case
/// sends them.
pub(crate) fn page_body(max_results: Option<i64>, next_token: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({});
    if let Some(max) = max_results {
        body["maxResults"] = serde_json::json!(max);
    }
    if let Some(token) = next_token {
        body["nextToken"] = serde_json::json!(token);
    }
    body
}

/// Walk one listing's pages for one page size, following the tokens it hands out: the
/// records collected in the order reported, and how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    listing: Listing,
    case: &str,
    max_results: Option<i64>,
) -> (Vec<String>, usize) {
    let mut collected: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut next: Option<String> = None;

    loop {
        let body = page_body(max_results, next.as_deref());
        let answer = list(router, listing, body.clone()).await;
        pages += 1;
        let page = identities(listing, &answer);
        if let Some(max) = max_results {
            assert!(
                page.len() as i64 <= max,
                "case {case:?} on {}: page {pages} ({body}) holds {} records, more than the \
                 {max} asked for: {page:?}",
                listing.path,
                page.len()
            );
        }
        collected.extend(page);
        next = reported_token(&answer);
        if next.is_none() {
            break;
        }
        assert!(
            pages <= collected.len() + 1,
            "case {case:?} on {}: the walk is not advancing — {pages} pages have reported \
             {} records in total",
            listing.path,
            collected.len()
        );
    }

    (collected, pages)
}

/// The page sizes a case drives one listing with: an omitted `maxResults`, then every size
/// inside the published range that the rule generalises over — each published bound, one
/// past the minimum, one exactly the number of records listed and one past it.
fn page_sizes(listing: Listing, total: usize) -> Vec<(String, Option<i64>)> {
    let (min, max) = listing.max_results_range;
    let total = total as i64;
    let mut sizes: Vec<(String, i64)> = vec![
        (format!("maxResults at the published minimum of {min}"), min),
        (
            format!("maxResults one past the published minimum ({})", min + 1),
            min + 1,
        ),
        (
            format!("maxResults exactly the {total} records listed"),
            total,
        ),
        (
            format!("maxResults one past the {total} records listed"),
            total + 1,
        ),
        (format!("maxResults at the published maximum of {max}"), max),
    ];
    sizes.retain(|(_, size)| (min..=max).contains(size));
    sizes.dedup_by(|left, right| left.1 == right.1);

    let mut cases: Vec<(String, Option<i64>)> = vec![("maxResults omitted".to_string(), None)];
    cases.extend(sizes.into_iter().map(|(case, size)| (case, Some(size))));
    cases
}

/// Assert the shared rule over one listing's whole population.
async fn the_rule_holds_over(router: &MultiServiceRouter, listing: Listing) {
    let unpaginated = list(router, listing, serde_json::json!({})).await;
    let population = identities(listing, &unpaginated);
    assert!(
        !population.is_empty(),
        "{}: the fixture seeded no records, so nothing below could be paged",
        listing.path
    );
    assert_eq!(
        reported_token(&unpaginated),
        None,
        "{}: a call that returned every record must carry no nextToken, got {:?}",
        listing.path,
        unpaginated.body
    );
    assert!(
        !population.iter().any(String::is_empty),
        "{}: every record must report an {}, got {population:?}",
        listing.path,
        listing.identity
    );

    for (case, max_results) in page_sizes(listing, population.len()) {
        let (walked, pages) = walk(router, listing, &case, max_results).await;
        assert_eq!(
            walked, population,
            "case {case:?} on {}: a paginated walk must yield the whole listing once, in the \
             order the unpaginated call reports, got {walked:?}",
            listing.path
        );
        let expected_pages = match max_results {
            None => 1,
            Some(max) => population.len().div_ceil(max.max(1) as usize).max(1),
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?} on {}: expected {expected_pages} page(s) for {} records, got {pages}",
            listing.path,
            population.len()
        );
    }

    // The empty token, where the model's `nextToken` type admits one, means the same as an
    // absent one; where its pattern does not, the front door refuses it before the rule.
    if listing.empty_token_reaches_the_handler {
        let from_empty = list(router, listing, page_body(None, Some(""))).await;
        assert_eq!(
            identities(listing, &from_empty),
            population,
            "case {:?} on {}: expected the listing from its start, got {:?}",
            "an empty nextToken",
            listing.path,
            from_empty.body
        );
    } else {
        let refused = call(router, listing.path, page_body(None, Some(""))).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some(FRONT_DOOR_CODE)),
            "case {:?} on {}: expected 400 {FRONT_DOOR_CODE}, got {:?} body {:?}",
            "an empty nextToken",
            listing.path,
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
    }

    an_unhandled_token_is_refused(router, listing, population.len()).await;
}

/// The one wording every listing refuses a token no page of it handed out with — the
/// message `state::unhandled_next_token_message` builds, read off the listing's own scope,
/// written here once so no suite asserting this rule can drift from another.
pub(crate) fn unhandled_token_refusal(listing: Listing, token: &str) -> String {
    format!(
        "nextToken {token} was not returned by a previous {} call{}",
        listing.operation,
        if listing.token_scoped_to_parameters {
            " with these parameters"
        } else {
            ""
        }
    )
}

/// Assert the refusal side of the rule over one listing: a token no page of a listing of
/// `total` records handed out is the operation's own `ValidationException`, worded the one
/// way every listing words it. An index the caller wrote itself is one of those tokens: a
/// token is not a bare index, so a number can never be read as a position in a listing the
/// caller may not even have paged.
async fn an_unhandled_token_is_refused(router: &MultiServiceRouter, listing: Listing, total: usize) {
    let cases: Vec<(String, String)> = vec![
        (
            "a token that is not a number".to_string(),
            "not-a-token".to_string(),
        ),
        ("a negative index".to_string(), "-1".to_string()),
        ("a fractional index".to_string(), "1.5".to_string()),
        (
            "the index of the listing's first record".to_string(),
            "0".to_string(),
        ),
        (
            format!("the index just past the {total} records listed"),
            total.to_string(),
        ),
        (
            format!("an index past the {total} records listed"),
            (total + 1).to_string(),
        ),
        (
            "a token from another operation's listing".to_string(),
            "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2".to_string(),
        ),
    ];

    for (case, token) in cases {
        let refused = call(router, listing.path, page_body(None, Some(&token))).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?} on {}: expected 400 ValidationException for {token:?}, got {:?} \
             body {:?}",
            listing.path,
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        let expected = unhandled_token_refusal(listing, &token);
        assert_eq!(
            refused.string("message"),
            expected,
            "case {case:?} on {}: expected the shared refusal wording {expected:?}, got {:?}",
            listing.path,
            refused.string("message")
        );
        assert!(
            refused.body.get(listing.records).is_none(),
            "case {case:?} on {}: a refusal must report no records, got {:?}",
            listing.path,
            refused.body
        );
    }
}

/// Seed every store the listings page over: the account's landing zone and the calls that
/// append to its operation log, the control enablements and their operation log, and the
/// baseline enablements.
pub(crate) async fn seed(router: &MultiServiceRouter) {
    let created = call(
        router,
        "/create-landingzone",
        serde_json::json!({
            "version": "2.0",
            "manifest": manifest_governing(&["us-west-2", "us-east-1", "eu-west-1"]),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "create-landingzone: expected 200, got {} body {:?}",
        created.status, created.body
    );
    let arn = created.string("arn");
    // Nothing sleeps server-side: the wait is the test's, because the next call needs the
    // landing zone out of its transient status.
    tokio::time::sleep(past_the_settle_span()).await;

    for (case, path, body) in [
        (
            "an update across the major boundary",
            "/update-landingzone",
            serde_json::json!({
                "version": "3.0",
                "manifest": manifest_governing(&["us-west-2"]),
            }),
        ),
        (
            "a reset of the deployed configuration",
            "/reset-landingzone",
            serde_json::json!({}),
        ),
        (
            "an update to the catalog's newest version, carrying no manifest",
            "/update-landingzone",
            serde_json::json!({ "version": "3.3" }),
        ),
    ] {
        let mut body = body;
        body["landingZoneIdentifier"] = serde_json::json!(arn);
        let answer = call(router, path, body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} ({body}): expected 200, got {} body {:?}",
            answer.status, answer.body
        );
        tokio::time::sleep(past_the_settle_span()).await;
    }

    // One control on two targets, one target carrying two controls, and a third control on
    // a target of its own: four enablements, so a page size below that count takes several
    // pages. Every pair is distinct, because a control is enabled at most once per target.
    for (control_identifier, target_identifier) in [
        (control("AWS-GR_ENCRYPTED_VOLUMES"), ou("fb01")),
        (control("AWS-GR_ENCRYPTED_VOLUMES"), ou("fb02")),
        (control("AWS-GR_RESTRICT_ROOT_USER"), ou("fb01")),
        (control("AWS-GR_RESTRICTED_COMMON_PORTS"), ou("fb03")),
    ] {
        let _ = enable_control(
            router,
            serde_json::json!({
                "controlIdentifier": control_identifier,
                "targetIdentifier": target_identifier,
            }),
        )
        .await;
    }

    for row in enablements() {
        let answer = enable_baseline(router, &row).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: enable-baseline expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );
    }

    // Past the settle span, so every seeded record is in its terminal status and no listing
    // below races the clock.
    tokio::time::sleep(past_the_settle_span()).await;
}

/// RULE: every listing of this service pages by the one shared rule — a token marks the
/// first record the previous page did not return, `maxResults` is how many records a page
/// holds, and a token is reported exactly while records remain.
#[tokio::test]
async fn every_listing_pages_by_the_one_shared_page_token_rule() {
    let router = emulator();
    seed(&router).await;

    for listing in listings() {
        the_rule_holds_over(&router, listing).await;
    }
}

/// RULE (the same rule at the other end of the count it generalises over): an EMPTY listing
/// is one empty, untokened page, and it refuses a token no page handed out in the same
/// wording a populated one does.
#[tokio::test]
async fn an_empty_listing_is_one_untokened_page_and_refuses_an_unhandled_token() {
    let router = emulator();

    for listing in listings() {
        // `ListBaselines` reports the AWS-managed catalog, which no call creates or
        // deletes, so it is the one listing that is never empty; its refusals are asserted
        // against the catalog's own length.
        let total = identities(listing, &list(&router, listing, serde_json::json!({})).await).len();
        if listing.path == BASELINES.path {
            assert!(
                total > 0,
                "{}: the catalog must publish at least one baseline, got {total}",
                listing.path
            );
        } else {
            let answer = list(&router, listing, serde_json::json!({})).await;
            assert!(
                total == 0 && reported_token(&answer).is_none(),
                "{}: an unseeded listing must be one empty, untokened page, got {:?}",
                listing.path,
                answer.body
            );
        }

        an_unhandled_token_is_refused(&router, listing, total).await;
    }
}
