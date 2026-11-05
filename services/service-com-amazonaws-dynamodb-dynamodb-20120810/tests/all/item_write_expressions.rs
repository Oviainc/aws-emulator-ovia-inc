// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DynamoDB item WRITE path: that a write acknowledged with
//! 200 is in the store, and that `ReturnValues` reports the item as it stood
//! immediately before (or after) that write.
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, so every
//! assertion is about what a LATER request reports — never about a handler's return
//! value in isolation.
//!
//! Each suite is a TABLE spanning the class its rule generalises over: the key schemas
//! a table can declare, the actions an `UpdateExpression` can state, the conditions a
//! conditional write can state, and the ways a request can be wrong.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    MAX_UPDATE_ACTIONS, STATUS_TRANSITION,
};
use smithy_server_runtime::MultiServiceRouter;

// One harness for the whole target: the router, the request and the error-code reader
// come from `tests/all/emulator.rs`, and the CREATING -> ACTIVE table helper from
// `tests/all/table_items.rs`, so the way this crate is driven over the wire — and the
// way a table is made ready — is described once.
use crate::emulator::{call, emulator, error_code, ok};
use crate::table_items::create_active_table;

/// One item: the key's attributes plus `extra`.
fn item_with(key: &Value, extra: &Value) -> Value {
    let mut merged = key.clone();
    for (name, value) in extra.as_object().expect("an object of attributes") {
        merged[name] = value.clone();
    }
    merged
}

/// The item stored under `key`, or `Value::Null` when the key holds none.
async fn stored_item(router: &MultiServiceRouter, table: &str, key: &Value) -> Value {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": table, "Key": key, "ConsistentRead": true}),
    )
    .await;
    read.get("Item").cloned().unwrap_or(Value::Null)
}

/// An `ExpressionAttributeValues`-shaped map, or no member at all when empty.
fn values_member(values: &Value) -> Option<Value> {
    values
        .as_object()
        .filter(|map| !map.is_empty())
        .map(|map| Value::Object(map.clone()))
}

/// Put the members of `extra` onto a request body.
fn request(base: Value, extra: Vec<(&str, Option<Value>)>) -> Value {
    let mut body = base;
    for (name, value) in extra {
        if let Some(value) = value {
            body[name] = value;
        }
    }
    body
}

