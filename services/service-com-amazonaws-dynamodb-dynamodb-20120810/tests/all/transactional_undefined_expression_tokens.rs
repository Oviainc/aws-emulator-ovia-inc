// SPDX-License-Identifier: CC0-1.0

//! Which of the two substitution-table diagnoses a TRANSACTIONAL write earns, driven
//! end to end.
//!
//! RULE under test: an expression member that DEREFERENCES a substitution token the
//! request never defines is refused naming that token and the member that carried the
//! expression, and that diagnosis comes BEFORE the surplus-entry one — a request that
//! both dereferences an undefined token and defines an entry no expression references is
//! told about the undefined token. The surplus-entry diagnosis is reached only once every
//! token the expressions reference is defined.
//!
//! The suite beside this one (`undefined_expression_tokens`) drives that rule through the
//! single-item and read operations. `TransactWriteItems` carries its expression members
//! per ELEMENT rather than per request, so the rows here span what THAT generalises over:
//! the element kind (`Put`, `Update`, `Delete`, `ConditionCheck`), the expression member
//! the token stood in (`ConditionExpression` and `UpdateExpression`), the token kind
//! (`#name` and `:value`), the position of the faulty element among its siblings, whether
//! a surplus entry is present beside the undefined token, and which substitution table the
//! surplus entry belongs to. Both diagnoses are stated ON THEIR OWN here, whichever member
//! the element carried: the `<N> validation error(s) detected: ` envelope reports violations
//! of the REQUEST's own members, and these are members of one element of it
//! (`element_expression_envelopes` is the suite that spans that rule).

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;
// The sentences both diagnoses carry are stated ONCE for this crate, by the suite that
// drives the same rule through the single-item and read operations.
use crate::undefined_expression_tokens::{
    expression_refusal_in_element, undefined_name_clause, undefined_value_clause,
    unused_entries_in_element,
};

/// The table every row below drives.
const TABLE: &str = "TransactUndefinedTokens";

/// The key of the seeded item, which every row addresses so no row is refused for its key
/// instead of for its expression.
fn key() -> Value {
    json!({"pk": {"S": "item-1"}})
}

/// A second seeded item, so a row can put the faulty element beside a sound one.
fn other_key() -> Value {
    json!({"pk": {"S": "item-2"}})
}

/// The key the `Put` rows address, which no row seeds: a refused transaction must leave
/// it unwritten, and the positive control never puts it.
fn put_key() -> Value {
    json!({"pk": {"S": "item-3"}})
}

/// One sound element: a condition check over a defined token, which no row's refusal may
/// come from.
fn sound_element() -> Value {
    json!({"ConditionCheck": {
        "TableName": TABLE,
        "Key": other_key(),
        "ConditionExpression": "#s = :v",
        "ExpressionAttributeNames": {"#s": "status"},
        "ExpressionAttributeValues": {":v": {"S": "active"}}}})
}

/// Seed the table and the two items the conditions below are evaluated against.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    for name in ["item-1", "item-2"] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": TABLE,
                   "Item": {"pk": {"S": name}, "status": {"S": "active"}}}),
        )
        .await;
    }
    router
}

/// Every key a row addresses, as it stands — including the one only a `Put` row names, so
/// an element the refusal was supposed to stop cannot have written it. For the assertions
/// that a refused transaction wrote nothing.
async fn stored(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<Value> {
    let mut items = Vec::new();
    for addressed in [key(), other_key(), put_key()] {
        let read = ok(
            router,
            "GetItem",
            json!({"TableName": TABLE, "Key": addressed}),
        )
        .await;
        items.push(read.get("Item").cloned().unwrap_or(Value::Null));
    }
    items
}

#[tokio::test]
async fn a_token_no_entry_defines_is_diagnosed_before_a_surplus_entry() {
    let router = seeded().await;
    let before = stored(&router).await;

    // (case, TransactItems, the member the token stood in, the clause expected)
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "an undefined #name in a put's condition, beside a surplus name entry",
            json!([{"Put": {
                "TableName": TABLE,
                "Item": {"pk": {"S": "item-3"}, "status": {"S": "paused"}},
                "ConditionExpression": "attribute_not_exists(#missing)",
                "ExpressionAttributeNames": {"#other": "status"}}}]),

            "ConditionExpression",
            undefined_name_clause("#missing"),
        ),
        (
            "an undefined #name in a delete's condition, with no surplus entry at all",
            json!([{"Delete": {
                "TableName": TABLE,
                "Key": key(),
                "ConditionExpression": "attribute_exists(#missing)"}}]),
            "ConditionExpression",
            undefined_name_clause("#missing"),
        ),
        (
            "an undefined :value in a delete's condition, beside a surplus value entry",
            json!([{"Delete": {
                "TableName": TABLE,
                "Key": key(),
                "ConditionExpression": "#s = :missing",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":other": {"S": "active"}}}}]),
            "ConditionExpression",
            undefined_value_clause(":missing"),
        ),
        (
            "an undefined #name in an update's update expression, beside a surplus entry",
            json!([{"Update": {
                "TableName": TABLE,
                "Key": key(),
                "UpdateExpression": "SET #missing = :v",
                "ExpressionAttributeNames": {"#other": "status"},
                "ExpressionAttributeValues": {":v": {"S": "paused"}}}}]),
            "UpdateExpression",
            undefined_name_clause("#missing"),
        ),
        (
            "an undefined :value in an update's update expression",
            json!([{"Update": {
                "TableName": TABLE,
                "Key": key(),
                "UpdateExpression": "SET #s = :missing",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":other": {"S": "paused"}}}}]),
            "UpdateExpression",
            undefined_value_clause(":missing"),
        ),
        (
            "an undefined :value in a condition check, beside a surplus value entry",
            json!([{"ConditionCheck": {
                "TableName": TABLE,
                "Key": key(),
                "ConditionExpression": "#s = :missing",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":other": {"S": "active"}}}}]),
            "ConditionExpression",
            undefined_value_clause(":missing"),
        ),
        (
            "an undefined #name in the SECOND element, behind a sound one",
            json!([
                sound_element(),
                {"Delete": {
                    "TableName": TABLE,
                    "Key": key(),
                    "ConditionExpression": "#missing = :v",
                    "ExpressionAttributeNames": {"#other": "status"},
                    "ExpressionAttributeValues": {":v": {"S": "active"}}}}]),
            "ConditionExpression",
            undefined_name_clause("#missing"),
        ),
    ];

    for (case, transact_items, member, clause) in rows {
        let expected = expression_refusal_in_element(member, &clause);
        let (status, body) = call(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": transact_items}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {body}"
        );
        let said = message(&body);
        assert_eq!(
            said, expected,
            "case {case:?}: expected {expected:?}, got {said:?}"
        );
    }

    let after = stored(&router).await;
    assert_eq!(
        after, before,
        "the refused transactions must write nothing: expected {before:?}, got {after:?}"
    );
}

