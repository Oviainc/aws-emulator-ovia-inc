// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `SearchVectors`: the vector similarity search a table's vector
//! index serves.
//!
//! The rules pinned here:
//!
//! 1. a search scores every item the index HOLDS — one that carries the index's vector
//!    attribute as a list of the index's dimensions, and every attribute the search schema
//!    partitions by — against the search vector with the index's OWN distance function,
//!    and reports the `TopK` of them most similar first: the `k` smallest scores for
//!    `COSINE` and `EUCLIDEAN`, the `k` highest for `DOT_PRODUCT`;
//! 2. a result carries the entry the index holds for its item — the attributes its
//!    `Projection` stores, never the vector it finds them by — narrowed further by a
//!    `ProjectionExpression`, which reports nothing for a name the index does not hold;
//! 3. a `SearchConditionExpression` filters the scored items by the index's search schema
//!    attributes, each PINNED by `=`, and may reference no others;
//! 4. a request that addresses no searchable index is refused: a table that is not serving
//!    items with `ResourceNotFoundException`, an `IndexName` the table declares no vector
//!    index for and a `SearchVector` that is not the index's shape with
//!    `ValidationException`;
//! 5. `ConsumedCapacity` reports the index data the search read, in bytes, and only when
//!    the request asked for it.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and each rule is asserted over a
//! SET of inputs — the distance function, the `TopK`, the projection, the condition — of
//! which the one-index case is a single row.

use serde_json::{json, Map, Value};

use crate::emulator::{call, emulator, error_code, message, ok};

/// Wait out one transient span, so the table's `CREATING` -> `ACTIVE` transition and its
/// indexes' are due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// One vector index body over the `emb` attribute.
fn vector_index(
    name: &str,
    dimensions: i64,
    distance_function: &str,
    projection: Value,
    search_schema: Value,
) -> Value {
    let mut index = json!({
        "IndexName": name,
        "VectorAttribute": {"AttributeName": "emb"},
        "Dimensions": dimensions,
        "DistanceFunction": distance_function,
        "Projection": projection,
    });
    if !search_schema.is_null() {
        index["SearchSchema"] = search_schema;
    }
    index
}

/// Create one on-demand table with a `pk` partition key, the attribute definitions and
/// the vector indexes given, and wait for it to become `ACTIVE`.
async fn table_with(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    definitions: Value,
    vector_indexes: Value,
) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": definitions,
            "VectorIndexes": vector_indexes,
        }),
    )
    .await;
    settle().await;
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

/// `PutItem` one item, which must succeed.
async fn put(
    router: &smithy_server_runtime::MultiServiceRouter,
    table: &str,
    item: Value,
) {
    ok(router, "PutItem", json!({"TableName": table, "Item": item})).await;
}

/// One `SearchVectors` request body.
fn search_body(table: &str, index: &str, vector: &[f64], top_k: i64) -> Value {
    json!({
        "TableName": table,
        "IndexName": index,
        "SearchVector": embedding(vector),
        "TopK": top_k,
    })
}

/// The `pk` of every item a search answered, in the order it answered them.
fn result_keys(response: &Value) -> Vec<String> {
    response["SearchResults"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|result| {
            result["Item"]["pk"]["S"]
                .as_str()
                .unwrap_or("<no pk>")
                .to_string()
        })
        .collect()
}

/// The score of every item a search answered, in the order it answered them.
fn result_scores(response: &Value) -> Vec<f64> {
    response["SearchResults"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|result| result["Score"].as_f64().unwrap_or(f64::NAN))
        .collect()
}

