// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a table that carries a VECTOR INDEX accepts: the writes it
//! admits, and the vector indexes `UpdateTable` adds to it or removes from it.
//!
//! The rules pinned here:
//!
//! 1. "If the table has vector indexes, the following validations apply to write
//!    operations. A violation of any of these constraints results in a
//!    `ValidationException`" (the `PutItem$Item` doc): the vector attribute must be a list
//!    of numbers with dimensions matching the index configuration, the values must fit in
//!    32-bit IEEE-754 floating point format, and the search-schema attributes must carry
//!    the types the table declares for them. The refused write lands NOTHING, and the rule
//!    holds on every operation that stores an item, not only `PutItem`;
//! 2. an item that carries none of the index's attributes is not in the index and is
//!    written as it is, so the constraints above bound what the index holds rather than
//!    what the table holds;
//! 3. `UpdateTable`'s `VectorIndexUpdates` carries ONE index action per call: a second
//!    create, or a create beside a delete, exceeds what one call may ask for and is
//!    answered the control plane's `LimitExceededException` without changing the table;
//! 4. an accepted `Create` adds the index it describes — the table is `UPDATING` while the
//!    index is `CREATING`, the description reports the configuration the request named, and
//!    once it is `ACTIVE` a `SearchVectors` through it answers the table's items, each
//!    carrying the attributes the new index projects.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and each rule is asserted over a
//! SET of inputs — the shape of the value written, which operation writes it, which index
//! action the call asks for — of which the recorded one-item case is a single row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The dimensions the table's own vector index is created with, so every row below states
/// its vector against one configured count.
const DIMENSIONS: usize = 3;

/// Wait out one transient span, so the table's `CREATING` -> `ACTIVE` transition and its
/// indexes' are due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// One embedding, in the DynamoDB list format a vector attribute is written in.
fn embedding(values: &[f64]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|value| json!({"N": value.to_string()}))
            .collect(),
    )
}

/// A list of `count` components, each a number any 32-bit float holds.
fn vector_of(count: usize) -> Value {
    let values: Vec<f64> = (0..count).map(|index| index as f64).collect();
    json!({"L": embedding(&values)})
}

/// Create the table every write row below is addressed to: a `pk` partition key, one
/// vector index of [`DIMENSIONS`] dimensions over `emb`, and a search schema over `tier`
/// (declared `S`), then wait for it to become `ACTIVE`.
async fn write_table(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "tier", "AttributeType": "S"},
            ],
            "VectorIndexes": [{
                "IndexName": "idx-emb",
                "VectorAttribute": {"AttributeName": "emb"},
                "Dimensions": DIMENSIONS,
                "DistanceFunction": "COSINE",
                "Projection": {"ProjectionType": "ALL"},
                "SearchSchema": [{"AttributeName": "tier", "SearchSchemaElementType": "HASH"}],
            }],
        }),
    )
    .await;
    settle().await;
}

/// Whether the table still holds no item under `key`, read back through `GetItem` — what a
/// refused write must leave behind.
async fn item_absent(router: &MultiServiceRouter, table: &str, key: &str) -> bool {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": table, "Key": {"pk": {"S": key}}}),
    )
    .await;
    read.get("Item").is_none_or(|item| !item.is_object())
}

