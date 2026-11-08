// SPDX-License-Identifier: CC0-1.0

//! WHERE the `<N> validation error(s) detected: ` envelope belongs, driven end to end.
//!
//! RULE under test: that envelope reports violations of the REQUEST's own members, so a
//! refusal about the expression members and substitution tables of ONE ELEMENT of a
//! multi-item request — one `TransactItems` action, one `RequestItems` entry — is stated as
//! the bare sentence, whichever member the element carried. A request that offers the same
//! tables through its OWN members is unchanged: there the envelope follows the member, which
//! `unused_substitution_key_envelopes` spans.
//!
//! The rows differ along what the rule generalises over: the operation (`TransactWriteItems`,
//! `TransactGetItems`, `BatchGetItem`, and the single-item writes and reads for the
//! boundary), the element kind (`Put`, `Update`, `Delete`, `ConditionCheck`, `Get`), the
//! expression member the element carried (`ConditionExpression`, `UpdateExpression`,
//! `ProjectionExpression`, and none at all), which diagnosis the element earns (an entry no
//! expression references, a token no entry defines, a table offered with no expression at
//! all), which substitution table carries it
//! (`ExpressionAttributeNames`, `ExpressionAttributeValues`), how many entries are unused
//! (one, two, three) and where the faulty element sits among its siblings (first, second).
//!
//! Both sentences are the ones this crate states once, in `undefined_expression_tokens`, so
//! no suite spells either of them twice.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;
use crate::undefined_expression_tokens::{
    expression_refusal_in_element, undefined_name_clause, undefined_value_clause, unused_entries,
    unused_entries_in_element,
};

/// The table every row below drives: one string partition key.
const TABLE: &str = "ElementExpressionEnvelopes";

/// The envelope a refusal about a REQUEST's own members is stated inside, over one clause.
const ENVELOPE: &str = "1 validation error detected: ";

/// The sentence a substitution table offered with NO expression to substitute into earns:
/// the member itself is the mistake, so the refusal names it rather than one of its
/// entries, and it is an element's own sentence like every other refusal about an element.
const SUBSTITUTION_WITHOUT_EXPRESSION: &str =
    "ExpressionAttributeNames can only be specified when using expressions";

/// The key of the first seeded item, which most rows address so no row is refused for its
/// key instead of for its substitution tables.
fn key() -> Value {
    json!({"pk": {"S": "item-1"}})
}

/// A second seeded item, so a row can put the faulty element beside a sound one.
fn other_key() -> Value {
    json!({"pk": {"S": "item-2"}})
}

/// The key the `Put` rows address, which nothing seeds: a refused request must leave it
/// unwritten.
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
                   "Item": {"pk": {"S": name}, "status": {"S": "active"},
                            "category": {"S": "books"}}}),
        )
        .await;
    }
    router
}

/// Every key a row addresses, as it stands, for the assertions that a refused request wrote
/// nothing.
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

/// One row: the case, the operation, the request, the whole message it must earn, and
/// whether that message is stated inside the envelope.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    expected: String,
    enveloped: bool,
}

