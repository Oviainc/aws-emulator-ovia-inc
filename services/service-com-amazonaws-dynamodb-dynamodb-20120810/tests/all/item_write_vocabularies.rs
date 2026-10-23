// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the vocabularies a DynamoDB write can be stated in.
//!
//! One item store is reachable through four of them: `UpdateExpression`, the legacy
//! `AttributeUpdates`/`Expected` parameters, `ConditionExpression` and PartiQL. The
//! rule these tests assert is that EVERY one of them mutates and gates that store the
//! same way — so each table states a mutation (or a condition) in two vocabularies and
//! compares what the store reports afterwards, rather than checking one recorded
//! example.
//!
//! The rows differ along what the rule generalises over: the action
//! (`PUT`/`ADD`/`DELETE`, `SET`/`REMOVE`), the value type the action acts on (number,
//! string, the three set types, list, map), the comparison operator a condition uses,
//! and the statement form PartiQL writes it in. The limits, bounds and refusal classes
//! each write path is held to are the subject of this crate's other suites
//! (`item_write_expressions`, `partiql_statements`, `batch_and_transact_items`,
//! `table_items`), not of this one.

use serde_json::{json, Value};

use crate::table_items::{call, create_active_table, emulator, ok};
// The refusal-code reader is the harness's, so there is one implementation of it.
use crate::emulator::error_code;
use smithy_server_runtime::MultiServiceRouter;

/// One mutation, stated BOTH ways: as the legacy `AttributeUpdates` map and as the
/// `UpdateExpression` the docs call its equivalent.
struct Mutation {
    case: &'static str,
    /// What the item holds before the update, beside its key.
    before: Value,
    /// The legacy `AttributeUpdates` map.
    legacy: Value,
    /// The same mutation as an expression, with its substitution values.
    expression: &'static str,
    values: Value,
    /// What the item holds after it, beside its key.
    after: Value,
}

