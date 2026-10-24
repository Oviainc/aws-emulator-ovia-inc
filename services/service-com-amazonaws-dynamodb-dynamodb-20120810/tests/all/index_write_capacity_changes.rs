// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH indexes a write is charged for.
//!
//! One rule is pinned here: a secondary index consumes write capacity only when the
//! write CHANGES what that index stores — the presence of its entry, its index key, or
//! one of the attributes its `Projection` keeps — while the table itself is charged on
//! the larger of the two versions of the item whether or not the item differs. The
//! Developer Guide's write-capacity rules for secondary indexes ("Provisioned throughput
//! considerations for local secondary indexes", and the same accounting for the global
//! ones) charge a write per CHANGE: putting an entry into the index, deleting it, moving
//! it when an indexed key value changes, and updating "the value of projected
//! attributes" — so an index whose stored entry a write leaves exactly as it stood is
//! charged nothing, which is why projecting fewer attributes lowers an index's write
//! cost.
//!
//! The rows below span that class: the write arrives as `PutItem`, `UpdateItem` and
//! `DeleteItem`; the item is new, replaced, re-written identically, or removed; and the
//! attribute it changes is one every index stores, one only an `ALL` projection stores,
//! one a `KEYS_ONLY` index drops, or one a `NonKeyAttributes` list names.
//!
//! Every call goes over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4-shaped `Authorization` header: its credential scope routes the request to
/// this service (signing name `dynamodb`) and names the region the ARNs it mints carry.
/// Request authentication is off in these tests, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/dynamodb/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000";

/// The table every row writes to: one local secondary index projecting every attribute,
/// one projecting only the keys, and one global secondary index projecting a single named
/// non-key attribute — so the rows differ in which indexes store the attribute they
/// touch.
const TABLE: &str = "IndexChargeTable";

/// One emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one operation that must succeed, and report its parsed body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Create [`TABLE`] and wait out the `CREATING` span the clock owes it.
async fn create_table(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "lsiSk", "AttributeType": "S"},
                {"AttributeName": "lsiNum", "AttributeType": "N"},
                {"AttributeName": "gsiPk", "AttributeType": "S"},
            ],
            "LocalSecondaryIndexes": [
                {
                    "IndexName": "allLocal",
                    "KeySchema": [
                        {"AttributeName": "pk", "KeyType": "HASH"},
                        {"AttributeName": "lsiSk", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": "keysLocal",
                    "KeySchema": [
                        {"AttributeName": "pk", "KeyType": "HASH"},
                        {"AttributeName": "lsiNum", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                },
            ],
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": "includeGlobal",
                    "KeySchema": [{"AttributeName": "gsiPk", "KeyType": "HASH"}],
                    "Projection": {
                        "ProjectionType": "INCLUDE",
                        "NonKeyAttributes": ["tracked"],
                    },
                },
            ],
        }),
    )
    .await;
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    let described = ok(router, "DescribeTable", json!({"TableName": TABLE})).await;
    assert_eq!(
        described["Table"]["TableStatus"],
        json!("ACTIVE"),
        "case create {TABLE}: the table becomes ACTIVE once the transition is due, \
         got {described}"
    );
}

/// The item every row starts from, at the one key they all write.
fn base_item() -> Value {
    json!({
        "pk": {"S": "p1"}, "sk": {"S": "s1"},
        "lsiSk": {"S": "a1"}, "lsiNum": {"N": "1"},
        "gsiPk": {"S": "g1"},
        "tracked": {"S": "t1"}, "untracked": {"S": "u1"},
    })
}

/// `base_item` with `edits` applied: an entry whose value is `null` is REMOVED from the
/// item, any other entry replaces (or adds) that attribute.
fn item_with(edits: &[(&str, Value)]) -> Value {
    let mut item = base_item();
    let map = item
        .as_object_mut()
        .expect("the base item is a JSON object");
    for (name, value) in edits {
        if value.is_null() {
            map.remove(*name);
        } else {
            map.insert((*name).to_string(), value.clone());
        }
    }
    item
}