/// The attribute names one result carries, sorted.
fn result_attributes(response: &Value, at: usize) -> Vec<String> {
    let empty = Map::new();
    let mut names: Vec<String> = response["SearchResults"]
        .get(at)
        .and_then(|result| result["Item"].as_object())
        .unwrap_or(&empty)
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn the_index_distance_function_decides_which_items_are_most_similar() {
    let router = emulator();
    let projection = json!({"ProjectionType": "ALL"});
    table_with(
        &router,
        "vector-search-ranking",
        json!([{"AttributeName": "pk", "AttributeType": "S"}]),
        json!([
            vector_index("cosine-index", 3, "COSINE", projection.clone(), Value::Null),
            vector_index(
                "euclidean-index",
                3,
                "EUCLIDEAN",
                projection.clone(),
                Value::Null
            ),
            vector_index(
                "dot-product-index",
                3,
                "DOT_PRODUCT",
                projection,
                Value::Null
            ),
        ]),
    )
    .await;

    // The items the index holds: `a` is the search vector itself, `f` points the same way
    // but twice as far, `c` points nearly that way, `b` is orthogonal to it. The last one
    // is NOT in any of the indexes — it carries no vector at all — so no search may ever
    // report it. A vector of the WRONG number of dimensions is not an item this table
    // accepts at all (`a_write_whose_vector_is_not_the_indexed_shape_is_refused`, and
    // `vector_write_validation` over every write operation).
    for (key, embedding_values) in [
        ("a", Some(vec![1.0, 0.0, 0.0])),
        ("b", Some(vec![0.0, 1.0, 0.0])),
        ("c", Some(vec![0.9, 0.1, 0.0])),
        ("f", Some(vec![2.0, 0.0, 0.0])),
        ("no-vector", None),
    ] {
        let mut item = json!({"pk": {"S": key}});
        if let Some(values) = embedding_values {
            item["emb"] = json!({"L": embedding(&values)});
        }
        put(&router, "vector-search-ranking", item).await;
    }

    // Rows differ along what the rule generalises over: the distance function the index
    // was created with (the three the model's `VectorDistanceFunction` enum declares), and
    // the `TopK` asked for — fewer than the index holds, exactly as many, one, and more.
    // The three functions rank the SAME four items in three different orders, so a search
    // that ignored the index's function could satisfy at most one row.
    let cases: [(&str, i64, &[&str], f64); 8] = [
        ("cosine-index", 4, &["a", "f", "c", "b"], 0.0),
        ("cosine-index", 2, &["a", "f"], 0.0),
        ("cosine-index", 9, &["a", "f", "c", "b"], 0.0),
        ("euclidean-index", 4, &["a", "c", "f", "b"], 0.0),
        ("euclidean-index", 1, &["a"], 0.0),
        ("dot-product-index", 4, &["f", "a", "c", "b"], 2.0),
        ("dot-product-index", 2, &["f", "a"], 2.0),
        ("dot-product-index", 9, &["f", "a", "c", "b"], 2.0),
    ];
    for (index, top_k, expected_keys, expected_first_score) in cases {
        let case = format!("{index} with TopK {top_k}");
        let response = ok(
            &router,
            "SearchVectors",
            search_body("vector-search-ranking", index, &[1.0, 0.0, 0.0], top_k),
        )
        .await;
        let keys = result_keys(&response);
        assert_eq!(
            keys,
            expected_keys.to_vec(),
            "case {case:?}: expected {expected_keys:?} most similar first, got {keys:?} from \
             {response}"
        );
        let scores = result_scores(&response);
        let first = scores.first().copied().unwrap_or(f64::NAN);
        assert!(
            (first - expected_first_score).abs() < 1e-6,
            "case {case:?}: expected the most similar item to score {expected_first_score}, got \
             {first} ({scores:?})"
        );
        // The reported order IS the score order, in the direction the function reads:
        // ascending for a distance, descending for the inner product.
        let descending = index == "dot-product-index";
        for pair in scores.windows(2) {
            let ordered = if descending {
                pair[0] >= pair[1]
            } else {
                pair[0] <= pair[1]
            };
            assert!(
                ordered,
                "case {case:?}: scores must run most similar first, got {scores:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_result_carries_the_attributes_the_index_projects() {
    let router = emulator();
    table_with(
        &router,
        "vector-search-projection",
        json!([
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "tenant", "AttributeType": "S"},
        ]),
        json!([
            vector_index(
                "all-index",
                3,
                "COSINE",
                json!({"ProjectionType": "ALL"}),
                json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}])
            ),
            vector_index(
                "keys-index",
                3,
                "COSINE",
                json!({"ProjectionType": "KEYS_ONLY"}),
                json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}])
            ),
            vector_index(
                "include-index",
                3,
                "COSINE",
                json!({"ProjectionType": "INCLUDE", "NonKeyAttributes": ["brand"]}),
                json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}])
            ),
        ]),
    )
    .await;
    put(
        &router,
        "vector-search-projection",
        json!({
            "pk": {"S": "i1"},
            "tenant": {"S": "t1"},
            "brand": {"S": "acme"},
            "note": {"S": "hello"},
            "emb": {"L": embedding(&[1.0, 0.0, 0.0])},
        }),
    )
    .await;

    // Rows differ along the index's `Projection` (the three `ProjectionType` values) and
    // along the `ProjectionExpression` the request narrows it with — absent, a written-out
    // name, several names, and a name reached through an `ExpressionAttributeNames` alias.
    // The vector attribute `emb` is in NO row: a search reports the attributes the index
    // projects, and the vector it finds them by is not one of them.
    let cases: [(&str, Value, Value, &[&str]); 8] = [
        (
            "all-index",
            Value::Null,
            Value::Null,
            &["brand", "note", "pk", "tenant"],
        ),
        ("keys-index", Value::Null, Value::Null, &["pk", "tenant"]),
        (
            "include-index",
            Value::Null,
            Value::Null,
            &["brand", "pk", "tenant"],
        ),
        ("all-index", json!("note"), Value::Null, &["note"]),
        ("all-index", json!("pk, note"), Value::Null, &["note", "pk"]),
        (
            "include-index",
            json!("#b"),
            json!({"#b": "brand"}),
            &["brand"],
        ),
        // A projection that names an attribute the item does not hold reports nothing for
        // it, the way a projection over a table's items does — and so does one naming an
        // attribute the index does not project, or the vector attribute itself.
        ("all-index", json!("absent"), Value::Null, &[]),
        ("keys-index", json!("note, emb"), Value::Null, &[]),
    ];
    for (index, projection_expression, names, expected) in cases {
        let case = format!("{index} projecting {projection_expression}");
        let mut body = search_body("vector-search-projection", index, &[1.0, 0.0, 0.0], 5);
        // Each of these indexes partitions by `tenant`, so every search pins it.
        body["SearchConditionExpression"] = json!("tenant = :t");
        body["ExpressionAttributeValues"] = json!({":t": {"S": "t1"}});
        if !projection_expression.is_null() {
            body["ProjectionExpression"] = projection_expression;
        }
        if !names.is_null() {
            body["ExpressionAttributeNames"] = names;
        }
        let response = ok(&router, "SearchVectors", body).await;
        let attributes = result_attributes(&response, 0);
        assert_eq!(
            attributes,
            expected.to_vec(),
            "case {case:?}: expected the result to carry {expected:?}, got {attributes:?} from \
             {response}"
        );
    }
}