/// `AttributeUpdates` is "the legacy equivalent of UpdateExpression": each action
/// mutates the stored item, and the two vocabularies leave the SAME item.
#[tokio::test]
async fn legacy_attribute_updates_and_update_expressions_leave_the_same_item() {
    // The rows span the three legacy actions and the value types each acts on: a
    // number, the three set types, a scalar, a document, and an attribute that is not
    // there yet (`AttributeValueUpdate$Action`).
    let mutations = vec![
        Mutation {
            case: "ADD adds to a number",
            before: json!({"counter": {"N": "10"}}),
            legacy: json!({"counter": {"Action": "ADD", "Value": {"N": "5"}}}),
            expression: "ADD counter :operand",
            values: json!({":operand": {"N": "5"}}),
            after: json!({"counter": {"N": "15"}}),
        },
        Mutation {
            case: "ADD of a negative number subtracts",
            before: json!({"counter": {"N": "10"}}),
            legacy: json!({"counter": {"Action": "ADD", "Value": {"N": "-4.5"}}}),
            expression: "ADD counter :operand",
            values: json!({":operand": {"N": "-4.5"}}),
            after: json!({"counter": {"N": "5.5"}}),
        },
        Mutation {
            case: "ADD on an attribute that does not exist stores the value",
            before: json!({}),
            legacy: json!({"counter": {"Action": "ADD", "Value": {"N": "7"}}}),
            expression: "ADD counter :operand",
            values: json!({":operand": {"N": "7"}}),
            after: json!({"counter": {"N": "7"}}),
        },
        Mutation {
            case: "ADD unions a string set, keeping the stored order",
            before: json!({"tags": {"SS": ["x", "b"]}}),
            legacy: json!({"tags": {"Action": "ADD", "Value": {"SS": ["c", "d"]}}}),
            expression: "ADD tags :operand",
            values: json!({":operand": {"SS": ["c", "d"]}}),
            after: json!({"tags": {"SS": ["x", "b", "c", "d"]}}),
        },
        Mutation {
            case: "ADD of a member a string set already holds changes nothing",
            before: json!({"tags": {"SS": ["x", "b"]}}),
            legacy: json!({"tags": {"Action": "ADD", "Value": {"SS": ["b"]}}}),
            expression: "ADD tags :operand",
            values: json!({":operand": {"SS": ["b"]}}),
            after: json!({"tags": {"SS": ["x", "b"]}}),
        },
        Mutation {
            case: "ADD unions a number set numerically",
            before: json!({"sizes": {"NS": ["1", "20"]}}),
            legacy: json!({"sizes": {"Action": "ADD", "Value": {"NS": ["3", "2e1"]}}}),
            expression: "ADD sizes :operand",
            values: json!({":operand": {"NS": ["3", "2e1"]}}),
            after: json!({"sizes": {"NS": ["1", "20", "3"]}}),
        },
        Mutation {
            case: "ADD unions a binary set",
            before: json!({"prints": {"BS": ["AQI="]}}),
            legacy: json!({"prints": {"Action": "ADD", "Value": {"BS": ["AwQ="]}}}),
            expression: "ADD prints :operand",
            values: json!({":operand": {"BS": ["AwQ="]}}),
            after: json!({"prints": {"BS": ["AQI=", "AwQ="]}}),
        },
        Mutation {
            case: "PUT overwrites a scalar",
            before: json!({"data": {"S": "before"}}),
            legacy: json!({"data": {"Action": "PUT", "Value": {"S": "after"}}}),
            expression: "SET data = :operand",
            values: json!({":operand": {"S": "after"}}),
            after: json!({"data": {"S": "after"}}),
        },
        Mutation {
            case: "PUT with no Action named defaults to PUT and creates the attribute",
            before: json!({}),
            legacy: json!({"defaulted": {"Value": {"S": "written"}}}),
            expression: "SET defaulted = :operand",
            values: json!({":operand": {"S": "written"}}),
            after: json!({"defaulted": {"S": "written"}}),
        },
        Mutation {
            case: "PUT stores a document value",
            before: json!({}),
            legacy: json!({
                "meta": {"Action": "PUT", "Value": {"M": {"label": {"S": "indie"}, "tracks": {"N": "12"}}}}
            }),
            expression: "SET meta = :operand",
            values: json!({":operand": {"M": {"label": {"S": "indie"}, "tracks": {"N": "12"}}}}),
            after: json!({"meta": {"M": {"label": {"S": "indie"}, "tracks": {"N": "12"}}}}),
        },
        Mutation {
            case: "PUT stores a list value",
            before: json!({"credits": {"L": [{"S": "vocals"}]}}),
            legacy: json!({"credits": {"Action": "PUT", "Value": {"L": [{"S": "bass"}, {"N": "3"}]}}}),
            expression: "SET credits = :operand",
            values: json!({":operand": {"L": [{"S": "bass"}, {"N": "3"}]}}),
            after: json!({"credits": {"L": [{"S": "bass"}, {"N": "3"}]}}),
        },
        Mutation {
            case: "DELETE subtracts the members it names",
            before: json!({"tags": {"SS": ["x", "b", "c"]}}),
            legacy: json!({"tags": {"Action": "DELETE", "Value": {"SS": ["b"]}}}),
            expression: "DELETE tags :operand",
            values: json!({":operand": {"SS": ["b"]}}),
            after: json!({"tags": {"SS": ["x", "c"]}}),
        },
        Mutation {
            case: "DELETE of every member removes the attribute",
            before: json!({"tags": {"SS": ["b"]}}),
            legacy: json!({"tags": {"Action": "DELETE", "Value": {"SS": ["b"]}}}),
            expression: "DELETE tags :operand",
            values: json!({":operand": {"SS": ["b"]}}),
            after: json!({}),
        },
        Mutation {
            case: "DELETE with no value removes the attribute, as REMOVE does",
            before: json!({"data": {"S": "before"}, "kept": {"S": "yes"}}),
            legacy: json!({"data": {"Action": "DELETE"}}),
            expression: "REMOVE data",
            values: json!({}),
            after: json!({"kept": {"S": "yes"}}),
        },
    ];

    let router = emulator();
    create_active_table(&router, "Writes", &[("pk", "HASH", "S")]).await;
    for mutation in &mutations {
        for (vocabulary, key) in [("legacy", "legacy"), ("expression", "expression")] {
            let mut item = mutation.before.clone();
            item["pk"] = json!({"S": format!("{key}-{}", mutation.case)});
            ok(
                &router,
                "PutItem",
                json!({"TableName": "Writes", "Item": item}),
            )
            .await;
            let update = if vocabulary == "legacy" {
                json!({
                    "TableName": "Writes",
                    "Key": {"pk": item["pk"].clone()},
                    "AttributeUpdates": mutation.legacy.clone(),
                })
            } else {
                let mut request = json!({
                    "TableName": "Writes",
                    "Key": {"pk": item["pk"].clone()},
                    "UpdateExpression": mutation.expression,
                });
                if mutation.values != json!({}) {
                    request["ExpressionAttributeValues"] = mutation.values.clone();
                }
                request
            };
            ok(&router, "UpdateItem", update).await;
            let mut expected = mutation.after.clone();
            expected["pk"] = item["pk"].clone();
            let read = ok(
                &router,
                "GetItem",
                json!({"TableName": "Writes", "Key": {"pk": item["pk"].clone()}}),
            )
            .await;
            assert_eq!(
                read["Item"], expected,
                "case {:?} through {vocabulary}: expected the item to read {expected}, got {}",
                mutation.case, read["Item"]
            );
        }
    }
}