/// Rules 1 and 2, over `PutItem`: the item's vector attribute and its search-schema
/// attributes are held to the index's configuration, and a refused write lands nothing.
#[tokio::test]
async fn a_write_is_held_to_the_shape_the_vector_index_was_created_with() {
    let router = emulator();
    let table = "VectorWriteShapes";
    write_table(&router, table).await;

    // Rows differ along what the rule generalises over: the NUMBER of components (one
    // short, one long, far off, none at all), the TYPE of the attribute and of its
    // components, the MAGNITUDE of a component (one past what a 32-bit float holds), and
    // the type of a search-schema attribute — plus the two accepting rows, an item of the
    // index's own shape and one that carries none of its attributes at all. Each refusal
    // states the CONSTRAINT the item broke, over the attribute, the counts and the types
    // the request and the index carry.
    let dimensions = format!(
        "One or more parameter values were invalid: The number of dimensions in the vector \
         attribute emb must match the number of dimensions configured for the vector index \
         idx-emb: {DIMENSIONS}"
    );
    let not_numbers = "One or more parameter values were invalid: The vector attribute emb must \
                       hold only numbers, each a 32-bit IEEE-754 floating point value"
        .to_string();
    let schema_type = "One or more parameter values were invalid: Type mismatch for Index Key \
                       tier Expected: S Actual: N IndexName: idx-emb";
    let cases: [(&str, Value, Option<String>); 11] = [
        (
            "one component short",
            vector_of(DIMENSIONS - 1),
            Some(dimensions.clone()),
        ),
        (
            "one component long",
            vector_of(DIMENSIONS + 1),
            Some(dimensions.clone()),
        ),
        ("a single component", vector_of(1), Some(dimensions.clone())),
        (
            "no components at all",
            json!({"L": []}),
            Some(dimensions.clone()),
        ),
        (
            "a value that is not a list",
            json!({"S": "1,0,0"}),
            Some(dimensions),
        ),
        (
            "a list of strings",
            json!({"L": [{"S": "1"}, {"S": "0"}, {"S": "0"}]}),
            Some(not_numbers.clone()),
        ),
        (
            "a list with one component that is not a number",
            json!({"L": [{"N": "1"}, {"BOOL": true}, {"N": "0"}]}),
            Some(not_numbers.clone()),
        ),
        (
            "a component past the 32-bit float range",
            json!({"L": [{"N": "1"}, {"N": "1e40"}, {"N": "0"}]}),
            Some(not_numbers),
        ),
        (
            "a component at the 32-bit float range",
            json!({"L": [{"N": "1"}, {"N": "3.4e38"}, {"N": "0"}]}),
            None,
        ),
        ("the index's own shape", vector_of(DIMENSIONS), None),
        // The vector attribute is absent, so this item is in no index entry at all.
        ("no vector attribute", Value::Null, None),
    ];

    for (case, written, refused) in cases {
        let key = case.replace(' ', "-");
        let mut item = json!({"pk": {"S": key.clone()}, "tier": {"S": "gold"}});
        if !written.is_null() {
            item["emb"] = written;
        }
        let (status, body) = call(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": item}),
        )
        .await;
        match refused {
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
                );
                assert_eq!(
                    message(&body),
                    expected,
                    "case {case:?}: expected the constraint's own sentence, got {:?}",
                    message(&body)
                );
                assert!(
                    item_absent(&router, table, &key).await,
                    "case {case:?}: a refused write must land nothing, got the item stored"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the write to be served, got {status} with {body}"
                );
                assert!(
                    !item_absent(&router, table, &key).await,
                    "case {case:?}: an accepted write must store the item, got nothing"
                );
            }
        }
    }

    // The search schema's own constraint, over the type the table declares for `tier`.
    let (status, body) = call(
        &router,
        "PutItem",
        json!({
            "TableName": table,
            "Item": {
                "pk": {"S": "wrong-tier-type"},
                "tier": {"N": "1"},
                "emb": vector_of(DIMENSIONS),
            },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body), message(&body)),
        (
            400,
            "ValidationException".to_string(),
            schema_type.to_string()
        ),
        "case \"a search-schema attribute of another type\": expected the search-schema \
         constraint, got {status} with {body}"
    );
    assert!(
        item_absent(&router, table, "wrong-tier-type").await,
        "case \"a search-schema attribute of another type\": a refused write lands nothing"
    );
}

/// Rule 1, over the OTHER operations that store an item: the constraint is the table's, so
/// it holds whichever operation the write arrives on.
#[tokio::test]
async fn every_write_operation_is_held_to_the_same_vector_constraints() {
    let router = emulator();
    let table = "VectorWriteOperations";
    write_table(&router, table).await;

    // Rows differ along the OPERATION the write arrives on — the item writes and the two
    // batched ones — each asked to store a vector one component short.
    let bad = vector_of(DIMENSIONS - 1);
    let item = |key: &str| json!({"pk": {"S": key}, "emb": bad.clone()});
    let cases: [(&str, &str, Value); 4] = [
        (
            "PutItem",
            "put",
            json!({"TableName": table, "Item": item("put")}),
        ),
        (
            "UpdateItem",
            "updated",
            json!({
                "TableName": table,
                "Key": {"pk": {"S": "updated"}},
                "UpdateExpression": "SET emb = :v",
                "ExpressionAttributeValues": {":v": bad.clone()},
            }),
        ),
        (
            "BatchWriteItem",
            "batched",
            json!({"RequestItems": {table: [{"PutRequest": {"Item": item("batched")}}]}}),
        ),
        (
            "TransactWriteItems",
            "transacted",
            json!({
                "TransactItems": [{"Put": {"TableName": table, "Item": item("transacted")}}],
            }),
        ),
    ];

    for (operation, key, request) in cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {operation:?}: expected a 400 ValidationException, got {status} with {body}"
        );
        assert!(
            item_absent(&router, table, key).await,
            "case {operation:?}: a refused write must land nothing, got the item stored"
        );
    }

    // The control: the same operations store an item of the index's own shape, so what the
    // rows above refuse is the vector and not the operation.
    for (operation, key, request) in [
        (
            "PutItem",
            "put-ok",
            json!({"TableName": table, "Item": {"pk": {"S": "put-ok"}, "emb": vector_of(DIMENSIONS)}}),
        ),
        (
            "UpdateItem",
            "updated-ok",
            json!({
                "TableName": table,
                "Key": {"pk": {"S": "updated-ok"}},
                "UpdateExpression": "SET emb = :v",
                "ExpressionAttributeValues": {":v": vector_of(DIMENSIONS)},
            }),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 200,
            "case {operation:?} with the index's own shape: expected HTTP 200, got {status} \
             with {body}"
        );
        assert!(
            !item_absent(&router, table, key).await,
            "case {operation:?} with the index's own shape: expected the item stored"
        );
    }
}