/// The rows whose expression members belong to one ELEMENT of the request.
fn element_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a name a transacted put's condition never references",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [{"Put": {
                "TableName": TABLE,
                "Item": {"pk": {"S": "item-3"}, "status": {"S": "paused"}},
                "ConditionExpression": "attribute_not_exists(pk)",
                "ExpressionAttributeNames": {"#unused": "labels"}}}]}),
            expected: unused_entries_in_element("ExpressionAttributeNames", &["#unused"]),
            enveloped: false,
        },
        Row {
            case: "the same name on the SECOND of two transacted puts",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [
                {"Put": {"TableName": TABLE,
                         "Item": {"pk": {"S": "item-4"}, "status": {"S": "paused"}}}},
                {"Put": {"TableName": TABLE,
                         "Item": {"pk": {"S": "item-3"}, "status": {"S": "paused"}},
                         "ConditionExpression": "attribute_not_exists(pk)",
                         "ExpressionAttributeNames": {"#unused": "labels"}}}]}),
            expected: unused_entries_in_element("ExpressionAttributeNames", &["#unused"]),
            enveloped: false,
        },
        Row {
            case: "a value a transacted update's update expression never references",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [{"Update": {
                "TableName": TABLE,
                "Key": key(),
                "UpdateExpression": "SET #s = :v",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":v": {"S": "paused"},
                                              ":spare": {"S": "gone"}}}}]}),
            expected: unused_entries_in_element("ExpressionAttributeValues", &[":spare"]),
            enveloped: false,
        },
        Row {
            case: "two names a transacted delete's condition never references",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [{"Delete": {
                "TableName": TABLE,
                "Key": key(),
                "ConditionExpression": "attribute_exists(pk)",
                "ExpressionAttributeNames": {"#a": "status", "#b": "category"}}}]}),
            expected: unused_entries_in_element("ExpressionAttributeNames", &["#a", "#b"]),
            enveloped: false,
        },
        Row {
            case: "three values a transacted condition check never references",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [{"ConditionCheck": {
                "TableName": TABLE,
                "Key": key(),
                "ConditionExpression": "attribute_exists(pk)",
                "ExpressionAttributeValues": {":a": {"S": "one"}, ":b": {"S": "two"},
                                              ":c": {"S": "three"}}}}]}),
            expected: unused_entries_in_element("ExpressionAttributeValues", &[":a", ":b", ":c"]),
            enveloped: false,
        },
        Row {
            case: "a name a transacted put offers with no expression at all",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [{"Put": {
                "TableName": TABLE,
                "Item": {"pk": {"S": "item-3"}, "status": {"S": "paused"}},
                "ExpressionAttributeNames": {"#unused": "labels"}}}]}),
            expected: SUBSTITUTION_WITHOUT_EXPRESSION.to_string(),
            enveloped: false,
        },
        Row {
            case: "a token a transacted put's condition dereferences and nothing defines",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [{"Put": {
                "TableName": TABLE,
                "Item": {"pk": {"S": "item-3"}, "status": {"S": "paused"}},
                "ConditionExpression": "attribute_not_exists(#missing)"}}]}),
            expected: expression_refusal_in_element(
                "ConditionExpression",
                &undefined_name_clause("#missing"),
            ),
            enveloped: false,
        },
        Row {
            case: "a value a transacted update's update expression dereferences and nothing \
                   defines",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [{"Update": {
                "TableName": TABLE,
                "Key": key(),
                "UpdateExpression": "SET #s = :missing",
                "ExpressionAttributeNames": {"#s": "status"}}}]}),
            expected: expression_refusal_in_element(
                "UpdateExpression",
                &undefined_value_clause(":missing"),
            ),
            enveloped: false,
        },
        Row {
            case: "a name one transacted read offers with no projection",
            operation: "TransactGetItems",
            request: json!({"TransactItems": [{"Get": {
                "TableName": TABLE,
                "Key": key(),
                "ExpressionAttributeNames": {"#unused": "status"}}}]}),
            expected: SUBSTITUTION_WITHOUT_EXPRESSION.to_string(),
            enveloped: false,
        },
        Row {
            case: "a name the SECOND transacted read offers, behind a sound one",
            operation: "TransactGetItems",
            request: json!({"TransactItems": [
                {"Get": {"TableName": TABLE, "Key": key()}},
                {"Get": {"TableName": TABLE, "Key": other_key(),
                         "ExpressionAttributeNames": {"#unused": "status"}}}]}),
            expected: SUBSTITUTION_WITHOUT_EXPRESSION.to_string(),
            enveloped: false,
        },
        Row {
            case: "two names one batch-read table entry offers with no projection",
            operation: "BatchGetItem",
            request: json!({"RequestItems": {TABLE: {
                "Keys": [key()],
                "ExpressionAttributeNames": {"#a": "status", "#b": "category"}}}}),
            expected: SUBSTITUTION_WITHOUT_EXPRESSION.to_string(),
            enveloped: false,
        },
    ]
}