/// `SET` reads the item it updates: arithmetic, `if_not_exists` and `list_append` all
/// compute from what is stored, and a document path writes inside a value.
#[tokio::test]
async fn an_update_expression_computes_from_the_stored_item() {
    let router = emulator();
    create_active_table(&router, "Computed", &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Computed", "Item": {
            "pk": {"S": "one"},
            "counter": {"N": "10"},
            "credits": {"L": [{"S": "vocals"}]},
            "meta": {"M": {"label": {"S": "indie"}}}
        }}),
    )
    .await;
    // One row per operand form `UpdateItem$UpdateExpression` documents.
    let cases = vec![
        (
            "a number plus a value",
            "SET counter = counter + :one",
            json!({":one": {"N": "1"}}),
            json!({"counter": {"N": "11"}}),
        ),
        (
            "a number minus a value",
            "SET counter = counter - :two",
            json!({":two": {"N": "2"}}),
            json!({"counter": {"N": "8"}}),
        ),
        (
            "if_not_exists keeps the stored value",
            "SET counter = if_not_exists(counter, :fallback)",
            json!({":fallback": {"N": "99"}}),
            json!({"counter": {"N": "10"}}),
        ),
        (
            "if_not_exists falls back for an absent path",
            "SET fresh = if_not_exists(missing, :fallback)",
            json!({":fallback": {"N": "99"}}),
            json!({"fresh": {"N": "99"}}),
        ),
        (
            "list_append extends a list",
            "SET credits = list_append(credits, :more)",
            json!({":more": {"L": [{"S": "bass"}]}}),
            json!({"credits": {"L": [{"S": "vocals"}, {"S": "bass"}]}}),
        ),
        (
            "a document path writes inside a map",
            "SET meta.tracks = :tracks",
            json!({":tracks": {"N": "12"}}),
            json!({"meta": {"M": {"label": {"S": "indie"}, "tracks": {"N": "12"}}}}),
        ),
        (
            "an #alias stands for a reserved attribute name",
            "SET #size = :size",
            json!({":size": {"N": "3"}}),
            json!({"size": {"N": "3"}}),
        ),
    ];
    for (case, expression, values, expected) in cases {
        let mut request = json!({
            "TableName": "Computed",
            "Key": {"pk": {"S": "one"}},
            "UpdateExpression": expression,
            "ExpressionAttributeValues": values,
            "ReturnValues": "UPDATED_NEW",
        });
        if expression.contains("#size") {
            request["ExpressionAttributeNames"] = json!({"#size": "size"});
        }
        let updated = ok(&router, "UpdateItem", request).await;
        for (attribute, value) in expected.as_object().expect("the expected attributes") {
            assert_eq!(
                updated["Attributes"][attribute], *value,
                "case {case:?}: expected UPDATED_NEW to report {attribute}={value}, got {}",
                updated["Attributes"]
            );
            let read = ok(
                &router,
                "GetItem",
                json!({"TableName": "Computed", "Key": {"pk": {"S": "one"}}}),
            )
            .await;
            assert_eq!(
                read["Item"][attribute], *value,
                "case {case:?}: expected the stored {attribute} to read {value}, got {}",
                read["Item"]
            );
        }
        // Put the item back the way it was, so each row starts from the same item.
        ok(
            &router,
            "PutItem",
            json!({"TableName": "Computed", "Item": {
                "pk": {"S": "one"},
                "counter": {"N": "10"},
                "credits": {"L": [{"S": "vocals"}]},
                "meta": {"M": {"label": {"S": "indie"}}}
            }}),
        )
        .await;
    }
}

