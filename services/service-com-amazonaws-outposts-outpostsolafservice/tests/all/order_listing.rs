// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListOrders`, driven over the wire through the same front
//! door as the rest of this crate's suites (whose harness it reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `ListOrders` reports every order of the region the request is addressed to,
//!    exactly once, in creation order, and each one as the summary of ITS OWN stored
//!    record: every member the modelled `OrderSummary` carries is the value the
//!    order's own read reports, `LineItemCountsByStatus` counts the line items of
//!    that record by the status each holds, and the members a read carries that a
//!    summary does not are absent from it.
//! 2. The list is cut into pages of at most `MaxResults` with a `NextToken` that
//!    resumes exactly where the previous page stopped — for any page size — and a
//!    page that exhausts the list issues no token.
//! 3. A `NextToken` this service did not issue for THIS list is a bad request (400
//!    `ValidationException`), never a silently truncated or empty page; so is a page
//!    size outside the modelled range.
//! 4. The Outpost filter narrows the list on the order's own `OutpostId`, by either
//!    identifier form the modelled `OutpostIdentifier` accepts, and an absent filter
//!    narrows nothing. A filter is a filter and not an identifier the request is
//!    addressed to, so one naming an Outpost that holds no order reports no order
//!    rather than failing.
//! 5. An order belongs to the region it was placed in, so each region's list reports
//!    that region's orders and no other's.
//! 6. The list reports the status each order holds at the instant it is asked for —
//!    the clock-owed move past `PREPARING`, and a cancellation, exactly as a read
//!    reports them — and listing changes nothing about any order.

use std::collections::{BTreeSet, HashMap};

use serde_json::{json, Map, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    LINE_ITEM_STATUS_BUILDING, LINE_ITEM_STATUS_CANCELLED, LINE_ITEM_STATUS_PREPARING,
    ORDER_PREPARATION, ORDER_STATUS_CANCELLED, ORDER_STATUS_IN_PROGRESS, ORDER_STATUS_PREPARING,
};

use crate::site_and_outpost_lifecycle::{
    query_value, refusal, segment, text, Outposts, Refusal, ABSENT_OUTPOST_ID, HOME_REGION,
};

/// The route `ListOrders` is bound to: an ACCOUNT-level collection, not one nested
/// under an Outpost — `GET /list-orders`, from
/// `smithy_query(query="operation", operation="ListOrders")`.
const ORDERS_ROUTE: &str = "/list-orders";

/// The seven members the modelled `OrderSummary` carries, from
/// `smithy_query(query="shape", shape="OrderSummary")`.
const SUMMARY_MEMBERS: [&str; 7] = [
    "OutpostId",
    "OrderId",
    "OrderType",
    "Status",
    "LineItemCountsByStatus",
    "OrderSubmissionDate",
    "OrderFulfilledDate",
];

/// The five members `GetOrder` reports that an `OrderSummary` does not, from the same
/// two shapes: the order's own line items — which the summary carries as counts by
/// status instead — and its purchase detail, which the model keeps on the read alone.
const NOT_IN_SUMMARY: [&str; 5] = [
    "LineItems",
    "QuoteIdentifier",
    "QuoteOptionIdentifier",
    "PaymentOption",
    "PaymentTerm",
];

/// Catalog item SKUs of the modelled `SkuCode` form (`^OR-[A-Z0-9]{7}$`), which is
/// the only thing this API says about a catalog item id a line item may name.
const SKUS: [&str; 4] = ["OR-1A2B3C4", "OR-ABCDEFG", "OR-0000001", "OR-ZZZZZZ9"];

/// Wait until an order's clock-owed `PREPARING` -> `IN_PROGRESS` transition has
/// fallen due.
///
/// Blocking with `std::thread::sleep` for the same reason
/// [`crate::site_and_outpost_lifecycle::settle`] does: the workspace's `tokio` carries
/// no timer driver.
fn settle_orders() {
    std::thread::sleep(ORDER_PREPARATION + std::time::Duration::from_millis(400));
}

/// `GET /list-orders` addressed to `region`, returning the whole output.
async fn list_in(outposts: &Outposts, region: &str, query: &str) -> Value {
    outposts
        .ok_in(region, "GET", &format!("{ORDERS_ROUTE}{query}"), None)
        .await
}

