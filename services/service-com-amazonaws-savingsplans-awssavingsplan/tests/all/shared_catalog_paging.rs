// SPDX-License-Identifier: CC0-1.0

//! The paging rule the crate's three catalog walks share, driven through one
//! operation of each.
//!
//! All three walks now page through the one window helper beside
//! `crate::state::paginate`, and the two catalog SEARCHES take their cursor from
//! the one rule that lives there too:
//!
//! * a page never exceeds the size the caller asked for, and paging yields the
//!   result set exactly once, in one order, however the set is cut into pages;
//! * a SEARCH always answers its first call with the cursor its walk reached —
//!   even when that page already held every match, and even when nothing
//!   matched — and a resumed call carries a cursor only while results remain
//!   past its page;
//! * a plan's rate card, which reports a cursor only while results remain, keeps
//!   that answer: sharing the window changed no cursor.
//!
//! The rows span what the rule generalises over — which operation, how large the
//! result set is (nothing, a handful, more than the service's own page) and the
//! page size asked for (one row at a time, a size that divides the set, exactly
//! its size, one past it, the largest the model admits). Everything goes through
//! the same `MultiServiceRouter` the emulator boots, so what is pinned is the
//! HTTP behaviour; the harness (`Emulator`, `text`) is the one the sibling
//! lifecycle suite defines.

use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR, THREE_YEARS};

/// The offering search: the first former copy of the cursor rule.
const OFFERINGS: &str = "DescribeSavingsPlansOfferings";

/// The offering-rate search: the second former copy of the cursor rule, and one
/// of the two former copies of the page window.
const OFFERING_RATES: &str = "DescribeSavingsPlansOfferingRates";

/// A purchased plan's rate card: the third former copy of the page window.
const PLAN_RATES: &str = "DescribeSavingsPlanRates";

/// The member every one of the three answers reports its page in.
const RESULTS: &str = "searchResults";

/// The largest page any of the three admits (`PageSize`/`MaxResults` top out at
/// 1000).
const LARGEST_PAGE: i64 = 1000;

/// A walk is finite: a bound far past the pages any row below needs, so a cursor
/// rule that failed to terminate fails the test instead of hanging it.
const MAX_PAGES: usize = 4_096;

/// Whether `token` satisfies the model's `PaginationToken` (length 0..1024,
/// pattern `^[A-Za-z0-9/=\+]+$`).
fn is_pagination_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 1024
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '=' | '+'))
}

/// A row, as its own canonical JSON: two different rows of one of these answers
/// differ somewhere, so this identifies a row without the three answer shapes
/// needing three key functions.
fn row_key(row: &Value) -> String {
    row.to_string()
}

/// Every page of one walk of `operation` at `page_size`, as `(rows, nextToken)`.
///
/// `request` is sent as supplied, with `maxResults` set for every page and
/// `nextToken` added for each page after the first.
async fn walk(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    request: &Value,
    page_size: i64,
) -> Vec<(Vec<Value>, Option<String>)> {
    let mut pages: Vec<(Vec<Value>, Option<String>)> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut page_request = request.clone();
        page_request["maxResults"] = json!(page_size);
        if let Some(token) = &next {
            page_request["nextToken"] = json!(token);
        }
        let answer = emulator.call(operation, page_request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from {operation}, got {} {:?}",
            answer.status, answer.body
        );
        let rows: Vec<Value> = answer.list(RESULTS).to_vec();
        next = answer.get("nextToken").as_str().map(ToString::to_string);
        if let Some(token) = &next {
            assert!(
                is_pagination_token(token),
                "case {case:?} page {}: {token:?} must satisfy PaginationToken",
                pages.len()
            );
        }
        pages.push((rows, next.clone()));
        assert!(
            pages.len() < MAX_PAGES,
            "case {case:?}: a walk must terminate, got {} pages",
            pages.len()
        );
        if next.is_none() {
            return pages;
        }
    }
}

/// The whole result set of `request`, as row keys in the order the walk serves
/// them.
async fn whole_set(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    request: &Value,
) -> Vec<String> {
    walk(emulator, case, operation, request, LARGEST_PAGE)
        .await
        .iter()
        .flat_map(|(rows, _)| rows.iter().map(row_key))
        .collect()
}