#[tokio::test]
async fn a_search_condition_filters_by_the_index_search_schema() {
    let router = emulator();
    table_with(
        &router,
        "vector-search-conditions",
        json!([
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "tenant", "AttributeType": "S"},
            {"AttributeName": "price", "AttributeType": "N"},
        ]),
        json!([vector_index(
            "filtered-index",
            2,
            "COSINE",
            json!({"ProjectionType": "ALL"}),
            json!([
                {"AttributeName": "tenant", "SearchSchemaElementType": "HASH"},
                {"AttributeName": "price", "SearchSchemaElementType": "INLINE_FILTER"},
            ])
        )]),
    )
    .await;
    for (key, tenant, price, embedding_values) in [
        ("t1-cheap", Some("t1"), 5, vec![1.0, 0.0]),
        ("t1-dear", Some("t1"), 50, vec![0.9, 0.1]),
        ("t2-cheap", Some("t2"), 5, vec![1.0, 0.0]),
        // No `tenant`: the search schema partitions by it, so this item is in no index
        // entry and no condition can bring it back.
        ("no-tenant", None, 5, vec![1.0, 0.0]),
    ] {
        let mut item = json!({
            "pk": {"S": key},
            "price": {"N": price.to_string()},
            "emb": {"L": embedding(&embedding_values)},
        });
        if let Some(tenant) = tenant {
            item["tenant"] = json!({"S": tenant});
        }
        put(&router, "vector-search-conditions", item).await;
    }

    // Rows differ along the schema element the condition pins and the way it spells the
    // equality (both ways round), and along how many of the schema's elements it reads:
    // the `HASH` element alone, and the `HASH` element with the `INLINE_FILTER` one beside
    // it. Every row pins the `HASH` element, because the index partitions by it, and every
    // row reads its elements by equality — the only way a search condition may read them.
    let cases: [(Value, Value, &[&str]); 6] = [
        (
            json!("tenant = :t"),
            json!({":t": {"S": "t1"}}),
            &["t1-cheap", "t1-dear"],
        ),
        (
            json!(":t = tenant"),
            json!({":t": {"S": "t1"}}),
            &["t1-cheap", "t1-dear"],
        ),
        (
            json!("tenant = :t"),
            json!({":t": {"S": "t2"}}),
            &["t2-cheap"],
        ),
        (
            json!("tenant = :t AND price = :p"),
            json!({":t": {"S": "t1"}, ":p": {"N": "5"}}),
            &["t1-cheap"],
        ),
        (
            json!("tenant = :t AND :p = price"),
            json!({":t": {"S": "t1"}, ":p": {"N": "50"}}),
            &["t1-dear"],
        ),
        // A condition nothing satisfies answers an empty result set, not an error.
        (
            json!("tenant = :t AND price = :p"),
            json!({":t": {"S": "t1"}, ":p": {"N": "100"}}),
            &[],
        ),
    ];
    for (condition, values, expected) in cases {
        let case = format!("condition {condition}");
        let mut body = search_body("vector-search-conditions", "filtered-index", &[1.0, 0.0], 9);
        if !condition.is_null() {
            body["SearchConditionExpression"] = condition;
        }
        if !values.is_null() {
            body["ExpressionAttributeValues"] = values;
        }
        let response = ok(&router, "SearchVectors", body).await;
        let keys = result_keys(&response);
        assert_eq!(
            keys,
            expected.to_vec(),
            "case {case:?}: expected {expected:?}, got {keys:?} from {response}"
        );
    }

    // The other half of the rule: a condition that reads the schema the wrong way is
    // refused rather than filtered. Rows differ along HOW it is wrong — a `HASH` element
    // left unpinned (no condition at all, and a condition that only filters), a schema
    // element read by a comparison, a range, a function, a negation or a path into it, in
    // each ROLE the schema can give it (which decides the sentence), and an attribute the
    // schema does not name at all.
    let refusals: [(Value, Value, &str); 10] = [
        (
            Value::Null,
            Value::Null,
            "SearchConditionExpression must be provided when SearchSchema has a HASH key",
        ),
        (
            json!("price = :p"),
            json!({":p": {"N": "10"}}),
            "must be provided in the SearchConditionExpression",
        ),
        (
            json!("tenant < :t"),
            json!({":t": {"S": "t2"}}),
            "Only the equality operator (=) is supported for HASH attributes",
        ),
        (
            json!("tenant BETWEEN :low AND :high"),
            json!({":low": {"S": "t0"}, ":high": {"S": "t9"}}),
            "Only the equality operator (=) is supported for HASH attributes",
        ),
        (
            json!("attribute_exists(tenant)"),
            Value::Null,
            "Only the equality operator (=) is supported for HASH attributes",
        ),
        (
            json!("begins_with(tenant, :t)"),
            json!({":t": {"S": "t"}}),
            "Only the equality operator (=) is supported for HASH attributes",
        ),
        (
            json!("NOT tenant <> :t"),
            json!({":t": {"S": "t1"}}),
            "Only the equality operator (=) is supported for HASH attributes",
        ),
        // The INLINE_FILTER element is held to the same one operator, and refused with the
        // sentence the service states for that role.
        (
            json!("tenant = :t AND price > :p"),
            json!({":t": {"S": "t1"}, ":p": {"N": "15"}}),
            "Invalid comparator used in SearchConditionExpression",
        ),
        (
            json!("tenant = :t AND price BETWEEN :low AND :high"),
            json!({":t": {"S": "t1"}, ":low": {"N": "0"}, ":high": {"N": "10"}}),
            "Invalid comparator used in SearchConditionExpression",
        ),
        (
            json!("tenant = :t AND note = :n"),
            json!({":t": {"S": "t1"}, ":n": {"S": "x"}}),
            "Only top-level attributes from the vector index search schema",
        ),
    ];
    for (condition, values, expected_message) in refusals {
        let case = format!("condition {condition}");
        let mut body = search_body("vector-search-conditions", "filtered-index", &[1.0, 0.0], 9);
        if !condition.is_null() {
            body["SearchConditionExpression"] = condition;
        }
        if !values.is_null() {
            body["ExpressionAttributeValues"] = values;
        }
        let (status, response) = call(&router, "SearchVectors", body).await;
        let code = error_code(&response);
        let reported = message(&response);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} {code}: {reported}"
        );
        assert!(
            reported.contains(expected_message),
            "case {case:?}: expected a message carrying {expected_message:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_request_that_addresses_no_searchable_index_is_refused() {
    let router = emulator();
    table_with(
        &router,
        "vector-search-refusals",
        json!([
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "tenant", "AttributeType": "S"},
        ]),
        json!([vector_index(
            "refusal-index",
            3,
            "COSINE",
            json!({"ProjectionType": "KEYS_ONLY"}),
            json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}])
        )]),
    )
    .await;
    // A table with no vector index at all: an `IndexName` is wrong there for the same
    // reason it is wrong on a table whose vector indexes are named something else.
    table_with(
        &router,
        "vector-search-plain",
        json!([{"AttributeName": "pk", "AttributeType": "S"}]),
        json!([]),
    )
    .await;

    // Rows differ along WHICH part of the request addresses nothing searchable: the table,
    // the index, the vector's element type, and the vector's length — one element short,
    // one long, and far off.
    let cases: [(&str, Value, &str, &str); 7] = [
        (
            "a table that does not exist",
            search_body("vector-search-absent", "refusal-index", &[1.0, 0.0, 0.0], 3),
            "ResourceNotFoundException",
            "Requested resource not found",
        ),
        (
            "an index the table does not have",
            search_body("vector-search-refusals", "other-index", &[1.0, 0.0, 0.0], 3),
            "ValidationException",
            "The table does not have the specified index: other-index",
        ),
        (
            "a table that has no vector index",
            search_body("vector-search-plain", "refusal-index", &[1.0, 0.0, 0.0], 3),
            "ValidationException",
            "The table does not have the specified index: refusal-index",
        ),
        (
            "a vector one dimension short",
            search_body("vector-search-refusals", "refusal-index", &[1.0, 0.0], 3),
            "ValidationException",
            "Input search vector dimension 2 does not match vector index dimension 3",
        ),
        (
            "a vector one dimension long",
            search_body(
                "vector-search-refusals",
                "refusal-index",
                &[1.0, 0.0, 0.0, 0.0],
                3,
            ),
            "ValidationException",
            "Input search vector dimension 4 does not match vector index dimension 3",
        ),
        (
            "a vector of one dimension",
            search_body("vector-search-refusals", "refusal-index", &[1.0], 3),
            "ValidationException",
            "Input search vector dimension 1 does not match vector index dimension 3",
        ),
        (
            "a vector element that is not a number",
            json!({
                "TableName": "vector-search-refusals",
                "IndexName": "refusal-index",
                "SearchVector": [{"S": "1.0"}, {"N": "0"}, {"N": "0"}],
                "TopK": 3,
            }),
            "ValidationException",
            "32-bit IEEE-754",
        ),
    ];
    for (case, body, expected_code, expected_message) in cases {
        let (status, response) = call(&router, "SearchVectors", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code}: {}",
            message(&response)
        );
        let reported = message(&response);
        assert!(
            reported.contains(expected_message),
            "case {case:?}: expected a message carrying {expected_message:?}, got {reported:?}"
        );
    }

    // A condition may read only the index's search schema, whichever operator reads it.
    for condition in [
        "note = :v",
        "attribute_exists(note)",
        "tenant = :v AND note < :v",
        "size(note) = :v",
    ] {
        let (status, response) = call(
            &router,
            "SearchVectors",
            json!({
                "TableName": "vector-search-refusals",
                "IndexName": "refusal-index",
                "SearchVector": embedding(&[1.0, 0.0, 0.0]),
                "TopK": 3,
                "SearchConditionExpression": condition,
                "ExpressionAttributeValues": {":v": {"S": "x"}},
            }),
        )
        .await;
        let code = error_code(&response);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {condition:?}: expected a 400 ValidationException, got {status} {code}: {}",
            message(&response)
        );
    }
}