/// `GET /list-orders` in [`HOME_REGION`], returning the whole output.
async fn list(outposts: &Outposts, query: &str) -> Value {
    list_in(outposts, HOME_REGION, query).await
}

/// `CreateOrder` addressed to `region`, returning the `Order` it answered with.
async fn create_order_in(outposts: &Outposts, region: &str, body: &Value) -> Value {
    outposts.ok_in(region, "POST", "/orders", Some(body)).await["Order"].clone()
}

/// `CreateOrder` in [`HOME_REGION`], returning the `Order` it answered with.
async fn create_order(outposts: &Outposts, body: &Value) -> Value {
    create_order_in(outposts, HOME_REGION, body).await
}

/// `GetOrder`, returning the `Order` it answered with.
async fn get_order_in(outposts: &Outposts, region: &str, order_id: &str) -> Value {
    outposts
        .ok_in(region, "GET", &format!("/orders/{}", segment(order_id)), None)
        .await["Order"]
        .clone()
}

/// `CancelOrder`, which answers a body with no members.
async fn cancel_order(outposts: &Outposts, order_id: &str) {
    outposts
        .ok(
            "POST",
            &format!("/orders/{}/cancel", segment(order_id)),
            None,
        )
        .await;
}

/// A `CreateOrder` body: the two members the model marks `@required`, with
/// `overrides` merged in.
fn order_request(outpost_identifier: &str, payment_option: &str, overrides: Value) -> Value {
    let mut body = json!({
        "OutpostIdentifier": outpost_identifier,
        "PaymentOption": payment_option,
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), overrides.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    body
}

/// One line item request: a catalog item SKU and a quantity.
fn line_item(sku: &str, quantity: i64) -> Value {
    json!({ "CatalogItemId": sku, "Quantity": quantity })
}

/// A set of `CreateOrder` bodies spanning what an order generalises over: how the
/// Outpost is named (bare id, full ARN), how many line items (one, three, the
/// modelled maximum of twenty), the quantities on them (the modelled minimum and a
/// large one), which catalog item SKUs, every modelled payment option, and whether a
/// payment term is named.
fn order_bodies(outpost_id: &str, outpost_arn: &str) -> Vec<(&'static str, Value)> {
    let twenty: Vec<Value> = (0..20)
        .map(|index| line_item(SKUS[index % SKUS.len()], (index as i64) + 1))
        .collect();
    vec![
        (
            "one line item of the minimum quantity, by bare id",
            order_request(
                outpost_id,
                "ALL_UPFRONT",
                json!({ "LineItems": [line_item(SKUS[0], 1)] }),
            ),
        ),
        (
            "three line items of different SKUs, by ARN",
            order_request(
                outpost_arn,
                "NO_UPFRONT",
                json!({
                    "LineItems": [
                        line_item(SKUS[1], 2),
                        line_item(SKUS[2], 4),
                        line_item(SKUS[3], 1),
                    ],
                    "PaymentTerm": "ONE_YEAR",
                }),
            ),
        ),
        (
            "one line item of a large quantity and a long payment term",
            order_request(
                outpost_id,
                "PARTIAL_UPFRONT",
                json!({
                    "LineItems": [line_item(SKUS[2], 500)],
                    "PaymentTerm": "FIVE_YEARS",
                }),
            ),
        ),
        (
            "the modelled maximum of twenty line items",
            order_request(
                outpost_arn,
                "ALL_UPFRONT",
                json!({ "LineItems": twenty, "PaymentTerm": "THREE_YEARS" }),
            ),
        ),
    ]
}

/// The `OrderSummary` list an output carries, which is always present — the modelled
/// `OrderSummaryListDefinition` allows the empty list, so "no orders" is `[]`.
fn summaries(output: &Value) -> Vec<Value> {
    output
        .get("Orders")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("ListOrders answers an Orders list, got {output}"))
}

/// The `OrderId` of every summary an output carries, in the order it reported them.
fn ids(output: &Value) -> Vec<String> {
    summaries(output)
        .iter()
        .map(|summary| text(summary, "OrderId"))
        .collect()
}

