// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListQuotes`, driven over the wire through the same front
//! door as the rest of this crate's suites (whose harness it reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `ListQuotes` reports every quote of the region the request is addressed to,
//!    exactly once, in creation order, and each one as the summary of ITS OWN stored
//!    record: every member the modelled `QuoteSummary` carries is the stored quote's
//!    own value, and `OrderingRequirements` is the one member of a `Quote` the
//!    summary does not carry.
//! 2. The list is cut into pages of at most `MaxResults` with a `NextToken` that
//!    resumes exactly where the previous page stopped — for any page size — and a
//!    page that exhausts the list issues no token.
//! 3. A `NextToken` this service did not issue for THIS list is a bad request (400
//!    `ValidationException`), never a silently truncated or empty page; so is a page
//!    size outside the modelled range.
//! 4. A quote belongs to the region it was created in, so each region's list reports
//!    that region's quotes and no other's.
//! 5. A list reports the status each quote holds at the instant it is asked for: the
//!    expiry owed at a quote's own `ExpirationDate` shows in the list exactly as it
//!    shows in a read, and listing changes nothing else about any quote.

use std::collections::BTreeSet;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use crate::quote_lifecycle::{create_quote, get_quote, request};
use crate::site_and_outpost_lifecycle::{
    query_value, refusal, text, without, Outposts, Refusal, HOME_REGION,
};

/// The one member a `Quote` carries that the modelled `QuoteSummary` does not.
///
/// Source: `smithy_query(query="shape", shape="Quote,QuoteSummary")` against this
/// service's model — the two shapes agree member for member apart from this one.
const NOT_IN_SUMMARY: &str = "OrderingRequirements";

/// `GET /quotes` addressed to `region`, returning the whole output.
async fn list_quotes_in(outposts: &Outposts, region: &str, query: &str) -> Value {
    outposts
        .ok_in(region, "GET", &format!("/quotes{query}"), None)
        .await
}

/// `GET /quotes` in [`HOME_REGION`], returning the whole output.
async fn list_quotes(outposts: &Outposts, query: &str) -> Value {
    list_quotes_in(outposts, HOME_REGION, query).await
}

/// `CreateQuote` addressed to `region`, returning the `Quote` it answered with.
async fn create_quote_in(outposts: &Outposts, region: &str, body: &Value) -> Value {
    outposts.ok_in(region, "POST", "/quotes", Some(body)).await["Quote"].clone()
}

/// The `QuoteSummary` list an output carries, which is always present — the modelled
/// `QuoteSummaryListDefinition` allows the empty list, so "no quotes" is `[]`.
fn summaries(output: &Value) -> Vec<Value> {
    output
        .get("Quotes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("ListQuotes answers a Quotes list, got {output}"))
}

/// The `QuoteId` of every summary an output carries, in the order it reported them.
fn ids(output: &Value) -> Vec<String> {
    summaries(output)
        .iter()
        .map(|summary| text(summary, "QuoteId"))
        .collect()
}

/// A set of `CreateQuote` bodies spanning what a quote generalises over: how many
/// requested capacities and of which modelled types, whether the optional
/// constraints, payment options, payment terms and description are present and how
/// many of each, the country code, and whether the quote is associated with an
/// Outpost.
fn quote_bodies(outpost_id: &str, outpost_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "required members only, empty capacity list",
            request("US", json!([]), json!({})),
        ),
        (
            "one EC2 capacity",
            request(
                "GB",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 3.0 }]),
                json!({}),
            ),
        ),
        (
            "every modelled capacity type and a description",
            request(
                "DE",
                json!([
                    { "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 6.0 },
                    { "QuoteCapacityType": "EBS", "Unit": "TB", "Quantity": 1.5 },
                    { "QuoteCapacityType": "S3", "Unit": "TB", "Quantity": 24.0 },
                ]),
                json!({ "Description": "three capacities" }),
            ),
        ),
        (
            "constraints, every payment option and every payment term",
            request(
                "JP",
                json!([{ "QuoteCapacityType": "S3", "Unit": "TB", "Quantity": 50.0 }]),
                json!({
                    "RequestedConstraints": [
                        { "QuoteConstraintType": "RACK_MAXIMUM", "Value": "2" },
                        { "QuoteConstraintType": "RACK_SPACE_CONSTRAINED", "Value": "true" },
                    ],
                    "RequestedPaymentOptions": ["ALL_UPFRONT", "NO_UPFRONT", "PARTIAL_UPFRONT"],
                    "RequestedPaymentTerms": ["ONE_YEAR", "THREE_YEARS", "FIVE_YEARS"],
                }),
            ),
        ),
        (
            "associated with an Outpost by bare id, empty description",
            request(
                "FR",
                json!([{ "QuoteCapacityType": "EBS", "Unit": "TB", "Quantity": 0.5 }]),
                json!({ "OutpostIdentifier": outpost_id, "Description": "" }),
            ),
        ),
        (
            "associated with the same Outpost by ARN, multi-line description",
            request(
                "CA",
                json!([]),
                json!({
                    "OutpostIdentifier": outpost_arn,
                    "Description": "first line\nsecond line",
                }),
            ),
        ),
    ]
}