/// One row of the key-schema table: the case name, the `(attribute, key type,
/// attribute type)` schema the table declares, and the primary key of the item the row
/// writes.
type KeySchemaCase = (
    &'static str,
    Vec<(&'static str, &'static str, &'static str)>,
    Value,
);

/// The request members one row adds to a request body, each present only when the row
/// states it.
type RequestMembers = Vec<(&'static str, Option<Value>)>;

/// One row of the condition table: the case name, the condition members it states, and
/// whether the write they guard may proceed.
type ConditionCase = (&'static str, RequestMembers, bool);

/// One row of the refusal table: the case name, the operation it calls, and the
/// members it sends beside `TableName`/`Key`.
type RefusalCase = (&'static str, &'static str, RequestMembers);

/// A write acknowledged with 200 is in the store: a `PutItem` replaces the whole item,
/// an `UpdateItem` is visible to a later `GetItem` AND `Query`, a `DeleteItem` removes
/// the item, and `ReturnValues=ALL_OLD` reports the item as it stood IMMEDIATELY
/// before that write. The rows span the key schemas a table can declare, since the
/// rule is about the write path rather than about one key type.
#[tokio::test]
async fn item_mutations_are_persisted_for_every_key_schema() {
    let router = emulator();
    let cases: Vec<KeySchemaCase> = vec![
        (
            "string partition key",
            vec![("pk", "HASH", "S")],
            json!({"pk": {"S": "k1"}}),
        ),
        (
            "string partition key and number sort key",
            vec![("pk", "HASH", "S"), ("sk", "RANGE", "N")],
            json!({"pk": {"S": "k1"}, "sk": {"N": "7"}}),
        ),
        (
            "number partition key",
            vec![("pk", "HASH", "N")],
            json!({"pk": {"N": "42"}}),
        ),
        (
            "binary partition key and string sort key",
            vec![("pk", "HASH", "B"), ("sk", "RANGE", "S")],
            json!({"pk": {"B": "AAH+/w=="}, "sk": {"S": "s1"}}),
        ),
    ];

    for (index, (case, keys, key)) in cases.iter().enumerate() {
        let table = format!("items-{index}");
        create_active_table(&router, &table, keys).await;
        let partition = keys[0].0;
        let partition_value = key[partition].clone();

        // PUT: the stored item is exactly what was written.
        let full = item_with(
            key,
            &json!({
                "ver": {"N": "1"},
                "tags": {"SS": ["x", "y"]},
                "meta": {"M": {"a": {"S": "1"}}},
                "blob": {"B": "AAH+/w=="},
            }),
        );
        ok(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": full}),
        )
        .await;
        let read = stored_item(&router, &table, key).await;
        assert_eq!(
            read, full,
            "case {case}: a put item reads back exactly as written, got {read}"
        );

        // PUT AGAIN: "the new item completely replaces the existing item", so the
        // attributes the second put omits are gone, and ALL_OLD is the item it
        // replaced.
        let smaller = item_with(key, &json!({"ver": {"N": "2"}}));
        let replaced = ok(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": smaller, "ReturnValues": "ALL_OLD"}),
        )
        .await;
        assert_eq!(
            replaced["Attributes"], full,
            "case {case}: PutItem ALL_OLD reports the item it replaced, got {replaced}"
        );
        let read = stored_item(&router, &table, key).await;
        assert_eq!(
            read, smaller,
            "case {case}: a put replaces the whole item, got {read}"
        );

        // UPDATE: the edit lands, and both readers see it.
        let updated = ok(
            &router,
            "UpdateItem",
            json!({
                "TableName": table,
                "Key": key,
                "UpdateExpression": "SET #s = :s ADD #v :one",
                "ExpressionAttributeNames": {"#s": "status", "#v": "ver"},
                "ExpressionAttributeValues": {":s": {"S": "UPDATED"}, ":one": {"N": "1"}},
                "ReturnValues": "ALL_NEW",
            }),
        )
        .await;
        let after_update = item_with(key, &json!({"ver": {"N": "3"}, "status": {"S": "UPDATED"}}));
        assert_eq!(
            updated["Attributes"], after_update,
            "case {case}: UpdateItem ALL_NEW reports the updated item, got {updated}"
        );
        let read = stored_item(&router, &table, key).await;
        assert_eq!(
            read, after_update,
            "case {case}: a GetItem after an update reports the updated item, got {read}"
        );
        let queried = ok(
            &router,
            "Query",
            json!({
                "TableName": table,
                "KeyConditionExpression": "#p = :p",
                "ExpressionAttributeNames": {"#p": partition},
                "ExpressionAttributeValues": {":p": partition_value},
            }),
        )
        .await;
        assert_eq!(
            (queried["Count"].clone(), queried["ScannedCount"].clone()),
            (json!(1), json!(1)),
            "case {case}: a Query after an update finds the updated item, got {queried}"
        );
        assert_eq!(
            queried["Items"][0], after_update,
            "case {case}: the queried item is the updated one, got {queried}"
        );

        // DELETE: the item goes, and ALL_OLD is the item as it stood immediately
        // before the delete — the UPDATED one, not the one the put left.
        let deleted = ok(
            &router,
            "DeleteItem",
            json!({"TableName": table, "Key": key, "ReturnValues": "ALL_OLD"}),
        )
        .await;
        assert_eq!(
            deleted["Attributes"], after_update,
            "case {case}: DeleteItem ALL_OLD reports the item as it stood before the delete, \
             got {deleted}"
        );
        let read = stored_item(&router, &table, key).await;
        assert_eq!(
            read,
            Value::Null,
            "case {case}: a deleted key holds no item, got {read}"
        );
        let queried = ok(
            &router,
            "Query",
            json!({
                "TableName": table,
                "KeyConditionExpression": "#p = :p",
                "ExpressionAttributeNames": {"#p": partition},
                "ExpressionAttributeValues": {":p": key[partition].clone()},
            }),
        )
        .await;
        assert_eq!(
            queried["Count"],
            json!(0),
            "case {case}: a Query after the delete finds nothing, got {queried}"
        );

        // DELETE AGAIN: "running it multiple times on the same item ... does not
        // result in an error response", and there is nothing left to report.
        let repeated = ok(
            &router,
            "DeleteItem",
            json!({"TableName": table, "Key": key, "ReturnValues": "ALL_OLD"}),
        )
        .await;
        assert_eq!(
            repeated.get("Attributes"),
            None,
            "case {case}: deleting a key that holds no item reports no Attributes, got {repeated}"
        );
    }
}

/// Every action an `UpdateExpression` states is applied to the stored item, and the
/// result is what a later read reports. One row per action form the member's doc lists
/// (`SET` with its two functions and its arithmetic, `REMOVE`, `ADD`, `DELETE`), each
/// against its own item so the rows are independent.
#[tokio::test]
async fn update_expression_actions_apply_to_the_stored_item() {
    let router = emulator();
    create_active_table(&router, "updates", &[("pk", "HASH", "S")]).await;

    // (case, starting attributes, UpdateExpression, names, values, expected attributes)
    let cases: Vec<(&str, Value, &str, Value, Value, Value)> = vec![
        (
            "SET a literal value",
            json!({}),
            "SET a = :v",
            json!({}),
            json!({":v": {"S": "x"}}),
            json!({"a": {"S": "x"}}),
        ),
        (
            "SET through an attribute-name alias",
            json!({}),
            "SET #reserved = :v",
            json!({"#reserved": "size"}),
            json!({":v": {"N": "4"}}),
            json!({"size": {"N": "4"}}),
        ),
        (
            "SET stores the number a value denotes, not its lexeme",
            json!({}),
            "SET n = :v",
            json!({}),
            json!({":v": {"N": "1e2"}}),
            json!({"n": {"N": "100"}}),
        ),
        (
            "SET adds to a number",
            json!({"n": {"N": "1"}}),
            "SET n = n + :d",
            json!({}),
            json!({":d": {"N": "1.5"}}),
            json!({"n": {"N": "2.5"}}),
        ),
        (
            "SET subtracts from a number",
            json!({"n": {"N": "10"}}),
            "SET n = n - :d",
            json!({}),
            json!({":d": {"N": "0.5"}}),
            json!({"n": {"N": "9.5"}}),
        ),
        (
            "SET keeps 38 significant digits",
            json!({"n": {"N": "99999999999999999999999999999999999999"}}),
            "SET n = n + :d",
            json!({}),
            json!({":d": {"N": "1"}}),
            json!({"n": {"N": "100000000000000000000000000000000000000"}}),
        ),
        (
            "if_not_exists keeps an attribute that is there",
            json!({"n": {"N": "5"}}),
            "SET n = if_not_exists(n, :z)",
            json!({}),
            json!({":z": {"N": "0"}}),
            json!({"n": {"N": "5"}}),
        ),
        (
            "if_not_exists fills an attribute that is not",
            json!({}),
            "SET n = if_not_exists(n, :z)",
            json!({}),
            json!({":z": {"N": "0"}}),
            json!({"n": {"N": "0"}}),
        ),
        (
            "list_append extends a list",
            json!({"l": {"L": [{"S": "a"}]}}),
            "SET l = list_append(l, :more)",
            json!({}),
            json!({":more": {"L": [{"S": "b"}]}}),
            json!({"l": {"L": [{"S": "a"}, {"S": "b"}]}}),
        ),
        (
            "SET writes into a map through a document path",
            json!({"m": {"M": {"a": {"N": "1"}}}}),
            "SET m.b = :v",
            json!({}),
            json!({":v": {"S": "x"}}),
            json!({"m": {"M": {"a": {"N": "1"}, "b": {"S": "x"}}}}),
        ),
        (
            "SET writes into a list element",
            json!({"l": {"L": [{"S": "a"}, {"S": "b"}]}}),
            "SET l[1] = :v",
            json!({}),
            json!({":v": {"S": "z"}}),
            json!({"l": {"L": [{"S": "a"}, {"S": "z"}]}}),
        ),
        (
            "REMOVE drops an attribute",
            json!({"a": {"S": "x"}, "b": {"S": "y"}}),
            "REMOVE b",
            json!({}),
            json!({}),
            json!({"a": {"S": "x"}}),
        ),
        (
            "REMOVE drops a list element and shifts the rest",
            json!({"l": {"L": [{"S": "a"}, {"S": "b"}]}}),
            "REMOVE l[0]",
            json!({}),
            json!({}),
            json!({"l": {"L": [{"S": "b"}]}}),
        ),
        (
            "ADD starts a missing number from zero",
            json!({}),
            "ADD n :d",
            json!({}),
            json!({":d": {"N": "3"}}),
            json!({"n": {"N": "3"}}),
        ),
        (
            "ADD adds to a negative number",
            json!({"n": {"N": "-3"}}),
            "ADD n :d",
            json!({}),
            json!({":d": {"N": "5"}}),
            json!({"n": {"N": "2"}}),
        ),
        (
            "ADD unions a string set",
            json!({"s": {"SS": ["a"]}}),
            "ADD s :m",
            json!({}),
            json!({":m": {"SS": ["b"]}}),
            json!({"s": {"SS": ["a", "b"]}}),
        ),
        (
            "ADD of a member a set already holds changes nothing",
            json!({"s": {"SS": ["a"]}}),
            "ADD s :m",
            json!({}),
            json!({":m": {"SS": ["a"]}}),
            json!({"s": {"SS": ["a"]}}),
        ),
        (
            "DELETE subtracts set members",
            json!({"s": {"NS": ["1", "2", "3"]}}),
            "DELETE s :m",
            json!({}),
            json!({":m": {"NS": ["2"]}}),
            json!({"s": {"NS": ["1", "3"]}}),
        ),
        (
            "DELETE of the last member removes the set",
            json!({"s": {"BS": ["AQI="]}}),
            "DELETE s :m",
            json!({}),
            json!({":m": {"BS": ["AQI="]}}),
            json!({}),
        ),
        (
            "every section in one expression",
            json!({"n": {"N": "1"}, "gone": {"S": "x"}, "s": {"SS": ["a", "b"]}}),
            "SET a = :v REMOVE gone ADD n :d DELETE s :m",
            json!({}),
            json!({":v": {"S": "new"}, ":d": {"N": "2"}, ":m": {"SS": ["a"]}}),
            json!({"a": {"S": "new"}, "n": {"N": "3"}, "s": {"SS": ["b"]}}),
        ),
    ];

    for (index, (case, start, expression, names, values, expected)) in cases.iter().enumerate() {
        let key = json!({"pk": {"S": format!("row-{index}")}});
        ok(
            &router,
            "PutItem",
            json!({"TableName": "updates", "Item": item_with(&key, start)}),
        )
        .await;
        let body = request(
            json!({
                "TableName": "updates",
                "Key": key,
                "UpdateExpression": expression,
                "ReturnValues": "ALL_NEW",
            }),
            vec![
                ("ExpressionAttributeNames", values_member(names)),
                ("ExpressionAttributeValues", values_member(values)),
            ],
        );
        let updated = ok(&router, "UpdateItem", body).await;
        let wanted = item_with(&key, expected);
        assert_eq!(
            updated["Attributes"], wanted,
            "case {case}: ALL_NEW reports the item the update leaves, got {updated}"
        );
        let read = stored_item(&router, "updates", &key).await;
        assert_eq!(
            read, wanted,
            "case {case}: the stored item is the one the update leaves, got {read}"
        );
    }
}

/// `ReturnValues` reports the item before or after the write, whole or projected to
/// the attributes the update touched — one row per value the `ReturnValue` enum
/// declares, over one and the same update.
#[tokio::test]
async fn update_item_return_values_report_the_requested_view() {
    let router = emulator();
    create_active_table(&router, "returns", &[("pk", "HASH", "S")]).await;

    // (ReturnValues, the Attributes expected beside the key, whether the key is part
    // of the reported view)
    let cases: Vec<(&str, Value, bool)> = vec![
        ("NONE", Value::Null, false),
        (
            "ALL_OLD",
            json!({"kept": {"S": "k"}, "n": {"N": "1"}}),
            true,
        ),
        ("UPDATED_OLD", json!({"n": {"N": "1"}}), false),
        (
            "ALL_NEW",
            json!({"kept": {"S": "k"}, "n": {"N": "4"}}),
            true,
        ),
        ("UPDATED_NEW", json!({"n": {"N": "4"}}), false),
    ];

    for (case, expected, with_key) in &cases {
        let key = json!({"pk": {"S": format!("row-{case}")}});
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": "returns",
                "Item": item_with(&key, &json!({"kept": {"S": "k"}, "n": {"N": "1"}})),
            }),
        )
        .await;
        let updated = ok(
            &router,
            "UpdateItem",
            json!({
                "TableName": "returns",
                "Key": key,
                "UpdateExpression": "SET n = :n",
                "ExpressionAttributeValues": {":n": {"N": "4"}},
                "ReturnValues": case,
            }),
        )
        .await;
        let wanted = if expected.is_null() {
            Value::Null
        } else if *with_key {
            item_with(&key, expected)
        } else {
            expected.clone()
        };
        let reported = updated.get("Attributes").cloned().unwrap_or(Value::Null);
        assert_eq!(
            reported, wanted,
            "case ReturnValues={case}: expected {wanted}, got {updated}"
        );
    }
}

