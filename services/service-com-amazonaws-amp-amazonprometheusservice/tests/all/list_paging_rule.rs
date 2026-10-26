// SPDX-License-Identifier: CC0-1.0

//! The ONE paging rule every list operation of this service serves its pages by,
//! exercised end to end over HTTP through each of the four listings that share it —
//! `ListWorkspaces`, `ListScrapers`, `ListAnomalyDetectors` and
//! `ListRuleGroupsNamespaces`.
//!
//! The rule: a listing serves at most `maxResults` of the records it selected, in its own
//! stable key order; a request that names no `maxResults` gets that operation's published
//! default page size; `nextToken` is the key of the last entry of the previous page, so
//! the page it opens starts STRICTLY after that key; and a page carries a `nextToken` only
//! while further records remain, so a walk terminates and partitions the listing exactly
//! once.
//!
//! Every suite spans the inputs the rule generalises over — the operation serving the
//! page, the page size (one, a divisor and a non-divisor of the total, the total itself,
//! one past it, the published maximum, and no `maxResults` at all), the cursor (every
//! stored key, one before all of them, one past all of them) and the key type each listing
//! pages by (a minted id, a seeded id, a caller-chosen name) — rather than one recorded
//! request's values.
//!
//! The request harness (`Amp`), the create helpers and the scraper seeding helpers are the
//! ones the other suites in this target already build.

use crate::anomaly_detector_lifecycle::{create_detector, minimal_configuration};
use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace};
use crate::scraper_listing::{eks_source, scraper, seed, workspace_destination};
use crate::workspace_lifecycle::{encode_label, text, Amp};
use serde_json::json;
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    ANOMALY_DETECTOR_PAGE_DEFAULT, ANOMALY_DETECTOR_PAGE_LIMIT, RULE_GROUPS_NAMESPACE_PAGE_DEFAULT,
    RULE_GROUPS_NAMESPACE_PAGE_LIMIT, SCRAPER_PAGE_DEFAULT, SCRAPER_PAGE_LIMIT, STATUS_ACTIVE,
    WORKSPACE_PAGE_DEFAULT, WORKSPACE_PAGE_LIMIT,
};

/// How many records each listing is seeded with: fewer than any published default page
/// size, so the default-page row below reports them all, and more than the page sizes the
/// table walks, so every row crosses at least one page boundary.
const SEEDED: usize = 7;

/// The four listings that page by the shared rule. One row of the outer table of every
/// suite here.
#[derive(Clone, Copy, Debug)]
enum Which {
    Workspaces,
    Scrapers,
    AnomalyDetectors,
    RuleGroupsNamespaces,
}

/// Every listing under test, so a suite cannot quietly cover three of the four.
const EVERY_LISTING: &[Which] = &[
    Which::Workspaces,
    Which::Scrapers,
    Which::AnomalyDetectors,
    Which::RuleGroupsNamespaces,
];

/// One seeded listing: where to GET it, which output member carries the page, which
/// summary member it is ordered and paged by, the published page-size pair it serves, and
/// the keys the store holds in the order the listing serves them.
struct Listing {
    operation: &'static str,
    path: String,
    member: &'static str,
    key: &'static str,
    default: i32,
    limit: i32,
    keys: Vec<String>,
}