/// Rule 1: every persisted quote is reported, once, in creation order, as the
/// summary of its own record — and listing is a read, so repeating it answers the
/// same thing and a later `GetQuote` still answers what the create reported.
#[tokio::test]
async fn list_quotes_reports_every_persisted_quote_as_its_own_summary() {
    let outposts = Outposts::new();

    // A region nothing was created in lists nothing, as the empty list the modelled
    // `length(min: 0)` allows rather than as an absent member.
    let empty = list_quotes(&outposts, "").await;
    assert!(
        summaries(&empty).is_empty() && empty.get("NextToken").is_none(),
        "with no quotes stored, ListQuotes answers an empty list and no NextToken, got {empty}"
    );

    let (outpost_id, outpost_arn) = outposts.site_and_outpost("quote-listing").await;
    let bodies = quote_bodies(&outpost_id, &outpost_arn);

    let mut created: Vec<(&str, Value)> = Vec::new();
    for (case, body) in &bodies {
        created.push((case, create_quote(&outposts, body).await));
    }

    let listed = list_quotes(&outposts, "").await;
    assert_eq!(
        ids(&listed),
        created
            .iter()
            .map(|(_, quote)| text(quote, "QuoteId"))
            .collect::<Vec<String>>(),
        "ListQuotes must report every persisted quote exactly once in creation order, got {listed}"
    );

    for (index, (case, quote)) in created.iter().enumerate() {
        let summary = &summaries(&listed)[index];
        assert_eq!(
            summary,
            &without(quote, NOT_IN_SUMMARY),
            "case {case:?}: the summary must be the stored quote member for member, got {summary}"
        );
        assert!(
            summary.get(NOT_IN_SUMMARY).is_none(),
            "case {case:?}: {NOT_IN_SUMMARY} is no member of a QuoteSummary, got {summary}"
        );
        // The same record, read on its own route: a list and a read cannot say
        // different things about one quote.
        let read = get_quote(&outposts, &text(quote, "QuoteId")).await;
        assert_eq!(
            summary,
            &without(&read, NOT_IN_SUMMARY),
            "case {case:?}: the summary must match what GetQuote answers, got {summary}"
        );
    }

    // Listing reports; it does not change what a later caller sees.
    let again = list_quotes(&outposts, "").await;
    assert_eq!(
        again, listed,
        "a second ListQuotes must answer exactly what the first did, got {again}"
    );
    for (case, quote) in &created {
        let read = get_quote(&outposts, &text(quote, "QuoteId")).await;
        assert_eq!(
            &read, quote,
            "case {case:?}: listing must not change the quote its create reported"
        );
    }
}

