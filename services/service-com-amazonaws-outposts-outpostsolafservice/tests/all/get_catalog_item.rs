// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetCatalogItem`, driven over the wire through the same front
//! door as every other suite (the harness in [`crate::site_and_outpost_lifecycle`]).
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//!
//! 1. A read by `CatalogItemId` reports the ONE published hardware configuration that
//!    `SkuCode` names, whole and exactly as the catalog publishes it — the same object
//!    `ListCatalogItems` reports for that SKU, member for member — for EVERY item the
//!    catalog carries, spanning both modelled item classes, both statuses and items
//!    with and without a `RackScalingType`.
//! 2. A `SkuCode` the catalog publishes nothing under is a 404 `NotFoundException`
//!    whose body is this service's identifier echo, and a label outside the modelled
//!    `SkuCode` constraints is refused at the boundary as a 400 `ValidationException`
//!    that names the member and the constraint and renders no value.
//! 3. The read is a READ: the catalog this service publishes is reference data, so
//!    reading every item (twice) leaves both the item and the whole catalog a later
//!    caller sees exactly as they were, and the same SKU reports the same item in
//!    every region and on an unsigned call — the catalog is not a regional inventory.

use serde_json::Value;

use crate::site_and_outpost_lifecycle::{segment, text, Outposts};

/// `ListCatalogItems`' own route, which publishes the catalog this operation reads
/// one entry of.
const CATALOG_ROUTE: &str = "/catalog/items";

/// The modelled `CatalogItemClass` values (`RACK` | `SERVER`), as
/// `smithy_query(query="shape", shape="CatalogItemClass")` reports them — what
/// `ListCatalogItems$ItemClassFilter` accepts, used here only to split the catalog
/// into rows that span both classes.
const ITEM_CLASSES: &[&str] = &["RACK", "SERVER"];

/// Well-formed `SkuCode`s — the modelled pattern `^OR-[A-Z0-9]{7}$` with length
/// 1..10, from `smithy_query(query="operation", operation="GetCatalogItem")` — that
/// the published catalog is expected to carry no configuration under. Every row is
/// checked against the catalog itself before it is used, so a case cannot quietly
/// start naming a published item.
const UNPUBLISHED_SKUS: &[&str] = &["OR-0000000", "OR-ZZZZZZZ", "OR-1A2B3C4", "OR-QQQQQQ9"];

