// SPDX-License-Identifier: CC0-1.0

//! WHICH SENTENCE a request that defines a substitution token no expression uses earns,
//! driven end to end.
//!
//! RULE under test: the refusal is the service's own, raised once a well-formed input has
//! already passed the request validator, so it is stated as the bare sentence
//! `Value provided in <member> unused in expressions: keys: {<keys>}` — and it carries the
//! `<N> validation error(s) detected: ` envelope only where every other refusal this
//! service raises about a request's expressions does, i.e. where the request states its
//! intent through `UpdateExpression` or `ConditionExpression`. A request whose expressions
//! only NARROW a read (`FilterExpression`, `ProjectionExpression`,
//! `KeyConditionExpression`) earns the sentence on its own.
//!
//! The rows span what the rule generalises over: the expression member the keys were
//! offered to (a filter, a projection, a key condition, an update and a condition), the
//! operation (a scan, a query, an item read, a batch read, an item write, an item delete
//! and an item update), which substitution table holds the unused keys
//! (`ExpressionAttributeNames` and `ExpressionAttributeValues`) and how many of them are
//! unused (one, two and three). A request that states NO expression at all offers its
//! tokens to nothing and earns a different sentence — that one member cannot be used
//! without an expression — which `substitution_without_expression` drives.
//!
//! The last assertion is the boundary the rule draws: a violation of a bound the MODEL
//! declares on the same operation (`Scan$Limit` is `PositiveIntegerObject` with
//! `@range{min:1}`, per `smithy_query(query="operation", operation="Scan")`) still carries
//! the envelope, so the envelope means what it says and is not merely absent everywhere.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;
// The sentence both substitution tables are refused with is stated ONCE for this crate.
use crate::undefined_expression_tokens::unused_entries;

/// The table every row below drives: one string partition key.
const TABLE: &str = "UnusedSubstitutionKeys";

/// The key of the seeded item, so no row is refused for its key instead of for its
/// substitution tables.
fn key() -> Value {
    json!({"pk": {"S": "item-1"}})
}

/// The table and the one item every row is driven against.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE,
               "Item": {"pk": {"S": "item-1"}, "status": {"S": "active"},
                        "category": {"S": "books"}, "note": {"S": "kept"}}}),
    )
    .await;
    router
}

/// One row: the case, the operation, the request, the expression member the request
/// states its intent through (empty when it carries none), the substitution table the
/// unused keys belong to, and those keys in the order the message lists them.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    expression_member: &'static str,
    table: &'static str,
    keys: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one name a scan's filter never references",
            operation: "Scan",
            request: json!({"TableName": TABLE, "ConsistentRead": true,
                            "FilterExpression": "attribute_exists(#a)",
                            "ExpressionAttributeNames": {"#a": "status", "#b": "category"}}),
            expression_member: "FilterExpression",
            table: "ExpressionAttributeNames",
            keys: &["#b"],
        },
        Row {
            case: "three names a scan's filter never references",
            operation: "Scan",
            request: json!({"TableName": TABLE, "ConsistentRead": true,
                            "FilterExpression": "attribute_exists(#a)",
                            "ExpressionAttributeNames": {"#a": "status", "#b": "category",
                                                         "#c": "note", "#d": "pk"}}),
            expression_member: "FilterExpression",
            table: "ExpressionAttributeNames",
            keys: &["#b", "#c", "#d"],
        },
        Row {
            case: "a value a scan's filter never references",
            operation: "Scan",
            request: json!({"TableName": TABLE, "ConsistentRead": true,
                            "FilterExpression": "category = :a",
                            "ExpressionAttributeValues": {":a": {"S": "books"},
                                                          ":b": {"S": "paused"}}}),
            expression_member: "FilterExpression",
            table: "ExpressionAttributeValues",
            keys: &[":b"],
        },
        Row {
            case: "a name a scan's projection never references",
            operation: "Scan",
            request: json!({"TableName": TABLE, "ConsistentRead": true,
                            "ProjectionExpression": "#a",
                            "ExpressionAttributeNames": {"#a": "status", "#b": "category"}}),
            expression_member: "ProjectionExpression",
            table: "ExpressionAttributeNames",
            keys: &["#b"],
        },
        Row {
            case: "two values a query's key condition never references",
            operation: "Query",
            request: json!({"TableName": TABLE,
                            "KeyConditionExpression": "pk = :a",
                            "ExpressionAttributeValues": {":a": {"S": "item-1"},
                                                          ":b": {"S": "x"},
                                                          ":c": {"S": "y"}}}),
            expression_member: "KeyConditionExpression",
            table: "ExpressionAttributeValues",
            keys: &[":b", ":c"],
        },
        Row {
            case: "a name neither of a query's two expressions references",
            operation: "Query",
            request: json!({"TableName": TABLE,
                            "KeyConditionExpression": "pk = :a",
                            "FilterExpression": "attribute_exists(#a)",
                            "ExpressionAttributeNames": {"#a": "status", "#b": "category"},
                            "ExpressionAttributeValues": {":a": {"S": "item-1"}}}),
            expression_member: "KeyConditionExpression",
            table: "ExpressionAttributeNames",
            keys: &["#b"],
        },
        Row {
            case: "a name an item read's projection never references",
            operation: "GetItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "ProjectionExpression": "#a",
                            "ExpressionAttributeNames": {"#a": "status", "#b": "category"}}),
            expression_member: "ProjectionExpression",
            table: "ExpressionAttributeNames",
            keys: &["#b"],
        },
        Row {
            case: "a name a batch read's projection never references",
            operation: "BatchGetItem",
            request: json!({"RequestItems": {TABLE: {
                "Keys": [key()],
                "ProjectionExpression": "#a",
                "ExpressionAttributeNames": {"#a": "status", "#b": "category"}}}}),
            expression_member: "ProjectionExpression",
            table: "ExpressionAttributeNames",
            keys: &["#b"],
        },
        Row {
            case: "a value a write's condition never references",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": {"pk": {"S": "item-2"}, "status": {"S": "paused"}},
                            "ConditionExpression": "attribute_not_exists(#a)",
                            "ExpressionAttributeNames": {"#a": "status"},
                            "ExpressionAttributeValues": {":b": {"S": "paused"}}}),
            expression_member: "ConditionExpression",
            table: "ExpressionAttributeValues",
            keys: &[":b"],
        },
        Row {
            case: "a name a delete's condition never references",
            operation: "DeleteItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "ConditionExpression": "attribute_exists(#a)",
                            "ExpressionAttributeNames": {"#a": "status", "#b": "category"}}),
            expression_member: "ConditionExpression",
            table: "ExpressionAttributeNames",
            keys: &["#b"],
        },
        Row {
            case: "a name neither an update's expression nor its condition references",
            operation: "UpdateItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "UpdateExpression": "SET #a = :a",
                            "ConditionExpression": "attribute_exists(pk)",
                            "ExpressionAttributeNames": {"#a": "status", "#b": "category"},
                            "ExpressionAttributeValues": {":a": {"S": "paused"}}}),
            expression_member: "UpdateExpression",
            table: "ExpressionAttributeNames",
            keys: &["#b"],
        },
        Row {
            case: "a value an update's expression never references",
            operation: "UpdateItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "UpdateExpression": "SET category = :a",
                            "ExpressionAttributeValues": {":a": {"S": "paused"},
                                                          ":b": {"S": "gone"}}}),
            expression_member: "UpdateExpression",
            table: "ExpressionAttributeValues",
            keys: &[":b"],
        },
    ]
}