/// An `UpdateItem` whose condition holds applies its update; one whose condition does
/// not is answered with `ConditionalCheckFailedException` and leaves the item alone.
///
/// The rows cover the whole legacy `ComparisonOperator` vocabulary (`EQ | NE | LE | LT
/// | GE | GT | NOT_NULL | NULL | CONTAINS | NOT_CONTAINS | BEGINS_WITH | IN |
/// BETWEEN`), the `Value`/`Exists` form, and both `ConditionalOperator` values.
#[tokio::test]
async fn a_legacy_expected_condition_gates_the_write() {
    let router = emulator();
    create_active_table(&router, "Gated", &[("pk", "HASH", "S")]).await;
    let cases: Vec<(&str, Value, Option<&str>, bool)> = vec![
        (
            "Value equal to the stored value holds",
            json!({"status": {"Value": {"S": "active"}}}),
            None,
            true,
        ),
        (
            "Value different from the stored value does not",
            json!({"status": {"Value": {"S": "retired"}}}),
            None,
            false,
        ),
        (
            "Exists false on an absent attribute holds",
            json!({"note": {"Exists": false}}),
            None,
            true,
        ),
        (
            "Exists false on a stored attribute does not",
            json!({"status": {"Exists": false}}),
            None,
            false,
        ),
        (
            "EQ holds for the stored value",
            json!({"status": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "active"}]}}),
            None,
            true,
        ),
        (
            "NE holds for another value",
            json!({"status": {"ComparisonOperator": "NE", "AttributeValueList": [{"S": "retired"}]}}),
            None,
            true,
        ),
        (
            "LT compares numbers numerically",
            json!({"counter": {"ComparisonOperator": "LT", "AttributeValueList": [{"N": "11"}]}}),
            None,
            true,
        ),
        (
            "LE holds at the bound",
            json!({"counter": {"ComparisonOperator": "LE", "AttributeValueList": [{"N": "10"}]}}),
            None,
            true,
        ),
        (
            "GT does not hold at the bound",
            json!({"counter": {"ComparisonOperator": "GT", "AttributeValueList": [{"N": "10"}]}}),
            None,
            false,
        ),
        (
            "GE holds at the bound",
            json!({"counter": {"ComparisonOperator": "GE", "AttributeValueList": [{"N": "10"}]}}),
            None,
            true,
        ),
        (
            "NOT_NULL holds for a stored attribute",
            json!({"status": {"ComparisonOperator": "NOT_NULL"}}),
            None,
            true,
        ),
        (
            "NULL holds for an absent attribute",
            json!({"note": {"ComparisonOperator": "NULL"}}),
            None,
            true,
        ),
        (
            "CONTAINS finds a set member",
            json!({"tags": {"ComparisonOperator": "CONTAINS", "AttributeValueList": [{"S": "x"}]}}),
            None,
            true,
        ),
        (
            "NOT_CONTAINS holds for a member the set lacks",
            json!({"tags": {"ComparisonOperator": "NOT_CONTAINS", "AttributeValueList": [{"S": "z"}]}}),
            None,
            true,
        ),
        (
            "BEGINS_WITH matches a prefix",
            json!({"status": {"ComparisonOperator": "BEGINS_WITH", "AttributeValueList": [{"S": "act"}]}}),
            None,
            true,
        ),
        (
            "IN holds for one of the candidates",
            json!({"status": {"ComparisonOperator": "IN", "AttributeValueList": [{"S": "retired"}, {"S": "active"}]}}),
            None,
            true,
        ),
        (
            "BETWEEN holds inside the range",
            json!({"counter": {"ComparisonOperator": "BETWEEN", "AttributeValueList": [{"N": "1"}, {"N": "10"}]}}),
            None,
            true,
        ),
        (
            "BETWEEN does not hold outside it",
            json!({"counter": {"ComparisonOperator": "BETWEEN", "AttributeValueList": [{"N": "11"}, {"N": "20"}]}}),
            None,
            false,
        ),
        (
            "AND needs every entry",
            json!({
                "status": {"Value": {"S": "active"}},
                "counter": {"Value": {"N": "99"}}
            }),
            Some("AND"),
            false,
        ),
        (
            "OR needs only one entry",
            json!({
                "status": {"Value": {"S": "active"}},
                "counter": {"Value": {"N": "99"}}
            }),
            Some("OR"),
            true,
        ),
        (
            "OR with no entry holding does not apply",
            json!({
                "status": {"Value": {"S": "retired"}},
                "counter": {"Value": {"N": "99"}}
            }),
            Some("OR"),
            false,
        ),
    ];

    for (case, expected, operator, applies) in cases {
        let key = json!({"pk": {"S": case}});
        ok(
            &router,
            "PutItem",
            json!({"TableName": "Gated", "Item": {
                "pk": key["pk"].clone(),
                "status": {"S": "active"},
                "counter": {"N": "10"},
                "tags": {"SS": ["x", "b"]}
            }}),
        )
        .await;
        let mut request = json!({
            "TableName": "Gated",
            "Key": key.clone(),
            "AttributeUpdates": {"status": {"Action": "PUT", "Value": {"S": "updated"}}},
            "Expected": expected,
        });
        if let Some(operator) = operator {
            request["ConditionalOperator"] = json!(operator);
        }
        let (status, body) = call(&router, "UpdateItem", &request).await;
        if applies {
            assert_eq!(
                status, 200,
                "case {case:?}: the condition holds, so the update applies, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: the condition does not hold, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ConditionalCheckFailedException",
                "case {case:?}: expected ConditionalCheckFailedException, got {body}"
            );
        }
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": "Gated", "Key": key.clone()}),
        )
        .await;
        let expected_status = if applies { "updated" } else { "active" };
        assert_eq!(
            read["Item"]["status"],
            json!({"S": expected_status}),
            "case {case:?}: expected the stored status to read {expected_status}, got {}",
            read["Item"]
        );
    }
}

