// SPDX-License-Identifier: CC0-1.0

//! What ONE `Scan` request evaluates: every item of what it walks, and — when the
//! request names `Segment`/`TotalSegments` — the share of the KEY SPACE that segment
//! covers.
//!
//! The rule these suites pin: a scan with no segmentation evaluates every item of the
//! table or index it walks, reporting `ScannedCount` for what it evaluated and `Count`
//! for what the filter kept; a parallel scan divides the key space being walked into one
//! block of PARTITIONS per segment, so every item that shares a partition key is
//! evaluated by exactly one segment and the segments together evaluate every item exactly
//! once. The division is of the key space, never of the row list, so it does not depend
//! on how many items a partition holds, on how many segments the request asks for, or on
//! the `FilterExpression` the request carries — a filter runs AFTER the page is read.
//!
//! The rows span the dimensions the rule generalises over: how many distinct partition
//! keys the table holds and how many items sit in each ([`SHAPES`]), how many segments
//! the request divides it into, the filter the request carries beside the segmentation,
//! and whether the walk is the table's own key space or a secondary index's.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front doors use, from `crate::emulator`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{describe_when_active, emulator, ok};

/// One seeded table: how many distinct partition keys it holds, and how many items sit in
/// each of them.
struct Shape {
    case: &'static str,
    table: &'static str,
    partitions: usize,
    rows: usize,
}

/// The table shapes every segmentation row below is driven over: one partition holding
/// every item (where a parallel scan's segments cannot split the item list without
/// splitting a partition), partitions holding several items each, and one item per
/// partition.
const SHAPES: [Shape; 4] = [
    Shape {
        case: "one partition of five items",
        table: "SegOnePartition",
        partitions: 1,
        rows: 5,
    },
    Shape {
        case: "two partitions of three items",
        table: "SegTwoPartitions",
        partitions: 2,
        rows: 3,
    },
    Shape {
        case: "three partitions of two items",
        table: "SegThreePartitions",
        partitions: 3,
        rows: 2,
    },
    Shape {
        case: "five partitions of one item",
        table: "SegFivePartitions",
        partitions: 5,
        rows: 1,
    },
];

impl Shape {
    /// How many items this shape seeds.
    fn total(&self) -> usize {
        self.partitions * self.rows
    }

    /// Every item key this shape seeds, in the order a scan of the table walks them:
    /// partition first, then sort key.
    fn keys(&self) -> Vec<String> {
        (0..self.partitions)
            .flat_map(|partition| {
                (0..self.rows).map(move |row| format!("p{partition}/{row}"))
            })
            .collect()
    }
}

/// The partition component of one of those item keys.
fn partition_of(key: &str) -> String {
    key.split('/').next().unwrap_or(key).to_string()
}