#[tokio::test]
async fn a_search_answers_what_fits_in_one_page() {
    let router = emulator();
    table_with(
        &router,
        "vector-search-page-budget",
        json!([{"AttributeName": "pk", "AttributeType": "S"}]),
        json!([vector_index(
            "big-index",
            2,
            "EUCLIDEAN",
            json!({"ProjectionType": "ALL"}),
            Value::Null
        )]),
    )
    .await;
    // Four items of ~400 KB each: more index data than one page carries, so a `TopK` that
    // asks for all four cannot be answered in full.
    let padding = "p".repeat(390 * 1024);
    for key in ["i1", "i2", "i3", "i4"] {
        put(
            &router,
            "vector-search-page-budget",
            json!({
                "pk": {"S": key},
                "pad": {"S": padding},
                "emb": {"L": embedding(&[1.0, 0.0])},
            }),
        )
        .await;
    }

    // Rows differ along the `TopK` asked for: inside the page budget, and past it.
    for (top_k, expected) in [(1_i64, 1_usize), (2, 2), (4, 3), (9, 3)] {
        let response = ok(
            &router,
            "SearchVectors",
            search_body("vector-search-page-budget", "big-index", &[1.0, 0.0], top_k),
        )
        .await;
        let keys = result_keys(&response);
        assert_eq!(
            keys.len(),
            expected,
            "case TopK {top_k}: expected the {expected} result(s) that fit one page, got \
             {keys:?}"
        );
    }
}