/// How many line items a `GetOrder` answer holds in each `LineItemStatus`, computed
/// from the read's own `LineItems` — what the summary's `LineItemCountsByStatus` has
/// to say about that same record.
fn counts_by_status(read: &Value) -> HashMap<String, i64> {
    let mut counts: HashMap<String, i64> = HashMap::new();
    for item in read
        .get("LineItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        *counts.entry(text(&item, "Status")).or_insert(0) += 1;
    }
    counts
}

/// The `LineItemCountsByStatus` a summary reports, as a map this suite can compare
/// against [`counts_by_status`].
fn reported_counts(summary: &Value) -> HashMap<String, i64> {
    summary
        .get("LineItemCountsByStatus")
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(status, count)| (status.clone(), count.as_i64().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default()
}

/// What a `GetOrder` answer looks like once narrowed to the members an `OrderSummary`
/// carries: the summary a list has to report for that order.
fn summary_of(read: &Value) -> Value {
    let mut narrowed = Map::new();
    for member in SUMMARY_MEMBERS {
        if let Some(value) = read.get(member) {
            narrowed.insert(member.to_string(), value.clone());
        }
    }
    // The one summary member a read does not carry: the counts the read's own line
    // items add up to.
    let counts: Map<String, Value> = counts_by_status(read)
        .into_iter()
        .map(|(status, count)| (status, json!(count)))
        .collect();
    if !counts.is_empty() {
        narrowed.insert(
            "LineItemCountsByStatus".to_string(),
            Value::Object(counts),
        );
    }
    Value::Object(narrowed)
}

/// Rule 1: every persisted order is reported, once, in creation order, as the summary
/// of its own record — and listing is a read, so repeating it answers the same thing
/// and a later `GetOrder` still answers that same record.
#[tokio::test]
async fn list_orders_reports_every_persisted_order_as_its_own_summary() {
    let outposts = Outposts::new();

    // A region nothing was ordered in lists nothing, as an empty list rather than as
    // an absent member.
    let empty = list(&outposts, "").await;
    assert!(
        summaries(&empty).is_empty() && empty.get("NextToken").is_none(),
        "with no orders stored, ListOrders answers an empty list and no NextToken, got {empty}"
    );

    let (outpost_id, outpost_arn) = outposts.site_and_outpost("order-listing").await;
    let bodies = order_bodies(&outpost_id, &outpost_arn);

    let mut created: Vec<(&str, String)> = Vec::new();
    for (case, body) in &bodies {
        let order = create_order(&outposts, body).await;
        created.push((case, text(&order, "OrderId")));
    }
    // Past the clock-owed transition, so the list and the read either side of it
    // cannot be racing it.
    settle_orders();

    let listed = list(&outposts, "").await;
    assert_eq!(
        ids(&listed),
        created
            .iter()
            .map(|(_, order_id)| order_id.clone())
            .collect::<Vec<String>>(),
        "ListOrders must report every persisted order exactly once in creation order, got {listed}"
    );

    for (index, (case, order_id)) in created.iter().enumerate() {
        let summary = &summaries(&listed)[index];
        let read = get_order_in(&outposts, HOME_REGION, order_id).await;
        assert_eq!(
            summary,
            &summary_of(&read),
            "case {case:?}: the summary must be the stored order member for member, got {summary}"
        );
        assert_eq!(
            reported_counts(summary),
            counts_by_status(&read),
            "case {case:?}: LineItemCountsByStatus must count the order's own line items \
             by status, got {summary}"
        );
        for member in NOT_IN_SUMMARY {
            assert!(
                summary.get(member).is_none(),
                "case {case:?}: {member} is no member of an OrderSummary, got {summary}"
            );
        }
    }

    // Listing reports; it does not change what a later caller sees.
    let again = list(&outposts, "").await;
    assert_eq!(
        again, listed,
        "a second ListOrders must answer exactly what the first did, got {again}"
    );
}

/// Rule 2: for EVERY page size, paging the list from the first `NextToken` to the
/// last yields exactly the persisted orders, once each, in the same stable order.
///
/// The page sizes span one order per page, an exact divisor of the count, a size that
/// leaves a short last page, the count itself, and more than the count — so a token
/// that duplicated or skipped an order at any boundary would show up. A token naming a
/// boundary past the end of the list is a page with nothing in it and no further
/// token.
#[tokio::test]
async fn paging_the_list_covers_every_order_exactly_once() {
    let outposts = Outposts::new();
    let (outpost_id, outpost_arn) = outposts.site_and_outpost("order-paging").await;
    let bodies = order_bodies(&outpost_id, &outpost_arn);

    let mut expected: Vec<String> = Vec::new();
    for (_, body) in &bodies {
        expected.push(text(&create_order(&outposts, body).await, "OrderId"));
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
            let page = list(&outposts, &query).await;
            let page_ids = ids(&page);
            assert!(
                page_ids.len() <= size,
                "case size {size}: a page holds at most MaxResults orders, got {page}"
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
            "case size {size}: paging must report every order once, in the list's order"
        );
        assert_eq!(
            pages,
            count.div_ceil(size),
            "case size {size}: a list of {count} orders served {size} at a time takes {} \
             pages, got {pages}",
            count.div_ceil(size)
        );
    }

    // A boundary past the end of the list: nothing left to report, and no token.
    let past_the_end = list(
        &outposts,
        &format!(
            "?NextToken={}",
            query_value(&format!("{}##orders", count + 100))
        ),
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
/// The rows span a token stamped for each of three other lists this service pages, a
/// token whose boundary is not a number, and both ends of the page-size range plus one
/// past each.
#[tokio::test]
async fn a_foreign_page_token_or_page_size_is_refused() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("order-tokens").await;
    create_order(
        &outposts,
        &order_request(
            &outpost_id,
            "ALL_UPFRONT",
            json!({ "LineItems": [line_item(SKUS[0], 1)] }),
        ),
    )
    .await;

    let cases: Vec<Refusal> = vec![
        refusal(
            "a token stamped for ListQuotes",
            "GET",
            format!("{ORDERS_ROUTE}?NextToken=1%23%23quotes"),
            400,
            "ValidationException",
        ),
        refusal(
            "a token stamped for ListOutposts",
            "GET",
            format!("{ORDERS_ROUTE}?NextToken=1%23%23outposts"),
            400,
            "ValidationException",
        ),
        refusal(
            "a token stamped for ListCapacityTasks",
            "GET",
            format!("{ORDERS_ROUTE}?NextToken=1%23%23capacityTasks"),
            400,
            "ValidationException",
        ),
        refusal(
            "a token whose boundary is not a number",
            "GET",
            format!("{ORDERS_ROUTE}?NextToken=abc%23%23orders"),
            400,
            "ValidationException",
        ),
        refusal(
            "a page size below the modelled minimum",
            "GET",
            format!("{ORDERS_ROUTE}?MaxResults=0"),
            400,
            "ValidationException",
        ),
        refusal(
            "a page size above the modelled maximum",
            "GET",
            format!("{ORDERS_ROUTE}?MaxResults=1001"),
            400,
            "ValidationException",
        ),
    ];
    outposts.expect_refusals(&cases).await;

    // Both published bounds of the range are served, not refused.
    for size in [1, 1000] {
        let page = list(&outposts, &format!("?MaxResults={size}")).await;
        assert_eq!(
            summaries(&page).len(),
            1,
            "case MaxResults={size}: the one stored order is reported, got {page}"
        );
    }

    // And the stamp cuts both ways: a token ListOrders issued is not a boundary of
    // another list.
    create_order(
        &outposts,
        &order_request(
            &outpost_id,
            "NO_UPFRONT",
            json!({ "LineItems": [line_item(SKUS[1], 1)] }),
        ),
    )
    .await;
    let first = list(&outposts, "?MaxResults=1").await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a short page issues a NextToken, got {first}"))
        .to_string();
    let replayed = outposts
        .call("GET", &format!("/quotes?NextToken={}", query_value(&token)), None)
        .await;
    assert_eq!(
        (replayed.status, replayed.error_code.as_str()),
        (400, "ValidationException"),
        "a ListOrders token replayed on ListQuotes is refused, got {} {} {}",
        replayed.status,
        replayed.error_code,
        replayed.body
    );
}

/// Rule 4: the Outpost filter narrows the list on the order's own `OutpostId`, by
/// either identifier form, and narrows nothing when it is absent; one naming an
/// Outpost that holds no order reports no order rather than failing.
///
/// The rows differ along how many orders the named Outpost holds (two, one, none) and
/// which identifier form names it — and one Outpost of the region is named by an
/// identifier no Outpost of this store carries at all.
#[tokio::test]
async fn the_outpost_filter_narrows_the_list_to_that_outposts_orders() {
    let outposts = Outposts::new();
    let (busy_id, busy_arn) = outposts.site_and_outpost("order-filter-busy").await;
    let (quiet_id, quiet_arn) = outposts.site_and_outpost("order-filter-quiet").await;
    let (idle_id, idle_arn) = outposts.site_and_outpost("order-filter-idle").await;

    let mut on_busy: Vec<String> = Vec::new();
    for sku in [SKUS[0], SKUS[1]] {
        let order = create_order(
            &outposts,
            &order_request(
                &busy_id,
                "ALL_UPFRONT",
                json!({ "LineItems": [line_item(sku, 1)] }),
            ),
        )
        .await;
        on_busy.push(text(&order, "OrderId"));
    }
    let on_quiet = vec![text(
        &create_order(
            &outposts,
            &order_request(
                &quiet_arn,
                "NO_UPFRONT",
                json!({ "LineItems": [line_item(SKUS[2], 3)] }),
            ),
        )
        .await,
        "OrderId",
    )];

    let mut every: Vec<String> = on_busy.clone();
    every.extend(on_quiet.clone());

    let cases: Vec<(&str, String, Vec<String>)> = vec![
        ("no filter at all", String::new(), every.clone()),
        (
            "an Outpost holding two orders, by bare id",
            format!("?OutpostIdentifierFilter={}", query_value(&busy_id)),
            on_busy.clone(),
        ),
        (
            "the same Outpost, by ARN",
            format!("?OutpostIdentifierFilter={}", query_value(&busy_arn)),
            on_busy.clone(),
        ),
        (
            "an Outpost holding one order, by bare id",
            format!("?OutpostIdentifierFilter={}", query_value(&quiet_id)),
            on_quiet.clone(),
        ),
        (
            "an Outpost holding no order, by bare id",
            format!("?OutpostIdentifierFilter={}", query_value(&idle_id)),
            Vec::new(),
        ),
        (
            "an Outpost holding no order, by ARN",
            format!("?OutpostIdentifierFilter={}", query_value(&idle_arn)),
            Vec::new(),
        ),
        (
            "an Outpost this store does not hold at all",
            format!("?OutpostIdentifierFilter={}", query_value(ABSENT_OUTPOST_ID)),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in &cases {
        let page = list(&outposts, query).await;
        assert_eq!(
            &ids(&page),
            expected,
            "case {case:?}: the filter reports exactly that Outpost's orders, got {page}"
        );
        for summary in summaries(&page) {
            assert!(
                every.contains(&text(&summary, "OrderId")),
                "case {case:?}: the list reports only orders this suite placed, got {summary}"
            );
        }
    }

    // The filter narrows a PAGE too: the boundary a token names is a position in the
    // same narrowed list.
    let first = list(
        &outposts,
        &format!(
            "?MaxResults=1&OutpostIdentifierFilter={}",
            query_value(&busy_id)
        ),
    )
    .await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a short filtered page issues a NextToken, got {first}"))
        .to_string();
    let second = list(
        &outposts,
        &format!(
            "?MaxResults=1&OutpostIdentifierFilter={}&NextToken={}",
            query_value(&busy_id),
            query_value(&token)
        ),
    )
    .await;
    assert_eq!(
        [ids(&first), ids(&second)].concat(),
        on_busy,
        "paging the filtered list reports that Outpost's orders once each, got {second}"
    );
}

/// Rule 5: each region's list reports that region's orders and no other's.
#[tokio::test]
async fn each_region_lists_only_its_own_orders() {
    let outposts = Outposts::new();
    // The counts differ per region so a list answering another region's inventory
    // cannot pass by coincidence.
    let plan: Vec<(&str, usize)> = vec![("us-east-1", 3), ("eu-west-1", 1), ("ap-south-1", 2)];

    let mut expected: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for (region, count) in &plan {
        let site = outposts
            .create_site_in(
                region,
                &crate::site_and_outpost_lifecycle::orderable_site(&format!(
                    "{region}-order-site"
                )),
            )
            .await;
        let outpost = outposts
            .create_outpost_in(
                region,
                &json!({ "Name": format!("{region}-order-outpost"), "SiteId": text(&site, "SiteId") }),
            )
            .await;
        let outpost_id = text(&outpost, "OutpostId");
        let mut order_ids: BTreeSet<String> = BTreeSet::new();
        for index in 0..*count {
            let order = create_order_in(
                &outposts,
                region,
                &order_request(
                    &outpost_id,
                    "ALL_UPFRONT",
                    json!({ "LineItems": [line_item(SKUS[index % SKUS.len()], 1)] }),
                ),
            )
            .await;
            order_ids.insert(text(&order, "OrderId"));
        }
        expected.push((region, order_ids));
    }

    for (region, order_ids) in &expected {
        let listed: BTreeSet<String> = ids(&list_in(&outposts, region, "").await)
            .into_iter()
            .collect();
        assert_eq!(
            &listed, order_ids,
            "case {region}: ListOrders in {region} reports exactly that region's orders, \
             got {listed:?}"
        );
    }
}

/// Rule 6: the list reports the status each order and its line items hold now — the
/// `PREPARING` the submit gave them, the clock-owed move to `IN_PROGRESS` /
/// `BUILDING`, and the `CANCELLED` a cancellation leaves behind — and the summary and
/// the read never disagree.
#[tokio::test]
async fn the_list_reports_the_status_each_order_holds_now() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("order-status").await;

    // Three line items per order, so the counts map is more than a count of one.
    let items = json!({
        "LineItems": [line_item(SKUS[0], 1), line_item(SKUS[1], 2), line_item(SKUS[2], 3)],
    });
    let submitted = text(
        &create_order(
            &outposts,
            &order_request(&outpost_id, "ALL_UPFRONT", items.clone()),
        )
        .await,
        "OrderId",
    );
    let cancelled = text(
        &create_order(
            &outposts,
            &order_request(&outpost_id, "NO_UPFRONT", items.clone()),
        )
        .await,
        "OrderId",
    );
    cancel_order(&outposts, &cancelled).await;

    // Straight after the submit, before the transition it owes falls due.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "an order still being prepared",
            &submitted,
            ORDER_STATUS_PREPARING,
            LINE_ITEM_STATUS_PREPARING,
        ),
        (
            "an order that was cancelled",
            &cancelled,
            ORDER_STATUS_CANCELLED,
            LINE_ITEM_STATUS_CANCELLED,
        ),
    ];
    assert_statuses(&outposts, &cases).await;

    // Once the clock has made the move due, the list reports it — and a cancelled
    // order, which the clock owes nothing, stays where the cancellation left it.
    settle_orders();
    let settled: Vec<(&str, &str, &str, &str)> = vec![
        (
            "an order the clock moved on",
            &submitted,
            ORDER_STATUS_IN_PROGRESS,
            LINE_ITEM_STATUS_BUILDING,
        ),
        (
            "an order that was cancelled",
            &cancelled,
            ORDER_STATUS_CANCELLED,
            LINE_ITEM_STATUS_CANCELLED,
        ),
    ];
    assert_statuses(&outposts, &settled).await;
}

/// Assert, for each row, that the list reports the order in `status` with all three of
/// its line items in `item_status`, and that `GetOrder` says exactly the same.
async fn assert_statuses(outposts: &Outposts, cases: &[(&str, &str, &str, &str)]) {
    let listed = list(outposts, "").await;
    let summaries = summaries(&listed);
    for (case, order_id, status, item_status) in cases {
        let summary = summaries
            .iter()
            .find(|summary| text(summary, "OrderId") == **order_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the list reports the stored order, got {listed}")
            });
        let reported = text(summary, "Status");
        assert_eq!(
            reported, *status,
            "case {case:?}: the order is {status}, got {reported:?}"
        );
        assert_eq!(
            reported_counts(summary),
            HashMap::from([((*item_status).to_string(), 3i64)]),
            "case {case:?}: all three line items are {item_status}, got {summary}"
        );
        // The same record, read on its own route: a list and a read cannot say
        // different things about one order.
        let read = get_order_in(outposts, HOME_REGION, order_id).await;
        assert_eq!(
            summary,
            &summary_of(&read),
            "case {case:?}: the summary and the read must agree member for member"
        );
    }
}
