// SPDX-License-Identifier: CC0-1.0

//! Which of the two substitution-table diagnoses a request earns, driven end to end.
//!
//! RULE under test: an expression member that DEREFERENCES a substitution token the
//! request never defines is refused naming that token and the member that carried the
//! expression, and that diagnosis comes BEFORE the surplus-entry one — a request that
//! both dereferences an undefined token and defines an entry no expression references is
//! told about the undefined token, because dropping the surplus entry would leave the
//! request refused anyway. The surplus-entry diagnosis is reached only once every token
//! the expressions reference is defined.
//!
//! The rows span what the rule generalises over: the token kind (`#name` and
//! `:value`), the operation (an item delete, write, update, read, query and scan), the
//! expression member the token stood in (`ConditionExpression`, `UpdateExpression`,
//! `KeyConditionExpression`, `FilterExpression`, `ProjectionExpression`), whether a
//! surplus entry is present beside the undefined token, and which substitution table the
//! surplus entry belongs to. The envelope follows the member, as the other refusals of
//! this service do: the members a WRITE states its intent through carry the
//! `<N> validation error(s) detected: ` envelope and the members a READ narrows its
//! result with do not.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row below drives, and the one item the conditions read.
const TABLE: &str = "UndefinedTokens";

/// The clause AWS states for a `#name` token no `ExpressionAttributeNames` entry defines.
///
/// The four wording helpers below are this crate's ONE statement of the sentences these
/// two diagnoses carry, shared with every suite that drives the same rule through another
/// operation, so the expected wording cannot drift between them.
pub(crate) fn undefined_name_clause(token: &str) -> String {
    format!(
        "An expression attribute name used in the document path is not defined; \
         attribute name: {token}"
    )
}

/// The clause AWS states for a `:value` token no `ExpressionAttributeValues` entry
/// defines.
pub(crate) fn undefined_value_clause(token: &str) -> String {
    format!("An expression attribute value used in expression is not defined; attribute value: {token}")
}

/// Whether a refusal about a request's expressions is stated inside the
/// `<N> validation error(s) detected: ` envelope, which follows the expression member
/// the request states its intent through: the two members a WRITE states its intent
/// through carry it, and the members a READ narrows its result with do not.
pub(crate) fn envelopes(expression_member: &str) -> bool {
    expression_member == "UpdateExpression" || expression_member == "ConditionExpression"
}

/// The whole sentence for a clause the expression language raises, stated behind the
/// member that carried it, in or out of the envelope that member earns.
pub(crate) fn expression_refusal(member: &str, clause: &str) -> String {
    let stated = expression_refusal_in_element(member, clause);
    if envelopes(member) {
        return format!("1 validation error detected: {stated}");
    }
    stated
}

/// The same sentence as an ELEMENT of a multi-item request earns it: the clause behind the
/// member that carried it, on its own.
///
/// The `<N> validation error(s) detected: ` envelope states violations of the REQUEST's own
/// members, and the expression members of one `TransactItems` action or one `RequestItems`
/// entry are members of that element — so an element's refusal carries no envelope whichever
/// member it names.
pub(crate) fn expression_refusal_in_element(member: &str, clause: &str) -> String {
    format!("Invalid {member}: {clause}")
}

/// The sentence AWS states for entries of `table` that no expression referenced, as the
/// request that offered them to `expression_member` earns it: the service's own rule, so
/// the sentence stands on its own except where that member's refusals carry the envelope.
pub(crate) fn unused_entries(expression_member: &str, table: &str, keys: &[&str]) -> String {
    let stated = unused_entries_in_element(table, keys);
    if envelopes(expression_member) {
        return format!("1 validation error detected: {stated}");
    }
    stated
}

/// The same sentence as an ELEMENT of a multi-item request earns it: stated on its own, for
/// the reason [`expression_refusal_in_element`] gives, so it names no expression member at
/// all.
pub(crate) fn unused_entries_in_element(table: &str, keys: &[&str]) -> String {
    format!(
        "Value provided in {table} unused in expressions: keys: {{{}}}",
        keys.join(", ")
    )
}

