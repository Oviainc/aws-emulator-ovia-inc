// SPDX-License-Identifier: CC0-1.0

//! A `Scan`'s `FilterExpression` over the dimensions the filter rule generalises over
//! that the response's shape does not: the TYPE of the attribute it speaks about, the
//! DEPTH of the path it reaches through, and the SEGMENT of the table the request
//! evaluates.
//!
//! The rule: a `FilterExpression` is the `ConditionExpression` language evaluated
//! against each item the request already read, so it decides only which of those items
//! the response carries — `ScannedCount` counts the items evaluated, `Count` the ones
//! that matched, and `ScannedCount >= Count` always. It is one operation, not two: a
//! filtered request is served wherever the unfiltered one is, including a parallel scan
//! and a scan of attributes of any type.
//!
//! What the suites below add to `crate::result_filter_expressions`, which spans the
//! comparators and the combinators over top-level string and number attributes:
//!
//! * `a_filter_reads_every_attribute_value_type` — one item per member of the
//!   `AttributeValue` union, filtered by `attribute_type`, so every type code the model
//!   declares is exercised and not only the `S` the report cited.
//! * `a_filter_reads_through_a_document_path` — a path into a map, a deeper path, a list
//!   index, an index past the list, and a path through an attribute that is not a
//!   document.
//! * `a_filtered_parallel_scan_covers_the_matching_items_once` — the filter and the
//!   segmentation compose: the segments together answer every matching item exactly
//!   once and together evaluate every item exactly once.
//!
//! The harness is `crate::emulator`'s and the table is created through
//! `crate::table_items::create_active_table`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{emulator, ok};
use crate::table_items::create_active_table;

/// The `Id` of every item a response carries, sorted: these suites are about WHICH
/// items survive the filter, and the order a scan reports them in is `crate::table_scan`'s
/// subject.
fn ids(response: &Value) -> Vec<String> {
    let mut ids: Vec<String> = response
        .get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item["Id"]["S"].as_str().unwrap_or("<no Id>").to_string())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// `ids` of a slice of expected names, in the same sorted form.
fn sorted(expected: &[&str]) -> Vec<String> {
    let mut expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
    expected.sort();
    expected
}

/// The accounting every filtered response owes, whatever the filter said: `Count` is the
/// number of items answered and `ScannedCount` the number evaluated, which is never
/// smaller.
fn assert_accounting(case: &str, response: &Value, expected_count: usize, scanned: usize) {
    let count = response["Count"].as_i64().unwrap_or(-1);
    let scanned_count = response["ScannedCount"].as_i64().unwrap_or(-1);
    assert_eq!(
        count,
        i64::try_from(expected_count).expect("the expected count fits"),
        "case {case:?}: Count is the number of items the filter kept, got {response}"
    );
    assert_eq!(
        scanned_count,
        i64::try_from(scanned).expect("the scanned count fits"),
        "case {case:?}: ScannedCount is the number of items evaluated, got {response}"
    );
    assert!(
        scanned_count >= count,
        "case {case:?}: ScannedCount must be at least Count, got {response}"
    );
    let answered = response
        .get("Items")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        answered, expected_count,
        "case {case:?}: Count must equal the number of items answered, got {response}"
    );
}

/// Write `items` into a freshly created table keyed by a single `Id` partition key.
async fn seed(router: &MultiServiceRouter, table: &str, items: &[Value]) {
    create_active_table(router, table, &[("Id", "HASH", "S")]).await;
    for item in items {
        ok(router, "PutItem", json!({"TableName": table, "Item": item})).await;
    }
}

/// Every member of the `AttributeValue` union — the type codes `attribute_type` names —
/// with one sample value each.
///
/// Source: this service's Smithy model, `AttributeValue` (its union members are `S`, `N`,
/// `B`, `SS`, `NS`, `BS`, `M`, `L`, `NULL`, `BOOL`), read with
/// `smithy_query(query="shape",shape="AttributeValue")`. The sample values are the ones
/// those members document.
fn attribute_value_types() -> Vec<(&'static str, Value)> {
    vec![
        ("S", json!({"S": "Hello"})),
        ("N", json!({"N": "123.45"})),
        ("B", json!({"B": "dGhpcyB0ZXh0"})),
        ("SS", json!({"SS": ["Giraffe", "Hippo", "Zebra"]})),
        ("NS", json!({"NS": ["42.2", "-19", "7.5"]})),
        ("BS", json!({"BS": ["U3Vubnk=", "UmFpbnk="]})),
        ("M", json!({"M": {"Name": {"S": "Joe"}, "Age": {"N": "35"}}})),
        ("L", json!({"L": [{"S": "Cookies"}, {"N": "3.14159"}]})),
        ("NULL", json!({"NULL": true})),
        ("BOOL", json!({"BOOL": true})),
    ]
}