/// The `CatalogItems` of one `ListCatalogItems` answer.
fn items(output: &Value) -> Vec<Value> {
    output
        .get("CatalogItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The whole published catalog, in the order `ListCatalogItems` reports it — the
/// page size is the modelled `MaxResults` maximum, so one call exhausts it.
async fn catalog(outposts: &Outposts) -> Vec<Value> {
    items(
        &outposts
            .ok("GET", &format!("{CATALOG_ROUTE}?MaxResults=1000"), None)
            .await,
    )
}

/// The catalog narrowed to one modelled `CatalogItemClass`.
async fn catalog_of_class(outposts: &Outposts, class: &str) -> Vec<Value> {
    items(
        &outposts
            .ok(
                "GET",
                &format!("{CATALOG_ROUTE}?MaxResults=1000&ItemClassFilter={class}"),
                None,
            )
            .await,
    )
}

/// `GetCatalogItem`'s route for `sku`.
fn route(sku: &str) -> String {
    format!("/catalog/item/{}", segment(sku))
}

/// `GetCatalogItem` on `sku` in `region`, requiring HTTP 200, returning the
/// `CatalogItem` it answered with.
async fn get_item_in(outposts: &Outposts, region: &str, sku: &str) -> Value {
    outposts
        .ok_in(region, "GET", &route(sku), None)
        .await
        .get("CatalogItem")
        .cloned()
        .unwrap_or(Value::Null)
}

/// `GetCatalogItem` on `sku`, requiring HTTP 200.
async fn get_item(outposts: &Outposts, sku: &str) -> Value {
    outposts.ok("GET", &route(sku), None).await["CatalogItem"].clone()
}

/// The error message a body carries under `key` (`Message` for a modelled error
/// shape, `message` for the framework's boundary report).
fn message(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Rule 1: every published item reads back WHOLE, over the whole catalog rather
/// than one row, with the rows grouped by the class they were published as so the
/// table spans both modelled classes.
#[tokio::test]
async fn every_published_catalog_item_reads_back_exactly_as_the_catalog_publishes_it() {
    let outposts = Outposts::new();
    let whole = catalog(&outposts).await;
    assert!(
        whole.len() > 1,
        "the service publishes a catalog of configurations to read one of, got {whole:?}"
    );

    // The rows: every published item, tagged with the class it was published as.
    let mut rows: Vec<(String, Value)> = Vec::new();
    for class in ITEM_CLASSES {
        let of_class = catalog_of_class(&outposts, class).await;
        assert!(
            !of_class.is_empty(),
            "case class {class:?}: the catalog publishes items of every modelled class, got {of_class:?}"
        );
        for item in of_class {
            rows.push(((*class).to_string(), item));
        }
    }
    assert_eq!(
        rows.len(),
        whole.len(),
        "the per-class rows must cover the whole catalog exactly once, got {} rows for {} items",
        rows.len(),
        whole.len()
    );

    let mut statuses: Vec<String> = Vec::new();
    let mut with_scaling = 0usize;
    let mut without_scaling = 0usize;
    for (class, published) in &rows {
        let sku = text(published, "CatalogItemId");
        let case = format!("{class} item {sku}");
        let read = get_item(&outposts, &sku).await;
        assert_eq!(
            &read, published,
            "case {case:?}: expected the catalog's own entry {published:?}, got {read:?}"
        );
        assert_eq!(
            text(&read, "CatalogItemId"),
            sku,
            "case {case:?}: the item read back must be the SKU the request named, got {read:?}"
        );
        statuses.push(text(&read, "ItemStatus"));
        match read.get("RackScalingType").is_some() {
            true => with_scaling += 1,
            false => without_scaling += 1,
        }
    }

    statuses.sort();
    statuses.dedup();
    assert!(
        statuses.len() > 1,
        "the rows must span more than one ItemStatus for the rule to be tested on both, got {statuses:?}"
    );
    assert!(
        with_scaling > 0 && without_scaling > 0,
        "the rows must span items that report a RackScalingType and items that omit it, got {with_scaling} with and {without_scaling} without"
    );
}

/// Rule 2, first half: a well-formed SKU the catalog publishes nothing under is a
/// 404 whose body is the identifier echo of the value AS SUPPLIED.
#[tokio::test]
async fn a_sku_the_catalog_publishes_nothing_under_is_not_found() {
    let outposts = Outposts::new();
    let published: Vec<String> = catalog(&outposts)
        .await
        .iter()
        .map(|item| text(item, "CatalogItemId"))
        .collect();

    for sku in UNPUBLISHED_SKUS {
        assert!(
            !published.contains(&(*sku).to_string()),
            "case {sku:?}: this row must name nothing in the catalog, but the catalog publishes {published:?}"
        );
        let reply = outposts.call("GET", &route(sku), None).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {sku:?}: expected 404 NotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let expected = format!("catalogItemId={sku}");
        let actual = message(&reply.body, "Message");
        assert_eq!(
            actual, expected,
            "case {sku:?}: expected $.Message {expected:?}, got {actual:?}"
        );
    }
}

/// Rule 2, second half: a label outside the modelled `SkuCode` constraints never
/// reaches the handler — it is the boundary's 400 report, naming the member and the
/// constraint it broke and rendering no value.
#[tokio::test]
async fn a_label_outside_the_modelled_sku_code_is_refused_as_a_bad_request() {
    let outposts = Outposts::new();
    // The modelled `SkuCode` pattern, spelled exactly as the model spells it.
    let pattern = "Value at 'catalogItemId' failed to satisfy constraint: \
                   Member must satisfy regular expression pattern: ^OR-[A-Z0-9]{7}$";
    // One row per way a label can fall outside the pattern, plus one past the
    // modelled length maximum of 10, which breaks the length bound as well.
    let pattern_only: &[(&str, &str)] = &[
        ("a body shorter than the pattern's seven characters", "OR-ABC"),
        ("a lowercase prefix", "or-ABCDEFG"),
        ("a lowercase body", "OR-abcdefg"),
        ("no separator at all", "ORABCDEFG"),
        ("a character outside the pattern's alphabet", "OR-A_BCDEF"),
        ("another service's identifier form", "os-0000000"),
    ];
    for (case, label) in pattern_only {
        let reply = outposts.call("GET", &route(label), None).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = message(&reply.body, "message");
        assert_eq!(
            actual,
            format!("1 validation error detected: {pattern}"),
            "case {case:?}: expected the boundary's one-clause report, got {actual:?}"
        );
    }

    // Past the length maximum: both constraints are reported, and neither clause
    // renders the label.
    let too_long = "OR-ABCDEFGH";
    let reply = outposts.call("GET", &route(too_long), None).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (400, "ValidationException"),
        "case \"a label past the length maximum\": expected 400 ValidationException, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    let actual = message(&reply.body, "message");
    assert!(
        actual.contains(pattern)
            && actual.contains(
                "Value at 'catalogItemId' failed to satisfy constraint: \
                 Member must have length less than or equal to 10"
            )
            && actual.starts_with("2 validation errors detected: "),
        "case \"a label past the length maximum\": expected both clauses, got {actual:?}"
    );
    assert!(
        !actual.contains(too_long),
        "case \"a label past the length maximum\": the report rendered the value, got {actual:?}"
    );
}

/// Rule 3: reading an item is a read — it changes neither the item nor the catalog
/// a later caller sees — and the catalog it reads is the service's, not one
/// region's.
#[tokio::test]
async fn reading_an_item_changes_nothing_and_answers_the_same_in_every_region() {
    let outposts = Outposts::new();
    let before = catalog(&outposts).await;
    let skus: Vec<String> = before
        .iter()
        .map(|item| text(item, "CatalogItemId"))
        .collect();

    // Read every item twice: a second read of the same SKU answers what the first
    // did, and a read of one item never disturbs another.
    for sku in &skus {
        let first = get_item(&outposts, sku).await;
        let second = get_item(&outposts, sku).await;
        assert_eq!(
            second, first,
            "case {sku:?}: a second read answers what the first did, got {second:?} after {first:?}"
        );
    }
    let after = catalog(&outposts).await;
    assert_eq!(
        after, before,
        "reading every item changes nothing the catalog reports, got {after:?} after {before:?}"
    );

    // Regions other than the one the reads above were addressed to, and an
    // unsigned call, which carries no credential scope to read a region from.
    let regions = ["us-east-1", "us-west-2", "eu-west-1", "ap-southeast-2"];
    for sku in &skus {
        let home = get_item(&outposts, sku).await;
        for region in regions {
            let read = get_item_in(&outposts, region, sku).await;
            assert_eq!(
                read, home,
                "case {sku:?} in {region}: the published catalog is the same in every region, got {read:?} instead of {home:?}"
            );
        }
        let unsigned = outposts.call_unsigned("GET", &route(sku), None).await;
        assert_eq!(
            unsigned.status, 200,
            "case {sku:?} unsigned: expected HTTP 200, got {} {}",
            unsigned.status, unsigned.body
        );
        assert_eq!(
            unsigned.body["CatalogItem"], home,
            "case {sku:?} unsigned: expected {home:?}, got {:?}",
            unsigned.body["CatalogItem"]
        );
    }
}
