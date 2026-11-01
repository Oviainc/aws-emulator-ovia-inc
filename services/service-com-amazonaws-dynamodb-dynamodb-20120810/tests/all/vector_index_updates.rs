// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateTable`'s `VectorIndexUpdates`, the member that removes a
//! vector index from a table.
//!
//! The rules pinned here:
//!
//! 1. a `Delete` addresses the vector index AS THE RESOURCE it changes, so a name the
//!    addressed table carries no vector index for is refused with
//!    `ResourceNotFoundException` at HTTP 400 over both names, and the refusal writes
//!    nothing: the table stays `ACTIVE` and every vector index it holds stays present and
//!    `ACTIVE`;
//! 2. a `Delete` naming a vector index the table does carry removes exactly that index —
//!    the update is index lifecycle work, so the table is `UPDATING` until the clock
//!    settles it `ACTIVE`, and the siblings are left searchable while a `SearchVectors`
//!    through the removed name gets the `ValidationException` an `IndexName` member no
//!    index of the table answers;
//! 3. a removed index takes its schema's `AttributeDefinitions` with it, exactly as a
//!    removed secondary index takes its key attributes;
//! 4. an entry that does not ask for exactly one of the two actions the model gives it,
//!    and a list that names one index twice, ask for no single change and are refused as
//!    `ValidationException`s.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and each rule is asserted over a
//! SET of inputs — which name a delete misses by, which of the table's indexes is removed
//! and from which position of its list, how many indexes the table carries, the distance
//! function each was created with — rather than one example.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// Wait out one transient span, so the table's `CREATING` -> `ACTIVE` transition and its
/// indexes' are due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// One vector index body over the `emb` attribute, with four dimensions.
fn vector_index(name: &str, distance_function: &str, search_schema: Value) -> Value {
    let mut index = json!({
        "IndexName": name,
        "VectorAttribute": {"AttributeName": "emb"},
        "Dimensions": 4,
        "DistanceFunction": distance_function,
        "Projection": {"ProjectionType": "ALL"},
    });
    if !search_schema.is_null() {
        index["SearchSchema"] = search_schema;
    }
    index
}

/// Create one on-demand table with a `pk` partition key, the attribute definitions, the
/// vector indexes and the global secondary indexes given, and wait for it to be `ACTIVE`.
async fn table_with(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    definitions: Value,
    vector_indexes: Value,
    global_secondary_indexes: Value,
) {
    let mut request = json!({
        "TableName": name,
        "BillingMode": "PAY_PER_REQUEST",
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": definitions,
        "VectorIndexes": vector_indexes,
    });
    if !global_secondary_indexes.is_null() {
        request["GlobalSecondaryIndexes"] = global_secondary_indexes;
    }
    ok(router, "CreateTable", request).await;
    settle().await;
}

/// One `UpdateTable` body asking for the vector index updates given.
fn update_request(table: &str, updates: Value) -> Value {
    json!({"TableName": table, "VectorIndexUpdates": updates})
}