/// A filter speaks about an attribute of ANY type: one item per `AttributeValue` member,
/// each selected by `attribute_type` on the code that member declares.
#[tokio::test]
async fn a_filter_reads_every_attribute_value_type() {
    let router = emulator();
    let catalog = attribute_value_types();
    let items: Vec<Value> = catalog
        .iter()
        .map(|(code, value)| json!({"Id": {"S": format!("typed-{code}")}, "Payload": value}))
        .collect();
    let total = items.len();
    seed(&router, "ScanFilterTypes", &items).await;

    for (code, _) in &catalog {
        let filtered = ok(
            &router,
            "Scan",
            json!({
                "TableName": "ScanFilterTypes",
                "FilterExpression": "attribute_type(#p, :type)",
                "ExpressionAttributeNames": {"#p": "Payload"},
                "ExpressionAttributeValues": {":type": {"S": code}},
            }),
        )
        .await;
        let case = format!("attribute_type {code}");
        assert_eq!(
            ids(&filtered),
            sorted(&[&format!("typed-{code}")]),
            "case {case:?}: the filter keeps the one item whose attribute has that type, \
             got {filtered}"
        );
        assert_accounting(&case, &filtered, 1, total);
    }

    // `attribute_exists` over the same table matches every item, which is the other end
    // of the same accounting: a filter that removes nothing leaves Count == ScannedCount.
    let all = ok(
        &router,
        "Scan",
        json!({
            "TableName": "ScanFilterTypes",
            "FilterExpression": "attribute_exists(#p)",
            "ExpressionAttributeNames": {"#p": "Payload"},
        }),
    )
    .await;
    assert_accounting("attribute_exists over every type", &all, total, total);
}

/// The items the document-path suite filters: two documents of different depth and one
/// item whose same-named attributes are scalars, so a path that reaches nothing has
/// something to not reach.
fn document_items() -> Vec<Value> {
    vec![
        json!({"Id": {"S": "nested-full"},
               "Profile": {"M": {"City": {"S": "Berlin"},
                                 "Nickname": {"S": "ace"},
                                 "Address": {"M": {"Zip": {"S": "10115"}}}}},
               "Scores": {"L": [{"N": "10"}, {"N": "20"}]}}),
        json!({"Id": {"S": "nested-other"},
               "Profile": {"M": {"City": {"S": "Lisbon"},
                                 "Address": {"M": {"Zip": {"S": "1100"}}}}},
               "Scores": {"L": [{"N": "30"}]}}),
        json!({"Id": {"S": "flat"},
               "Profile": {"S": "not a document"},
               "Scores": {"N": "5"}}),
    ]
}