/// Rule 2: for EVERY page size, paging the list from the first `NextToken` to the
/// last yields exactly the persisted quotes, once each, in the same stable order.
///
/// The page sizes span one quote per page, an exact divisor of the count, a size
/// that leaves a short last page, the count itself, and more than the count — so a
/// token that duplicated or skipped a quote at any boundary would show up. A token
/// naming a boundary past the end of the list is a page with nothing in it and no
/// further token.
#[tokio::test]
async fn paging_the_list_covers_every_quote_exactly_once() {
    let outposts = Outposts::new();
    let (outpost_id, outpost_arn) = outposts.site_and_outpost("quote-paging").await;
    let bodies = quote_bodies(&outpost_id, &outpost_arn);

    let mut expected: Vec<String> = Vec::new();
    for (_, body) in &bodies {
        expected.push(text(&create_quote(&outposts, body).await, "QuoteId"));
    }

    let count = expected.len();
    for size in [1usize, 2, 3, count, count + 4] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                Some(token) => format!("?MaxResults={size}&NextToken={}", query_value(token)),
                None => format!("?MaxResults={size}"),
            };
            let page = list_quotes(&outposts, &query).await;
            let page_ids = ids(&page);
            assert!(
                page_ids.len() <= size,
                "case size {size}: a page holds at most MaxResults quotes, got {page}"
            );
            collected.extend(page_ids);
            pages += 1;
            assert!(
                pages <= count + 1,
                "case size {size}: paging must terminate, got {collected:?}"
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case size {size}: paging must report every quote once, in the list's order"
        );
        assert_eq!(
            pages,
            count.div_ceil(size),
            "case size {size}: a list of {count} quotes served {size} at a time takes \
             {} pages, got {pages}",
            count.div_ceil(size)
        );
    }

    // A boundary past the end of the list: nothing left to report, and no token.
    let past_the_end = list_quotes(
        &outposts,
        &format!("?NextToken={}", query_value(&format!("{}##quotes", count + 100))),
    )
    .await;
    assert!(
        summaries(&past_the_end).is_empty() && past_the_end.get("NextToken").is_none(),
        "a token past the end of the list is an empty page with no token, got {past_the_end}"
    );
}

/// Rule 3: a `NextToken` this service did not issue for THIS list, and a page size
/// outside the modelled `range(1..1000)`, are refused as bad requests.
///
/// The rows span a token stamped for each of the other lists this service pages, a
/// token whose boundary is not a number, and both ends of the page-size range plus
/// one past each.
#[tokio::test]
async fn a_foreign_page_token_or_page_size_is_refused() {
    let outposts = Outposts::new();
    create_quote(&outposts, &request("US", json!([]), json!({}))).await;

    let cases: Vec<Refusal> = vec![
        refusal(
            "a token stamped for ListSites",
            "GET",
            "/quotes?NextToken=1%23%23sites".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "a token stamped for ListOutposts",
            "GET",
            "/quotes?NextToken=1%23%23outposts".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "a token stamped for ListCatalogItems",
            "GET",
            "/quotes?NextToken=1%23%23catalogItems".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "a token whose boundary is not a number",
            "GET",
            "/quotes?NextToken=abc%23%23quotes".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "a page size below the modelled minimum",
            "GET",
            "/quotes?MaxResults=0".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "a page size above the modelled maximum",
            "GET",
            "/quotes?MaxResults=1001".to_string(),
            400,
            "ValidationException",
        ),
    ];
    outposts.expect_refusals(&cases).await;

    // Both published bounds of the range are served, not refused.
    for size in [1, 1000] {
        let page = list_quotes(&outposts, &format!("?MaxResults={size}")).await;
        assert_eq!(
            summaries(&page).len(),
            1,
            "case MaxResults={size}: the one stored quote is reported, got {page}"
        );
    }

    // And the stamp cuts both ways: a token ListQuotes issued is not a boundary of
    // another list.
    for (case, body) in [
        ("second", request("GB", json!([]), json!({}))),
        ("third", request("DE", json!([]), json!({}))),
    ] {
        create_quote(&outposts, &body).await;
        let first = list_quotes(&outposts, "?MaxResults=1").await;
        let token = first
            .get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: a short page issues a NextToken, got {first}"))
            .to_string();
        let replayed = outposts
            .call(
                "GET",
                &format!("/sites?NextToken={}", query_value(&token)),
                None,
            )
            .await;
        assert_eq!(
            (replayed.status, replayed.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a ListQuotes token replayed on ListSites is refused, got {} {} {}",
            replayed.status,
            replayed.error_code,
            replayed.body
        );
    }
}

/// Rule 4: each region's list reports that region's quotes and no other's.
#[tokio::test]
async fn each_region_lists_only_its_own_quotes() {
    let outposts = Outposts::new();
    // The counts differ per region so a list answering another region's inventory
    // cannot pass by coincidence.
    let plan: Vec<(&str, usize)> = vec![("us-east-1", 3), ("eu-west-1", 1), ("ap-south-1", 2)];

    let mut expected: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for (region, count) in &plan {
        let mut quote_ids: BTreeSet<String> = BTreeSet::new();
        for index in 0..*count {
            let quote = create_quote_in(
                &outposts,
                region,
                &request(
                    "US",
                    json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 1.0 }]),
                    json!({ "Description": format!("{region} quote {index}") }),
                ),
            )
            .await;
            quote_ids.insert(text(&quote, "QuoteId"));
        }
        expected.push((region, quote_ids));
    }

    for (region, quote_ids) in &expected {
        let listed: BTreeSet<String> = ids(&list_quotes_in(&outposts, region, "").await)
            .into_iter()
            .collect();
        assert_eq!(
            &listed, quote_ids,
            "case {region}: ListQuotes in {region} reports exactly that region's quotes, \
             got {listed:?}"
        );
    }
}