/// One `Delete` entry for `index`.
fn delete_action(index: &str) -> Value {
    json!({"Delete": {"IndexName": index}})
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

/// The names of the vector indexes a description reports, in name order.
fn reported_index_names(description: &Value) -> Vec<String> {
    reported_indexes(description)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// Those names paired with `ACTIVE`, which is what a settled table reports them in.
fn active(names: &[&str]) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = names
        .iter()
        .map(|name| ((*name).to_string(), "ACTIVE".to_string()))
        .collect();
    expected.sort();
    expected
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

/// The `pk` of every item a search answered, in the order it answered them.
fn result_keys(response: &Value) -> Vec<String> {
    response["SearchResults"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|result| {
            result["Item"]["pk"]
                .get("S")
                .and_then(Value::as_str)
                .unwrap_or("<no pk>")
                .to_string()
        })
        .collect()
}

/// `SearchVectors` through `index` for the one embedding the tests store.
async fn search(
    router: &smithy_server_runtime::MultiServiceRouter,
    table: &str,
    index: &str,
) -> (u16, Value) {
    call(
        router,
        "SearchVectors",
        json!({
            "TableName": table,
            "IndexName": index,
            "SearchVector": embedding(&[1.0, 0.0, 0.0, 0.0]),
            "TopK": 4,
        }),
    )
    .await
}

/// The three vector indexes the tables below carry, each with a different distance
/// function (the values `smithy_query(query="shape", shape="VectorDistanceFunction")`
/// declares) so no row can be satisfied by a single configuration.
fn three_indexes() -> Value {
    json!([
        vector_index("idx-keep", "EUCLIDEAN", Value::Null),
        vector_index("idx-drop", "DOT_PRODUCT", Value::Null),
        vector_index("idx.third", "COSINE", Value::Null),
    ])
}

#[tokio::test]
async fn a_delete_naming_no_vector_index_of_the_table_is_refused_and_changes_nothing() {
    let router = emulator();
    let table = "VectorDeleteUnknown";
    table_with(
        &router,
        table,
        json!([
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "tenant", "AttributeType": "S"},
        ]),
        three_indexes(),
        // A global secondary index on the same table, so one row can delete a name that
        // IS an index of the table — just not a vector one.
        json!([{
            "IndexName": "by-tenant",
            "KeySchema": [{"AttributeName": "tenant", "KeyType": "HASH"}],
            "Projection": {"ProjectionType": "ALL"},
        }]),
    )
    .await;

    // Rows differ along what the rule generalises over: how the name misses (a near miss
    // of a stored name, the same name in another case, the name of an index of ANOTHER
    // kind on this very table, a name nothing carries), and its length — the shortest the
    // model's `@length` 3-255 allows is a row of its own.
    let cases: [(&str, &str); 5] = [
        ("a near miss of a stored vector index name", "idx-dropped"),
        ("a stored name in another case", "IDX-DROP"),
        ("the name of a global secondary index", "by-tenant"),
        ("a name no index of the table carries", "absent.index"),
        ("the shortest name the model allows", "zzz"),
    ];

    for (case, missing) in cases {
        let (status, body) = call(
            &router,
            "UpdateTable",
            update_request(table, json!([delete_action(missing)])),
        )
        .await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ResourceNotFoundException".to_string(),
                format!("Requested resource not found: Index {missing} for table {table}")
            ),
            "case {case:?}: a delete of a vector index the table does not carry is refused over \
             both names, got {status} with {body}"
        );

        // The refusal wrote nothing: the table never left ACTIVE, and every index it was
        // created with is still there and still serving.
        let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
        let described = &described["Table"];
        assert_eq!(
            described["TableStatus"].as_str().unwrap_or_default(),
            "ACTIVE",
            "case {case:?}: a refused update leaves the table settled, got {described}"
        );
        assert_eq!(
            reported_indexes(described),
            active(&["idx-keep", "idx-drop", "idx.third"]),
            "case {case:?}: a refused update leaves every vector index ACTIVE, got {described}"
        );
    }
}

#[tokio::test]
async fn a_delete_removes_exactly_the_vector_index_it_names() {
    // Rows differ along what the rule generalises over: which position of the table's
    // index list the removed index sits in (first, middle, last), the distance function it
    // was created with, and how many vector indexes the table carries — the last row
    // removes the only one it has.
    let cases: [(&str, Value, &str, &[&str]); 4] = [
        (
            "the first of three",
            three_indexes(),
            "idx-keep",
            &["idx-drop", "idx.third"],
        ),
        (
            "the middle of three",
            three_indexes(),
            "idx-drop",
            &["idx-keep", "idx.third"],
        ),
        (
            "the last of three",
            three_indexes(),
            "idx.third",
            &["idx-keep", "idx-drop"],
        ),
        (
            "the only index the table has",
            json!([vector_index("idx-keep", "COSINE", Value::Null)]),
            "idx-keep",
            &[],
        ),
    ];

    for (position, (case, indexes, removed, survivors)) in cases.into_iter().enumerate() {
        let router = emulator();
        let table = format!("VectorDelete{position}");
        table_with(
            &router,
            &table,
            json!([{"AttributeName": "pk", "AttributeType": "S"}]),
            indexes,
            Value::Null,
        )
        .await;
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": table,
                "Item": {"pk": {"S": "item-1"}, "emb": {"L": embedding(&[1.0, 0.0, 0.0, 0.0])}},
            }),
        )
        .await;

        let accepted = ok(
            &router,
            "UpdateTable",
            update_request(&table, json!([delete_action(removed)])),
        )
        .await;
        let described = &accepted["TableDescription"];
        assert_eq!(
            described["TableStatus"].as_str().unwrap_or_default(),
            "UPDATING",
            "case {case:?}: removing an index is work that outlives the call, got {described}"
        );
        let mut expected_survivors: Vec<String> =
            survivors.iter().map(|name| (*name).to_string()).collect();
        expected_survivors.sort();
        assert_eq!(
            reported_index_names(described),
            expected_survivors,
            "case {case:?}: the accepted update reports the table without {removed}, got \
             {described}"
        );

        // The table settles back to ACTIVE with exactly that index gone and its siblings
        // ACTIVE.
        let settled = describe_when_active(&router, case, &table).await;
        assert_eq!(
            reported_indexes(&settled),
            active(survivors),
            "case {case:?}: the settled table carries exactly the surviving indexes, got \
             {settled}"
        );

        // Every surviving index still answers a search with the stored item...
        for survivor in survivors {
            let (status, response) = search(&router, &table, survivor).await;
            assert_eq!(
                (status, result_keys(&response)),
                (200, vec!["item-1".to_string()]),
                "case {case:?}: index {survivor} still serves SearchVectors, got {status} with \
                 {response}"
            );
        }
        // ... and the removed one is an index the table no longer has, which a search
        // refuses as a request mistake rather than a missing resource.
        let (status, response) = search(&router, &table, removed).await;
        assert_eq!(
            (status, error_code(&response), message(&response)),
            (
                400,
                "ValidationException".to_string(),
                format!("The table does not have the specified index: {removed}")
            ),
            "case {case:?}: a search through the removed index is refused, got {status} with \
             {response}"
        );
    }
}