/// A conditional write runs only when its condition holds for the item as it stood
/// before the write: when it does, the mutation is in the store; when it does not, the
/// request is refused with `ConditionalCheckFailedException` and the store is
/// UNCHANGED. The rows span the condition forms — the expression language's operators
/// and functions, and the legacy `Expected` map with its `ConditionalOperator`.
#[tokio::test]
async fn conditional_writes_gate_the_mutation_on_the_stored_item() {
    let router = emulator();
    create_active_table(&router, "conditions", &[("pk", "HASH", "S")]).await;
    let start = json!({
        "ver": {"N": "2"},
        "name": {"S": "widget"},
        "tags": {"SS": ["red", "blue"]},
    });

    // (case, the condition members to add to an UpdateItem, whether it may proceed)
    let cases: Vec<ConditionCase> = vec![
        (
            "attribute_exists on an attribute that is there",
            vec![("ConditionExpression", Some(json!("attribute_exists(#n)")))],
            true,
        ),
        (
            "attribute_exists on an attribute that is not",
            vec![("ConditionExpression", Some(json!("attribute_exists(absent)")))],
            false,
        ),
        (
            "attribute_not_exists on an attribute that is not there",
            vec![(
                "ConditionExpression",
                Some(json!("attribute_not_exists(absent)")),
            )],
            true,
        ),
        (
            "equality on a number that matches",
            vec![("ConditionExpression", Some(json!("ver = :two")))],
            true,
        ),
        (
            "equality on a number that does not match",
            vec![("ConditionExpression", Some(json!("ver = :nine")))],
            false,
        ),
        (
            "inequality",
            vec![("ConditionExpression", Some(json!("ver <> :nine")))],
            true,
        ),
        (
            "ordering compares numbers numerically",
            vec![("ConditionExpression", Some(json!("ver < :ten")))],
            true,
        ),
        (
            "ordering that does not hold",
            vec![("ConditionExpression", Some(json!("ver >= :ten")))],
            false,
        ),
        (
            "BETWEEN inside its bounds",
            vec![(
                "ConditionExpression",
                Some(json!("ver BETWEEN :one AND :ten")),
            )],
            true,
        ),
        (
            "BETWEEN outside its bounds",
            vec![(
                "ConditionExpression",
                Some(json!("ver BETWEEN :nine AND :ten")),
            )],
            false,
        ),
        (
            "IN a list of candidates",
            vec![(
                "ConditionExpression",
                Some(json!("ver IN (:one, :two, :nine)")),
            )],
            true,
        ),
        (
            "begins_with on a string",
            vec![("ConditionExpression", Some(json!("begins_with(#n, :wid)")))],
            true,
        ),
        (
            "contains a set member",
            vec![("ConditionExpression", Some(json!("contains(tags, :red)")))],
            true,
        ),
        (
            "contains a member the set does not hold",
            vec![("ConditionExpression", Some(json!("contains(tags, :wid)")))],
            false,
        ),
        (
            "attribute_type names the stored type",
            vec![(
                "ConditionExpression",
                Some(json!("attribute_type(#n, :string)")),
            )],
            true,
        ),
        (
            "size of a set",
            vec![("ConditionExpression", Some(json!("size(tags) = :two")))],
            true,
        ),
        (
            "AND of two conditions that both hold",
            vec![(
                "ConditionExpression",
                Some(json!("attribute_exists(#n) AND ver = :two")),
            )],
            true,
        ),
        (
            "AND where one does not hold",
            vec![(
                "ConditionExpression",
                Some(json!("attribute_exists(#n) AND ver = :nine")),
            )],
            false,
        ),
        (
            "OR where one holds",
            vec![(
                "ConditionExpression",
                Some(json!("ver = :nine OR ver = :two")),
            )],
            true,
        ),
        (
            "NOT of a condition that does not hold",
            vec![("ConditionExpression", Some(json!("NOT ver = :nine")))],
            true,
        ),
        (
            "parentheses group the operators",
            vec![(
                "ConditionExpression",
                Some(json!("(ver = :nine OR ver = :two) AND attribute_exists(tags)")),
            )],
            true,
        ),
        (
            "legacy Expected Value that matches",
            vec![("Expected", Some(json!({"ver": {"Value": {"N": "2"}}})))],
            true,
        ),
        (
            "legacy Expected Value that does not match",
            vec![("Expected", Some(json!({"ver": {"Value": {"N": "9"}}})))],
            false,
        ),
        (
            "legacy Expected Exists false on an attribute that is there",
            vec![("Expected", Some(json!({"ver": {"Exists": false}})))],
            false,
        ),
        (
            "legacy Expected Exists false on an attribute that is not",
            vec![("Expected", Some(json!({"absent": {"Exists": false}})))],
            true,
        ),
        (
            "legacy Expected comparison operator",
            vec![(
                "Expected",
                Some(json!({"ver": {
                    "ComparisonOperator": "LT",
                    "AttributeValueList": [{"N": "10"}],
                }})),
            )],
            true,
        ),
        (
            "legacy Expected NOT_NULL takes no operand",
            vec![(
                "Expected",
                Some(json!({"name": {"ComparisonOperator": "NOT_NULL"}})),
            )],
            true,
        ),
        (
            "legacy ConditionalOperator OR where one entry holds",
            vec![
                (
                    "Expected",
                    Some(json!({
                        "ver": {"Value": {"N": "9"}},
                        "name": {"Value": {"S": "widget"}},
                    })),
                ),
                ("ConditionalOperator", Some(json!("OR"))),
            ],
            true,
        ),
        (
            "legacy ConditionalOperator AND where one entry does not hold",
            vec![
                (
                    "Expected",
                    Some(json!({
                        "ver": {"Value": {"N": "9"}},
                        "name": {"Value": {"S": "widget"}},
                    })),
                ),
                ("ConditionalOperator", Some(json!("AND"))),
            ],
            false,
        ),
    ];

    for (index, (case, condition, may_proceed)) in cases.iter().enumerate() {
        let key = json!({"pk": {"S": format!("row-{index}")}});
        let stored = item_with(&key, &start);
        ok(
            &router,
            "PutItem",
            json!({"TableName": "conditions", "Item": stored}),
        )
        .await;
        // Every token a request DEFINES has to be one its expressions USE, so each row
        // sends the slice of the value vocabulary its own condition reads (beside the
        // one the update reads): an `ExpressionAttributeValues` entry no expression
        // references makes the whole request invalid.
        const UPDATE: &str = "SET #n = :touched";
        let vocabulary = json!({
            ":touched": {"S": "touched"},
            ":one": {"N": "1"},
            ":two": {"N": "2"},
            ":nine": {"N": "9"},
            ":ten": {"N": "10"},
            ":wid": {"S": "wid"},
            ":red": {"S": "red"},
            ":string": {"S": "S"},
        });
        let stated = serde_json::to_string(condition).expect("the row's members render");
        // A request states its intent through the expression members or through the
        // legacy ones they replaced, never through both, so each row's update is
        // spelled in the generation its own condition belongs to: the same write — the
        // `name` attribute set to "touched" — either way.
        let legacy_condition =
            stated.contains("Expected") || stated.contains("ConditionalOperator");
        let body = request(
            if legacy_condition {
                json!({
                    "TableName": "conditions",
                    "Key": key,
                    "AttributeUpdates": {"name": {"Action": "PUT",
                                                  "Value": {"S": "touched"}}},
                })
            } else {
                let values: Map<String, Value> = vocabulary
                    .as_object()
                    .expect("the vocabulary is a map")
                    .iter()
                    .filter(|(token, _)| {
                        UPDATE.contains(token.as_str()) || stated.contains(token.as_str())
                    })
                    .map(|(token, value)| (token.clone(), value.clone()))
                    .collect();
                json!({
                    "TableName": "conditions",
                    "Key": key,
                    "UpdateExpression": UPDATE,
                    "ExpressionAttributeNames": {"#n": "name"},
                    "ExpressionAttributeValues": values,
                })
            },
            condition.clone(),
        );
        let (status, parsed) = call(&router, "UpdateItem", body.clone()).await;
        let read = stored_item(&router, "conditions", &key).await;
        if *may_proceed {
            assert_eq!(
                status, 200,
                "case {case}: the condition holds, so the update runs, got {status} with {parsed}"
            );
            assert_eq!(
                read,
                item_with(&key, &{
                    let mut touched = start.clone();
                    touched["name"] = json!({"S": "touched"});
                    touched
                }),
                "case {case}: the update landed, got {read}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: the condition does not hold, so the write is refused, got {status} \
                 with {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ConditionalCheckFailedException",
                "case {case}: an unmet condition is a ConditionalCheckFailedException, got \
                 {parsed}"
            );
            assert_eq!(
                read, stored,
                "case {case}: a refused write leaves the stored item alone, got {read}"
            );
        }
    }
}