/// Rule 5: the list reports the status each quote holds now — the expiry owed at a
/// quote's own `ExpirationDate` — and nothing else about any quote changes.
///
/// The expiry falls far out, which no test may sleep for, so each row ages the
/// STORED quote to the `ExpirationDate` the emulator itself would have reached and
/// then asks over the wire. The rows span both sides of the boundary.
#[tokio::test]
async fn the_list_reports_the_status_each_quote_holds_now() {
    let outposts = Outposts::new();
    let validity =
        service_com_amazonaws_outposts_outpostsolafservice::state::QUOTE_VALIDITY.as_secs_f64();
    let cases: Vec<(&str, f64, &str)> = vec![
        ("a month past its expiry", -validity, "EXPIRED"),
        ("one second past its expiry", -1.0, "EXPIRED"),
        ("a minute before its expiry", 60.0, "CREATED"),
        ("a whole validity window ahead", validity, "CREATED"),
    ];

    let mut created: Vec<(&str, String, &str)> = Vec::new();
    for (case, offset, expected) in &cases {
        let quote = create_quote(
            &outposts,
            &request(
                "US",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 1.0 }]),
                json!({ "Description": *case }),
            ),
        )
        .await;
        let quote_id = text(&quote, "QuoteId");
        let aged = DateTime::from_secs_f64(
            DateTime::from(std::time::SystemTime::now()).as_secs_f64() + offset,
        );
        {
            let store = outposts.store();
            let mut store = store.lock();
            let record = store
                .quotes
                .get_mut(&quote_id)
                .unwrap_or_else(|| panic!("case {case:?}: the create persisted the quote"));
            record.quote.expiration_date = Some(aged);
        }
        created.push((case, quote_id, expected));
    }

    let listed = list_quotes(&outposts, "").await;
    let summaries = summaries(&listed);
    for (case, quote_id, expected) in &created {
        let summary = summaries
            .iter()
            .find(|summary| text(summary, "QuoteId") == *quote_id)
            .unwrap_or_else(|| panic!("case {case:?}: the list reports the stored quote, got {listed}"));
        let status = text(summary, "QuoteStatus");
        assert_eq!(
            status, *expected,
            "case {case:?}: a quote whose ExpirationDate has{} passed is {expected}, got \
             {status:?}",
            if *expected == "EXPIRED" { "" } else { " not" }
        );
        // The list materialised the expiry the clock owed and nothing else: the
        // quote's own read answers the same record, status included.
        let read = get_quote(&outposts, quote_id).await;
        assert_eq!(
            summary,
            &without(&read, NOT_IN_SUMMARY),
            "case {case:?}: the summary and the read must agree member for member"
        );
    }
}