#[tokio::test]
async fn a_removed_vector_index_takes_the_definitions_only_it_consumed_with_it() {
    // A definition is kept while SOME schema of the table still consumes it, so removing
    // an index drops the attributes only that index named and keeps the rest. Rows differ
    // in which index is removed: the one whose search schema named `tenant`, and the one
    // that named nothing — the first drops the definition, the second keeps it.
    let cases: [(&str, &str, &[&str]); 2] = [
        ("the index whose search schema named it", "idx-tenant", &["pk"]),
        ("an index that named it nowhere", "idx-plain", &["pk", "tenant"]),
    ];

    for (position, (case, removed, kept)) in cases.into_iter().enumerate() {
        let router = emulator();
        let table = format!("VectorDeleteDefinitions{position}");
        table_with(
            &router,
            &table,
            json!([
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "tenant", "AttributeType": "S"},
            ]),
            json!([
                vector_index(
                    "idx-tenant",
                    "COSINE",
                    json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}]),
                ),
                vector_index("idx-plain", "EUCLIDEAN", Value::Null),
            ]),
            Value::Null,
        )
        .await;

        ok(
            &router,
            "UpdateTable",
            update_request(&table, json!([delete_action(removed)])),
        )
        .await;
        let settled = describe_when_active(&router, case, &table).await;
        let mut reported: Vec<String> = settled["AttributeDefinitions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|definition| {
                definition["AttributeName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        reported.sort();
        assert_eq!(
            reported,
            kept.iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: the table keeps exactly the definitions a schema still consumes, got \
             {settled}"
        );
    }
}

#[tokio::test]
async fn a_vector_index_update_that_names_no_single_change_is_refused() {
    let router = emulator();
    let table = "VectorUpdateShape";
    table_with(
        &router,
        table,
        json!([{"AttributeName": "pk", "AttributeType": "S"}]),
        three_indexes(),
        Value::Null,
    )
    .await;

    // A `VectorIndexUpdate` carries the two actions the model declares on it (Create and
    // Delete), so an entry asking for both or for neither names no single change, and a
    // list naming one index twice asks for two changes to one resource. Rows cover each.
    let create = |name: &str| {
        json!({
            "IndexName": name,
            "VectorAttribute": {"AttributeName": "emb"},
            "Dimensions": 4,
            "DistanceFunction": "COSINE",
            "Projection": {"ProjectionType": "ALL"},
        })
    };
    let cases: [(&str, Value, String); 4] = [
        (
            "an entry that asks for nothing",
            json!([{}]),
            "Invalid VectorIndexUpdates: each entry must request exactly one of Create or \
             Delete, got 0"
                .to_string(),
        ),
        (
            "an entry that asks for both actions",
            json!([{"Create": create("idx-new"), "Delete": {"IndexName": "idx-drop"}}]),
            "Invalid VectorIndexUpdates: each entry must request exactly one of Create or \
             Delete, got 2"
                .to_string(),
        ),
        (
            "one index deleted twice",
            json!([delete_action("idx-drop"), delete_action("idx-drop")]),
            "Invalid VectorIndexUpdates: index idx-drop is named more than once".to_string(),
        ),
        (
            "an index deleted and created in one list",
            json!([delete_action("idx-keep"), {"Create": create("idx-keep")}]),
            "Invalid VectorIndexUpdates: index idx-keep is named more than once".to_string(),
        ),
    ];

    for (case, updates, expected) in cases {
        let (status, body) = call(&router, "UpdateTable", update_request(table, updates)).await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (400, "ValidationException".to_string(), expected),
            "case {case:?}: the request names no single change and is refused, got {status} with \
             {body}"
        );
    }

    // None of those refusals touched the table.
    let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
    assert_eq!(
        reported_indexes(&described["Table"]),
        active(&["idx-keep", "idx-drop", "idx.third"]),
        "every refusal left the table's vector indexes alone, got {described}"
    );
}