/// The `{index name -> CapacityUnits}` pairs one `ConsumedCapacity` member carries, in
/// name order, and an empty vector when the member is absent.
fn index_units(consumed: &Value, member: &str) -> Vec<(String, f64)> {
    let mut entries: Vec<(String, f64)> = consumed
        .get(member)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(name, capacity)| {
                    (
                        name.clone(),
                        capacity["CapacityUnits"].as_f64().unwrap_or(f64::NAN),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

/// Assert one `ConsumedCapacity` charges exactly `table` units on the table and exactly
/// `indexes` on the named indexes — nothing on any other index, and an aggregate that is
/// their sum.
fn assert_charged(case: &str, consumed: &Value, table: f64, indexes: &[(&str, f64)]) {
    let expected_total = table + indexes.iter().map(|(_, units)| units).sum::<f64>();
    assert_eq!(
        consumed["CapacityUnits"].as_f64(),
        Some(expected_total),
        "case {case}: the aggregate CapacityUnits is the table's units plus every \
         charged index's, got {consumed}"
    );
    assert_eq!(
        consumed["Table"]["CapacityUnits"].as_f64(),
        Some(table),
        "case {case}: Table carries the units the table itself took, got {consumed}"
    );
    let mut expected: Vec<(String, f64)> = indexes
        .iter()
        .map(|(name, units)| ((*name).to_string(), *units))
        .collect();
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    let local: Vec<(String, f64)> = expected
        .iter()
        .filter(|(name, _)| name.ends_with("Local"))
        .cloned()
        .collect();
    let global: Vec<(String, f64)> = expected
        .iter()
        .filter(|(name, _)| name.ends_with("Global"))
        .cloned()
        .collect();
    assert_eq!(
        index_units(consumed, "LocalSecondaryIndexes"),
        local,
        "case {case}: only the local indexes whose stored entry the write changed are \
         charged, got {consumed}"
    );
    assert_eq!(
        index_units(consumed, "GlobalSecondaryIndexes"),
        global,
        "case {case}: only the global indexes whose stored entry the write changed are \
         charged, got {consumed}"
    );
    if local.is_empty() {
        assert!(
            consumed.get("LocalSecondaryIndexes").is_none(),
            "case {case}: a write that changes no local index entry carries no \
             LocalSecondaryIndexes map at all, got {consumed}"
        );
    }
    if global.is_empty() {
        assert!(
            consumed.get("GlobalSecondaryIndexes").is_none(),
            "case {case}: a write that changes no global index entry carries no \
             GlobalSecondaryIndexes map at all, got {consumed}"
        );
    }
}

/// One row: what the key holds before the write, what `PutItem` writes, and the units
/// the table and each charged index take for it.
struct PutRow {
    case: &'static str,
    /// The item seeded at the key first, or `None` to write onto an empty key.
    before: Option<Value>,
    after: Value,
    table: f64,
    indexes: Vec<(&'static str, f64)>,
}

/// The rows, each differing in WHAT the write changes rather than in what it is named.
///
/// One write capacity unit covers 1 KB ("Read/write capacity mode" in the Developer
/// Guide), so every item here costs one unit per entry it changes except the ~1.5 KB
/// rows, which cost two wherever the big attribute is stored.
fn put_rows() -> Vec<PutRow> {
    let big = "p".repeat(1500);
    vec![
        PutRow {
            case: "a new item puts an entry into every index it reaches",
            before: None,
            after: base_item(),
            table: 1.0,
            indexes: vec![("allLocal", 1.0), ("keysLocal", 1.0), ("includeGlobal", 1.0)],
        },
        PutRow {
            case: "a new item reaching one index only is charged for that one",
            before: None,
            after: json!({"pk": {"S": "p1"}, "sk": {"S": "s1"}, "lsiSk": {"S": "a1"}}),
            table: 1.0,
            indexes: vec![("allLocal", 1.0)],
        },
        PutRow {
            case: "a new item reaching no index is charged on the table alone",
            before: None,
            after: json!({"pk": {"S": "p1"}, "sk": {"S": "s1"}, "untracked": {"S": "u1"}}),
            table: 1.0,
            indexes: vec![],
        },
        PutRow {
            case: "re-writing the identical item changes no index entry",
            before: Some(base_item()),
            after: base_item(),
            table: 1.0,
            indexes: vec![],
        },
        PutRow {
            case: "an index key respelt to the same number changes no index entry",
            before: Some(base_item()),
            after: item_with(&[("lsiNum", json!({"N": "1.0e0"}))]),
            table: 1.0,
            indexes: vec![],
        },
        PutRow {
            case: "changing an attribute only the ALL projection stores charges it alone",
            before: Some(base_item()),
            after: item_with(&[("untracked", json!({"S": "u2"}))]),
            table: 1.0,
            indexes: vec![("allLocal", 1.0)],
        },
        PutRow {
            case: "adding an attribute only the ALL projection stores charges it alone",
            before: Some(base_item()),
            after: item_with(&[("extra", json!({"BOOL": true}))]),
            table: 1.0,
            indexes: vec![("allLocal", 1.0)],
        },
        PutRow {
            case: "changing an INCLUDEd attribute charges the index that includes it",
            before: Some(base_item()),
            after: item_with(&[("tracked", json!({"S": "t2"}))]),
            table: 1.0,
            indexes: vec![("allLocal", 1.0), ("includeGlobal", 1.0)],
        },
        PutRow {
            case: "moving a local index key costs a delete and a write there",
            before: Some(base_item()),
            after: item_with(&[("lsiSk", json!({"S": "a2"}))]),
            table: 1.0,
            indexes: vec![("allLocal", 2.0)],
        },
        PutRow {
            case: "moving the global index key costs a delete and a write there",
            before: Some(base_item()),
            after: item_with(&[("gsiPk", json!({"S": "g2"}))]),
            table: 1.0,
            indexes: vec![("allLocal", 1.0), ("includeGlobal", 2.0)],
        },
        PutRow {
            case: "dropping an index key deletes that index's projection",
            before: Some(base_item()),
            after: item_with(&[("lsiNum", Value::Null)]),
            table: 1.0,
            indexes: vec![("allLocal", 1.0), ("keysLocal", 1.0)],
        },
        PutRow {
            case: "defining a previously undefined index key puts the item into it",
            before: Some(item_with(&[("lsiNum", Value::Null)])),
            after: base_item(),
            table: 1.0,
            indexes: vec![("allLocal", 1.0), ("keysLocal", 1.0)],
        },
        PutRow {
            case: "a long untracked attribute is charged on the ALL index only",
            before: Some(base_item()),
            after: item_with(&[("untracked", json!({"S": big.clone()}))]),
            table: 2.0,
            indexes: vec![("allLocal", 2.0)],
        },
        PutRow {
            case: "a long tracked attribute is charged on both indexes storing it",
            before: Some(base_item()),
            after: item_with(&[("tracked", json!({"S": big}))]),
            table: 2.0,
            indexes: vec![("allLocal", 2.0), ("includeGlobal", 2.0)],
        },
    ]
}

#[tokio::test]
async fn a_put_is_charged_only_for_the_index_entries_it_changes() {
    let router = emulator();
    create_table(&router).await;

    for row in put_rows() {
        // Put the key into the row's starting state: the seeding write's own capacity is
        // not what the row is about.
        ok(
            &router,
            "DeleteItem",
            json!({"TableName": TABLE, "Key": {"pk": {"S": "p1"}, "sk": {"S": "s1"}}}),
        )
        .await;
        if let Some(before) = &row.before {
            ok(
                &router,
                "PutItem",
                json!({"TableName": TABLE, "Item": before.clone()}),
            )
            .await;
        }

        let written = ok(
            &router,
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": row.after.clone(),
                "ReturnConsumedCapacity": "INDEXES",
            }),
        )
        .await;
        assert_charged(
            row.case,
            &written["ConsumedCapacity"],
            row.table,
            &row.indexes,
        );
    }
}

/// One `UpdateItem` row: the expression it carries (the key, table and
/// `ReturnConsumedCapacity` are filled in below) and the indexes its change charges.
struct UpdateRow {
    case: &'static str,
    request: Value,
    indexes: Vec<(&'static str, f64)>,
}

#[tokio::test]
async fn an_update_and_a_delete_follow_the_same_rule() {
    let router = emulator();
    create_table(&router).await;
    let key = json!({"pk": {"S": "p1"}, "sk": {"S": "s1"}});

    // Each row is an UpdateItem against the base item and the charge it draws. An update
    // reaches the indexes through the attributes it changes, exactly as a put does.
    let updates: Vec<UpdateRow> = vec![
        UpdateRow {
            case: "setting an attribute to the value it already holds",
            request: json!({"UpdateExpression": "SET untracked = :same",
                   "ExpressionAttributeValues": {":same": {"S": "u1"}}}),
            indexes: vec![],
        },
        UpdateRow {
            case: "setting an attribute only the ALL projection stores",
            request: json!({"UpdateExpression": "SET untracked = :next",
                   "ExpressionAttributeValues": {":next": {"S": "u2"}}}),
            indexes: vec![("allLocal", 1.0)],
        },
        UpdateRow {
            case: "setting the attribute the global index includes",
            request: json!({"UpdateExpression": "SET tracked = :next",
                   "ExpressionAttributeValues": {":next": {"S": "t2"}}}),
            indexes: vec![("allLocal", 1.0), ("includeGlobal", 1.0)],
        },
        UpdateRow {
            case: "removing a local index key",
            request: json!({"UpdateExpression": "REMOVE lsiNum"}),
            indexes: vec![("allLocal", 1.0), ("keysLocal", 1.0)],
        },
    ];

    for UpdateRow {
        case,
        mut request,
        indexes,
    } in updates
    {
        ok(
            &router,
            "PutItem",
            json!({"TableName": TABLE, "Item": base_item()}),
        )
        .await;
        let body = request
            .as_object_mut()
            .expect("every update row is a JSON object");
        body.insert("TableName".to_string(), json!(TABLE));
        body.insert("Key".to_string(), key.clone());
        body.insert("ReturnConsumedCapacity".to_string(), json!("INDEXES"));
        let updated = ok(&router, "UpdateItem", request).await;
        assert_charged(case, &updated["ConsumedCapacity"], 1.0, &indexes);
    }

    // A delete removes the item from every index it reached, so each of those entries
    // changes and is charged; a delete that finds nothing changes none of them and costs
    // the table its one-unit minimum.
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": base_item()}),
    )
    .await;
    let deleted = ok(
        &router,
        "DeleteItem",
        json!({
            "TableName": TABLE,
            "Key": key.clone(),
            "ReturnConsumedCapacity": "INDEXES",
        }),
    )
    .await;
    assert_charged(
        "deleting an item removes it from every index it reached",
        &deleted["ConsumedCapacity"],
        1.0,
        &[("allLocal", 1.0), ("keysLocal", 1.0), ("includeGlobal", 1.0)],
    );

    let absent = ok(
        &router,
        "DeleteItem",
        json!({
            "TableName": TABLE,
            "Key": key,
            "ReturnConsumedCapacity": "INDEXES",
        }),
    )
    .await;
    assert_charged(
        "deleting a key the table does not hold",
        &absent["ConsumedCapacity"],
        1.0,
        &[],
    );
}