#[tokio::test]
async fn an_unused_substitution_key_is_refused_as_the_service_words_it() {
    let router = seeded().await;

    for row in rows() {
        let expected = unused_entries(row.expression_member, row.table, row.keys);
        let (status, body) = call(&router, row.operation, row.request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {:?} ({}): expected 400 ValidationException, got {status} {body}",
            row.case,
            row.operation
        );
        let said = message(&body);
        assert_eq!(
            said, expected,
            "case {:?} ({}): expected {expected:?}, got {said:?}",
            row.case, row.operation
        );
        // Only the members a WRITE states its intent through are stated inside the
        // envelope, so a read's refusal must not carry it at all.
        assert_eq!(
            said.starts_with("1 validation error detected: "),
            row.expression_member == "UpdateExpression"
                || row.expression_member == "ConditionExpression",
            "case {:?} ({}): the envelope follows the expression member, got {said:?}",
            row.case,
            row.operation
        );
    }

    // Every refusal above left the store where it was: the seeded item still carries the
    // values it was written with, and the item a refused write would have added is absent.
    let read = ok(&router, "GetItem", json!({"TableName": TABLE, "Key": key()})).await;
    assert_eq!(
        read["Item"]["status"],
        json!({"S": "active"}),
        "the refused requests must leave the item alone, got {read}"
    );
    let absent = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "item-2"}}}),
    )
    .await;
    assert_eq!(
        absent.get("Item"),
        None,
        "a refused write must store nothing, got {absent}"
    );
}

#[tokio::test]
async fn a_request_that_references_every_key_it_defines_is_served() {
    let router = seeded().await;

    // The control for every row above: the same shape of request with each key
    // referenced is served, so the refusals are about the UNUSED key and not about
    // carrying the substitution table at all.
    let scanned = ok(
        &router,
        "Scan",
        json!({"TableName": TABLE, "ConsistentRead": true,
               "FilterExpression": "attribute_exists(#a) AND #b = :b",
               "ExpressionAttributeNames": {"#a": "status", "#b": "category"},
               "ExpressionAttributeValues": {":b": {"S": "books"}}}),
    )
    .await;
    assert_eq!(
        scanned["Count"],
        json!(1),
        "the scan whose every token is referenced must report the seeded item, got {scanned}"
    );
    let queried = ok(
        &router,
        "Query",
        json!({"TableName": TABLE,
               "KeyConditionExpression": "#k = :k",
               "ExpressionAttributeNames": {"#k": "pk"},
               "ExpressionAttributeValues": {":k": {"S": "item-1"}}}),
    )
    .await;
    assert_eq!(
        queried["Count"],
        json!(1),
        "the query whose every token is referenced must report the seeded item, got {queried}"
    );
    let updated = ok(
        &router,
        "UpdateItem",
        json!({"TableName": TABLE, "Key": key(),
               "UpdateExpression": "SET #a = :a",
               "ExpressionAttributeNames": {"#a": "status"},
               "ExpressionAttributeValues": {":a": {"S": "paused"}},
               "ReturnValues": "ALL_NEW"}),
    )
    .await;
    assert_eq!(
        updated["Attributes"]["status"],
        json!({"S": "paused"}),
        "the update whose every token is referenced must apply, got {updated}"
    );
}

#[tokio::test]
async fn a_modelled_bound_on_the_same_operation_keeps_the_envelope() {
    let router = seeded().await;

    // `Scan$Limit` is `PositiveIntegerObject` with `@range{min:1}`, so a Limit of 0 is a
    // violation of a bound the MODEL declares — the class the envelope belongs to — while
    // the unused-key rule beside it is the service's own.
    let (status, body) = call(&router, "Scan", json!({"TableName": TABLE, "Limit": 0})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a Limit below its declared minimum is a 400 ValidationException, got {status} {body}"
    );
    let said = message(&body);
    assert!(
        said.starts_with("1 validation error detected: "),
        "a modelled constraint violation keeps the envelope, got {said:?}"
    );
}