/// Seed the table and the item every condition below is evaluated against.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": "item-1"}, "status": {"S": "active"}}}),
    )
    .await;
    router
}

/// The key of the seeded item, which every row addresses so no row is refused for its
/// key instead of for its expression.
fn key() -> Value {
    json!({"pk": {"S": "item-1"}})
}

#[tokio::test]
async fn a_token_no_entry_defines_is_diagnosed_before_a_surplus_entry() {
    let router = seeded().await;

    // (case, operation, request, the member the token stood in, the clause expected)
    let rows: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            "an undefined #name in a delete's condition, beside a surplus name entry",
            "DeleteItem",
            json!({"TableName": TABLE, "Key": key(),
                   "ConditionExpression": "#missing = :v",
                   "ExpressionAttributeNames": {"#other": "status"},
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            "ConditionExpression",
            undefined_name_clause("#missing"),
        ),
        (
            "an undefined #name in a delete's condition, with no surplus entry at all",
            "DeleteItem",
            json!({"TableName": TABLE, "Key": key(),
                   "ConditionExpression": "attribute_exists(#missing)"}),
            "ConditionExpression",
            undefined_name_clause("#missing"),
        ),
        (
            "an undefined :value in a delete's condition, beside a surplus value entry",
            "DeleteItem",
            json!({"TableName": TABLE, "Key": key(),
                   "ConditionExpression": "#s = :missing",
                   "ExpressionAttributeNames": {"#s": "status"},
                   "ExpressionAttributeValues": {":other": {"S": "active"}}}),
            "ConditionExpression",
            undefined_value_clause(":missing"),
        ),
        (
            "an undefined #name in a write's condition, beside a surplus name entry",
            "PutItem",
            json!({"TableName": TABLE,
                   "Item": {"pk": {"S": "item-1"}, "status": {"S": "paused"}},
                   "ConditionExpression": "attribute_not_exists(#missing)",
                   "ExpressionAttributeNames": {"#other": "status"}}),
            "ConditionExpression",
            undefined_name_clause("#missing"),
        ),
        (
            "an undefined #name in an update, beside a surplus name entry",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key(),
                   "UpdateExpression": "SET #missing = :v",
                   "ExpressionAttributeNames": {"#other": "status"},
                   "ExpressionAttributeValues": {":v": {"S": "paused"}}}),
            "UpdateExpression",
            undefined_name_clause("#missing"),
        ),
        (
            "an undefined :value in an update, beside a surplus value entry",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key(),
                   "UpdateExpression": "SET #s = :missing",
                   "ExpressionAttributeNames": {"#s": "status"},
                   "ExpressionAttributeValues": {":other": {"S": "paused"}}}),
            "UpdateExpression",
            undefined_value_clause(":missing"),
        ),
        (
            "an undefined #name in a read's projection, beside a surplus name entry",
            "GetItem",
            json!({"TableName": TABLE, "Key": key(),
                   "ProjectionExpression": "#missing",
                   "ExpressionAttributeNames": {"#other": "status"}}),
            "ProjectionExpression",
            undefined_name_clause("#missing"),
        ),
        (
            "an undefined #name in a scan's filter, beside a surplus name entry",
            "Scan",
            json!({"TableName": TABLE,
                   "FilterExpression": "#missing = :v",
                   "ExpressionAttributeNames": {"#other": "status"},
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            "FilterExpression",
            undefined_name_clause("#missing"),
        ),
        (
            "an undefined :value in a query's key condition, beside a surplus value entry",
            "Query",
            json!({"TableName": TABLE,
                   "KeyConditionExpression": "pk = :missing",
                   "ExpressionAttributeValues": {":other": {"S": "item-1"}}}),
            "KeyConditionExpression",
            undefined_value_clause(":missing"),
        ),
    ];

    for (case, operation, request, member, clause) in rows {
        let expected = expression_refusal(member, &clause);
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} {body}"
        );
        let said = message(&body);
        assert_eq!(
            said, expected,
            "case {case:?} ({operation}): expected {expected:?}, got {said:?}"
        );
    }

    // Every refusal above left the item where it was: the seeded item is still readable
    // with the value it was stored with.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key()}),
    )
    .await;
    assert_eq!(
        read["Item"]["status"],
        json!({"S": "active"}),
        "the refused requests must leave the item alone, got {read}"
    );
}