/// Seed one listing with [`SEEDED`] records, each on its OWN engine so one listing's
/// records cannot reach another's page.
async fn seeded(which: Which, region: &str) -> (Amp, Listing) {
    let amp = Amp::new();
    let mut keys: Vec<String> = Vec::new();
    let listing = match which {
        Which::Workspaces => {
            for index in 0..SEEDED {
                let created = amp
                    .call(
                        "POST",
                        "/workspaces",
                        region,
                        &json!({ "alias": format!("paged-{index:03}") }).to_string(),
                    )
                    .await;
                assert_eq!(
                    created.status, 202,
                    "seeding workspace {index}: the create must answer 202, got {created:?}"
                );
                keys.push(text(&created.body["workspaceId"]));
            }
            Listing {
                operation: "ListWorkspaces",
                path: "/workspaces".to_string(),
                member: "workspaces",
                key: "workspaceId",
                default: WORKSPACE_PAGE_DEFAULT,
                limit: WORKSPACE_PAGE_LIMIT,
                keys: Vec::new(),
            }
        }
        Which::Scrapers => {
            for index in 0..SEEDED {
                let scraper_id = format!("scraper-{index:03}");
                seed(
                    &amp,
                    &scraper(
                        &scraper_id,
                        Some("paged"),
                        STATUS_ACTIVE,
                        eks_source("paged-cluster"),
                        workspace_destination("ws-33333333-3333-3333-3333-333333333333"),
                    ),
                );
                keys.push(scraper_id);
            }
            Listing {
                operation: "ListScrapers",
                path: "/scrapers".to_string(),
                member: "scrapers",
                key: "scraperId",
                default: SCRAPER_PAGE_DEFAULT,
                limit: SCRAPER_PAGE_LIMIT,
                keys: Vec::new(),
            }
        }
        Which::AnomalyDetectors => {
            let workspace_id = amp.active_workspace(region, "detector-paging-host").await;
            for index in 0..SEEDED {
                let created = create_detector(
                    &amp,
                    region,
                    &workspace_id,
                    &json!({
                        "alias": format!("paged-{index:03}"),
                        "configuration": minimal_configuration("avg(rate(requests_total[5m]))"),
                    }),
                )
                .await;
                assert_eq!(
                    created.status, 202,
                    "seeding detector {index}: the create must answer 202, got {created:?}"
                );
                keys.push(text(&created.body["anomalyDetectorId"]));
            }
            Listing {
                operation: "ListAnomalyDetectors",
                path: format!("/workspaces/{workspace_id}/anomalydetectors"),
                member: "anomalyDetectors",
                key: "anomalyDetectorId",
                default: ANOMALY_DETECTOR_PAGE_DEFAULT,
                limit: ANOMALY_DETECTOR_PAGE_LIMIT,
                keys: Vec::new(),
            }
        }
        Which::RuleGroupsNamespaces => {
            let workspace_id = amp.active_workspace(region, "namespace-paging-host").await;
            for index in 0..SEEDED {
                let name = format!("rules-{index:03}");
                let created = create_namespace(
                    &amp,
                    region,
                    &workspace_id,
                    &create_body(
                        &name,
                        format!("groups: [{{name: {name}}}]").as_bytes(),
                        &[],
                    ),
                )
                .await;
                assert_eq!(
                    created.status, 202,
                    "seeding namespace {name:?}: the create must answer 202, got {created:?}"
                );
                keys.push(name);
            }
            Listing {
                operation: "ListRuleGroupsNamespaces",
                path: format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
                member: "ruleGroupsNamespaces",
                key: "name",
                default: RULE_GROUPS_NAMESPACE_PAGE_DEFAULT,
                limit: RULE_GROUPS_NAMESPACE_PAGE_LIMIT,
                keys: Vec::new(),
            }
        }
    };

    // The order the listing serves its records in is its key order, whatever order the
    // seeding happened to use.
    keys.sort();
    (amp, Listing { keys, ..listing })
}

/// One page of one listing: the keys it carries in page order, and the `nextToken` it
/// carries (`None` when it carries none).
async fn page(
    amp: &Amp,
    region: &str,
    listing: &Listing,
    case: &str,
    max_results: Option<i32>,
    cursor: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut query: Vec<String> = Vec::new();
    if let Some(max) = max_results {
        query.push(format!("maxResults={max}"));
    }
    if let Some(token) = cursor {
        query.push(format!("nextToken={}", encode_label(token)));
    }
    let uri = if query.is_empty() {
        listing.path.clone()
    } else {
        format!("{}?{}", listing.path, query.join("&"))
    };

    let reply = amp.call("GET", &uri, region, "").await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: {} must answer 200, got {reply:?}",
        listing.operation
    );
    let entries = reply.body[listing.member].as_array().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {} must report {} as a list, got {reply:?}",
            listing.operation, listing.member
        )
    });
    let keys: Vec<String> = entries
        .iter()
        .map(|entry| text(&entry[listing.key]))
        .collect();
    let token = reply.body["nextToken"].as_str().map(ToString::to_string);
    (keys, token)
}

