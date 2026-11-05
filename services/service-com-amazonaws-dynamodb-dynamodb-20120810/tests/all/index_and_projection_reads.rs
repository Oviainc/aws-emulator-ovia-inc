// SPDX-License-Identifier: CC0-1.0

//! A read is VALIDATED before it is served, and then serves what it was pointed at.
//!
//! The rules these suites pin:
//!
//! * every read operation settles the facts of the REQUEST — the index it names, the key
//!   schema its condition covers, the types its operands carry, whether its `Select` and
//!   its projection hold together, whether its projection is one the expression language
//!   accepts — before it decides what it can serve, so a request the service refuses is
//!   answered its modelled `ValidationException` at HTTP 400 and never a 5xx;
//! * a `Query` naming an `IndexName` reads that secondary index: it binds its key
//!   conditions to the INDEX's key schema and answers the entries of the one index
//!   partition they select, in the order that index holds them, each carrying the
//!   attributes the index projects;
//! * what a read REPORTS of each item is the projection the request carried — a
//!   `ProjectionExpression` naming whole attributes or document paths into them, or the
//!   legacy `AttributesToGet` list — on every path that takes one: `GetItem`, `Query`,
//!   `Scan`, `BatchGetItem` and `TransactGetItems`.
//!
//! Each suite checks its rule over a SET of inputs: a table of refusals spanning every
//! member a read can get wrong across the five operations that take one, a table of
//! projection forms (a whole attribute, an alias, a map path, a list index, several at
//! once) crossed with every read path that takes one, and both index kinds with all three
//! projection types.
//!
//! The axis here is the READ PATH: that every operation taking an index or a projection
//! validates and serves it alike. `query_indexes_and_projections.rs` and
//! `scan_indexes_and_projections.rs` take the other axis — what one `Query` or one `Scan`
//! of an index evaluates, orders, pages and bills — so the index rows below are the ones
//! this rule needs rather than a second reading of those.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};
use crate::table_scan::{last_evaluated_key, rendered, reported_items};

/// The table every suite here reads. One table name per emulator, and each suite builds
/// its own emulator, so the name is a constant.
const TABLE: &str = "QueryIndexed";

/// The items every suite seeds. `gsk` is numeric, so a condition on it that supplies a
/// string is a type mismatch against the schema; one item carries no `gpk` at all (so no
/// global index covers it) and one no `lsk` (so the local index does not), and one
/// carries a nested document the projection suites reach into.
fn stored_items() -> Vec<Value> {
    vec![
        json!({"pk": {"S": "p1"}, "sk": {"S": "a"}, "gpk": {"S": "g1"}, "gsk": {"N": "2"},
               "lsk": {"S": "x"}, "note": {"S": "first"},
               "doc": {"M": {"leaf": {"S": "deep"}, "branch": {"N": "7"}}},
               "odd.name": {"S": "literal"},
               "parts": {"L": [{"S": "zero"}, {"S": "one"}, {"S": "two"}]}}),
        json!({"pk": {"S": "p1"}, "sk": {"S": "b"}, "gpk": {"S": "g1"}, "gsk": {"N": "1"},
               "lsk": {"S": "y"}, "note": {"S": "second"}}),
        json!({"pk": {"S": "p1"}, "sk": {"S": "c"}, "gpk": {"S": "g2"}, "gsk": {"N": "3"},
               "note": {"S": "third"}}),
        json!({"pk": {"S": "p2"}, "sk": {"S": "a"}, "gpk": {"S": "g1"}, "gsk": {"N": "9"},
               "lsk": {"S": "z"}, "note": {"S": "fourth"}}),
        json!({"pk": {"S": "p2"}, "sk": {"S": "b"}, "note": {"S": "fifth"}}),
    ]
}