/// One surplus-entry row: the case, the operation, the request, the expression member
/// the keys were offered to, the substitution table they belong to, and those keys in
/// the order the message states them.
type SurplusRow = (
    &'static str,
    &'static str,
    Value,
    &'static str,
    &'static str,
    Vec<&'static str>,
);

#[tokio::test]
async fn a_surplus_entry_is_diagnosed_once_every_referenced_token_is_defined() {
    let router = seeded().await;

    // (case, operation, request, the expression member the keys were offered to, the
    // table the surplus entry belongs to, its keys)
    let rows: Vec<SurplusRow> = vec![
        (
            "a surplus name entry beside a condition whose tokens are all defined",
            "DeleteItem",
            json!({"TableName": TABLE, "Key": key(),
                   "ConditionExpression": "#s = :v",
                   "ExpressionAttributeNames": {"#s": "status", "#other": "status"},
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            "ConditionExpression",
            "ExpressionAttributeNames",
            vec!["#other"],
        ),
        (
            "two surplus name entries, reported in one stable order",
            "DeleteItem",
            json!({"TableName": TABLE, "Key": key(),
                   "ConditionExpression": "#s = :v",
                   "ExpressionAttributeNames": {"#s": "status", "#b": "status", "#a": "status"},
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            "ConditionExpression",
            "ExpressionAttributeNames",
            vec!["#a", "#b"],
        ),
        (
            "a surplus value entry beside a condition whose tokens are all defined",
            "DeleteItem",
            json!({"TableName": TABLE, "Key": key(),
                   "ConditionExpression": "#s = :v",
                   "ExpressionAttributeNames": {"#s": "status"},
                   "ExpressionAttributeValues": {":v": {"S": "active"},
                                                 ":other": {"S": "paused"}}}),
            "ConditionExpression",
            "ExpressionAttributeValues",
            vec![":other"],
        ),
        (
            "a surplus name entry beside a read's projection",
            "GetItem",
            json!({"TableName": TABLE, "Key": key(),
                   "ProjectionExpression": "#s",
                   "ExpressionAttributeNames": {"#s": "status", "#other": "status"}}),
            "ProjectionExpression",
            "ExpressionAttributeNames",
            vec!["#other"],
        ),
    ];

    for (case, operation, request, expression_member, member, keys) in rows {
        let expected = unused_entries(expression_member, member, &keys);
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} {body}"
        );
        let said = message(&body);
        assert_eq!(
            said, expected,
            "case {case:?} ({operation}): expected {expected:?}, got {said:?}"
        );
    }

    // The positive control the precedence rule is about: with every token defined and
    // every entry referenced, the same shape of request is served — so the refusals above
    // come from the substitution tables and not from the operation being unimplemented.
    let deleted = ok(
        &router,
        "DeleteItem",
        json!({"TableName": TABLE, "Key": key(),
               "ConditionExpression": "#s = :v",
               "ExpressionAttributeNames": {"#s": "status"},
               "ExpressionAttributeValues": {":v": {"S": "active"}},
               "ReturnValues": "ALL_OLD"}),
    )
    .await;
    assert_eq!(
        deleted["Attributes"]["status"],
        json!({"S": "active"}),
        "the condition the item satisfies must delete it, got {deleted}"
    );
    let gone = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key()}),
    )
    .await;
    assert_eq!(
        gone.get("Item"),
        None,
        "the deleted item must be gone, got {gone}"
    );
}