#[tokio::test]
async fn a_surplus_entry_is_diagnosed_once_every_referenced_token_is_defined() {
    let router = seeded().await;

    // (case, TransactItems, the table the surplus entry belongs to, its keys). The
    // sentence an element earns names the substitution table and not the expression member
    // the keys were offered to, so the member is not a column here.
    let rows: Vec<(&str, Value, &str, Vec<&str>)> = vec![
        (
            "a surplus name entry beside a put whose condition names no token",
            json!([{"Put": {
                "TableName": TABLE,
                "Item": {"pk": {"S": "item-3"}, "status": {"S": "paused"}},
                "ConditionExpression": "attribute_not_exists(pk)",
                "ExpressionAttributeNames": {"#other": "status"}}}]),
            "ExpressionAttributeNames",
            vec!["#other"],
        ),
        (
            "a surplus value entry beside an update whose tokens are all defined",
            json!([{"Update": {
                "TableName": TABLE,
                "Key": key(),
                "UpdateExpression": "SET #s = :v",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":v": {"S": "paused"},
                                              ":other": {"S": "active"}}}}]),
            "ExpressionAttributeValues",
            vec![":other"],
        ),
        (
            "two surplus name entries on a condition check, reported in one stable order",
            json!([{"ConditionCheck": {
                "TableName": TABLE,
                "Key": key(),
                "ConditionExpression": "#s = :v",
                "ExpressionAttributeNames": {"#s": "status", "#b": "status",
                                             "#a": "status"},
                "ExpressionAttributeValues": {":v": {"S": "active"}}}}]),
            "ExpressionAttributeNames",
            vec!["#a", "#b"],
        ),
        (
            "a surplus entry on the SECOND element, behind a sound one",
            json!([
                sound_element(),
                {"Delete": {
                    "TableName": TABLE,
                    "Key": key(),
                    "ConditionExpression": "#s = :v",
                    "ExpressionAttributeNames": {"#s": "status", "#other": "status"},
                    "ExpressionAttributeValues": {":v": {"S": "active"}}}}]),
            "ExpressionAttributeNames",
            vec!["#other"],
        ),
    ];

    for (case, transact_items, member, keys) in rows {
        let expected = unused_entries_in_element(member, &keys);
        let (status, body) = call(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": transact_items}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {body}"
        );
        let said = message(&body);
        assert_eq!(
            said, expected,
            "case {case:?}: expected {expected:?}, got {said:?}"
        );
    }

    // The positive control the precedence rule is about: with every token defined and
    // every entry referenced, the same shape of transaction LANDS — so the refusals above
    // come from the substitution tables and not from the elements being unserved. Read
    // back through GetItem: the update applied and the delete removed its item.
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [
            {"Update": {
                "TableName": TABLE,
                "Key": key(),
                "UpdateExpression": "SET #s = :paused",
                "ConditionExpression": "#s = :active",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":paused": {"S": "paused"},
                                              ":active": {"S": "active"}}}},
            {"Delete": {
                "TableName": TABLE,
                "Key": other_key(),
                "ConditionExpression": "#s = :v",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":v": {"S": "active"}}}}]}),
    )
    .await;
    let settled = stored(&router).await;
    assert_eq!(
        settled,
        vec![
            json!({"pk": {"S": "item-1"}, "status": {"S": "paused"}}),
            Value::Null,
            Value::Null,
        ],
        "the transaction whose tokens all resolve must apply: got {settled:?}"
    );
}