/// A `ConditionExpression` gates the same writes the legacy `Expected` map does, on
/// `PutItem`, `UpdateItem` and `DeleteItem` alike.
#[tokio::test]
async fn a_condition_expression_gates_every_single_item_write() {
    let router = emulator();
    create_active_table(&router, "Conditions", &[("pk", "HASH", "S")]).await;
    // One row per function and operator `UpdateItem$ConditionExpression` lists.
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "attribute_exists on a stored attribute",
            "attribute_exists(status)",
            json!({}),
            true,
        ),
        (
            "attribute_not_exists on a stored attribute",
            "attribute_not_exists(status)",
            json!({}),
            false,
        ),
        (
            "attribute_not_exists on an absent attribute",
            "attribute_not_exists(note)",
            json!({}),
            true,
        ),
        (
            "attribute_type names the stored type",
            "attribute_type(counter, :type)",
            json!({":type": {"S": "N"}}),
            true,
        ),
        (
            "begins_with on a prefix",
            "begins_with(status, :prefix)",
            json!({":prefix": {"S": "act"}}),
            true,
        ),
        (
            "contains on a set member",
            "contains(tags, :member)",
            json!({":member": {"S": "x"}}),
            true,
        ),
        (
            "size of a set",
            "size(tags) = :two",
            json!({":two": {"N": "2"}}),
            true,
        ),
        (
            "an equality",
            "status = :active",
            json!({":active": {"S": "active"}}),
            true,
        ),
        (
            "an inequality",
            "counter <> :eleven",
            json!({":eleven": {"N": "11"}}),
            true,
        ),
        (
            "a BETWEEN",
            "counter BETWEEN :low AND :high",
            json!({":low": {"N": "1"}, ":high": {"N": "10"}}),
            true,
        ),
        (
            "an IN list",
            "status IN (:retired, :active)",
            json!({":retired": {"S": "retired"}, ":active": {"S": "active"}}),
            true,
        ),
        (
            "a NOT",
            "NOT status = :active",
            json!({":active": {"S": "active"}}),
            false,
        ),
        (
            "an OR of a failing and a holding branch",
            "status = :retired OR counter = :ten",
            json!({":retired": {"S": "retired"}, ":ten": {"N": "10"}}),
            true,
        ),
        (
            "an AND with a failing branch",
            "(status = :active) AND counter = :eleven",
            json!({":active": {"S": "active"}, ":eleven": {"N": "11"}}),
            false,
        ),
    ];

    for (case, condition, values, holds) in cases {
        for operation in ["UpdateItem", "PutItem", "DeleteItem"] {
            let key = json!({"pk": {"S": format!("{operation}-{case}")}});
            let stored = json!({
                "pk": key["pk"].clone(),
                "status": {"S": "active"},
                "counter": {"N": "10"},
                "tags": {"SS": ["x", "b"]}
            });
            ok(
                &router,
                "PutItem",
                json!({"TableName": "Conditions", "Item": stored.clone()}),
            )
            .await;
            let mut request = json!({
                "TableName": "Conditions",
                "ConditionExpression": condition,
            });
            if values != json!({}) {
                request["ExpressionAttributeValues"] = values.clone();
            }
            match operation {
                "UpdateItem" => {
                    request["Key"] = key.clone();
                    request["UpdateExpression"] = json!("SET status = :next");
                    request["ExpressionAttributeValues"] = {
                        let mut merged = values.clone();
                        merged["::next"] = json!(null);
                        let object = merged.as_object_mut().expect("an object");
                        object.remove("::next");
                        object.insert(":next".to_string(), json!({"S": "updated"}));
                        Value::Object(object.clone())
                    };
                }
                "PutItem" => {
                    let mut replacement = stored.clone();
                    replacement["status"] = json!({"S": "replaced"});
                    request["Item"] = replacement;
                }
                _ => request["Key"] = key.clone(),
            }
            let (status, body) = call(&router, operation, &request).await;
            let expected_status = if holds { 200 } else { 400 };
            assert_eq!(
                status, expected_status,
                "case {case:?} on {operation}: expected HTTP {expected_status}, got {status} {body}"
            );
            if !holds {
                assert_eq!(
                    error_code(&body),
                    "ConditionalCheckFailedException",
                    "case {case:?} on {operation}: expected ConditionalCheckFailedException, got \
                     {body}"
                );
            }
            let read = ok(
                &router,
                "GetItem",
                json!({"TableName": "Conditions", "Key": key.clone()}),
            )
            .await;
            let expected_item = match (operation, holds) {
                ("DeleteItem", true) => Value::Null,
                ("UpdateItem", true) => json!({"S": "updated"}),
                ("PutItem", true) => json!({"S": "replaced"}),
                _ => json!({"S": "active"}),
            };
            let actual = if read["Item"].is_null() {
                Value::Null
            } else {
                read["Item"]["status"].clone()
            };
            assert_eq!(
                actual, expected_item,
                "case {case:?} on {operation}: expected the store to read {expected_item}, got {}",
                read["Item"]
            );
        }
    }
}

