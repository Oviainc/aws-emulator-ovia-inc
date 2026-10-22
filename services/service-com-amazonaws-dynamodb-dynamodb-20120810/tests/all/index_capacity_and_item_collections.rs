// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a WRITE reports about the indexes it touched.
//!
//! Three rules are pinned here, each over a table of inputs rather than one recorded
//! example:
//!
//! 1. a write's `ConsumedCapacity` FANS OUT: with `ReturnConsumedCapacity=INDEXES` the
//!    response carries the units consumed on the table and one entry per secondary index
//!    the written (or removed) item reaches, and the aggregate `CapacityUnits` is their
//!    sum; with `TOTAL` only that aggregate; with `NONE` (or no member) nothing;
//! 2. `ReturnItemCollectionMetrics=SIZE` reports the item collection the write landed
//!    in — its `ItemCollectionKey` is the item's partition key attribute alone and its
//!    `SizeEstimateRangeGB` brackets the collection's measured size — and only for a
//!    table that has at least one local secondary index;
//! 3. a table created WITH local secondary indexes is a table like any other: it is
//!    described with its indexes and it serves `PutItem`/`GetItem`/`DeleteItem`, and a
//!    key it does not hold is an empty 200 rather than a refusal.
//!
//! Every call goes over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    size_estimate_range_gb, MAX_LOCAL_SECONDARY_INDEXES, STATUS_TRANSITION,
};
use service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4-shaped `Authorization` header: its credential scope routes the request to
/// this service (signing name `dynamodb`) and names the region the ARNs it mints carry.
/// Request authentication is off in these tests, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/dynamodb/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000";

/// One emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one operation and return its HTTP status and parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Send one operation that must succeed.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code (`__type`) of a refusal.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Wait out the `CREATING` span the clock owes a new table and report its description.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> Value {
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    let described = ok(router, "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        described["Table"]["TableStatus"],
        json!("ACTIVE"),
        "case wait {name}: the table becomes ACTIVE once the transition is due, got {described}"
    );
    described
}

/// A capacity figure as a number, whichever JSON spelling the wire used.
fn units(value: &Value) -> Option<f64> {
    value.as_f64()
}