/// The item keys a response reports, in the order it reported them. A response that
/// carries no `Items` member reports none, which is what a segment with nothing to
/// evaluate — and a filter that kept nothing — answers.
fn item_keys(page: &Value) -> Vec<String> {
    page.get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    format!(
                        "{}/{}",
                        item["pk"]["S"].as_str().unwrap_or("<no pk>"),
                        item["sk"]["N"].as_str().unwrap_or("<no sk>")
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `ScannedCount` a response reports, as a count.
fn scanned_count(page: &Value) -> usize {
    usize::try_from(page["ScannedCount"].as_i64().unwrap_or(-1)).unwrap_or(usize::MAX)
}

/// Create every [`SHAPES`] table and seed its items. The creates are issued together and
/// awaited afterwards, so the suite waits out ONE transition span rather than one per
/// table.
async fn seed_shapes(router: &MultiServiceRouter) {
    for shape in &SHAPES {
        ok(
            router,
            "CreateTable",
            json!({
                "TableName": shape.table,
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": "sk", "KeyType": "RANGE"},
                ],
                "AttributeDefinitions": [
                    {"AttributeName": "pk", "AttributeType": "S"},
                    {"AttributeName": "sk", "AttributeType": "N"},
                ],
                "BillingMode": "PAY_PER_REQUEST",
            }),
        )
        .await;
    }
    for shape in &SHAPES {
        describe_when_active(router, shape.case, shape.table).await;
        for partition in 0..shape.partitions {
            for row in 0..shape.rows {
                ok(
                    router,
                    "PutItem",
                    json!({
                        "TableName": shape.table,
                        "Item": {
                            "pk": {"S": format!("p{partition}")},
                            "sk": {"N": row.to_string()},
                            "val": {"S": format!("v{partition}-{row}")},
                        },
                    }),
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn an_unsegmented_scan_evaluates_every_item_of_the_table() {
    let router = emulator();
    seed_shapes(&router).await;

    for shape in &SHAPES {
        let page = ok(&router, "Scan", json!({"TableName": shape.table})).await;
        let expected = shape.keys();
        assert_eq!(
            item_keys(&page),
            expected,
            "case {:?}: a scan with no filter reports every item in key order, got {page}",
            shape.case
        );
        assert_eq!(
            (page["Count"].clone(), page["ScannedCount"].clone()),
            (json!(expected.len()), json!(expected.len())),
            "case {:?}: Count and ScannedCount are both the {} items evaluated, got {page}",
            shape.case,
            expected.len()
        );
    }
}

#[tokio::test]
async fn the_segments_of_a_parallel_scan_are_whole_partitions() {
    let router = emulator();
    seed_shapes(&router).await;

    // Each row: the filter the request carries beside its segmentation, and which of the
    // seeded items it keeps. A filter is applied after the page has been read, so it
    // changes `Count` and `Items` and never which items a segment EVALUATES.
    let filters: [(&str, Option<&str>, bool); 3] = [
        ("no filter", None, true),
        ("a filter every item matches", Some("#v <> :absent"), true),
        ("a filter no item matches", Some("#v = :absent"), false),
    ];

    for shape in &SHAPES {
        let seeded = shape.keys();
        for (filter_case, filter, keeps) in &filters {
            // One segment (the sequential scan), fewer segments than the table has
            // partitions, as many, and more — the last of which leaves segments with no
            // partition to evaluate at all.
            for total in [1usize, 2, 3, 4, shape.total(), shape.total() + 2] {
                let case =
                    format!("{:?} with {filter_case} over {total} segment(s)", shape.case);
                let mut answered: Vec<String> = Vec::new();
                let mut evaluated = 0usize;
                let mut segments_of_partition: BTreeMap<String, BTreeSet<usize>> =
                    BTreeMap::new();
                let mut scanned_by_segment: Vec<usize> = Vec::new();
                for segment in 0..total {
                    let mut request = json!({
                        "TableName": shape.table,
                        "Segment": segment,
                        "TotalSegments": total,
                    });
                    if let Some(expression) = filter {
                        request["FilterExpression"] = json!(expression);
                        request["ExpressionAttributeNames"] = json!({"#v": "val"});
                        request["ExpressionAttributeValues"] =
                            json!({":absent": {"S": "no item carries this"}});
                    }
                    let page = ok(&router, "Scan", request).await;
                    let keys = item_keys(&page);
                    let scanned = scanned_count(&page);
                    assert_eq!(
                        page["Count"].clone(),
                        json!(keys.len()),
                        "case {case} segment {segment}: Count is the number of items the \
                         response carries, got {page}"
                    );
                    assert!(
                        scanned >= keys.len(),
                        "case {case} segment {segment}: ScannedCount counts what was \
                         evaluated, so it is never below Count, got {page}"
                    );
                    for key in &keys {
                        segments_of_partition
                            .entry(partition_of(key))
                            .or_default()
                            .insert(segment);
                    }
                    answered.extend(keys);
                    evaluated += scanned;
                    scanned_by_segment.push(scanned);
                }

                assert_eq!(
                    evaluated,
                    shape.total(),
                    "case {case}: the segments together evaluate every item exactly once, \
                     got {scanned_by_segment:?}"
                );
                answered.sort();
                let distinct: BTreeSet<&String> = answered.iter().collect();
                assert_eq!(
                    distinct.len(),
                    answered.len(),
                    "case {case}: no item is answered by two segments, got {answered:?}"
                );
                let mut expected = if *keeps { seeded.clone() } else { Vec::new() };
                expected.sort();
                assert_eq!(
                    answered, expected,
                    "case {case}: the segments together answer the items the filter keeps, \
                     got {answered:?}"
                );
                for (partition, segments) in &segments_of_partition {
                    assert_eq!(
                        segments.len(),
                        1,
                        "case {case}: every item of partition {partition:?} is evaluated by \
                         ONE segment, got {segments:?}"
                    );
                }
                // A table of ONE partition is one block of the key space whatever the
                // request divides it into: one segment evaluates all of it and every
                // other segment evaluates nothing at all.
                if shape.partitions == 1 {
                    let busy = scanned_by_segment
                        .iter()
                        .filter(|scanned| **scanned > 0)
                        .count();
                    assert_eq!(
                        busy, 1,
                        "case {case}: a single partition is not split across segments, got \
                         {scanned_by_segment:?}"
                    );
                }
            }
        }
    }
}

/// The table a parallel INDEX scan is driven over: six items under six primary keys, but
/// only two distinct values of the index's partition key.
const INDEX_TABLE: &str = "SegIndexScan";

/// The global secondary index that table declares.
const INDEX_NAME: &str = "ByGpk";

/// Create and seed [`INDEX_TABLE`], answering the index partition each item sits in, by
/// item key.
async fn seed_index_table(router: &MultiServiceRouter) -> BTreeMap<String, String> {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": INDEX_TABLE,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "N"},
                {"AttributeName": "gpk", "AttributeType": "S"},
            ],
            "GlobalSecondaryIndexes": [{
                "IndexName": INDEX_NAME,
                "KeySchema": [{"AttributeName": "gpk", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(router, "a table with a global secondary index", INDEX_TABLE).await;

    let mut partitions: BTreeMap<String, String> = BTreeMap::new();
    for position in 0..6 {
        let group = format!("g{}", position % 2);
        ok(
            router,
            "PutItem",
            json!({
                "TableName": INDEX_TABLE,
                "Item": {
                    "pk": {"S": format!("p{position}")},
                    "sk": {"N": "0"},
                    "gpk": {"S": group.clone()},
                    "val": {"S": format!("v{position}")},
                },
            }),
        )
        .await;
        partitions.insert(format!("p{position}/0"), group);
    }
    partitions
}

#[tokio::test]
async fn the_segments_of_a_parallel_index_scan_are_whole_index_partitions() {
    let router = emulator();
    let partitions = seed_index_table(&router).await;
    let mut seeded: Vec<String> = partitions.keys().cloned().collect();
    seeded.sort();

    for total in [1usize, 2, 3, 6, 8] {
        let case = format!("an index scan over {total} segment(s)");
        let mut answered: Vec<String> = Vec::new();
        let mut evaluated = 0usize;
        let mut segments_of_partition: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
        for segment in 0..total {
            let page = ok(
                &router,
                "Scan",
                json!({
                    "TableName": INDEX_TABLE,
                    "IndexName": INDEX_NAME,
                    "Segment": segment,
                    "TotalSegments": total,
                }),
            )
            .await;
            let keys = item_keys(&page);
            evaluated += scanned_count(&page);
            for key in &keys {
                let group = partitions.get(key).cloned().unwrap_or_default();
                segments_of_partition
                    .entry(group)
                    .or_default()
                    .insert(segment);
            }
            answered.extend(keys);
        }

        assert_eq!(
            evaluated,
            seeded.len(),
            "case {case}: the segments together evaluate every index entry exactly once, got \
             {evaluated}"
        );
        answered.sort();
        assert_eq!(
            answered, seeded,
            "case {case}: the segments together answer every index entry exactly once, got \
             {answered:?}"
        );
        for (group, segments) in &segments_of_partition {
            assert_eq!(
                segments.len(),
                1,
                "case {case}: every entry of index partition {group:?} is evaluated by ONE \
                 segment, got {segments:?}"
            );
        }
    }
}
