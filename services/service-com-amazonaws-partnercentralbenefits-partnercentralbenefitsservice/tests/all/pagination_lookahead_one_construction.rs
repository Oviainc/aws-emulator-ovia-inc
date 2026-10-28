// SPDX-License-Identifier: CC0-1.0

//! The single construction of the page-and-cursor lookahead, driven end to end over the
//! wire through one operation per former hand-rolled copy.
//!
//! The rule under test: a `@paginated` listing of this service reports at most the page
//! size the request asked for, takes exactly ONE matching record past the page to decide
//! whether a further page exists, issues its `NextToken` for that extra record and reports
//! that record on the NEXT page rather than this one — so a page carrying a `NextToken` is
//! full, a page holding the last matching record carries none, and paging reports every
//! matching record exactly once. Every listing computes that step in the one place
//! (`state::page_with_cursor`), beside the page size, the cursor encoding and the resume
//! position it already shared, so none of the three can drift from the others.
//!
//! The table spans what the rule generalises over: all three listings that used to compute
//! the lookahead inline (`ListBenefits`, `ListBenefitApplications`,
//! `ListBenefitAllocations`), every seeded catalog, an unfiltered and a filtered walk of
//! each (so the lookahead is shown to count MATCHING records, not stored ones), and page
//! sizes spanning the modelled `MaxResults` range `1..100`
//! (`smithy_query(query="operation",operation="ListBenefits")`): the smallest, two more,
//! the exact size of the listing, and the published maximum.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::SEEDED_CATALOGS;

use crate::benefit_application_lifecycle::{text, Harness};

/// The largest `MaxResults` the model admits (`range(min: 1, max: 100)`).
const MAX_RESULTS_CEILING: usize = 100;

/// What a pagination token holds apart from the record id it names: the catalog it was
/// issued for, ahead of the record id, so a cursor of one catalog's listing cannot resume
/// another's.
const CURSOR_SEPARATOR: char = ':';

/// One `@paginated` listing, as the rule covers it: the three differ only in the collection
/// they page and the output member their page arrives in.
struct Listing {
    /// The `X-Amz-Target` operation name.
    operation: &'static str,
    /// The output member carrying the page's summaries.
    summaries: &'static str,
}

/// Every listing that used to compute the lookahead for itself.
const LISTINGS: [Listing; 3] = [
    Listing {
        operation: "ListBenefits",
        summaries: "BenefitSummaries",
    },
    Listing {
        operation: "ListBenefitApplications",
        summaries: "BenefitApplicationSummaries",
    },
    Listing {
        operation: "ListBenefitAllocations",
        summaries: "BenefitAllocationSummaries",
    },
];

/// One page of `listing`, which must answer 200: the ids it reported, and its `NextToken`.
async fn page_of(
    harness: &Harness,
    case: &str,
    listing: &Listing,
    body: &Value,
) -> (Vec<String>, Option<String>) {
    let out = harness.ok(case, listing.operation, body).await;
    let ids = out[listing.summaries]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {} list, got {out:?}",
                listing.summaries
            )
        })
        .iter()
        .map(|summary| summary["Id"].as_str().unwrap_or("<no Id>").to_string())
        .collect();
    (ids, out["NextToken"].as_str().map(str::to_string))
}

/// The record id a `NextToken` names, so a row can assert WHICH record the cursor was
/// issued for. The token is the hex of the catalog, the separator and the record id.
fn cursor_id(case: &str, token: &str) -> String {
    assert!(
        token.len().is_multiple_of(2),
        "case {case:?}: a cursor is hex-encoded, so it holds an even number of characters, \
         got {token:?}"
    );
    let bytes: Vec<u8> = (0..token.len())
        .step_by(2)
        .map(|at| {
            u8::from_str_radix(&token[at..at + 2], 16)
                .unwrap_or_else(|error| panic!("case {case:?}: {token:?} is not hex: {error}"))
        })
        .collect();
    let decoded = String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("case {case:?}: {token:?} is not text: {error}"));
    decoded
        .split_once(CURSOR_SEPARATOR)
        .unwrap_or_else(|| panic!("case {case:?}: the cursor names no record: {decoded:?}"))
        .1
        .to_string()
}

/// A harness whose every seeded catalog also holds benefit applications, so all three
/// listings hold more records than the smallest page size and a lookahead happens.
async fn harness_with_applications() -> Harness {
    let harness = Harness::new();
    for catalog in SEEDED_CATALOGS {
        let benefit = harness.active_benefit_of(catalog).await;
        let benefit_id = text(catalog, &benefit, "Id");
        for index in 0..4 {
            harness
                .create(
                    catalog,
                    &json!({
                        "Catalog": catalog,
                        "ClientToken": format!("tok-lookahead-{catalog}-{index}"),
                        "BenefitIdentifier": benefit_id,
                        "Name": format!("lookahead-{index}"),
                    }),
                )
                .await;
        }
    }
    harness
}