/// The `{index name -> CapacityUnits}` map one `ConsumedCapacity` member carries.
fn index_units(consumed: &Value, member: &str) -> Vec<(String, f64)> {
    let mut entries: Vec<(String, f64)> = consumed
        .get(member)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(name, capacity)| {
                    (
                        name.clone(),
                        units(&capacity["CapacityUnits"]).unwrap_or(f64::NAN),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

/// The table the capacity tests write to: two local secondary indexes (one projecting
/// every attribute, one only the keys) and one global secondary index projecting a named
/// non-key attribute, so the rows below differ in which indexes the item reaches and in
/// how much each of them stores of it.
const FAN_OUT: &str = "FanOutTable";

/// A table with the same primary key and NO secondary index, which is what makes the
/// index maps and the item collection metrics absent rather than empty.
const NO_INDEX: &str = "PlainTable";

/// Create [`FAN_OUT`] and wait for it to become `ACTIVE`.
async fn create_fan_out_table(router: &MultiServiceRouter) -> Value {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": FAN_OUT,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "lsiAll", "AttributeType": "S"},
                {"AttributeName": "lsiKeys", "AttributeType": "N"},
                {"AttributeName": "gsiPk", "AttributeType": "S"},
            ],
            "LocalSecondaryIndexes": [
                {
                    "IndexName": "lsiAll",
                    "KeySchema": [
                        {"AttributeName": "pk", "KeyType": "HASH"},
                        {"AttributeName": "lsiAll", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": "lsiKeys",
                    "KeySchema": [
                        {"AttributeName": "pk", "KeyType": "HASH"},
                        {"AttributeName": "lsiKeys", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                },
            ],
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": "gsi1",
                    "KeySchema": [{"AttributeName": "gsiPk", "KeyType": "HASH"}],
                    "Projection": {
                        "ProjectionType": "INCLUDE",
                        "NonKeyAttributes": ["payload"],
                    },
                },
            ],
        }),
    )
    .await;
    wait_until_active(router, FAN_OUT).await
}

/// One row of the capacity table: an item, the units the table takes for it, and the
/// units each index it reaches takes.
struct CapacityCase {
    case: &'static str,
    item: Value,
    table: f64,
    local: Vec<(&'static str, f64)>,
    global: Vec<(&'static str, f64)>,
}

/// The rows: an item reaching every index, one reaching a single local index, one
/// reaching none, and one large enough to cost more than a single unit — where what each
/// index charges depends on what its projection stores of it.
///
/// One write capacity unit covers 1 KB ("Read/write capacity mode" in the Developer
/// Guide), so the small items below cost one unit everywhere, and the ~2 KB item costs
/// two on the table, two on the `ALL` local index, two on the global index that
/// `INCLUDE`s the big attribute, and one on the `KEYS_ONLY` local index, which stores
/// only the key attributes.
fn capacity_cases() -> Vec<CapacityCase> {
    let big = "p".repeat(2000);
    vec![
        CapacityCase {
            case: "every index",
            item: json!({
                "pk": {"S": "coll-a"}, "sk": {"S": "s1"},
                "lsiAll": {"S": "a1"}, "lsiKeys": {"N": "1"},
                "gsiPk": {"S": "g1"}, "payload": {"S": "small"},
            }),
            table: 1.0,
            local: vec![("lsiAll", 1.0), ("lsiKeys", 1.0)],
            global: vec![("gsi1", 1.0)],
        },
        CapacityCase {
            case: "one local index only",
            item: json!({
                "pk": {"S": "coll-a"}, "sk": {"S": "s2"},
                "lsiAll": {"S": "a2"},
            }),
            table: 1.0,
            local: vec![("lsiAll", 1.0)],
            global: vec![],
        },
        CapacityCase {
            case: "no secondary index",
            item: json!({"pk": {"S": "coll-b"}, "sk": {"S": "s3"}}),
            table: 1.0,
            local: vec![],
            global: vec![],
        },
        CapacityCase {
            case: "two kilobyte item",
            item: json!({
                "pk": {"S": "coll-a"}, "sk": {"S": "s4"},
                "lsiAll": {"S": "a4"}, "lsiKeys": {"N": "4"},
                "gsiPk": {"S": "g4"}, "payload": {"S": big},
            }),
            table: 2.0,
            local: vec![("lsiAll", 2.0), ("lsiKeys", 1.0)],
            global: vec![("gsi1", 2.0)],
        },
    ]
}

/// The expected aggregate: the table's units plus every affected index's.
fn expected_total(row: &CapacityCase) -> f64 {
    row.table
        + row.local.iter().map(|(_, units)| units).sum::<f64>()
        + row.global.iter().map(|(_, units)| units).sum::<f64>()
}

/// Assert one `ConsumedCapacity` carries exactly the breakdown `row` predicts.
fn assert_breakdown(case: &str, consumed: &Value, row: &CapacityCase) {
    assert_eq!(
        units(&consumed["CapacityUnits"]),
        Some(expected_total(row)),
        "case {case}: the aggregate CapacityUnits is the table's units plus every \
         affected index's, got {consumed}"
    );
    assert_eq!(
        units(&consumed["Table"]["CapacityUnits"]),
        Some(row.table),
        "case {case}: Table carries the units the table itself took, got {consumed}"
    );
    let expected_local: Vec<(String, f64)> = row
        .local
        .iter()
        .map(|(name, units)| ((*name).to_string(), *units))
        .collect();
    assert_eq!(
        index_units(consumed, "LocalSecondaryIndexes"),
        expected_local,
        "case {case}: one LocalSecondaryIndexes entry per local index the item reaches, \
         got {consumed}"
    );
    let expected_global: Vec<(String, f64)> = row
        .global
        .iter()
        .map(|(name, units)| ((*name).to_string(), *units))
        .collect();
    assert_eq!(
        index_units(consumed, "GlobalSecondaryIndexes"),
        expected_global,
        "case {case}: one GlobalSecondaryIndexes entry per global index the item reaches, \
         got {consumed}"
    );
    if row.local.is_empty() {
        assert!(
            consumed.get("LocalSecondaryIndexes").is_none(),
            "case {case}: an item that reaches no local index carries no map at all, \
             got {consumed}"
        );
    }
    if row.global.is_empty() {
        assert!(
            consumed.get("GlobalSecondaryIndexes").is_none(),
            "case {case}: an item that reaches no global index carries no map at all, \
             got {consumed}"
        );
    }
}

/// The `Key` map of an item: just its primary key attributes.
fn key_of(item: &Value) -> Value {
    json!({"pk": item["pk"].clone(), "sk": item["sk"].clone()})
}

#[tokio::test]
async fn a_write_reports_the_capacity_it_consumed_on_every_index_it_affects() {
    let router = emulator();
    create_fan_out_table(&router).await;

    for row in capacity_cases() {
        // INDEXES: "the aggregate ConsumedCapacity for the operation, together with
        // ConsumedCapacity for each table and secondary index that was accessed".
        let put = ok(
            &router,
            "PutItem",
            json!({
                "TableName": FAN_OUT,
                "Item": row.item.clone(),
                "ReturnConsumedCapacity": "INDEXES",
            }),
        )
        .await;
        assert_eq!(
            put["ConsumedCapacity"]["TableName"],
            json!(FAN_OUT),
            "case put {}: ConsumedCapacity names the table it was consumed on, got {put}",
            row.case
        );
        assert_breakdown(&format!("put {}", row.case), &put["ConsumedCapacity"], &row);

        // TOTAL: "only the aggregate ConsumedCapacity for the operation".
        let total = ok(
            &router,
            "PutItem",
            json!({
                "TableName": FAN_OUT,
                "Item": row.item.clone(),
                "ReturnConsumedCapacity": "TOTAL",
            }),
        )
        .await;
        let consumed = &total["ConsumedCapacity"];
        assert_eq!(
            units(&consumed["CapacityUnits"]),
            Some(expected_total(&row)),
            "case put TOTAL {}: the aggregate is the same figure INDEXES breaks down, \
             got {total}",
            row.case
        );
        for member in ["Table", "LocalSecondaryIndexes", "GlobalSecondaryIndexes"] {
            assert!(
                consumed.get(member).is_none(),
                "case put TOTAL {}: TOTAL carries no {member} breakdown, got {total}",
                row.case
            );
        }

        // NONE, and no member at all: "No ConsumedCapacity details are included in the
        // response."
        for asked in [Some("NONE"), None] {
            let mut body = json!({"TableName": FAN_OUT, "Item": row.item.clone()});
            if let Some(asked) = asked {
                body["ReturnConsumedCapacity"] = json!(asked);
            }
            let quiet = ok(&router, "PutItem", body).await;
            assert!(
                quiet.get("ConsumedCapacity").is_none(),
                "case put {asked:?} {}: no ConsumedCapacity is reported, got {quiet}",
                row.case
            );
        }

        // A delete fans out over the indexes the REMOVED item reached, by the same rule.
        let deleted = ok(
            &router,
            "DeleteItem",
            json!({
                "TableName": FAN_OUT,
                "Key": key_of(&row.item),
                "ReturnConsumedCapacity": "INDEXES",
            }),
        )
        .await;
        assert_breakdown(
            &format!("delete {}", row.case),
            &deleted["ConsumedCapacity"],
            &row,
        );

        // Deleting a key the table no longer holds moves no item, so it reaches no index
        // and costs the table its one minimum unit.
        let absent = ok(
            &router,
            "DeleteItem",
            json!({
                "TableName": FAN_OUT,
                "Key": key_of(&row.item),
                "ReturnConsumedCapacity": "INDEXES",
            }),
        )
        .await;
        assert_breakdown(
            &format!("delete absent {}", row.case),
            &absent["ConsumedCapacity"],
            &CapacityCase {
                case: row.case,
                item: row.item.clone(),
                table: 1.0,
                local: vec![],
                global: vec![],
            },
        );
    }
}

#[tokio::test]
async fn a_write_reports_the_item_collection_it_touched_only_for_a_table_with_a_local_index() {
    let router = emulator();
    create_fan_out_table(&router).await;
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": NO_INDEX,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
            ],
        }),
    )
    .await;
    wait_until_active(&router, NO_INDEX).await;

    // The collection grows item by item; each write reports the collection its own
    // partition key names, and nothing about the other partition.
    let rows: [(&str, &str); 4] = [
        ("coll-a", "s1"),
        ("coll-a", "s2"),
        ("coll-a", "s3"),
        ("coll-b", "s1"),
    ];
    for (partition, sort) in rows {
        let item = json!({
            "pk": {"S": partition}, "sk": {"S": sort},
            "lsiAll": {"S": sort}, "payload": {"S": "x".repeat(64)},
        });
        let put = ok(
            &router,
            "PutItem",
            json!({
                "TableName": FAN_OUT,
                "Item": item.clone(),
                "ReturnItemCollectionMetrics": "SIZE",
            }),
        )
        .await;
        let metrics = &put["ItemCollectionMetrics"];
        assert_eq!(
            metrics["ItemCollectionKey"],
            json!({"pk": {"S": partition}}),
            "case put {partition}/{sort}: ItemCollectionKey is the item's partition key \
             attribute alone, got {put}"
        );
        let range: Vec<f64> = metrics["SizeEstimateRangeGB"]
            .as_array()
            .map(|bounds| bounds.iter().filter_map(units).collect())
            .unwrap_or_default();
        assert_eq!(
            range,
            vec![0.0, 1.0],
            "case put {partition}/{sort}: a collection well under a gigabyte brackets to \
             [0.0, 1.0], got {put}"
        );

        // A delete reports the same collection, by the same rule.
        let deleted = ok(
            &router,
            "DeleteItem",
            json!({
                "TableName": FAN_OUT,
                "Key": json!({"pk": {"S": partition}, "sk": {"S": sort}}),
                "ReturnItemCollectionMetrics": "SIZE",
            }),
        )
        .await;
        assert_eq!(
            deleted["ItemCollectionMetrics"]["ItemCollectionKey"],
            json!({"pk": {"S": partition}}),
            "case delete {partition}/{sort}: a delete reports the collection it touched, \
             got {deleted}"
        );
        // Put it back, so the next row's collection really does hold the earlier items.
        ok(
            &router,
            "PutItem",
            json!({"TableName": FAN_OUT, "Item": item}),
        )
        .await;
    }

    // "If set to NONE (the default), no statistics are returned."
    for asked in [Some("NONE"), None] {
        let mut body = json!({
            "TableName": FAN_OUT,
            "Item": {"pk": {"S": "coll-a"}, "sk": {"S": "quiet"}, "lsiAll": {"S": "q"}},
        });
        if let Some(asked) = asked {
            body["ReturnItemCollectionMetrics"] = json!(asked);
        }
        let quiet = ok(&router, "PutItem", body).await;
        assert!(
            quiet.get("ItemCollectionMetrics").is_none(),
            "case put {asked:?}: no ItemCollectionMetrics is reported, got {quiet}"
        );
    }

    // "If the table does not have any local secondary indexes, this information is not
    // returned in the response" — asked for or not.
    for operation in ["PutItem", "DeleteItem"] {
        let body = if operation == "PutItem" {
            json!({
                "TableName": NO_INDEX,
                "Item": {"pk": {"S": "coll-a"}, "sk": {"S": "s1"}},
                "ReturnItemCollectionMetrics": "SIZE",
            })
        } else {
            json!({
                "TableName": NO_INDEX,
                "Key": {"pk": {"S": "coll-a"}, "sk": {"S": "s1"}},
                "ReturnItemCollectionMetrics": "SIZE",
            })
        };
        let answered = ok(&router, operation, body).await;
        assert!(
            answered.get("ItemCollectionMetrics").is_none(),
            "case {operation} on a table with no local secondary index: no \
             ItemCollectionMetrics is reported, got {answered}"
        );
    }

    // A value the `ReturnItemCollectionMetrics` enum does not name is a bad request, not
    // a silent NONE.
    for operation in ["PutItem", "DeleteItem"] {
        let body = if operation == "PutItem" {
            json!({
                "TableName": FAN_OUT,
                "Item": {"pk": {"S": "coll-a"}, "sk": {"S": "s9"}},
                "ReturnItemCollectionMetrics": "ALL",
            })
        } else {
            json!({
                "TableName": FAN_OUT,
                "Key": {"pk": {"S": "coll-a"}, "sk": {"S": "s9"}},
                "ReturnItemCollectionMetrics": "ALL",
            })
        };
        let (status, refused) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {operation} ReturnItemCollectionMetrics=ALL: an unmodelled value is \
             refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {operation} ReturnItemCollectionMetrics=ALL: the refusal is a \
             ValidationException, got {refused}"
        );
    }
}