/// The page sizes a result set of `n` rows is walked at: below it, at it, one
/// past it, and the largest page the model admits.
///
/// A small set is walked at the sharpest sizes, one row at a time included; a
/// larger one at a size that halves it, so a row stays a handful of calls while
/// still spanning several pages.
fn page_sizes(n: usize) -> Vec<i64> {
    let size = i64::try_from(n).unwrap_or(LARGEST_PAGE).clamp(1, LARGEST_PAGE);
    let mut sizes = vec![size, size.saturating_add(1), LARGEST_PAGE];
    if n <= 32 {
        sizes.extend([1, 2, 3]);
    } else {
        sizes.push((size / 2).max(1));
    }
    sizes.retain(|size| (1..=LARGEST_PAGE).contains(size));
    sizes.sort_unstable();
    sizes.dedup();
    sizes
}

/// How many calls a SEARCH's walk takes to serve `n` rows at `page_size`.
///
/// The walk serves the set in pages of `page_size`, and the first answer always
/// carries a cursor — so a set that fits in one page (`n == 0` included) costs
/// one more call to learn the walk is over, and a set that does not ends on the
/// page that reaches it.
fn expected_search_pages(n: usize, page_size: usize) -> usize {
    n.div_ceil(page_size).max(2)
}

/// Assert one walk served `whole` once, in order, in pages no larger than asked
/// for.
fn assert_walked_once(
    case: &str,
    pages: &[(Vec<Value>, Option<String>)],
    whole: &[String],
    page_size: usize,
) {
    for (index, (rows, _)) in pages.iter().enumerate() {
        assert!(
            rows.len() <= page_size,
            "case {case:?} page {index}: a page must hold at most {page_size} rows, got {}",
            rows.len()
        );
    }
    let walked: Vec<String> = pages
        .iter()
        .flat_map(|(rows, _)| rows.iter().map(row_key))
        .collect();
    assert_eq!(
        walked, whole,
        "case {case:?}: paging must yield the result set once, in order: expected {} rows, got {}",
        whole.len(),
        walked.len()
    );
}

/// Assert the cursor came back on exactly the pages the rule says: the last page
/// of a walk carries none and every page before it carries one.
fn assert_cursor_ends_the_walk(case: &str, pages: &[(Vec<Value>, Option<String>)]) {
    for (index, (_, next)) in pages.iter().enumerate() {
        let last = index + 1 == pages.len();
        assert_eq!(
            next.is_none(),
            last,
            "case {case:?} page {index}: only the page a walk ends on carries no cursor, \
             got {next:?}"
        );
    }
}