/// A conditional `PutItem` and `DeleteItem` are gated the same way `UpdateItem` is,
/// and `ReturnValuesOnConditionCheckFailure=ALL_OLD` carries the item the condition
/// was read against.
#[tokio::test]
async fn conditional_put_and_delete_are_gated_and_report_the_blocking_item() {
    let router = emulator();
    create_active_table(&router, "gated", &[("pk", "HASH", "S")]).await;
    let key = json!({"pk": {"S": "k1"}});
    let stored = item_with(&key, &json!({"ver": {"N": "1"}}));
    ok(
        &router,
        "PutItem",
        json!({"TableName": "gated", "Item": stored}),
    )
    .await;

    // A put guarded by attribute_not_exists cannot overwrite an item that is there,
    // and the refusal carries the item that blocked it.
    let (status, parsed) = call(
        &router,
        "PutItem",
        json!({
            "TableName": "gated",
            "Item": item_with(&key, &json!({"ver": {"N": "2"}})),
            "ConditionExpression": "attribute_not_exists(pk)",
            "ReturnValuesOnConditionCheckFailure": "ALL_OLD",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case guarded put: the item exists, so the put is refused, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ConditionalCheckFailedException",
        "case guarded put: expected a ConditionalCheckFailedException, got {parsed}"
    );
    assert_eq!(
        parsed["Item"], stored,
        "case guarded put: ALL_OLD on failure carries the blocking item, got {parsed}"
    );
    let read = stored_item(&router, "gated", &key).await;
    assert_eq!(
        read, stored,
        "case guarded put: the refused put changed nothing, got {read}"
    );

    // A delete guarded by a condition that does not hold leaves the item.
    let (status, parsed) = call(
        &router,
        "DeleteItem",
        json!({
            "TableName": "gated",
            "Key": key,
            "ConditionExpression": "ver = :nine",
            "ExpressionAttributeValues": {":nine": {"N": "9"}},
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case guarded delete: the condition fails, so nothing is deleted, got {status} with \
         {parsed}"
    );
    assert_eq!(
        parsed.get("Item"),
        None,
        "case guarded delete: no ReturnValuesOnConditionCheckFailure was asked for, got {parsed}"
    );
    let read = stored_item(&router, "gated", &key).await;
    assert_eq!(
        read, stored,
        "case guarded delete: the item is still there, got {read}"
    );

    // The same delete, with a condition that holds, removes it.
    let deleted = ok(
        &router,
        "DeleteItem",
        json!({
            "TableName": "gated",
            "Key": key,
            "ConditionExpression": "ver = :one",
            "ExpressionAttributeValues": {":one": {"N": "1"}},
            "ReturnValues": "ALL_OLD",
        }),
    )
    .await;
    assert_eq!(
        deleted["Attributes"], stored,
        "case met delete condition: ALL_OLD reports the removed item, got {deleted}"
    );
    let read = stored_item(&router, "gated", &key).await;
    assert_eq!(
        read,
        Value::Null,
        "case met delete condition: the item is gone, got {read}"
    );

    // And a put guarded by attribute_not_exists now succeeds on the free key.
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": "gated",
            "Item": item_with(&key, &json!({"ver": {"N": "3"}})),
            "ConditionExpression": "attribute_not_exists(pk)",
        }),
    )
    .await;
    let read = stored_item(&router, "gated", &key).await;
    assert_eq!(
        read,
        item_with(&key, &json!({"ver": {"N": "3"}})),
        "case free key: the guarded put stored its item, got {read}"
    );
}

/// The legacy `AttributeUpdates` map states the same four actions, one per attribute,
/// `PUT` by default — and an `UpdateItem` of a key that holds no item creates it.
#[tokio::test]
async fn legacy_attribute_updates_apply_the_same_actions() {
    let router = emulator();
    create_active_table(&router, "legacy", &[("pk", "HASH", "S")]).await;

    // (case, starting attributes, AttributeUpdates, expected attributes)
    let cases: Vec<(&str, Option<Value>, Value, Value)> = vec![
        (
            "PUT is the default action",
            Some(json!({"a": {"S": "old"}})),
            json!({"a": {"Value": {"S": "new"}}}),
            json!({"a": {"S": "new"}}),
        ),
        (
            "an explicit PUT replaces the attribute",
            Some(json!({"a": {"S": "old"}})),
            json!({"a": {"Action": "PUT", "Value": {"S": "new"}}}),
            json!({"a": {"S": "new"}}),
        ),
        (
            "DELETE without a value removes the attribute",
            Some(json!({"a": {"S": "old"}, "b": {"N": "1"}})),
            json!({"a": {"Action": "DELETE"}}),
            json!({"b": {"N": "1"}}),
        ),
        (
            "DELETE with a set value subtracts its members",
            Some(json!({"s": {"SS": ["a", "b"]}})),
            json!({"s": {"Action": "DELETE", "Value": {"SS": ["a"]}}}),
            json!({"s": {"SS": ["b"]}}),
        ),
        (
            "ADD adds to a number",
            Some(json!({"n": {"N": "1"}})),
            json!({"n": {"Action": "ADD", "Value": {"N": "2"}}}),
            json!({"n": {"N": "3"}}),
        ),
        (
            "ADD on a missing attribute starts from zero",
            Some(json!({})),
            json!({"n": {"Action": "ADD", "Value": {"N": "2"}}}),
            json!({"n": {"N": "2"}}),
        ),
        (
            "an update of a key that holds no item creates it",
            None,
            json!({"a": {"Value": {"S": "created"}}}),
            json!({"a": {"S": "created"}}),
        ),
        (
            "two attributes in one request",
            Some(json!({"a": {"S": "old"}, "n": {"N": "1"}})),
            json!({
                "a": {"Value": {"S": "new"}},
                "n": {"Action": "ADD", "Value": {"N": "1"}},
            }),
            json!({"a": {"S": "new"}, "n": {"N": "2"}}),
        ),
    ];

    for (index, (case, start, updates, expected)) in cases.iter().enumerate() {
        let key = json!({"pk": {"S": format!("row-{index}")}});
        if let Some(start) = start {
            ok(
                &router,
                "PutItem",
                json!({"TableName": "legacy", "Item": item_with(&key, start)}),
            )
            .await;
        }
        ok(
            &router,
            "UpdateItem",
            json!({
                "TableName": "legacy",
                "Key": key,
                "AttributeUpdates": updates,
            }),
        )
        .await;
        let read = stored_item(&router, "legacy", &key).await;
        assert_eq!(
            read,
            item_with(&key, expected),
            "case {case}: the stored item is the one the legacy update leaves, got {read}"
        );
    }
}

/// A value an update COMPUTES is held to the same bounds as one a request sends: a
/// `SET` whose result would not fit the documented 400 KB item cannot land, whether the
/// result is copied from another attribute or built by `list_append`.
#[tokio::test]
async fn a_computed_value_is_held_to_the_item_bounds() {
    let router = emulator();
    create_active_table(&router, "computed", &[("pk", "HASH", "S")]).await;
    let key = json!({"pk": {"S": "k1"}});
    // Two thirds of the documented 400 KB limit, so the item fits but a second copy of
    // the attribute cannot.
    let bulk = "x".repeat(270 * 1024);
    let stored = item_with(&key, &json!({"bulk": {"S": bulk}, "list": {"L": [{"N": "1"}]}}));
    ok(
        &router,
        "PutItem",
        json!({"TableName": "computed", "Item": stored}),
    )
    .await;

    // (case, UpdateExpression, ExpressionAttributeValues)
    let cases: Vec<(&str, &str, Option<Value>)> = vec![
        ("a copy of an attribute that would not fit", "SET copy = bulk", None),
        (
            "a list_append whose result would not fit",
            "SET list = list_append(list, :bulk)",
            Some(json!({":bulk": {"L": [{"S": "x".repeat(270 * 1024)}]}})),
        ),
    ];

    for (case, expression, values) in &cases {
        let body = request(
            json!({
                "TableName": "computed",
                "Key": key,
                "UpdateExpression": expression,
            }),
            vec![("ExpressionAttributeValues", values.clone())],
        );
        let (status, parsed) = call(&router, "UpdateItem", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case}: the computed item does not fit, so the write is refused, got {status}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: expected a ValidationException, got {parsed}"
        );
        let read = stored_item(&router, "computed", &key).await;
        assert_eq!(
            read.get("copy"),
            None,
            "case {case}: the refused update stored nothing"
        );
        assert_eq!(
            read["list"],
            json!({"L": [{"N": "1"}]}),
            "case {case}: the refused update left the list alone"
        );
    }
}