#[tokio::test]
async fn the_size_estimate_brackets_the_measured_collection_size() {
    // A gigabyte is 10^9 bytes, the decimal unit DynamoDB's size quotas use ("The
    // maximum size of any item collection is 10 GB" — "Service, Account, and Table
    // Quotas"). Each row is a measured size; the reported range must contain it. The
    // rows span both published bounds of a band and one byte past each.
    const GB: i64 = 1_000_000_000;
    let rows: [(i64, [f64; 2]); 9] = [
        (-1, [0.0, 1.0]),
        (0, [0.0, 1.0]),
        (1, [0.0, 1.0]),
        (1024, [0.0, 1.0]),
        (GB - 1, [0.0, 1.0]),
        (GB, [1.0, 2.0]),
        (GB + 1, [1.0, 2.0]),
        (2 * GB, [2.0, 3.0]),
        (10 * GB + GB / 2, [10.0, 11.0]),
    ];
    for (bytes, expected) in rows {
        let range = size_estimate_range_gb(bytes);
        assert_eq!(
            range,
            expected.to_vec(),
            "case {bytes} bytes: the estimate brackets the measured size, got {range:?}"
        );
        let measured = bytes.max(0) as f64 / GB as f64;
        assert!(
            range[0] <= measured && measured <= range[1],
            "case {bytes} bytes: the measured {measured} GB lies inside {range:?}"
        );
    }
}

