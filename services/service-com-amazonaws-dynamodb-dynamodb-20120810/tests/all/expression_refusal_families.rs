// SPDX-License-Identifier: CC0-1.0

//! WHICH FAMILY a refusal about a request's expressions belongs to, driven end to end.
//!
//! DynamoDB words a 400 `ValidationException` in two families: the constraint envelope
//! `<N> validation error(s) detected: ` over one clause, and a bare sentence. This suite
//! pins the rule that decides which family two refusals about a request's expressions
//! belong to:
//!
//! * the documented 4 KB ceiling on one expression string ("Maximum length of any
//!   expression string is 4 KB") is refused with `Invalid <member>: Expression size has
//!   exceeded the maximum allowed size;`, and
//! * a request that states ONE thing through a legacy member and through the expression
//!   member that replaced it is refused with `Can not use both expression and
//!   non-expression parameters in the same request: …`,
//!
//! and each of them carries the envelope exactly when the members it is about are the ones
//! a WRITE states its intent through (`UpdateExpression`, `ConditionExpression` and the
//! legacy `AttributeUpdates`/`Expected` they replaced) and is stated as the bare sentence
//! when they are the ones a READ narrows its result with (`ProjectionExpression`,
//! `FilterExpression`, `KeyConditionExpression` and the legacy `AttributesToGet`,
//! `ScanFilter`, `QueryFilter`, `KeyConditions`). A refusal about the expression members of
//! ONE ELEMENT of a multi-item request is stated bare whichever member it names, because
//! the envelope reports violations of the REQUEST's own members
//! (`element_expression_envelopes` pins that rule for the substitution tables).
//!
//! The rows differ along what the rule generalises over: the expression member, the
//! operation carrying it, whether the member is the request's own or one element's, how far
//! past the 4 KB ceiling the expression went, and which pair of generations collided. The
//! ceiling is tested AT the published bound (served) and one byte past it (refused).
//!
//! Also pinned here: the FIXED sentence an `UpdateTable` that asks for no change earns,
//! which carries nothing of the request.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row addresses: one string partition key.
const TABLE: &str = "ExpressionRefusalFamilies";

/// The envelope a refusal about the REQUEST's own write-intent members is stated inside,
/// over one clause.
const ENVELOPE: &str = "1 validation error detected: ";

/// The most one expression string may carry: "Maximum length of any expression string is
/// 4 KB" ("Service, Account, and Table Quotas").
const MAX_EXPRESSION_BYTES: usize = 4096;

/// An expression of exactly `bytes` bytes: one attribute name, which is what every member
/// below accepts in place of a shorter one, so the only thing a row varies is its SIZE.
fn sized_expression(bytes: usize) -> String {
    "a".repeat(bytes)
}

/// The whole message an expression past the ceiling earns, in or out of the envelope.
fn size_refusal(member: &str, enveloped: bool) -> String {
    let stated = format!("Invalid {member}: Expression size has exceeded the maximum allowed size;");
    if enveloped {
        return format!("{ENVELOPE}{stated}");
    }
    stated
}

/// The whole message a request stating one thing in both generations earns, in or out of
/// the envelope.
fn conflict_refusal(legacy: &str, expression: &str, enveloped: bool) -> String {
    let stated = format!(
        "Can not use both expression and non-expression parameters in the same request: \
         Non-expression parameters: {{{legacy}}} Expression parameters: {{{expression}}}"
    );
    if enveloped {
        return format!("{ENVELOPE}{stated}");
    }
    stated
}

/// The key of the item every row addresses, which the seed writes.
fn key() -> Value {
    json!({"pk": {"S": "item-1"}})
}

/// The emulator with the table and one item in place, so no row is refused for its table
/// or its key instead of for the rule under test.
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

/// One row: the case, the operation, the request, and the whole message it must earn.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    expected: String,
}