/// Every listing bounds its page the same way and carries the same cursor: a walk over
/// any page size reports every stored key once, in key order, with the token equal to the
/// last key of the page it ends and absent on the last page.
#[tokio::test]
async fn every_listing_pages_by_the_same_rule() {
    let region = "us-west-2";

    for which in EVERY_LISTING {
        let (amp, listing) = seeded(*which, region).await;
        assert_eq!(
            listing.keys.len(),
            SEEDED,
            "case {:?}: the seeding must leave {SEEDED} records, got {:?}",
            listing.operation,
            listing.keys
        );

        // One row per page size the rule generalises over: a single entry, a divisor and a
        // non-divisor of the total, the total itself, one past it, the published maximum,
        // and no `maxResults` at all (the operation's documented default).
        let sizes: Vec<Option<i32>> = vec![
            Some(1),
            Some(2),
            Some(3),
            Some(SEEDED as i32 - 1),
            Some(SEEDED as i32),
            Some(SEEDED as i32 + 1),
            Some(listing.limit),
            None,
        ];

        for max_results in sizes {
            let case = format!(
                "{} at maxResults {}",
                listing.operation,
                max_results.map_or("<omitted>".to_string(), |size| size.to_string())
            );
            let bound = max_results.unwrap_or(listing.default) as usize;
            let mut walked: Vec<String> = Vec::new();
            let mut cursor: Option<String> = None;
            let mut pages = 0usize;

            loop {
                let (keys, token) =
                    page(&amp, region, &listing, &case, max_results, cursor.as_deref()).await;
                pages += 1;
                assert!(
                    keys.len() <= bound,
                    "case {case:?}: a page holds at most {bound} entries, got {keys:?}"
                );
                let remaining = listing.keys.len() - walked.len();
                assert_eq!(
                    keys.len(),
                    remaining.min(bound),
                    "case {case:?}: page {pages} must hold {} entries, got {keys:?}",
                    remaining.min(bound)
                );
                walked.extend(keys.clone());

                match &token {
                    Some(emitted) => {
                        assert_eq!(
                            Some(emitted.as_str()),
                            keys.last().map(String::as_str),
                            "case {case:?}: the token must be the last key of the page it \
                             ends, got {token:?} after {keys:?}"
                        );
                        assert!(
                            walked.len() < listing.keys.len(),
                            "case {case:?}: a page holding the last record owes no token, \
                             got {token:?}"
                        );
                    }
                    None => assert_eq!(
                        walked.len(),
                        listing.keys.len(),
                        "case {case:?}: the walk stopped after {} of {} records, got {walked:?}",
                        walked.len(),
                        listing.keys.len()
                    ),
                }

                cursor = token;
                if cursor.is_none() {
                    break;
                }
                assert!(
                    pages <= listing.keys.len(),
                    "case {case:?}: the listing kept emitting a token after {pages} pages"
                );
            }

            assert_eq!(
                walked, listing.keys,
                "case {case:?}: the pages must partition the listing in key order, got {walked:?}"
            );
            assert_eq!(
                pages,
                listing.keys.len().div_ceil(bound).max(1),
                "case {case:?}: {} records at {bound} per page is {} pages, walked {pages}",
                listing.keys.len(),
                listing.keys.len().div_ceil(bound).max(1)
            );
        }
    }
}