/// The BOUNDARY rows: the same two diagnoses about a request's OWN expression members, which
/// keep the envelope wherever they carried it before.
fn request_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a value a write's own condition never references",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": {"pk": {"S": "item-3"}, "status": {"S": "paused"}},
                            "ConditionExpression": "attribute_not_exists(pk)",
                            "ExpressionAttributeValues": {":spare": {"S": "gone"}}}),
            expected: unused_entries(
                "ConditionExpression",
                "ExpressionAttributeValues",
                &[":spare"],
            ),
            enveloped: true,
        },
        Row {
            case: "a name an update's own update expression never references",
            operation: "UpdateItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "UpdateExpression": "SET category = :v",
                            "ExpressionAttributeNames": {"#unused": "labels"},
                            "ExpressionAttributeValues": {":v": {"S": "music"}}}),
            expected: unused_entries("UpdateExpression", "ExpressionAttributeNames", &["#unused"]),
            enveloped: true,
        },
        Row {
            case: "a name a read's own projection never references",
            operation: "GetItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "ProjectionExpression": "#s",
                            "ExpressionAttributeNames": {"#s": "status",
                                                         "#unused": "labels"}}),
            expected: unused_entries(
                "ProjectionExpression",
                "ExpressionAttributeNames",
                &["#unused"],
            ),
            enveloped: false,
        },
    ]
}

#[tokio::test]
async fn an_elements_substitution_tables_are_refused_without_the_request_envelope() {
    let router = seeded().await;
    let before = stored(&router).await;

    for row in element_rows().into_iter().chain(request_rows()) {
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
            said, row.expected,
            "case {:?} ({}): expected {:?}, got {said:?}",
            row.case, row.operation, row.expected
        );
        assert_eq!(
            said.starts_with(ENVELOPE),
            row.enveloped,
            "case {:?} ({}): the envelope belongs to the request's own members, got {said:?}",
            row.case,
            row.operation
        );
    }

    let after = stored(&router).await;
    assert_eq!(
        after, before,
        "the refused requests must write nothing: expected {before:?}, got {after:?}"
    );
}

#[tokio::test]
async fn an_element_that_references_every_entry_it_defines_is_served() {
    let router = seeded().await;

    // The control for every element row above: the same shapes with each entry referenced
    // are SERVED, so the refusals come from the substitution tables and not from the
    // elements being unserved.
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [
            sound_element(),
            {"Update": {
                "TableName": TABLE,
                "Key": key(),
                "UpdateExpression": "SET #s = :paused",
                "ConditionExpression": "#s = :active",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":paused": {"S": "paused"},
                                              ":active": {"S": "active"}}}},
            {"Put": {
                "TableName": TABLE,
                "Item": {"pk": {"S": "item-3"}, "status": {"S": "new"}},
                "ConditionExpression": "attribute_not_exists(#k)",
                "ExpressionAttributeNames": {"#k": "pk"}}}]}),
    )
    .await;
    let settled = stored(&router).await;
    assert_eq!(
        settled[0]["status"],
        json!({"S": "paused"}),
        "the transacted update whose every token resolves must apply, got {settled:?}"
    );
    assert_eq!(
        settled[2]["status"],
        json!({"S": "new"}),
        "the transacted put whose every token resolves must land, got {settled:?}"
    );

    // And the two multi-item READS, whose entries define nothing at all: each answers the
    // item it addressed.
    let transacted = ok(
        &router,
        "TransactGetItems",
        json!({"TransactItems": [{"Get": {"TableName": TABLE, "Key": key()}}]}),
    )
    .await;
    assert_eq!(
        transacted["Responses"][0]["Item"]["status"],
        json!({"S": "paused"}),
        "the transacted read must answer the item it addressed, got {transacted}"
    );
    let batched = ok(
        &router,
        "BatchGetItem",
        json!({"RequestItems": {TABLE: {"Keys": [key()]}}}),
    )
    .await;
    assert_eq!(
        batched["Responses"][TABLE][0]["status"],
        json!({"S": "paused"}),
        "the batch read must answer the item it addressed, got {batched}"
    );
}