/// The rows for the 4 KB ceiling: one oversized expression per request, every other
/// expression it carries sound, so the message can only name the oversized member.
fn size_rows() -> Vec<Row> {
    let past = sized_expression(MAX_EXPRESSION_BYTES + 1);
    let far_past = sized_expression(MAX_EXPRESSION_BYTES * 4);
    vec![
        Row {
            case: "a read's own projection, one byte past the ceiling",
            operation: "GetItem",
            request: json!({"TableName": TABLE, "Key": key(), "ProjectionExpression": past}),
            expected: size_refusal("ProjectionExpression", false),
        },
        Row {
            case: "the same member four times past the ceiling",
            operation: "GetItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "ProjectionExpression": far_past}),
            expected: size_refusal("ProjectionExpression", false),
        },
        Row {
            case: "a scan's own projection",
            operation: "Scan",
            request: json!({"TableName": TABLE, "ProjectionExpression": past}),
            expected: size_refusal("ProjectionExpression", false),
        },
        Row {
            case: "a scan's own filter",
            operation: "Scan",
            request: json!({"TableName": TABLE, "FilterExpression": past}),
            expected: size_refusal("FilterExpression", false),
        },
        Row {
            case: "a query's own key condition",
            operation: "Query",
            request: json!({"TableName": TABLE, "KeyConditionExpression": past}),
            expected: size_refusal("KeyConditionExpression", false),
        },
        Row {
            case: "a query's own projection, beside a sound key condition",
            operation: "Query",
            request: json!({"TableName": TABLE, "KeyConditionExpression": "pk = :p",
                            "ExpressionAttributeValues": {":p": {"S": "item-1"}},
                            "ProjectionExpression": past}),
            expected: size_refusal("ProjectionExpression", false),
        },
        Row {
            case: "a write's own condition",
            operation: "PutItem",
            request: json!({"TableName": TABLE, "Item": {"pk": {"S": "item-2"}},
                            "ConditionExpression": past}),
            expected: size_refusal("ConditionExpression", true),
        },
        Row {
            case: "a delete's own condition",
            operation: "DeleteItem",
            request: json!({"TableName": TABLE, "Key": key(), "ConditionExpression": past}),
            expected: size_refusal("ConditionExpression", true),
        },
        Row {
            case: "an update's own update expression",
            operation: "UpdateItem",
            request: json!({"TableName": TABLE, "Key": key(), "UpdateExpression": past}),
            expected: size_refusal("UpdateExpression", true),
        },
        Row {
            case: "one batched read entry's projection",
            operation: "BatchGetItem",
            request: json!({"RequestItems": {TABLE: {"Keys": [key()],
                                                     "ProjectionExpression": past}}}),
            expected: size_refusal("ProjectionExpression", false),
        },
        Row {
            case: "one transacted read element's projection",
            operation: "TransactGetItems",
            request: json!({"TransactItems": [{"Get": {"TableName": TABLE, "Key": key(),
                                                       "ProjectionExpression": past}}]}),
            expected: size_refusal("ProjectionExpression", false),
        },
        Row {
            case: "one transacted put element's condition",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [{"Put": {"TableName": TABLE,
                                                       "Item": {"pk": {"S": "item-3"}},
                                                       "ConditionExpression": past}}]}),
            expected: size_refusal("ConditionExpression", false),
        },
        Row {
            case: "one transacted update element's update expression",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [{"Update": {"TableName": TABLE, "Key": key(),
                                                          "UpdateExpression": past}}]}),
            expected: size_refusal("UpdateExpression", false),
        },
        Row {
            case: "one transacted condition check element's condition",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [{"ConditionCheck": {"TableName": TABLE,
                                                                  "Key": key(),
                                                                  "ConditionExpression": past}}]}),
            expected: size_refusal("ConditionExpression", false),
        },
    ]
}