/// The cursor is EXCLUSIVE and is only a filter over the records the listing selected: a
/// token opens the page strictly after that key, a key no record carries drops exactly the
/// records at or before it, and a cursor past every key leaves an empty page owing no
/// token.
#[tokio::test]
async fn the_cursor_opens_the_page_strictly_after_its_key() {
    let region = "eu-west-1";

    for which in EVERY_LISTING {
        let (amp, listing) = seeded(*which, region).await;
        let whole = listing.keys.clone();
        let last = whole.last().expect("a seeded listing").clone();

        // Every stored key as the cursor, so the row holds at each page boundary the
        // listing can have — including the last one.
        for (index, cursor) in whole.iter().enumerate() {
            let case = format!("{} resumed at key {index}", listing.operation);
            let expected: Vec<String> = whole[index + 1..].to_vec();
            let (keys, token) = page(
                &amp,
                region,
                &listing,
                &case,
                Some(SEEDED as i32),
                Some(cursor),
            )
            .await;
            assert_eq!(
                keys, expected,
                "case {case:?}: expected {expected:?}, got {keys:?}"
            );
            assert_eq!(
                token, None,
                "case {case:?}: the page holds every remaining record, so it owes no \
                 token, got {token:?}"
            );
        }

        // A cursor no record carries is still a bound on the keys, not a lookup: one that
        // sorts before every key keeps the whole listing, one past every key keeps none.
        let rows: Vec<(&str, String, Vec<String>)> = vec![
            ("a cursor before every key", "!".to_string(), whole.clone()),
            (
                "a cursor one character past the last key",
                format!("{last}~"),
                Vec::new(),
            ),
            (
                "a cursor no record carries, sorting past them all",
                "zzzz-no-such-record".to_string(),
                Vec::new(),
            ),
        ];
        for (row, cursor, expected) in rows {
            let case = format!("{}: {row}", listing.operation);
            let (keys, token) = page(
                &amp,
                region,
                &listing,
                &case,
                Some(SEEDED as i32),
                Some(&cursor),
            )
            .await;
            assert_eq!(
                keys, expected,
                "case {case:?}: expected {expected:?}, got {keys:?}"
            );
            assert_eq!(
                token, None,
                "case {case:?}: nothing remains behind this page, so it owes no token, \
                 got {token:?}"
            );
        }

        // Reading a page twice reports the same page: paging is a read, so the cursor
        // consumes nothing a later caller would have seen.
        let case = format!("{}: read twice", listing.operation);
        let (first, first_token) = page(&amp, region, &listing, &case, Some(2), None).await;
        let (again, again_token) = page(&amp, region, &listing, &case, Some(2), None).await;
        assert_eq!(
            (first, first_token),
            (again, again_token),
            "case {case:?}: a listing must not change what a later caller sees"
        );
    }
}

/// A request that names no `maxResults` gets the page size that operation publishes, and
/// every listing of this service publishes the same pair of bounds, so no listing bounds a
/// page differently from the next.
#[tokio::test]
async fn an_omitted_max_results_serves_the_published_default_page() {
    let region = "ap-south-1";

    for which in EVERY_LISTING {
        let (amp, listing) = seeded(*which, region).await;
        let case = format!("{}: no maxResults", listing.operation);
        assert!(
            listing.default as usize >= SEEDED,
            "case {case:?}: this suite seeds fewer than the default page size, which is \
             {}",
            listing.default
        );
        let (keys, token) = page(&amp, region, &listing, &case, None, None).await;
        assert_eq!(
            keys, listing.keys,
            "case {case:?}: the default page holds all {SEEDED} records, got {keys:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: one page holds everything, so no token is owed, got {token:?}"
        );
        assert_eq!(
            (listing.default, listing.limit),
            (WORKSPACE_PAGE_DEFAULT, WORKSPACE_PAGE_LIMIT),
            "case {case:?}: every listing of this service publishes the same page-size \
             pair, got ({}, {})",
            listing.default,
            listing.limit
        );
    }
}