/// Every vector index a table description reports, as (name, status) pairs in name order.
fn reported_indexes(description: &Value) -> Vec<(String, String)> {
    let mut reported: Vec<(String, String)> = description["VectorIndexes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|index| {
            (
                index["IndexName"].as_str().unwrap_or_default().to_string(),
                index["IndexStatus"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    reported.sort();
    reported
}

/// One `Create` action for an index named `index` over `attribute`, of `dimensions`
/// dimensions.
fn create_action(index: &str, attribute: &str, dimensions: usize) -> Value {
    json!({
        "Create": {
            "IndexName": index,
            "VectorAttribute": {"AttributeName": attribute},
            "Dimensions": dimensions,
            "DistanceFunction": "EUCLIDEAN",
            "Projection": {"ProjectionType": "INCLUDE", "NonKeyAttributes": ["note"]},
        }
    })
}

/// Rule 3: one `UpdateTable` carries one vector index action, and a call that asks for
/// more is answered the control plane's limit error without changing the table.
#[tokio::test]
async fn more_than_one_vector_index_action_in_one_call_is_refused() {
    let router = emulator();
    let table = "VectorIndexActionLimit";
    write_table(&router, table).await;

    // Rows differ along which pair of actions the call asks for: two creates, two deletes,
    // and a create beside a delete — every combination that is more than one action.
    let cases: [(&str, Value); 3] = [
        (
            "two creates",
            json!([
                create_action("idx-new", "emb2", 2),
                create_action("idx-other", "emb3", 4),
            ]),
        ),
        (
            "two deletes",
            json!([
                {"Delete": {"IndexName": "idx-emb"}},
                {"Delete": {"IndexName": "idx-gone"}},
            ]),
        ),
        (
            "a create beside a delete",
            json!([
                create_action("idx-new", "emb2", 2),
                {"Delete": {"IndexName": "idx-emb"}},
            ]),
        ),
    ];

    for (case, updates) in cases {
        let (status, body) = call(
            &router,
            "UpdateTable",
            json!({"TableName": table, "VectorIndexUpdates": updates}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "LimitExceededException"),
            "case {case:?}: expected the control plane's limit error, got {status} with {body}"
        );
        assert_eq!(
            message(&body),
            "Subscriber limit exceeded: Only 1 online index can be created or deleted \
             simultaneously per table",
            "case {case:?}: expected AWS's fixed limit sentence, got {:?}",
            message(&body)
        );

        // The refusal changed nothing: the table never left ACTIVE and still carries
        // exactly the index it was created with.
        let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
        let described = &described["Table"];
        assert_eq!(
            (
                described["TableStatus"].as_str().unwrap_or_default(),
                reported_indexes(described)
            ),
            ("ACTIVE", vec![("idx-emb".to_string(), "ACTIVE".to_string())]),
            "case {case:?}: a refused update leaves the table as it was, got {described}"
        );
    }

    // The create's own caller-chosen list is bounded too: a search schema longer than a
    // vector index may carry is refused before anything walks or stores it, with the same
    // sentence `CreateTable` states for an index declared with the table.
    let elements: Vec<Value> = (0..=service_com_amazonaws_dynamodb_dynamodb_20120810::state::MAX_VECTOR_SEARCH_SCHEMA_ELEMENTS)
        .map(|index| json!({"AttributeName": format!("a{index}"),
                            "SearchSchemaElementType": "INLINE_FILTER"}))
        .collect();
    let mut oversized = create_action("idx-wide", "emb2", 2);
    oversized["Create"]["SearchSchema"] = Value::Array(elements.clone());
    let (status, body) = call(
        &router,
        "UpdateTable",
        json!({"TableName": table, "VectorIndexUpdates": [oversized]}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body), message(&body)),
        (
            400,
            "ValidationException".to_string(),
            format!(
                "Invalid SearchSchema for index idx-wide: a vector index's search schema may \
                 carry at most {} elements, got {}",
                service_com_amazonaws_dynamodb_dynamodb_20120810::state::MAX_VECTOR_SEARCH_SCHEMA_ELEMENTS,
                elements.len()
            )
        ),
        "case \"a search schema one element past the bound\": expected the per-index element \
         bound, got {status} with {body}"
    );
}

/// Rule 4: an accepted `Create` adds the index it describes, which becomes searchable.
#[tokio::test]
async fn a_created_vector_index_becomes_a_searchable_index_of_the_table() {
    let router = emulator();
    let table = "VectorIndexCreated";
    write_table(&router, table).await;

    let accepted = ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": table,
            "VectorIndexUpdates": [create_action("idx-added", "emb2", 2)],
        }),
    )
    .await;
    let described = &accepted["TableDescription"];
    assert_eq!(
        described["TableStatus"].as_str().unwrap_or_default(),
        "UPDATING",
        "case \"the accepted create\": adding an index is work that outlives the call, got \
         {described}"
    );
    assert_eq!(
        reported_indexes(described),
        vec![
            ("idx-added".to_string(), "CREATING".to_string()),
            ("idx-emb".to_string(), "ACTIVE".to_string()),
        ],
        "case \"the accepted create\": the new index is reported CREATING beside its sibling, \
         got {described}"
    );

    // Once the table settles, the index reports the configuration the request named.
    let settled = describe_when_active(&router, "the created index", table).await;
    assert_eq!(
        reported_indexes(&settled),
        vec![
            ("idx-added".to_string(), "ACTIVE".to_string()),
            ("idx-emb".to_string(), "ACTIVE".to_string()),
        ],
        "case \"the settled table\": both indexes are ACTIVE, got {settled}"
    );
    let added = settled["VectorIndexes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|index| index["IndexName"] == json!("idx-added"))
        .unwrap_or(Value::Null);
    assert_eq!(
        (
            added["Dimensions"].clone(),
            added["DistanceFunction"].clone(),
            added["VectorAttribute"]["AttributeName"].clone(),
            added["Projection"].clone(),
        ),
        (
            json!(2),
            json!("EUCLIDEAN"),
            json!("emb2"),
            json!({"ProjectionType": "INCLUDE", "NonKeyAttributes": ["note"]}),
        ),
        "case \"the created index\": the description reports what the request named, got {added}"
    );
    assert_eq!(
        added["IndexArn"],
        json!(service_com_amazonaws_dynamodb_dynamodb_20120810::state::index_arn(
            crate::emulator::REGION,
            table,
            "idx-added"
        )),
        "case \"the created index\": it carries the ARN this service mints for an index, got \
         {added}"
    );

    // The new index holds the items the table carries under its own attribute, so a search
    // through it answers them — each with the attributes it projects and never the vector.
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": table,
            "Item": {
                "pk": {"S": "item-1"},
                "note": {"S": "kept"},
                "extra": {"S": "dropped"},
                "emb2": {"L": embedding(&[1.0, 0.0])},
            },
        }),
    )
    .await;
    let found = ok(
        &router,
        "SearchVectors",
        json!({
            "TableName": table,
            "IndexName": "idx-added",
            "SearchVector": embedding(&[1.0, 0.0]),
            "TopK": 5,
        }),
    )
    .await;
    let results = found["SearchResults"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        results.len(),
        1,
        "case \"a search through the created index\": expected the one item it holds, got {found}"
    );
    let mut attributes: Vec<String> = results[0]["Item"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .keys()
        .cloned()
        .collect();
    attributes.sort();
    assert_eq!(
        attributes,
        vec!["note".to_string(), "pk".to_string()],
        "case \"a search through the created index\": expected the projected attributes, got \
         {found}"
    );
}