/// The rows for a request that states one thing in both generations: the collision is the
/// legacy member beside the expression member that replaced it.
fn conflict_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a read stating its projection both ways",
            operation: "GetItem",
            request: json!({"TableName": TABLE, "Key": key(), "AttributesToGet": ["status"],
                            "ProjectionExpression": "status"}),
            expected: conflict_refusal("AttributesToGet", "ProjectionExpression", false),
        },
        Row {
            case: "a scan stating its filter both ways",
            operation: "Scan",
            request: json!({"TableName": TABLE,
                            "ScanFilter": {"status": {"ComparisonOperator": "NOT_NULL"}},
                            "FilterExpression": "attribute_exists(status)"}),
            expected: conflict_refusal("ScanFilter", "FilterExpression", false),
        },
        Row {
            case: "a query stating its key condition both ways",
            operation: "Query",
            request: json!({"TableName": TABLE,
                            "KeyConditions": {"pk": {"ComparisonOperator": "EQ",
                                                     "AttributeValueList": [{"S": "item-1"}]}},
                            "KeyConditionExpression": "pk = :p",
                            "ExpressionAttributeValues": {":p": {"S": "item-1"}}}),
            expected: conflict_refusal("KeyConditions", "KeyConditionExpression", false),
        },
        Row {
            case: "a put stating its condition both ways",
            operation: "PutItem",
            request: json!({"TableName": TABLE, "Item": {"pk": {"S": "item-4"}},
                            "Expected": {"status": {"Exists": false}},
                            "ConditionExpression": "attribute_not_exists(status)"}),
            expected: conflict_refusal("Expected", "ConditionExpression", true),
        },
        Row {
            case: "a delete stating its condition both ways",
            operation: "DeleteItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "Expected": {"status": {"Exists": false}},
                            "ConditionExpression": "attribute_not_exists(status)"}),
            expected: conflict_refusal("Expected", "ConditionExpression", true),
        },
        Row {
            case: "an update stating its update both ways",
            operation: "UpdateItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "AttributeUpdates": {"status": {"Action": "PUT",
                                                            "Value": {"S": "paused"}}},
                            "UpdateExpression": "SET #s = :v",
                            "ExpressionAttributeNames": {"#s": "status"},
                            "ExpressionAttributeValues": {":v": {"S": "paused"}}}),
            expected: conflict_refusal("AttributeUpdates", "UpdateExpression", true),
        },
        Row {
            case: "an update stating its condition both ways, beside a sound update",
            operation: "UpdateItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "UpdateExpression": "SET #s = :v",
                            "ExpressionAttributeNames": {"#s": "status"},
                            "ExpressionAttributeValues": {":v": {"S": "paused"}},
                            "Expected": {"status": {"Exists": false}},
                            "ConditionExpression": "attribute_not_exists(status)"}),
            expected: conflict_refusal("Expected", "UpdateExpression, ConditionExpression", true),
        },
    ]
}

#[tokio::test]
async fn a_refusal_about_a_requests_expressions_carries_the_envelope_the_member_earns() {
    let router = seeded().await;

    for row in size_rows().into_iter().chain(conflict_rows()) {
        let (status, body) = call(&router, row.operation, row.request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {:?} ({}): expected 400 ValidationException, got {status} with {body}",
            row.case,
            row.operation
        );
        let stated = message(&body);
        assert_eq!(
            stated, row.expected,
            "case {:?} ({}): expected {:?}, got {stated:?}",
            row.case, row.operation, row.expected
        );
    }

    // Nothing any of those rows asked for was applied: the one seeded item still reads
    // back as it was written, and the keys the refused writes addressed hold nothing.
    let read = ok(&router, "GetItem", json!({"TableName": TABLE, "Key": key()})).await;
    assert_eq!(
        read["Item"]["status"],
        json!({"S": "active"}),
        "the refused requests must change nothing, got {read}"
    );
    for addressed in ["item-2", "item-3", "item-4"] {
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE, "Key": {"pk": {"S": addressed}}}),
        )
        .await;
        assert!(
            read.get("Item").is_none(),
            "case {addressed:?}: a refused write stores nothing, got {read}"
        );
    }
}