/// The requests one listing of `catalog` is walked with, each with the number of records it
/// must hold for the row to say anything: unfiltered — where the lookahead must actually
/// happen, so at least two — and narrowed to the `Status` of its own first record (read back
/// from the service, so the row filters on a value this listing really holds rather than a
/// written-in one), which may select a single record and then must carry no cursor at all.
async fn walks(
    harness: &Harness,
    listing: &Listing,
    catalog: &str,
) -> Vec<(String, Value, usize)> {
    let case = format!("{} in {catalog}, unfiltered", listing.operation);
    let out = harness
        .ok(&case, listing.operation, &json!({ "Catalog": catalog }))
        .await;
    let first_status = out[listing.summaries]
        .as_array()
        .and_then(|summaries| summaries.first())
        .map(|summary| text(&case, summary, "Status"))
        .unwrap_or_else(|| panic!("case {case:?}: the listing must report at least one record"));
    vec![
        (case, json!({ "Catalog": catalog }), 2),
        (
            format!(
                "{} in {catalog}, narrowed to Status {first_status}",
                listing.operation
            ),
            json!({ "Catalog": catalog, "Status": [first_status] }),
            1,
        ),
    ]
}

/// Every listing pages through the one lookahead: each page holds exactly the records that
/// remain up to the page size, its `NextToken` is issued for the next matching record and
/// absent once none remains, and the walk reports every matching record exactly once.
#[tokio::test]
async fn every_listing_pages_through_the_one_lookahead() {
    let harness = harness_with_applications().await;

    for listing in &LISTINGS {
        for catalog in SEEDED_CATALOGS {
            for (walk, filter, least) in walks(&harness, listing, catalog).await {
                let (whole, token) = page_of(&harness, &walk, listing, &filter).await;
                assert!(
                    whole.len() >= least,
                    "case {walk:?}: the walk must hold at least {least} records for the rule to \
                     say anything, got {whole:?}"
                );
                assert!(
                    whole.len() < MAX_RESULTS_CEILING,
                    "case {walk:?}: this walk must fit inside one default page, got {} records",
                    whole.len()
                );
                assert_eq!(
                    token, None,
                    "case {walk:?}: a page holding every matching record carries no NextToken"
                );

                // The smallest page the model admits, two more, the exact size of the walk
                // (where the lookahead finds nothing past the page) and the published
                // maximum.
                let sizes = [1usize, 2, 3, whole.len(), MAX_RESULTS_CEILING];
                for page_size in sizes {
                    let case = format!("{walk}, MaxResults {page_size}");
                    let mut reported: Vec<String> = Vec::new();
                    let mut cursor: Option<String> = None;
                    let mut pages = 0usize;
                    loop {
                        let mut body = filter.clone();
                        body["MaxResults"] = json!(page_size);
                        if let Some(cursor) = &cursor {
                            body["NextToken"] = json!(cursor);
                        }
                        let (ids, next) = page_of(&harness, &case, listing, &body).await;

                        let remaining = whole.len() - reported.len();
                        assert_eq!(
                            ids.len(),
                            remaining.min(page_size),
                            "case {case:?}: with {remaining} matching records left a page reports \
                             exactly {} of them, got {ids:?}",
                            remaining.min(page_size)
                        );
                        assert_eq!(
                            next.is_some(),
                            remaining > page_size,
                            "case {case:?}: a NextToken is carried exactly when records remain \
                             past this page ({remaining} left, reported {ids:?}), got {next:?}"
                        );
                        if let Some(next) = &next {
                            assert_eq!(
                                cursor_id(&case, next),
                                whole[reported.len() + ids.len()],
                                "case {case:?}: the cursor must name the next matching record, \
                                 not one this page already reported ({ids:?})"
                            );
                        }

                        reported.extend(ids);
                        cursor = next;
                        pages += 1;
                        assert!(
                            pages <= whole.len() + 1,
                            "case {case:?}: paging did not terminate after {pages} pages, \
                             reported {reported:?}"
                        );
                        if cursor.is_none() {
                            break;
                        }
                    }

                    assert_eq!(
                        reported, whole,
                        "case {case:?}: paging must report every matching record exactly once, in \
                         the unpaged order, got {reported:?}"
                    );
                    let distinct: BTreeSet<&String> = reported.iter().collect();
                    assert_eq!(
                        distinct.len(),
                        reported.len(),
                        "case {case:?}: the record a cursor was issued for is reported once, on \
                         the page that follows, got {reported:?}"
                    );
                    assert_eq!(
                        pages,
                        whole.len().div_ceil(page_size),
                        "case {case:?}: {} records at {page_size} per page is {} pages, got \
                         {pages}",
                        whole.len(),
                        whole.len().div_ceil(page_size)
                    );
                }
            }
        }
    }
}