/// A write is authorized before it lands: a table whose resource-based policy denies
/// the caller `dynamodb:UpdateItem` is not updated at all, and the stored item is left
/// exactly as it was — the same gate `PutItem`, `DeleteItem`, `GetItem` and `Query`
/// apply, read from the same stored policy.
#[tokio::test]
async fn an_update_a_resource_policy_denies_does_not_land() {
    let router = emulator();
    create_active_table(&router, "policy-gated", &[("pk", "HASH", "S")]).await;
    let key = json!({"pk": {"S": "k1"}});
    let stored = item_with(&key, &json!({"n": {"N": "1"}}));
    ok(
        &router,
        "PutItem",
        json!({"TableName": "policy-gated", "Item": stored}),
    )
    .await;

    let arn = format!("arn:aws:dynamodb:us-east-1:000000000000:table/{}", "policy-gated");
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": {"AWS": "*"},
            "Action": "dynamodb:UpdateItem",
            "Resource": arn,
        }],
    })
    .to_string();
    ok(
        &router,
        "PutResourcePolicy",
        json!({"ResourceArn": arn, "Policy": document}),
    )
    .await;
    // "PutResourcePolicy is an asynchronous operation", so the data plane is asked only
    // once the policy's application is due.
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));

    let (status, parsed) = call(
        &router,
        "UpdateItem",
        json!({
            "TableName": "policy-gated",
            "Key": key,
            "UpdateExpression": "SET n = :n",
            "ExpressionAttributeValues": {":n": {"N": "2"}},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "AccessDeniedException"),
        "case denied update: expected 400 AccessDeniedException, got {status} with {parsed}"
    );
    let read = stored_item(&router, "policy-gated", &key).await;
    assert_eq!(
        read, stored,
        "case denied update: the refused update left the stored item alone, got {read}"
    );
}