#[tokio::test]
async fn an_expression_at_the_published_ceiling_is_not_refused_for_its_size() {
    let router = seeded().await;

    // The same members filled to exactly the published bound: whatever each operation
    // answers, none of them may be refused for the expression's SIZE — which is what makes
    // the rows above the bound and not an off-by-one. A projection naming one attribute
    // the item does not carry is served and selects nothing.
    let at_bound = sized_expression(MAX_EXPRESSION_BYTES);
    for (case, operation, request) in [
        (
            "a read's projection at the ceiling",
            "GetItem",
            json!({"TableName": TABLE, "Key": key(), "ProjectionExpression": at_bound.clone()}),
        ),
        (
            "a scan's projection at the ceiling",
            "Scan",
            json!({"TableName": TABLE, "ProjectionExpression": at_bound.clone()}),
        ),
        (
            "one transacted read element's projection at the ceiling",
            "TransactGetItems",
            json!({"TransactItems": [{"Get": {"TableName": TABLE, "Key": key(),
                                              "ProjectionExpression": at_bound.clone()}}]}),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        let stated = message(&body);
        assert!(
            !stated.contains("Expression size has exceeded"),
            "case {case:?} ({operation}): an expression at the published bound is not past it, \
             got {status} with {stated:?}"
        );
        // And it is not refused at all: a projection at the ceiling names one attribute the
        // item does not carry, which selects nothing and is served.
        assert_eq!(
            status, 200,
            "case {case:?} ({operation}): an expression at the published bound is served, got \
             {status} with {body}"
        );
    }
}

/// Every way a request can ask an `UpdateTable` for no change at all.
///
/// The sentence is the service's own fixed text: it names the members the service accepts,
/// in the order it names them, and carries nothing of the request — so every row earns the
/// same message, and it is stated on its own rather than inside the constraint envelope.
#[tokio::test]
async fn an_update_table_that_asks_for_nothing_earns_one_fixed_sentence() {
    let router = emulator();
    create_active_table(&router, "UpdateTableNothing", &[("pk", "HASH", "S")]).await;

    const EXPECTED: &str = "At least one of ProvisionedThroughput, BillingMode, \
                            UpdateStreamEnabled, GlobalSecondaryIndexUpdates, SSESpecification, \
                            ReplicaUpdates, MultiAccountReplicaReady, ReplicaTransitRoleArn, \
                            MultiRegionConsistency, DeletionProtectionEnabled, \
                            OnDemandThroughput, WarmThroughput or TableClass is required";

    // The rows differ in what the request carries BESIDES the table name: nothing, a
    // declaration that changes nothing on its own, a list that asks for no entry, and both
    // at once. A member with a `@length` minimum of its own (`ReplicaUpdates`) is left out:
    // an empty one of those breaks that bound and is reported against it, which is a
    // different refusal.
    for (case, extra) in [
        ("the table name alone", json!({})),
        (
            "attribute definitions only",
            json!({"AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}]}),
        ),
        (
            "an empty index update list",
            json!({"GlobalSecondaryIndexUpdates": []}),
        ),
        (
            "an empty index update list and a declaration together",
            json!({"AttributeDefinitions": [], "GlobalSecondaryIndexUpdates": []}),
        ),
    ] {
        let mut request = extra.clone();
        request["TableName"] = json!("UpdateTableNothing");
        let (status, body) = call(&router, "UpdateTable", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: an update that asks for nothing is invalid, got {status} with {body}"
        );
        let stated = message(&body);
        assert_eq!(
            stated, EXPECTED,
            "case {case:?}: expected {EXPECTED:?}, got {stated:?}"
        );
        assert!(
            !stated.starts_with(ENVELOPE),
            "case {case:?}: the sentence is stated on its own, got {stated:?}"
        );
    }

    // A request that DOES ask for a change is served, so the rows above are about the
    // empty request and not about the table.
    let served = ok(
        &router,
        "UpdateTable",
        json!({"TableName": "UpdateTableNothing", "DeletionProtectionEnabled": true}),
    )
    .await;
    assert_eq!(
        served["TableDescription"]["DeletionProtectionEnabled"],
        json!(true),
        "an update that names a member is served, got {served}"
    );
}