#[tokio::test]
async fn consumed_capacity_reports_the_index_data_the_search_read() {
    let router = emulator();
    table_with(
        &router,
        "vector-search-capacity",
        json!([
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "tenant", "AttributeType": "S"},
        ]),
        json!([
            vector_index(
                "all-index",
                2,
                "EUCLIDEAN",
                json!({"ProjectionType": "ALL"}),
                json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}])
            ),
            vector_index(
                "keys-index",
                2,
                "EUCLIDEAN",
                json!({"ProjectionType": "KEYS_ONLY"}),
                json!([{"AttributeName": "tenant", "SearchSchemaElementType": "HASH"}])
            ),
        ]),
    )
    .await;
    for key in ["i1", "i2", "i3"] {
        put(
            &router,
            "vector-search-capacity",
            json!({
                "pk": {"S": key},
                "tenant": {"S": "t1"},
                "note": {"S": "a note long enough to be worth projecting or not"},
                "emb": {"L": embedding(&[1.0, 0.0])},
            }),
        )
        .await;
    }

    // Rows differ along what the request asked for: nothing, each of the three
    // `ReturnConsumedCapacity` values.
    let cases: [(Value, bool); 4] = [
        (Value::Null, false),
        (json!("NONE"), false),
        (json!("TOTAL"), true),
        (json!("INDEXES"), true),
    ];
    let mut reported: Vec<f64> = Vec::new();
    for (requested, expected) in cases {
        let case = format!("ReturnConsumedCapacity {requested}");
        let mut body = search_body("vector-search-capacity", "all-index", &[1.0, 0.0], 3);
        // The index partitions by `tenant`, so the search pins it.
        body["SearchConditionExpression"] = json!("tenant = :t");
        body["ExpressionAttributeValues"] = json!({":t": {"S": "t1"}});
        if !requested.is_null() {
            body["ReturnConsumedCapacity"] = requested;
        }
        let response = ok(&router, "SearchVectors", body).await;
        let bytes = response["ConsumedCapacity"]["VectorSearchRequestBytes"].as_f64();
        match (expected, bytes) {
            (true, Some(bytes)) => {
                assert!(
                    bytes > 0.0,
                    "case {case:?}: expected the bytes the search read, got {bytes} from \
                     {response}"
                );
                // Nothing was written, so no write figure is reported.
                assert!(
                    response["ConsumedCapacity"]["VectorWriteRequestBytes"].is_null(),
                    "case {case:?}: a search writes nothing, got {response}"
                );
                reported.push(bytes);
            }
            (true, None) => panic!("case {case:?}: expected a ConsumedCapacity, got {response}"),
            (false, None) => {}
            (false, Some(bytes)) => panic!(
                "case {case:?}: expected no ConsumedCapacity, got {bytes} from {response}"
            ),
        }
    }
    let total = reported.first().copied().unwrap_or_default();
    assert!(
        reported.iter().all(|bytes| *bytes == total),
        "every search over the same index must read the same bytes, got {reported:?}"
    );

    // The figure is the data the INDEX holds, so the index that projects less reads less
    // over the very same items.
    let keys_only = ok(
        &router,
        "SearchVectors",
        json!({
            "TableName": "vector-search-capacity",
            "IndexName": "keys-index",
            "SearchVector": embedding(&[1.0, 0.0]),
            "TopK": 3,
            "ReturnConsumedCapacity": "TOTAL",
            "SearchConditionExpression": "tenant = :t",
            "ExpressionAttributeValues": {":t": {"S": "t1"}},
        }),
    )
    .await;
    let narrow = keys_only["ConsumedCapacity"]["VectorSearchRequestBytes"]
        .as_f64()
        .unwrap_or(f64::NAN);
    assert!(
        narrow > 0.0 && narrow < total,
        "a KEYS_ONLY index holds less of each item than an ALL one, so it must read fewer \
         bytes: got {narrow} against {total}"
    );
}