/// A request that cannot be applied is refused with the modelled client error rather
/// than half-applied: one row per way the write path can be asked for the impossible.
#[tokio::test]
async fn invalid_write_requests_are_refused() {
    let router = emulator();
    create_active_table(&router, "refusals", &[("pk", "HASH", "S")]).await;
    let key = json!({"pk": {"S": "k1"}});
    let stored = item_with(&key, &json!({"n": {"N": "1"}, "text": {"S": "x"}}));
    ok(
        &router,
        "PutItem",
        json!({"TableName": "refusals", "Item": stored}),
    )
    .await;
    // "Maximum length of any expression string is 4 KB" ("Service, Account, and Table
    // Quotas"): one byte past it is refused.
    let oversized = format!("SET a = :v, b = :v{}", " ".repeat(4096));
    // One action past the most an update may state, which the legacy map is held to
    // because it declares no length of its own.
    let too_many: Value = Value::Object(
        (0..=MAX_UPDATE_ACTIONS)
            .map(|index| {
                (
                    format!("a{index}"),
                    json!({"Value": {"N": index.to_string()}}),
                )
            })
            .collect(),
    );

    // (case, operation, the members to add beside TableName/Key)
    let cases: Vec<RefusalCase> = vec![
        (
            "a key attribute cannot be updated",
            "UpdateItem",
            vec![
                ("UpdateExpression", Some(json!("SET pk = :v"))),
                (
                    "ExpressionAttributeValues",
                    Some(json!({":v": {"S": "other"}})),
                ),
            ],
        ),
        (
            "UpdateExpression and AttributeUpdates cannot both be used",
            "UpdateItem",
            vec![
                ("UpdateExpression", Some(json!("SET a = :v"))),
                (
                    "ExpressionAttributeValues",
                    Some(json!({":v": {"S": "x"}})),
                ),
                ("AttributeUpdates", Some(json!({"b": {"Value": {"S": "y"}}}))),
            ],
        ),
        (
            "ConditionExpression and Expected cannot both be used",
            "UpdateItem",
            vec![
                ("UpdateExpression", Some(json!("REMOVE text"))),
                ("ConditionExpression", Some(json!("attribute_exists(n)"))),
                ("Expected", Some(json!({"n": {"Value": {"N": "1"}}}))),
            ],
        ),
        (
            "an undefined expression attribute value",
            "UpdateItem",
            vec![("UpdateExpression", Some(json!("SET a = :missing")))],
        ),
        (
            "an undefined expression attribute name",
            "UpdateItem",
            vec![
                ("UpdateExpression", Some(json!("SET #missing = :v"))),
                (
                    "ExpressionAttributeValues",
                    Some(json!({":v": {"S": "x"}})),
                ),
            ],
        ),
        (
            "an expression longer than 4 KB",
            "UpdateItem",
            vec![
                ("UpdateExpression", Some(json!(oversized))),
                (
                    "ExpressionAttributeValues",
                    Some(json!({":v": {"S": "x"}})),
                ),
            ],
        ),
        (
            "two actions writing the same path",
            "UpdateItem",
            vec![
                ("UpdateExpression", Some(json!("SET a = :v REMOVE a"))),
                (
                    "ExpressionAttributeValues",
                    Some(json!({":v": {"S": "x"}})),
                ),
            ],
        ),
        (
            "a syntax error in the update expression",
            "UpdateItem",
            vec![("UpdateExpression", Some(json!("SET a =")))],
        ),
        (
            "two actions where one path is inside the other",
            "UpdateItem",
            vec![
                ("UpdateExpression", Some(json!("SET m = :v, m.b = :v"))),
                (
                    "ExpressionAttributeValues",
                    Some(json!({":v": {"S": "x"}})),
                ),
            ],
        ),
        (
            "more actions than one update may state",
            "UpdateItem",
            vec![("AttributeUpdates", Some(too_many))],
        ),
        (
            "ADD on a string attribute",
            "UpdateItem",
            vec![
                ("UpdateExpression", Some(json!("ADD text :v"))),
                (
                    "ExpressionAttributeValues",
                    Some(json!({":v": {"N": "1"}})),
                ),
            ],
        ),
        (
            "DELETE on a number attribute",
            "UpdateItem",
            vec![
                ("UpdateExpression", Some(json!("DELETE n :v"))),
                (
                    "ExpressionAttributeValues",
                    Some(json!({":v": {"SS": ["a"]}})),
                ),
            ],
        ),
        (
            "arithmetic on a string attribute",
            "UpdateItem",
            vec![
                ("UpdateExpression", Some(json!("SET text = text + :v"))),
                (
                    "ExpressionAttributeValues",
                    Some(json!({":v": {"N": "1"}})),
                ),
            ],
        ),
        (
            "an ADD of an empty set",
            "UpdateItem",
            vec![
                ("UpdateExpression", Some(json!("ADD s :v"))),
                ("ExpressionAttributeValues", Some(json!({":v": {"SS": []}}))),
            ],
        ),
        (
            "a SET from an attribute that does not exist",
            "UpdateItem",
            vec![("UpdateExpression", Some(json!("SET a = absent")))],
        ),
        (
            "ALL_NEW is not a ReturnValues an UpdateItem's writes reject, but a put's are",
            "PutItem",
            vec![
                ("Item", Some(stored.clone())),
                ("ReturnValues", Some(json!("ALL_NEW"))),
            ],
        ),
        (
            "ALL_NEW on a delete",
            "DeleteItem",
            vec![("ReturnValues", Some(json!("ALL_NEW")))],
        ),
        (
            "an Expected entry mixing Value with ComparisonOperator",
            "DeleteItem",
            vec![(
                "Expected",
                Some(json!({"n": {
                    "Value": {"N": "1"},
                    "ComparisonOperator": "EQ",
                    "AttributeValueList": [{"N": "1"}],
                }})),
            )],
        ),
        (
            "an Expected comparison with the wrong number of operands",
            "DeleteItem",
            vec![(
                "Expected",
                Some(json!({"n": {
                    "ComparisonOperator": "BETWEEN",
                    "AttributeValueList": [{"N": "1"}],
                }})),
            )],
        ),
        (
            "a ConditionalOperator with no Expected map",
            "DeleteItem",
            vec![("ConditionalOperator", Some(json!("OR")))],
        ),
    ];

    for (case, operation, members) in &cases {
        let body = request(
            json!({"TableName": "refusals", "Key": key}),
            members.clone(),
        );
        let (status, parsed) = call(&router, operation, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the request to be refused with HTTP 400, got {status} with \
             {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: expected a ValidationException, got {parsed}"
        );
        let read = stored_item(&router, "refusals", &key).await;
        assert_eq!(
            read, stored,
            "case {case}: a refused request leaves the stored item alone, got {read}"
        );
    }

    // Nothing above left a stray attribute behind: the item is still the one the put
    // wrote, attribute for attribute.
    let read = stored_item(&router, "refusals", &key).await;
    let attributes: &Map<String, Value> = read.as_object().expect("the item is an object");
    assert_eq!(
        attributes.len(),
        3,
        "case refusals: the item still holds exactly its key and two attributes, got {read}"
    );
}