/// PartiQL and the classic API are two ways to reach ONE item store: a statement's
/// write is visible to `GetItem`, and a classic write is visible to `SELECT`.
#[tokio::test]
async fn partiql_dml_and_the_classic_api_share_one_item_store() {
    let router = emulator();
    create_active_table(&router, "Shared", &[("pk", "HASH", "S")]).await;

    // Each row writes through PartiQL and is read back through the classic API: the
    // statement forms span INSERT, the two UPDATE clauses, arithmetic, `?` parameters
    // and DELETE.
    let statements: Vec<(&str, &str, Value, Value)> = vec![
        (
            "INSERT stores an item",
            "INSERT INTO \"Shared\" VALUE {'pk':'one','body':'written','counter':1}",
            json!([]),
            json!({"pk": {"S": "one"}, "body": {"S": "written"}, "counter": {"N": "1"}}),
        ),
        (
            "UPDATE sets an attribute",
            "UPDATE \"Shared\" SET body = 'partiql-updated' WHERE pk = 'one'",
            json!([]),
            json!({"pk": {"S": "one"}, "body": {"S": "partiql-updated"}, "counter": {"N": "1"}}),
        ),
        (
            "UPDATE computes from the stored value",
            "UPDATE \"Shared\" SET counter = counter + 4 WHERE pk = 'one'",
            json!([]),
            json!({"pk": {"S": "one"}, "body": {"S": "partiql-updated"}, "counter": {"N": "5"}}),
        ),
        (
            "UPDATE fills its ? parameters in order",
            "UPDATE \"Shared\" SET body = ? WHERE pk = ?",
            json!([{"S": "parameterised"}, {"S": "one"}]),
            json!({"pk": {"S": "one"}, "body": {"S": "parameterised"}, "counter": {"N": "5"}}),
        ),
        (
            "UPDATE with several clauses sets and removes",
            "UPDATE \"Shared\" SET note = 'kept' REMOVE counter WHERE pk = 'one'",
            json!([]),
            json!({"pk": {"S": "one"}, "body": {"S": "parameterised"}, "note": {"S": "kept"}}),
        ),
        (
            "UPDATE stores a document value",
            "UPDATE \"Shared\" SET meta = {'label':'indie','tracks':12} WHERE pk = 'one'",
            json!([]),
            json!({
                "pk": {"S": "one"},
                "body": {"S": "parameterised"},
                "note": {"S": "kept"},
                "meta": {"M": {"label": {"S": "indie"}, "tracks": {"N": "12"}}}
            }),
        ),
        (
            "UPDATE stores a list value",
            "UPDATE \"Shared\" SET credits = ['vocals', 3, true] WHERE pk = 'one'",
            json!([]),
            json!({
                "pk": {"S": "one"},
                "body": {"S": "parameterised"},
                "note": {"S": "kept"},
                "meta": {"M": {"label": {"S": "indie"}, "tracks": {"N": "12"}}},
                "credits": {"L": [{"S": "vocals"}, {"N": "3"}, {"BOOL": true}]}
            }),
        ),
    ];
    for (case, statement, parameters, expected) in statements {
        let mut request = json!({"Statement": statement});
        if parameters != json!([]) {
            request["Parameters"] = parameters;
        }
        let response = ok(&router, "ExecuteStatement", request).await;
        assert_eq!(
            response["Items"],
            json!([]),
            "case {case:?}: a write statement reports an empty Items list, got {response}"
        );
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": "Shared", "Key": {"pk": {"S": "one"}}}),
        )
        .await;
        assert_eq!(
            read["Item"], expected,
            "case {case:?}: expected the item to read {expected}, got {}",
            read["Item"]
        );
    }

    // A classic write is visible to a statement that reads the same item.
    ok(
        &router,
        "UpdateItem",
        json!({
            "TableName": "Shared",
            "Key": {"pk": {"S": "one"}},
            "UpdateExpression": "SET body = :body",
            "ExpressionAttributeValues": {":body": {"S": "classic"}},
        }),
    )
    .await;
    let selected = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": "SELECT * FROM \"Shared\" WHERE pk = 'one'"}),
    )
    .await;
    assert_eq!(
        selected["Items"][0]["body"],
        json!({"S": "classic"}),
        "case SELECT after a classic update: expected body to read classic, got {selected}"
    );
    let projected = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": "SELECT body, note FROM \"Shared\" WHERE begins_with(body, 'clas')"}),
    )
    .await;
    assert_eq!(
        projected["Items"],
        json!([{"body": {"S": "classic"}, "note": {"S": "kept"}}]),
        "case SELECT with a projection: expected only the named attributes, got {projected}"
    );

    // A long chain of conjunctions nests nothing, so the depth budget is not spent on
    // it: the clause is read and the filter applied.
    let conjunctions = vec!["body = 'classic'"; 20].join(" AND ");
    let chained = ok(
        &router,
        "ExecuteStatement",
        json!({
            "Statement": format!("SELECT * FROM \"Shared\" WHERE pk = 'one' AND {conjunctions}"),
        }),
    )
    .await;
    assert_eq!(
        chained["Items"].as_array().map(Vec::len),
        Some(1),
        "case SELECT with 21 conjunctions: expected the one matching item, got {chained}"
    );

    // INSERT never replaces, DELETE needs the item, and both say so.
    let (status, body) = call(
        &router,
        "ExecuteStatement",
        &json!({"Statement": "INSERT INTO \"Shared\" VALUE {'pk':'one'}"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case INSERT over an existing key: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "DuplicateItemException",
        "case INSERT over an existing key: expected DuplicateItemException, got {body}"
    );
    ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": "DELETE FROM \"Shared\" WHERE pk = 'one'"}),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "Shared", "Key": {"pk": {"S": "one"}}}),
    )
    .await;
    assert!(
        read["Item"].is_null(),
        "case DELETE: expected the item to be gone, got {read}"
    );
    let (status, body) = call(
        &router,
        "ExecuteStatement",
        &json!({"Statement": "DELETE FROM \"Shared\" WHERE pk = 'one'"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case DELETE of an absent item: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConditionalCheckFailedException",
        "case DELETE of an absent item: expected ConditionalCheckFailedException, got {body}"
    );
    let (status, body) = call(
        &router,
        "ExecuteStatement",
        &json!({"Statement": "UPDATE \"Shared\" SET body = 'x' WHERE body = 'y'"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case UPDATE without the key in its WHERE clause: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case UPDATE without the key in its WHERE clause: expected ValidationException, got {body}"
    );
}

/// `ReturnValues` reports the item as it was or as it is, and only the attributes the
/// update touched for the two `UPDATED_*` settings.
#[tokio::test]
async fn return_values_reports_the_item_before_or_after_the_update() {
    let router = emulator();
    create_active_table(&router, "Returned", &[("pk", "HASH", "S")]).await;
    // One row per value of the model's `ReturnValue` enum.
    let cases: Vec<(&str, Value)> = vec![
        ("NONE", Value::Null),
        (
            "ALL_OLD",
            json!({"pk": {"S": "one"}, "status": {"S": "active"}, "counter": {"N": "1"}}),
        ),
        (
            "ALL_NEW",
            json!({"pk": {"S": "one"}, "status": {"S": "updated"}, "counter": {"N": "1"}}),
        ),
        ("UPDATED_OLD", json!({"status": {"S": "active"}})),
        ("UPDATED_NEW", json!({"status": {"S": "updated"}})),
    ];
    for (setting, expected) in cases {
        ok(
            &router,
            "PutItem",
            json!({"TableName": "Returned", "Item": {
                "pk": {"S": "one"}, "status": {"S": "active"}, "counter": {"N": "1"}
            }}),
        )
        .await;
        let updated = ok(
            &router,
            "UpdateItem",
            json!({
                "TableName": "Returned",
                "Key": {"pk": {"S": "one"}},
                "AttributeUpdates": {"status": {"Action": "PUT", "Value": {"S": "updated"}}},
                "ReturnValues": setting,
            }),
        )
        .await;
        let reported = if updated["Attributes"].is_null() {
            Value::Null
        } else {
            updated["Attributes"].clone()
        };
        assert_eq!(
            reported, expected,
            "case ReturnValues {setting}: expected {expected}, got {reported}"
        );
    }
}

/// A request may not state its update or its condition in both vocabularies at once,
/// and the refusal names the members of each that it carried.
#[tokio::test]
async fn one_request_may_not_mix_the_two_vocabularies() {
    let router = emulator();
    create_active_table(&router, "Mixed", &[("pk", "HASH", "S")]).await;
    let cases: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "AttributeUpdates with UpdateExpression",
            "UpdateItem",
            json!({
                "TableName": "Mixed",
                "Key": {"pk": {"S": "one"}},
                "AttributeUpdates": {"status": {"Value": {"S": "a"}}},
                "UpdateExpression": "SET status = :next",
                "ExpressionAttributeValues": {":next": {"S": "b"}},
            }),
            "AttributeUpdates",
            "UpdateExpression",
        ),
        (
            "Expected with ConditionExpression",
            "UpdateItem",
            json!({
                "TableName": "Mixed",
                "Key": {"pk": {"S": "one"}},
                "UpdateExpression": "SET status = :next",
                "ExpressionAttributeValues": {":next": {"S": "b"}},
                "Expected": {"status": {"Exists": false}},
                "ConditionExpression": "attribute_not_exists(status)",
            }),
            "Expected",
            "ConditionExpression",
        ),
        (
            "Expected with ConditionExpression on PutItem",
            "PutItem",
            json!({
                "TableName": "Mixed",
                "Item": {"pk": {"S": "one"}},
                "Expected": {"status": {"Exists": false}},
                "ConditionExpression": "attribute_not_exists(status)",
            }),
            "Expected",
            "ConditionExpression",
        ),
        (
            "Expected with ConditionExpression on DeleteItem",
            "DeleteItem",
            json!({
                "TableName": "Mixed",
                "Key": {"pk": {"S": "one"}},
                "Expected": {"status": {"Exists": false}},
                "ConditionExpression": "attribute_not_exists(status)",
            }),
            "Expected",
            "ConditionExpression",
        ),
    ];
    for (case, operation, request, legacy_member, expression_member) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or("<no message>");
        assert!(
            message.contains(legacy_member) && message.contains(expression_member),
            "case {case:?}: expected the refusal to name {legacy_member} and \
             {expression_member}, got {message}"
        );
    }
}