/// Both catalog searches page by the one shared rule: the first answer always
/// carries the cursor its walk reached, a resumed one carries it only while
/// results remain past its page, and the set comes back once in order whatever
/// page size cuts it.
#[tokio::test]
async fn both_catalog_searches_page_by_the_same_cursor_rule() {
    let emulator = Emulator::new();
    // Two offerings whose rate sets are small, so the page sizes below straddle
    // them: one that prices a line item per database product, and one scoped to
    // a single instance family.
    let database = text(
        &emulator
            .offering("Database", state::PAYMENT_NO_UPFRONT, ONE_YEAR)
            .await,
        "offeringId",
    );
    let instance = text(
        &emulator
            .offering("EC2Instance", state::PAYMENT_ALL_UPFRONT, THREE_YEARS)
            .await,
        "offeringId",
    );

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "the offerings of one plan type",
            OFFERINGS,
            json!({ "planTypes": ["Compute"] }),
        ),
        (
            "the offerings of one plan type, term and payment option",
            OFFERINGS,
            json!({
                "planTypes": ["SageMaker"],
                "paymentOptions": [state::PAYMENT_ALL_UPFRONT],
                "durations": [ONE_YEAR],
            }),
        ),
        (
            "the offerings of one plan type in one region",
            OFFERINGS,
            json!({
                "planTypes": ["EC2Instance"],
                "paymentOptions": [state::PAYMENT_NO_UPFRONT],
                "durations": [ONE_YEAR],
                "filters": [{ "name": "region", "values": ["us-east-1"] }],
            }),
        ),
        (
            "offerings nothing admits",
            OFFERINGS,
            json!({ "offeringIds": ["00000000-0000-4000-8000-000000000000"] }),
        ),
        (
            "the rates of one offering",
            OFFERING_RATES,
            json!({ "savingsPlanOfferingIds": [database.clone()] }),
        ),
        (
            "the rates of one scoped offering",
            OFFERING_RATES,
            json!({ "savingsPlanOfferingIds": [instance.clone()] }),
        ),
        (
            "the rates of one offering and one product",
            OFFERING_RATES,
            json!({ "savingsPlanOfferingIds": [database.clone()], "products": ["DynamoDB"] }),
        ),
        (
            "rates nothing admits",
            OFFERING_RATES,
            json!({ "savingsPlanOfferingIds": [instance.clone()], "products": ["Lambda"] }),
        ),
    ];

    for (label, operation, request) in cases {
        let whole = whole_set(&emulator, label, operation, &request).await;

        for page_size in page_sizes(whole.len()) {
            let case = format!("{label} at maxResults {page_size}");
            let size = usize::try_from(page_size).unwrap_or(1);
            let pages = walk(&emulator, &case, operation, &request, page_size).await;

            assert!(
                pages[0].1.is_some(),
                "case {case:?}: the first answer of a search always carries the cursor to \
                 resume at, got none beside {} rows",
                pages[0].0.len()
            );
            assert_cursor_ends_the_walk(&case, &pages);
            assert_walked_once(&case, &pages, &whole, size);
            let expected = expected_search_pages(whole.len(), size);
            assert_eq!(
                pages.len(),
                expected,
                "case {case:?}: a {}-row search answered at {page_size} a page takes \
                 {expected} calls, got {}",
                whole.len(),
                pages.len()
            );
        }
    }
}

/// A purchased plan's rate card pages through the same shared window, and keeps
/// its own cursor rule: a cursor comes back only while results remain, so the
/// walk is exactly the pages that carry rows.
#[tokio::test]
async fn the_plan_rate_card_pages_through_the_shared_window() {
    let emulator = Emulator::new();

    // Two cards of different size and scope, each bought from an offering the
    // catalog sells.
    let plans: [(&str, &str, &str, i64, &str); 2] = [
        (
            "a database plan",
            "Database",
            state::PAYMENT_NO_UPFRONT,
            ONE_YEAR,
            "0.5",
        ),
        (
            "an instance plan",
            "EC2Instance",
            state::PAYMENT_ALL_UPFRONT,
            THREE_YEARS,
            "1.25",
        ),
    ];

    for (label, plan_type, payment_option, duration, commitment) in plans {
        let offering = emulator.offering(plan_type, payment_option, duration).await;
        let savings_plan_id = emulator
            .purchase(json!({
                "savingsPlanOfferingId": text(&offering, "offeringId"),
                "commitment": commitment,
            }))
            .await;
        let request = json!({ "savingsPlanId": savings_plan_id });
        let whole = whole_set(&emulator, label, PLAN_RATES, &request).await;
        assert!(
            !whole.is_empty(),
            "case {label:?}: a purchased plan must have a rate card, got none"
        );

        for page_size in page_sizes(whole.len()) {
            let case = format!("{label} at maxResults {page_size}");
            let size = usize::try_from(page_size).unwrap_or(1);
            let pages = walk(&emulator, &case, PLAN_RATES, &request, page_size).await;

            assert_cursor_ends_the_walk(&case, &pages);
            assert_walked_once(&case, &pages, &whole, size);
            let expected = whole.len().div_ceil(size);
            assert_eq!(
                pages.len(),
                expected,
                "case {case:?}: a {}-line card carries a cursor only while rows remain, so it \
                 takes {expected} calls, got {}",
                whole.len(),
                pages.len()
            );
            for (index, (rows, next)) in pages.iter().enumerate() {
                if next.is_some() {
                    assert_eq!(
                        rows.len(),
                        size,
                        "case {case:?} page {index}: a page that carries a cursor is a full \
                         page, got {} rows",
                        rows.len()
                    );
                }
            }
        }
    }
}