/// A filter reads through a document path — a map member, a deeper member, a list index —
/// and an item the path does not reach simply does not match.
#[tokio::test]
async fn a_filter_reads_through_a_document_path() {
    let router = emulator();
    let items = document_items();
    let total = items.len();
    seed(&router, "ScanFilterPaths", &items).await;

    // (case, expression, values, the Ids the filter keeps)
    let rows: Vec<(&str, &str, Value, Vec<&str>)> = vec![
        (
            "a path into a map",
            "Profile.City = :city",
            json!({":city": {"S": "Berlin"}}),
            vec!["nested-full"],
        ),
        (
            "a path two levels into a map",
            "Profile.Address.Zip = :zip",
            json!({":zip": {"S": "1100"}}),
            vec!["nested-other"],
        ),
        (
            "a list index",
            "Scores[1] = :score",
            json!({":score": {"N": "20"}}),
            vec!["nested-full"],
        ),
        (
            "a list index past every list",
            "Scores[5] = :score",
            json!({":score": {"N": "30"}}),
            vec![],
        ),
        (
            "attribute_exists on a nested path only one document carries",
            "attribute_exists(Profile.Nickname)",
            json!({}),
            vec!["nested-full"],
        ),
        (
            "attribute_not_exists on that nested path, its complement",
            "attribute_not_exists(Profile.Nickname)",
            json!({}),
            vec!["nested-other", "flat"],
        ),
        (
            "attribute_type on a nested path",
            "attribute_type(Profile.Address, :type)",
            json!({":type": {"S": "M"}}),
            vec!["nested-full", "nested-other"],
        ),
        (
            "size of a list, which is its element count",
            "size(Scores) = :len",
            json!({":len": {"N": "2"}}),
            vec!["nested-full"],
        ),
        (
            "a comparison on a path no item resolves",
            "Profile.Country = :country",
            json!({":country": {"S": "DE"}}),
            vec![],
        ),
    ];

    for (case, expression, values, expected) in rows {
        let mut request = json!({"TableName": "ScanFilterPaths", "FilterExpression": expression});
        if values.as_object().is_some_and(|map| !map.is_empty()) {
            request["ExpressionAttributeValues"] = values;
        }
        let filtered = ok(&router, "Scan", request).await;
        assert_eq!(
            ids(&filtered),
            sorted(&expected),
            "case {case:?}: expected the items the path selects, got {filtered}"
        );
        assert_accounting(case, &filtered, expected.len(), total);
    }
}

/// The items the parallel suite filters: more items than segments in some rows and fewer
/// in others, with `Kind` splitting them into the matching and the rest.
fn kinded_items() -> Vec<Value> {
    (0..7)
        .map(|index| {
            let kind = if index % 2 == 0 { "a" } else { "b" };
            json!({"Id": {"S": format!("item-{index}")}, "Kind": {"S": kind}})
        })
        .collect()
}

/// A filtered parallel scan: the filter and `Segment`/`TotalSegments` compose, so the
/// segments together answer every matching item exactly once while together evaluating
/// every item exactly once.
#[tokio::test]
async fn a_filtered_parallel_scan_covers_the_matching_items_once() {
    let router = emulator();
    let items = kinded_items();
    let total = items.len();
    seed(&router, "ScanFilterSegments", &items).await;

    let sequential = ok(
        &router,
        "Scan",
        json!({
            "TableName": "ScanFilterSegments",
            "FilterExpression": "#k = :kind",
            "ExpressionAttributeNames": {"#k": "Kind"},
            "ExpressionAttributeValues": {":kind": {"S": "a"}},
        }),
    )
    .await;
    let matching = ids(&sequential);
    assert_accounting("the sequential filtered scan", &sequential, 4, total);

    // One segment (the sequential scan), fewer segments than items, as many as there are
    // items, and more segments than items — the last of which leaves a segment with
    // nothing to evaluate at all.
    for segments in [1usize, 2, 3, total, total + 2] {
        let mut seen: Vec<String> = Vec::new();
        let mut evaluated = 0usize;
        for segment in 0..segments {
            let page = ok(
                &router,
                "Scan",
                json!({
                    "TableName": "ScanFilterSegments",
                    "Segment": segment,
                    "TotalSegments": segments,
                    "FilterExpression": "#k = :kind",
                    "ExpressionAttributeNames": {"#k": "Kind"},
                    "ExpressionAttributeValues": {":kind": {"S": "a"}},
                }),
            )
            .await;
            let case = format!("segment {segment} of {segments}");
            let answered = page
                .get("Items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let scanned = page["ScannedCount"].as_i64().unwrap_or(-1);
            assert_accounting(
                &case,
                &page,
                answered,
                usize::try_from(scanned.max(0)).expect("a non-negative ScannedCount"),
            );
            evaluated += usize::try_from(scanned.max(0)).expect("a non-negative ScannedCount");
            seen.extend(ids(&page));
        }
        seen.sort();
        let distinct: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            distinct.len(),
            seen.len(),
            "case {segments} segments: no matching item is answered by two segments, got \
             {seen:?}"
        );
        assert_eq!(
            seen, matching,
            "case {segments} segments: the segments together answer the items the filter \
             keeps, got {seen:?}"
        );
        assert_eq!(
            evaluated, total,
            "case {segments} segments: the segments together evaluate every item exactly \
             once, got {evaluated}"
        );
    }
}