/// Create [`TABLE`] with a global index of each projection type and one local index,
/// wait out the `CREATING` -> `ACTIVE` transition, write every item of
/// [`stored_items`], and answer the items written.
async fn seeded(router: &MultiServiceRouter) -> Vec<Value> {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "gpk", "AttributeType": "S"},
                {"AttributeName": "gsk", "AttributeType": "N"},
                {"AttributeName": "lsk", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": "ByGpkAll",
                    "KeySchema": [
                        {"AttributeName": "gpk", "KeyType": "HASH"},
                        {"AttributeName": "gsk", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": "ByGpkKeysOnly",
                    "KeySchema": [{"AttributeName": "gpk", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                },
                {
                    "IndexName": "ByGpkInclude",
                    "KeySchema": [{"AttributeName": "gpk", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "INCLUDE",
                                   "NonKeyAttributes": ["note"]},
                },
            ],
            "LocalSecondaryIndexes": [
                {
                    "IndexName": "ByLsk",
                    "KeySchema": [
                        {"AttributeName": "pk", "KeyType": "HASH"},
                        {"AttributeName": "lsk", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
            ],
        }),
    )
    .await;
    describe_when_active(router, "seed the indexed table", TABLE).await;
    let items = stored_items();
    for item in &items {
        ok(router, "PutItem", json!({"TableName": TABLE, "Item": item})).await;
    }
    items
}

/// One item narrowed to the named top-level attributes: "If a requested attribute is not
/// found, it does not appear in the result" (`ProjectionExpression`), so a name the item
/// does not carry contributes nothing.
fn narrowed(item: &Value, names: &[&str]) -> Value {
    let mut narrowed = Map::new();
    for name in names {
        if let Some(value) = item.get(*name) {
            narrowed.insert((*name).to_string(), value.clone());
        }
    }
    Value::Object(narrowed)
}

/// The `note` of every item a response reported, in the order reported — a `Query`
/// answers its page in the order of the key schema it walked, so these comparisons are
/// between SEQUENCES.
fn reported_notes(response: &Value) -> Vec<String> {
    response["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["note"]["S"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ===========================================================================
// 1. Every request the service refuses is refused as a client error
// ===========================================================================

/// One refusal row: what the request gets wrong, the operation it is sent to, the body,
/// and the exact message the service must answer with.
struct RefusalRow {
    case: &'static str,
    operation: &'static str,
    body: Value,
    message: String,
}

/// Every way a read can get its index, key condition, `Select` or projection wrong,
/// across the five read operations that take them. None of these is an unimplemented
/// feature: each is a request AWS refuses, so each must be the modelled 400.
fn refusal_rows() -> Vec<RefusalRow> {
    let select_conflict = |select: &str| {
        format!(
            "One or more parameter values were invalid: Select type {select} is not supported \
             when the attributes to return are listed; the only Select type that is, is \
             SPECIFIC_ATTRIBUTES"
        )
    };
    vec![
        RefusalRow {
            case: "a query naming an index the table does not declare",
            operation: "Query",
            body: json!({"TableName": TABLE, "IndexName": "NoSuchIndex",
                         "KeyConditionExpression": "gpk = :g",
                         "ExpressionAttributeValues": {":g": {"S": "g1"}}}),
            message: "The table does not have the specified index: NoSuchIndex".to_string(),
        },
        RefusalRow {
            case: "a scan naming an index the table does not declare",
            operation: "Scan",
            body: json!({"TableName": TABLE, "IndexName": "NoSuchIndex"}),
            message: "The table does not have the specified index: NoSuchIndex".to_string(),
        },
        RefusalRow {
            case: "an index query whose condition omits the index partition key",
            operation: "Query",
            body: json!({"TableName": TABLE, "IndexName": "ByGpkAll",
                         "KeyConditionExpression": "gsk = :s",
                         "ExpressionAttributeValues": {":s": {"N": "2"}}}),
            message: "Query condition missed key schema element: gpk".to_string(),
        },
        RefusalRow {
            case: "an index query whose condition names the TABLE's partition key",
            operation: "Query",
            body: json!({"TableName": TABLE, "IndexName": "ByGpkAll",
                         "KeyConditionExpression": "pk = :p",
                         "ExpressionAttributeValues": {":p": {"S": "p1"}}}),
            message: "Query condition missed key schema element: gpk".to_string(),
        },
        RefusalRow {
            case: "an index query supplying a string for the numeric index sort key",
            operation: "Query",
            body: json!({"TableName": TABLE, "IndexName": "ByGpkAll",
                         "KeyConditionExpression": "gpk = :g AND gsk = :s",
                         "ExpressionAttributeValues": {":g": {"S": "g1"},
                                                       ":s": {"S": "2"}}}),
            message: "One or more parameter values were invalid: Type mismatch for key gsk \
                      expected: N actual: S"
                .to_string(),
        },
        RefusalRow {
            case: "a table query supplying a number for the string partition key",
            operation: "Query",
            body: json!({"TableName": TABLE, "KeyConditionExpression": "pk = :p",
                         "ExpressionAttributeValues": {":p": {"N": "1"}}}),
            message: "One or more parameter values were invalid: Type mismatch for key pk \
                      expected: S actual: N"
                .to_string(),
        },
        RefusalRow {
            case: "a strongly consistent query of a global secondary index",
            operation: "Query",
            body: json!({"TableName": TABLE, "IndexName": "ByGpkAll", "ConsistentRead": true,
                         "KeyConditionExpression": "gpk = :g",
                         "ExpressionAttributeValues": {":g": {"S": "g1"}}}),
            message: "Consistent reads are not supported on global secondary indexes"
                .to_string(),
        },
        RefusalRow {
            case: "a query of every attribute of a KEYS_ONLY global index",
            operation: "Query",
            body: json!({"TableName": TABLE, "IndexName": "ByGpkKeysOnly",
                         "Select": "ALL_ATTRIBUTES",
                         "KeyConditionExpression": "gpk = :g",
                         "ExpressionAttributeValues": {":g": {"S": "g1"}}}),
            message: "One or more parameter values were invalid: Select type ALL_ATTRIBUTES is \
                      not supported for global secondary index ByGpkKeysOnly because its \
                      projection type is not ALL"
                .to_string(),
        },
        RefusalRow {
            case: "a query of every attribute together with a projection",
            operation: "Query",
            body: json!({"TableName": TABLE, "Select": "ALL_ATTRIBUTES",
                         "ProjectionExpression": "note",
                         "KeyConditionExpression": "pk = :p",
                         "ExpressionAttributeValues": {":p": {"S": "p1"}}}),
            message: select_conflict("ALL_ATTRIBUTES"),
        },
        RefusalRow {
            case: "a counting query together with a projection",
            operation: "Query",
            body: json!({"TableName": TABLE, "Select": "COUNT",
                         "ProjectionExpression": "note",
                         "KeyConditionExpression": "pk = :p",
                         "ExpressionAttributeValues": {":p": {"S": "p1"}}}),
            message: select_conflict("COUNT"),
        },
        RefusalRow {
            case: "a counting scan together with a projection",
            operation: "Scan",
            body: json!({"TableName": TABLE, "Select": "COUNT",
                         "ProjectionExpression": "note"}),
            message: select_conflict("COUNT"),
        },
        RefusalRow {
            case: "a query projecting a path through an undefined alias",
            operation: "Query",
            body: json!({"TableName": TABLE, "ProjectionExpression": "doc.#u",
                         "KeyConditionExpression": "pk = :p",
                         "ExpressionAttributeValues": {":p": {"S": "p1"}}}),
            message: "Invalid ProjectionExpression: An expression attribute name used in the \
                      document path is not defined; attribute name: #u"
                .to_string(),
        },
        RefusalRow {
            case: "a batch read projecting an unaliased reserved word",
            operation: "BatchGetItem",
            body: json!({"RequestItems": {TABLE: {
                "Keys": [{"pk": {"S": "p1"}, "sk": {"S": "a"}}],
                "ProjectionExpression": "status",
            }}}),
            message: "Invalid ProjectionExpression: Attribute name is a reserved keyword; \
                      reserved keyword: status"
                .to_string(),
        },
        RefusalRow {
            case: "a transactional read projecting an unaliased reserved word",
            operation: "TransactGetItems",
            body: json!({"TransactItems": [{"Get": {
                "TableName": TABLE,
                "Key": {"pk": {"S": "p1"}, "sk": {"S": "a"}},
                "ProjectionExpression": "status",
            }}]}),
            message: "Invalid ProjectionExpression: Attribute name is a reserved keyword; \
                      reserved keyword: status"
                .to_string(),
        },
        RefusalRow {
            case: "a batch read stating one table's projection both ways",
            operation: "BatchGetItem",
            body: json!({"RequestItems": {TABLE: {
                "Keys": [{"pk": {"S": "p1"}, "sk": {"S": "a"}}],
                "ProjectionExpression": "note",
                "AttributesToGet": ["note"],
            }}}),
            message: "Can not use both expression and non-expression parameters in the same \
                      request: Non-expression parameters: {AttributesToGet} Expression \
                      parameters: {ProjectionExpression}"
                .to_string(),
        },
        RefusalRow {
            case: "an index query filtering on one of the TABLE's key attributes",
            operation: "Query",
            body: json!({"TableName": TABLE, "IndexName": "ByGpkAll",
                         "KeyConditionExpression": "gpk = :g",
                         "FilterExpression": "sk = :s",
                         "ExpressionAttributeValues": {":g": {"S": "g1"},
                                                       ":s": {"S": "a"}}}),
            message: "Filter Expression can only contain non-primary key attributes: \
                      Primary key attribute: sk"
                .to_string(),
        },
        RefusalRow {
            case: "an index query filtering on one of the INDEX's key attributes",
            operation: "Query",
            body: json!({"TableName": TABLE, "IndexName": "ByGpkAll",
                         "KeyConditionExpression": "gpk = :g",
                         "FilterExpression": "gsk = :n",
                         "ExpressionAttributeValues": {":g": {"S": "g1"},
                                                       ":n": {"N": "2"}}}),
            message: "Filter Expression can only contain non-primary key attributes: \
                      Primary key attribute: gsk"
                .to_string(),
        },
        RefusalRow {
            case: "a legacy attribute list naming one attribute twice",
            operation: "GetItem",
            body: json!({"TableName": TABLE, "Key": {"pk": {"S": "p1"}, "sk": {"S": "a"}},
                         "AttributesToGet": ["note", "note"]}),
            // The refusal names the member the request stated its projection THROUGH,
            // which for a request carrying no `ProjectionExpression` is the legacy list.
            message: "Invalid AttributesToGet: Two document paths overlap with each \
                      other; must remove or rewrite one of these paths; path one: [note], \
                      path two: [note]"
                .to_string(),
        },
        RefusalRow {
            case: "a read projecting two paths that overlap",
            operation: "GetItem",
            body: json!({"TableName": TABLE, "Key": {"pk": {"S": "p1"}, "sk": {"S": "a"}},
                         "ProjectionExpression": "doc, doc.leaf"}),
            message: "Invalid ProjectionExpression: Two document paths overlap with each \
                      other; must remove or rewrite one of these paths; path one: [doc], \
                      path two: [doc, leaf]"
                .to_string(),
        },
        RefusalRow {
            case: "a read projecting the same attribute twice",
            operation: "Scan",
            body: json!({"TableName": TABLE, "ProjectionExpression": "note, note"}),
            message: "Invalid ProjectionExpression: Two document paths overlap with each \
                      other; must remove or rewrite one of these paths; path one: [note], \
                      path two: [note]"
                .to_string(),
        },
    ]
}

#[tokio::test]
async fn a_read_the_service_refuses_is_a_client_error_not_an_unimplemented_one() {
    let router = emulator();
    seeded(&router).await;

    for row in refusal_rows() {
        let (status, refused) = call(&router, row.operation, row.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: a request the service refuses is the caller's to fix, got HTTP \
             {status} with {refused}",
            row.case
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {:?}: expected the modelled ValidationException, got {refused}",
            row.case
        );
        assert_eq!(
            message(&refused),
            row.message,
            "case {:?}: expected the message {:?}, got {refused}",
            row.case,
            row.message
        );
    }
}

// ===========================================================================
// 2. An index query reads the entries that index holds for one index partition
// ===========================================================================

/// One index query: the index, the condition's values, and the `note` of every item the
/// query must answer, in order.
struct IndexQueryRow {
    case: &'static str,
    index: &'static str,
    condition: &'static str,
    values: Value,
    /// Written out from the index's key schema over the items of [`stored_items`]: the
    /// entries whose index partition key the condition pins, in the index's own sort
    /// order with the table's key breaking the tie.
    notes: &'static [&'static str],
}

fn index_query_rows() -> Vec<IndexQueryRow> {
    vec![
        IndexQueryRow {
            case: "a global index partition, in the index's sort order",
            index: "ByGpkAll",
            condition: "gpk = :g",
            values: json!({":g": {"S": "g1"}}),
            // gsk 1, 2, 9 — NOT the table's sk order (a, b, a).
            notes: &["second", "first", "fourth"],
        },
        IndexQueryRow {
            case: "a global index partition narrowed by the index sort key",
            index: "ByGpkAll",
            condition: "gpk = :g AND gsk > :s",
            values: json!({":g": {"S": "g1"}, ":s": {"N": "1"}}),
            notes: &["first", "fourth"],
        },
        IndexQueryRow {
            case: "a global index partition narrowed by a sort-key range",
            index: "ByGpkAll",
            condition: "gpk = :g AND gsk BETWEEN :low AND :high",
            values: json!({":g": {"S": "g1"}, ":low": {"N": "2"}, ":high": {"N": "9"}}),
            notes: &["first", "fourth"],
        },
        IndexQueryRow {
            case: "another partition of the same index",
            index: "ByGpkAll",
            condition: "gpk = :g",
            values: json!({":g": {"S": "g2"}}),
            notes: &["third"],
        },
        IndexQueryRow {
            case: "a partition no item of the index carries",
            index: "ByGpkAll",
            condition: "gpk = :g",
            values: json!({":g": {"S": "absent"}}),
            notes: &[],
        },
        IndexQueryRow {
            case: "an INCLUDE index, which projects the note",
            index: "ByGpkInclude",
            condition: "gpk = :g",
            values: json!({":g": {"S": "g1"}}),
            notes: &["first", "second", "fourth"],
        },
        IndexQueryRow {
            case: "a local index partition, in the local sort order",
            index: "ByLsk",
            condition: "pk = :p AND begins_with(lsk, :prefix)",
            values: json!({":p": {"S": "p1"}, ":prefix": {"S": "x"}}),
            notes: &["first"],
        },
        IndexQueryRow {
            case: "a local index partition with every entry it holds",
            index: "ByLsk",
            condition: "pk = :p",
            values: json!({":p": {"S": "p1"}}),
            notes: &["first", "second"],
        },
    ]
}

#[tokio::test]
async fn an_index_query_answers_the_entries_of_one_index_partition() {
    let router = emulator();
    seeded(&router).await;

    for row in index_query_rows() {
        let queried = ok(
            &router,
            "Query",
            json!({"TableName": TABLE, "IndexName": row.index,
                   "KeyConditionExpression": row.condition,
                   "ExpressionAttributeValues": row.values}),
        )
        .await;
        let expected: Vec<String> = row.notes.iter().map(|note| (*note).to_string()).collect();
        assert_eq!(
            reported_notes(&queried),
            expected,
            "case {:?}: expected the entries {expected:?} in that order, got {queried}",
            row.case
        );
        assert_eq!(
            queried["Count"],
            json!(row.notes.len()),
            "case {:?}: Count counts the entries answered, got {queried}",
            row.case
        );
    }

    // The KEYS_ONLY index projects only the key attributes, so each entry it answers
    // carries the index's keys and the table's and nothing else — which is why its rows
    // above are checked by a different handle than `note`.
    let keys_only = ok(
        &router,
        "Query",
        json!({"TableName": TABLE, "IndexName": "ByGpkKeysOnly",
               "KeyConditionExpression": "gpk = :g",
               "ExpressionAttributeValues": {":g": {"S": "g2"}}}),
    )
    .await;
    assert_eq!(
        reported_items(&keys_only),
        rendered(&[json!({"gpk": {"S": "g2"}, "pk": {"S": "p1"}, "sk": {"S": "c"}})]),
        "case a KEYS_ONLY index: the entry carries the index and table keys only, got \
         {keys_only}"
    );

    // Reversing the order reverses the INDEX's order, not the table's.
    let reversed = ok(
        &router,
        "Query",
        json!({"TableName": TABLE, "IndexName": "ByGpkAll",
               "KeyConditionExpression": "gpk = :g", "ScanIndexForward": false,
               "ExpressionAttributeValues": {":g": {"S": "g1"}}}),
    )
    .await;
    assert_eq!(
        reported_notes(&reversed),
        vec![
            "fourth".to_string(),
            "first".to_string(),
            "second".to_string()
        ],
        "case ScanIndexForward=false on an index: the index order is reversed, got {reversed}"
    );

    // "COUNT - Returns the number of matching items, rather than the matching items
    // themselves", counted over the entries of that one index partition.
    let counted = ok(
        &router,
        "Query",
        json!({"TableName": TABLE, "IndexName": "ByGpkAll", "Select": "COUNT",
               "KeyConditionExpression": "gpk = :g",
               "ExpressionAttributeValues": {":g": {"S": "g1"}}}),
    )
    .await;
    assert_eq!(
        counted["Count"],
        json!(3),
        "case COUNT on an index query: the count is over the index's entries, got {counted}"
    );
    assert!(
        counted.get("Items").is_none_or(Value::is_null),
        "case COUNT on an index query: the count is reported rather than the entries, got \
         {counted}"
    );

    // A strongly consistent read of a LOCAL index is served — the refusal is the global
    // index's alone — and reports the same entries the eventually consistent read does.
    let consistent = ok(
        &router,
        "Query",
        json!({"TableName": TABLE, "IndexName": "ByLsk", "ConsistentRead": true,
               "KeyConditionExpression": "pk = :p",
               "ExpressionAttributeValues": {":p": {"S": "p1"}}}),
    )
    .await;
    assert_eq!(
        reported_notes(&consistent),
        vec!["first".to_string(), "second".to_string()],
        "case a consistent read of a local index: it is served, got {consistent}"
    );
}

#[tokio::test]
async fn an_index_query_pages_through_the_index_order() {
    let router = emulator();
    seeded(&router).await;

    // Every `Limit` from one to past the partition's entry count: each page carries at
    // most that many entries, reports where it stopped while entries remain, and the
    // pages together are the whole partition in index order, each entry exactly once.
    for limit in 1..=4 {
        let mut notes: Vec<String> = Vec::new();
        let mut start: Option<Value> = None;
        for page_number in 0..5 {
            let mut request = json!({"TableName": TABLE, "IndexName": "ByGpkAll", "Limit": limit,
                                     "KeyConditionExpression": "gpk = :g",
                                     "ExpressionAttributeValues": {":g": {"S": "g1"}}});
            if let Some(start) = &start {
                request["ExclusiveStartKey"] = start.clone();
            }
            let page = ok(&router, "Query", request).await;
            let reported = reported_notes(&page);
            assert!(
                reported.len() <= limit,
                "case limit {limit} page {page_number}: a page carries at most the limit, \
                 got {page}"
            );
            notes.extend(reported);
            start = last_evaluated_key(&page);
            if start.is_none() {
                break;
            }
        }
        assert_eq!(
            start, None,
            "case limit {limit}: the pages end with no LastEvaluatedKey, got {start:?}"
        );
        assert_eq!(
            notes,
            vec![
                "second".to_string(),
                "first".to_string(),
                "fourth".to_string()
            ],
            "case limit {limit}: the pages together are the index partition in index order"
        );
    }

    // A starting key from ANOTHER index partition than the condition selects is the
    // caller's mistake, not an empty page.
    let (status, refused) = call(
        &router,
        "Query",
        json!({"TableName": TABLE, "IndexName": "ByGpkAll",
               "KeyConditionExpression": "gpk = :g",
               "ExpressionAttributeValues": {":g": {"S": "g1"}},
               "ExclusiveStartKey": {"gpk": {"S": "g2"}, "gsk": {"N": "3"},
                                     "pk": {"S": "p1"}, "sk": {"S": "c"}}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a starting key from another index partition: expected HTTP 400, got {status} \
         with {refused}"
    );
    assert_eq!(
        message(&refused),
        "The provided starting key does not match the key condition's partition key",
        "case a starting key from another index partition: got {refused}"
    );
}

// ===========================================================================
// 3. A projection reports the locations it names, on every read path
// ===========================================================================

/// One projection form: the expression, the aliases it resolves through, and the item the
/// read must report for the seeded item `p1/a`.
struct ProjectionRow {
    case: &'static str,
    expression: &'static str,
    names: Value,
    expected: Value,
}

/// The forms a projection takes, from "These attributes can include scalars, sets, or
/// elements of a JSON document" (`ProjectionExpression`): a whole attribute, several of
/// them, an aliased name, a path into a map, a path into a list, and a path whose every
/// step is aliased.
fn projection_rows() -> Vec<ProjectionRow> {
    vec![
        ProjectionRow {
            case: "one whole attribute",
            expression: "note",
            names: Value::Null,
            expected: json!({"note": {"S": "first"}}),
        },
        ProjectionRow {
            case: "several whole attributes",
            expression: "note, pk",
            names: Value::Null,
            expected: json!({"note": {"S": "first"}, "pk": {"S": "p1"}}),
        },
        ProjectionRow {
            case: "an attribute reached through an alias",
            expression: "#n",
            names: json!({"#n": "note"}),
            expected: json!({"note": {"S": "first"}}),
        },
        ProjectionRow {
            case: "an attribute the item does not carry",
            expression: "absent",
            names: Value::Null,
            expected: json!({}),
        },
        ProjectionRow {
            case: "an alias standing for an attribute name that CONTAINS a dot",
            // The alias substitutes a NAME, not a path, so the dot in it is part of the
            // attribute's name: "#dotted" projects the top-level attribute `odd.name`
            // rather than stepping into an attribute called `odd`.
            expression: "#dotted",
            names: json!({"#dotted": "odd.name"}),
            expected: json!({"odd.name": {"S": "literal"}}),
        },
        ProjectionRow {
            case: "a path into a map, which reports the containers it steps through",
            expression: "doc.leaf",
            names: Value::Null,
            expected: json!({"doc": {"M": {"leaf": {"S": "deep"}}}}),
        },
        ProjectionRow {
            case: "a path into a map with every step aliased",
            expression: "#d.#i",
            names: json!({"#d": "doc", "#i": "leaf"}),
            expected: json!({"doc": {"M": {"leaf": {"S": "deep"}}}}),
        },
        ProjectionRow {
            case: "two paths into one map",
            expression: "doc.leaf, doc.branch",
            names: Value::Null,
            expected: json!({"doc": {"M": {"leaf": {"S": "deep"}, "branch": {"N": "7"}}}}),
        },
        ProjectionRow {
            case: "a path into a list, which reports a one-element list",
            expression: "parts[1]",
            names: Value::Null,
            expected: json!({"parts": {"L": [{"S": "one"}]}}),
        },
        ProjectionRow {
            case: "a map path the item holds nothing at",
            expression: "doc.absent",
            names: Value::Null,
            expected: json!({}),
        },
        ProjectionRow {
            case: "a whole attribute beside a path into another",
            expression: "note, doc.branch",
            names: Value::Null,
            expected: json!({"note": {"S": "first"}, "doc": {"M": {"branch": {"N": "7"}}}}),
        },
    ]
}

/// The one item `p1/a`, as each read path reports it under `projection`.
async fn projected_everywhere(
    router: &MultiServiceRouter,
    expression: &str,
    names: &Value,
) -> Vec<(&'static str, Value)> {
    let key = json!({"pk": {"S": "p1"}, "sk": {"S": "a"}});
    let with_names = |mut body: Value| {
        if !names.is_null() {
            body["ExpressionAttributeNames"] = names.clone();
        }
        body
    };

    let read = ok(
        router,
        "GetItem",
        with_names(
            json!({"TableName": TABLE, "Key": key, "ProjectionExpression": expression}),
        ),
    )
    .await;
    let from_get = read.get("Item").cloned().unwrap_or(json!({}));

    let queried = ok(
        router,
        "Query",
        with_names(json!({"TableName": TABLE, "ProjectionExpression": expression,
                          "KeyConditionExpression": "pk = :p AND sk = :s",
                          "ExpressionAttributeValues": {":p": {"S": "p1"},
                                                        ":s": {"S": "a"}}})),
    )
    .await;
    let from_query = queried["Items"]
        .as_array()
        .and_then(|items| items.first().cloned())
        .unwrap_or(json!({}));

    let scanned = ok(
        router,
        "Scan",
        with_names(json!({"TableName": TABLE, "ProjectionExpression": expression,
                          "FilterExpression": "note = :note",
                          "ExpressionAttributeValues": {":note": {"S": "first"}}})),
    )
    .await;
    let from_scan = scanned["Items"]
        .as_array()
        .and_then(|items| items.first().cloned())
        .unwrap_or(json!({}));

    let mut entry = json!({"Keys": [key], "ProjectionExpression": expression});
    if !names.is_null() {
        entry["ExpressionAttributeNames"] = names.clone();
    }
    let batch = ok(
        router,
        "BatchGetItem",
        json!({"RequestItems": {TABLE: entry}}),
    )
    .await;
    let from_batch = batch["Responses"][TABLE]
        .as_array()
        .and_then(|items| items.first().cloned())
        .unwrap_or(json!({}));

    let mut get = json!({"TableName": TABLE, "Key": key, "ProjectionExpression": expression});
    if !names.is_null() {
        get["ExpressionAttributeNames"] = names.clone();
    }
    let transacted = ok(
        router,
        "TransactGetItems",
        json!({"TransactItems": [{"Get": get}]}),
    )
    .await;
    let from_transaction = transacted["Responses"]
        .as_array()
        .and_then(|responses| responses.first().cloned())
        .and_then(|response| response.get("Item").cloned())
        .unwrap_or(json!({}));

    vec![
        ("GetItem", from_get),
        ("Query", from_query),
        ("Scan", from_scan),
        ("BatchGetItem", from_batch),
        ("TransactGetItems", from_transaction),
    ]
}

#[tokio::test]
async fn a_projection_reports_the_locations_it_names_on_every_read_path() {
    let router = emulator();
    seeded(&router).await;

    for row in projection_rows() {
        for (path, reported) in projected_everywhere(&router, row.expression, &row.names).await {
            assert_eq!(
                reported, row.expected,
                "case {:?} through {path}: expected {}, got {reported}",
                row.case, row.expected
            );
        }
    }
}

#[tokio::test]
async fn the_legacy_attribute_list_projects_what_the_expression_projects() {
    let router = emulator();
    let items = seeded(&router).await;
    let key = json!({"pk": {"S": "p1"}, "sk": {"S": "a"}});
    let expected = narrowed(&items[0], &["note", "pk"]);

    // "AttributesToGet - This is a legacy parameter. Use ProjectionExpression instead"
    // (that member's doc): the same ask, so it reports the same attributes — on each path
    // that still takes it.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key, "AttributesToGet": ["note", "pk"]}),
    )
    .await;
    assert_eq!(
        read.get("Item").cloned().unwrap_or(json!({})),
        expected,
        "case AttributesToGet on GetItem: expected {expected}, got {read}"
    );

    let batch = ok(
        &router,
        "BatchGetItem",
        json!({"RequestItems": {TABLE: {"Keys": [key],
                                        "AttributesToGet": ["note", "pk"]}}}),
    )
    .await;
    assert_eq!(
        batch["Responses"][TABLE]
            .as_array()
            .and_then(|items| items.first().cloned())
            .unwrap_or(json!({})),
        expected,
        "case AttributesToGet on BatchGetItem: expected {expected}, got {batch}"
    );

    let queried = ok(
        &router,
        "Query",
        json!({"TableName": TABLE, "AttributesToGet": ["note", "pk"],
               "KeyConditionExpression": "pk = :p AND sk = :s",
               "ExpressionAttributeValues": {":p": {"S": "p1"}, ":s": {"S": "a"}}}),
    )
    .await;
    assert_eq!(
        reported_items(&queried),
        rendered(std::slice::from_ref(&expected)),
        "case AttributesToGet on Query: expected {expected}, got {queried}"
    );

    let scanned = ok(
        &router,
        "Scan",
        json!({"TableName": TABLE, "AttributesToGet": ["note", "pk"],
               "FilterExpression": "note = :note",
               "ExpressionAttributeValues": {":note": {"S": "first"}}}),
    )
    .await;
    assert_eq!(
        reported_items(&scanned),
        rendered(std::slice::from_ref(&expected)),
        "case AttributesToGet on Scan: expected {expected}, got {scanned}"
    );
}

#[tokio::test]
async fn a_projection_over_an_index_reports_what_that_index_holds() {
    let router = emulator();
    let items = seeded(&router).await;

    // A projection over an index narrows what the INDEX holds: the attributes it names,
    // out of that index's entry.
    let projected = ok(
        &router,
        "Query",
        json!({"TableName": TABLE, "IndexName": "ByGpkAll",
               "ProjectionExpression": "#n, gsk",
               "ExpressionAttributeNames": {"#n": "note"},
               "KeyConditionExpression": "gpk = :g",
               "ExpressionAttributeValues": {":g": {"S": "g2"}}}),
    )
    .await;
    assert_eq!(
        reported_items(&projected),
        rendered(&[narrowed(&items[2], &["note", "gsk"])]),
        "case a projection over an index: only the projected names are reported, got \
         {projected}"
    );

    // A local index that projects ALL answers an attribute from the entry itself, and a
    // query of it with no projection at all reports every attribute it holds.
    let whole = ok(
        &router,
        "Query",
        json!({"TableName": TABLE, "IndexName": "ByLsk",
               "KeyConditionExpression": "pk = :p AND lsk = :l",
               "ExpressionAttributeValues": {":p": {"S": "p1"}, ":l": {"S": "y"}}}),
    )
    .await;
    assert_eq!(
        reported_items(&whole),
        rendered(&[items[1].clone()]),
        "case a local index projecting ALL: the whole item is reported, got {whole}"
    );

    // The projection is applied after the key conditions selected the page, so a
    // projection that reports nothing of an entry still counts that entry.
    let counted = ok(
        &router,
        "Query",
        json!({"TableName": TABLE, "IndexName": "ByGpkAll",
               "ProjectionExpression": "absent",
               "KeyConditionExpression": "gpk = :g",
               "ExpressionAttributeValues": {":g": {"S": "g1"}}}),
    )
    .await;
    assert_eq!(
        counted["Count"],
        json!(3),
        "case a projection naming nothing the entries carry: the entries are still \
         counted, got {counted}"
    );
    assert_eq!(
        counted["ScannedCount"],
        json!(3),
        "case a projection naming nothing the entries carry: every entry was still \
         evaluated, got {counted}"
    );
}