/// An attribute name that is not a bare word goes through `ExpressionAttributeNames`
/// in EVERY expression grammar, because they share one lexer: a name carrying a `-`
/// is a syntax error and a name carrying a `.` is read as a document path.
#[tokio::test]
async fn a_name_outside_the_lexers_alphabet_needs_an_alias() {
    let router = emulator();
    create_active_table(&router, "Named", &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Named", "Item": {"pk": {"S": "one"}, "a": {"M": {"b": {"N": "1"}}}}}),
    )
    .await;
    // One row per grammar the lexer serves, each with the two spellings it refuses.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a hyphenated name in a key condition",
            "Query",
            json!({
                "TableName": "Named",
                "KeyConditionExpression": "pk-name = :pk",
                "ExpressionAttributeValues": {":pk": {"S": "one"}},
            }),
        ),
        (
            "a dotted name in a key condition",
            "Query",
            json!({
                "TableName": "Named",
                "KeyConditionExpression": "a.b = :pk",
                "ExpressionAttributeValues": {":pk": {"S": "one"}},
            }),
        ),
        (
            "a hyphenated name in an update expression",
            "UpdateItem",
            json!({
                "TableName": "Named",
                "Key": {"pk": {"S": "one"}},
                "UpdateExpression": "SET some-name = :value",
                "ExpressionAttributeValues": {":value": {"N": "1"}},
            }),
        ),
        (
            "a hyphenated name in a condition expression",
            "UpdateItem",
            json!({
                "TableName": "Named",
                "Key": {"pk": {"S": "one"}},
                "UpdateExpression": "SET fresh = :value",
                "ExpressionAttributeValues": {":value": {"N": "1"}},
                "ConditionExpression": "attribute_exists(some-name)",
            }),
        ),
    ];
    for (case, operation, request) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {body}"
        );
    }
    // The same names through `ExpressionAttributeNames` are served, in every grammar.
    ok(
        &router,
        "UpdateItem",
        json!({
            "TableName": "Named",
            "Key": {"pk": {"S": "one"}},
            "UpdateExpression": "SET #hyphenated = :value",
            "ExpressionAttributeNames": {"#hyphenated": "some-name", "#dotted": "a"},
            "ExpressionAttributeValues": {":value": {"N": "1"}},
            "ConditionExpression": "attribute_exists(#dotted.b)",
        }),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "Named", "Key": {"pk": {"S": "one"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["some-name"],
        json!({"N": "1"}),
        "case an #alias for a hyphenated name: expected it to be written, got {}",
        read["Item"]
    );
}

/// An `UpdateItem` whose key holds no item creates one: "adds a new item to the table
/// if it does not already exist".
#[tokio::test]
async fn an_update_to_an_absent_key_creates_the_item() {
    let router: MultiServiceRouter = emulator();
    create_active_table(&router, "Upserts", &[("pk", "HASH", "S"), ("sk", "RANGE", "N")]).await;
    let key = json!({"pk": {"S": "new"}, "sk": {"N": "1"}});
    ok(
        &router,
        "UpdateItem",
        json!({
            "TableName": "Upserts",
            "Key": key.clone(),
            "AttributeUpdates": {"counter": {"Action": "ADD", "Value": {"N": "3"}}},
        }),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "Upserts", "Key": key.clone()}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"pk": {"S": "new"}, "sk": {"N": "1"}, "counter": {"N": "3"}}),
        "case upsert: expected the key plus what the update wrote, got {}",
        read["Item"]
    );

    // An update that states no action at all still creates the item its key names.
    let bare = json!({"pk": {"S": "bare"}, "sk": {"N": "2"}});
    ok(
        &router,
        "UpdateItem",
        json!({"TableName": "Upserts", "Key": bare.clone()}),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "Upserts", "Key": bare.clone()}),
    )
    .await;
    assert_eq!(
        read["Item"], bare,
        "case bare upsert: expected the key itself, got {}",
        read["Item"]
    );
}