#[tokio::test]
async fn a_table_created_with_local_secondary_indexes_serves_its_items() {
    let router = emulator();
    let described = create_fan_out_table(&router).await;

    // The description echoes each local index: its name, ARN, key schema and projection.
    let local = described["Table"]["LocalSecondaryIndexes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let names: Vec<&str> = local
        .iter()
        .filter_map(|index| index["IndexName"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["lsiAll", "lsiKeys"],
        "case describe: the local indexes are reported in the order they were declared, \
         got {described}"
    );
    for (index, expected_projection) in local.iter().zip([
        json!({"ProjectionType": "ALL"}),
        json!({"ProjectionType": "KEYS_ONLY"}),
    ]) {
        let name = index["IndexName"].as_str().unwrap_or_default();
        assert_eq!(
            index["Projection"], expected_projection,
            "case describe {name}: the projection is echoed as declared, got {index}"
        );
        assert_eq!(
            index["IndexArn"],
            json!(format!(
                "arn:aws:dynamodb:us-east-1:000000000000:table/{FAN_OUT}/index/{name}"
            )),
            "case describe {name}: the index carries its own ARN, got {index}"
        );
        assert_eq!(
            index["KeySchema"][0],
            json!({"AttributeName": "pk", "KeyType": "HASH"}),
            "case describe {name}: a local index shares the table's partition key, \
             got {index}"
        );
        assert_eq!(
            index["ItemCount"],
            json!(0),
            "case describe {name}: an index of an empty table holds no items, got {index}"
        );
    }

    // The table serves items: a put lands, a get reads it back, a get of a key the table
    // does not hold is an empty 200, and a delete removes it.
    let item = json!({
        "pk": {"S": "coll-a"}, "sk": {"S": "s1"},
        "lsiAll": {"S": "a1"}, "lsiKeys": {"N": "7"}, "payload": {"S": "kept"},
    });
    ok(
        &router,
        "PutItem",
        json!({"TableName": FAN_OUT, "Item": item.clone()}),
    )
    .await;
    let fetched = ok(
        &router,
        "GetItem",
        json!({"TableName": FAN_OUT, "Key": key_of(&item), "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        fetched["Item"], item,
        "case get: the item written through an indexed table reads back whole, got {fetched}"
    );

    let (status, missing) = call(
        &router,
        "GetItem",
        &json!({"TableName": FAN_OUT, "Key": {"pk": {"S": "coll-a"}, "sk": {"S": "absent"}}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case get absent: a key the table does not hold is a success, got {status} \
         with {missing}"
    );
    assert!(
        missing.get("Item").is_none(),
        "case get absent: a missing item yields no Item member, got {missing}"
    );

    // The index figures are the METERED ones: "DynamoDB updates this value approximately
    // every six hours. Recent changes might not be reflected in this value"
    // (`LocalSecondaryIndexDescription$IndexSizeBytes`), so the write above does not move
    // them. That the refresh then counts exactly the projected items is pinned by
    // `table_description_members`, which drives the refresh the clock owes.
    let after_put = ok(&router, "DescribeTable", json!({"TableName": FAN_OUT})).await;
    for index in after_put["Table"]["LocalSecondaryIndexes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let name = index["IndexName"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            (index["ItemCount"].clone(), index["IndexSizeBytes"].clone()),
            (json!(0), json!(0)),
            "case describe after put {name}: expected the figures of the last metering \
             refresh, which a write does not move, got {index}"
        );
    }

    ok(
        &router,
        "DeleteItem",
        json!({"TableName": FAN_OUT, "Key": key_of(&item)}),
    )
    .await;
    let gone = ok(
        &router,
        "GetItem",
        json!({"TableName": FAN_OUT, "Key": key_of(&item), "ConsistentRead": true}),
    )
    .await;
    assert!(
        gone.get("Item").is_none(),
        "case get after delete: the deleted item is gone, got {gone}"
    );
}

#[tokio::test]
async fn a_write_that_replaces_an_item_is_charged_by_the_documented_replacement_rules() {
    let router = emulator();
    create_fan_out_table(&router).await;

    // Each row is a (before, after) pair at ONE key, and the breakdown the Developer
    // Guide's write-capacity rules predict for the replacement:
    //
    // * the table is charged on "the larger of the two" item sizes;
    // * an index whose key value is unchanged takes one write, on the larger of the two
    //   projections;
    // * an index whose key value CHANGED takes two — "one to delete the previous item
    //   from the index and another write to put the new item into the index";
    // * an index the new version no longer reaches still takes the one write that
    //   deletes its old projection;
    // * an index neither version reaches is not charged at all.
    let big = "p".repeat(2000);
    let large_item = json!({
        "pk": {"S": "coll-r"}, "sk": {"S": "s1"},
        "lsiAll": {"S": "a1"}, "lsiKeys": {"N": "1"},
        "gsiPk": {"S": "g1"}, "payload": {"S": big},
    });
    let small_same_keys = json!({
        "pk": {"S": "coll-r"}, "sk": {"S": "s1"},
        "lsiAll": {"S": "a1"}, "lsiKeys": {"N": "1"},
        "gsiPk": {"S": "g1"}, "payload": {"S": "small"},
    });
    let small_moved_local_key = json!({
        "pk": {"S": "coll-r"}, "sk": {"S": "s1"},
        "lsiAll": {"S": "a2"}, "lsiKeys": {"N": "1"},
        "gsiPk": {"S": "g1"}, "payload": {"S": "small"},
    });
    let small_without_local_key = json!({
        "pk": {"S": "coll-r"}, "sk": {"S": "s1"},
        "lsiAll": {"S": "a1"},
        "gsiPk": {"S": "g1"}, "payload": {"S": "small"},
    });
    // A number respelt is the same key, so the index entry is updated, not moved.
    let small_respelt_number_key = json!({
        "pk": {"S": "coll-r"}, "sk": {"S": "s1"},
        "lsiAll": {"S": "a1"}, "lsiKeys": {"N": "1e0"},
        "gsiPk": {"S": "g1"}, "payload": {"S": "small"},
    });

    let rows: Vec<(&str, Value, CapacityCase)> = vec![
        (
            "shrinking replacement is charged on the larger item",
            small_same_keys.clone(),
            CapacityCase {
                case: "shrink",
                item: small_same_keys.clone(),
                table: 2.0,
                local: vec![("lsiAll", 2.0), ("lsiKeys", 1.0)],
                global: vec![("gsi1", 2.0)],
            },
        ),
        (
            "a changed local index key costs a delete and a write",
            small_moved_local_key.clone(),
            CapacityCase {
                case: "moved local key",
                item: small_moved_local_key.clone(),
                table: 1.0,
                local: vec![("lsiAll", 2.0), ("lsiKeys", 1.0)],
                global: vec![("gsi1", 1.0)],
            },
        ),
        (
            "dropping an index key costs the delete of its old projection",
            small_without_local_key.clone(),
            CapacityCase {
                case: "dropped local key",
                item: small_without_local_key.clone(),
                table: 1.0,
                local: vec![("lsiAll", 1.0), ("lsiKeys", 1.0)],
                global: vec![("gsi1", 1.0)],
            },
        ),
        (
            "an index key respelt to the same number is not a move",
            small_respelt_number_key.clone(),
            CapacityCase {
                case: "respelt number key",
                item: small_respelt_number_key.clone(),
                table: 1.0,
                local: vec![("lsiAll", 1.0), ("lsiKeys", 1.0)],
                global: vec![("gsi1", 1.0)],
            },
        ),
    ];

    for (case, after, expected) in rows {
        // Seed the key with the large item, then replace it; the seeding write's own
        // capacity is not what this row is about.
        ok(
            &router,
            "PutItem",
            json!({"TableName": FAN_OUT, "Item": large_item.clone()}),
        )
        .await;
        if case.contains("respelt") || case.contains("dropping") || case.contains("changed") {
            // These rows move or drop an index key, so the "before" version must be the
            // small item that holds the original keys.
            ok(
                &router,
                "PutItem",
                json!({"TableName": FAN_OUT, "Item": small_same_keys.clone()}),
            )
            .await;
        }
        let replaced = ok(
            &router,
            "PutItem",
            json!({
                "TableName": FAN_OUT,
                "Item": after,
                "ReturnConsumedCapacity": "INDEXES",
            }),
        )
        .await;
        assert_breakdown(case, &replaced["ConsumedCapacity"], &expected);
    }
}

#[tokio::test]
async fn a_local_secondary_index_is_held_to_the_table_partition_key_and_the_index_limit() {
    let router = emulator();

    // A local secondary index "has the same partition key as the table, but a different
    // sort key": one whose HASH key is another attribute is a bad request.
    let (status, refused) = call(
        &router,
        "CreateTable",
        &json!({
            "TableName": "WrongHash",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "other", "AttributeType": "S"},
            ],
            "LocalSecondaryIndexes": [{
                "IndexName": "byOther",
                "KeySchema": [
                    {"AttributeName": "other", "KeyType": "HASH"},
                    {"AttributeName": "sk", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case local index on another partition key: refused, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case local index on another partition key: the refusal is a ValidationException, \
         got {refused}"
    );

    // An index name must be "unique among all other indexes on this table", the global
    // ones included.
    let (status, clash) = call(
        &router,
        "CreateTable",
        &json!({
            "TableName": "NameClash",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "alt", "AttributeType": "S"},
            ],
            "LocalSecondaryIndexes": [{
                "IndexName": "shared",
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": "alt", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            }],
            "GlobalSecondaryIndexes": [{
                "IndexName": "shared",
                "KeySchema": [{"AttributeName": "alt", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case duplicate index name: refused, got {status} with {clash}"
    );
    assert_eq!(
        error_code(&clash),
        "ValidationException",
        "case duplicate index name: the refusal is a ValidationException, got {clash}"
    );

    // "You can define a maximum of 5 local secondary indexes per table" ("Service,
    // Account, and Table Quotas"): the sixth is refused.
    let too_many: Vec<Value> = (0..=MAX_LOCAL_SECONDARY_INDEXES)
        .map(|which| {
            json!({
                "IndexName": format!("lsi{which}"),
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": format!("alt{which}"), "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            })
        })
        .collect();
    let mut definitions = vec![
        json!({"AttributeName": "pk", "AttributeType": "S"}),
        json!({"AttributeName": "sk", "AttributeType": "S"}),
    ];
    definitions.extend((0..=MAX_LOCAL_SECONDARY_INDEXES).map(|which| {
        json!({"AttributeName": format!("alt{which}"), "AttributeType": "S"})
    }));
    let (status, over) = call(
        &router,
        "CreateTable",
        &json!({
            "TableName": "TooManyLocal",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": definitions,
            "LocalSecondaryIndexes": too_many,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {} local indexes: one past the quota is refused, got {status} with {over}",
        MAX_LOCAL_SECONDARY_INDEXES + 1
    );
    assert_eq!(
        error_code(&over),
        "ValidationException",
        "case {} local indexes: the refusal is a ValidationException, got {over}",
        MAX_LOCAL_SECONDARY_INDEXES + 1
    );

    // The quota itself is accepted: the same request with one index fewer creates the
    // table, so the refusal above is about the count and nothing else.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "AtTheLocalQuota",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": definitions
                .iter()
                .take(2 + MAX_LOCAL_SECONDARY_INDEXES)
                .cloned()
                .collect::<Vec<Value>>(),
            "LocalSecondaryIndexes": too_many
                .iter()
                .take(MAX_LOCAL_SECONDARY_INDEXES)
                .cloned()
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    let described = wait_until_active(&router, "AtTheLocalQuota").await;
    assert_eq!(
        described["Table"]["LocalSecondaryIndexes"]
            .as_array()
            .map_or(0, Vec::len),
        MAX_LOCAL_SECONDARY_INDEXES,
        "case {MAX_LOCAL_SECONDARY_INDEXES} local indexes: every one is stored and \
         described, got {described}"
    );
}
