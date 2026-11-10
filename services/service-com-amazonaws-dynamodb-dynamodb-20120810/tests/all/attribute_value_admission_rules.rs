// SPDX-License-Identifier: CC0-1.0

//! Two rules about the VALUE a request carries, each held wherever a value arrives.
//!
//! * The `NULL` member of an attribute value states the attribute's TYPE, and the only
//!   value that states it is `true` ("An attribute of type Null. For example:
//!   `\"NULL\": true`" — `AttributeValue$NULL`), so a `false` is refused and nothing is
//!   stored. The rows span the write surfaces a value can arrive through, where in the
//!   value it sits (top level, inside a list, inside a map, as a set-free scalar of an
//!   update) and the read surface that compares against one.
//! * A number's EXPONENT is read as a 64-bit integer and only then held to the
//!   documented range, so the two defects stay apart: a text whose exponent is a number
//!   states a magnitude, which over- or underflows that range, and only a text whose
//!   exponent is no 64-bit integer cannot be converted to a number at all. The rows walk
//!   both published bounds, one past each of them, both limits of the integer the
//!   exponent is read into and one past each of those.
//!
//! Everything is driven over the wire, so nothing is asserted that a client could not
//! observe.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table every row below writes to.
const TABLE: &str = "ValueAdmission";

/// The envelope a clause about a value an item-API member carried is stated inside.
const ENVELOPE: &str = "1 validation error detected: ";

/// The clause a `NULL` that does not state `true` earns.
const NULL_CLAUSE: &str = "One or more parameter values were invalid: Null attribute value types \
                           must have the value of true";

/// The documented exponent range of a DynamoDB number: "Number: ... can have up to 38
/// digits of precision. Exponent range is 10^-130 to 10^125" ("Service, Account, and
/// Table Quotas").
const MAX_EXPONENT: i32 = 125;
const MIN_EXPONENT: i32 = -130;

/// The two limits of the 64-bit integer an exponent is read into, as a text spells them.
const MAX_SIGNED_64: &str = "9223372036854775807";
const MIN_SIGNED_64: &str = "-9223372036854775808";

/// The clause a magnitude outside the documented range earns, either way out.
const OVERFLOW_CLAUSE: &str =
    "Number overflow. Attempting to store a number with magnitude larger than supported range";
const UNDERFLOW_CLAUSE: &str =
    "Number underflow. Attempting to store a number with magnitude smaller than supported range";

/// The clause a text that is no number at all earns, echoing the literal as sent.
fn conversion_clause(text: &str) -> String {
    format!("The parameter cannot be converted to a numeric value: {text}")
}

/// An emulator holding one active table keyed on `pk` alone.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "the value-admission table", TABLE).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": "seed"}, "a": {"S": "x"}}}),
    )
    .await;
    router
}

#[tokio::test]
async fn a_null_attribute_value_must_state_true() {
    let router = seeded().await;

    // Each row: the case, the operation, the request, and the whole message it must earn
    // — the envelope follows the surface the value arrived through, as every other value
    // clause of this service does.
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a top-level NULL of a put item",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "n1"}, "flag": {"NULL": false}},
            }),
            format!("{ENVELOPE}{NULL_CLAUSE}"),
        ),
        (
            "a NULL inside a list",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "n2"}, "flags": {"L": [{"BOOL": true}, {"NULL": false}]}},
            }),
            format!("{ENVELOPE}{NULL_CLAUSE}"),
        ),
        (
            "a NULL two levels down, inside a map inside a list",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {
                    "pk": {"S": "n3"},
                    "nested": {"L": [{"M": {"flag": {"NULL": false}}}]},
                },
            }),
            format!("{ENVELOPE}{NULL_CLAUSE}"),
        ),
        (
            "a NULL an update puts",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "seed"}},
                "AttributeUpdates": {"flag": {"Action": "PUT", "Value": {"NULL": false}}},
            }),
            format!("{ENVELOPE}{NULL_CLAUSE}"),
        ),
        (
            "a NULL an update expression binds",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "seed"}},
                "UpdateExpression": "SET flag = :v",
                "ExpressionAttributeValues": {":v": {"NULL": false}},
            }),
            format!("{ENVELOPE}{NULL_CLAUSE}"),
        ),
        (
            "a NULL a batched put carries",
            "BatchWriteItem",
            json!({
                "RequestItems": {
                    TABLE: [{"PutRequest": {"Item": {"pk": {"S": "n4"},
                                                     "flag": {"NULL": false}}}}],
                },
            }),
            format!("{ENVELOPE}{NULL_CLAUSE}"),
        ),
        (
            "a NULL a transactional put carries",
            "TransactWriteItems",
            json!({
                "TransactItems": [
                    {"Put": {"TableName": TABLE,
                             "Item": {"pk": {"S": "n5"}, "flag": {"NULL": false}}}},
                ],
            }),
            format!("{ENVELOPE}{NULL_CLAUSE}"),
        ),
        (
            // A read states its value clauses bare, which is the same split every other
            // value rule of this service follows.
            "a NULL a scan's filter compares against",
            "Scan",
            json!({
                "TableName": TABLE,
                "FilterExpression": "flag = :v",
                "ExpressionAttributeValues": {":v": {"NULL": false}},
            }),
            NULL_CLAUSE.to_string(),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.clone()),
            "case {case:?}: expected {expected:?}, got {status} with {answered}"
        );
    }

    // Nothing a refused write carried was stored: every key those rows named holds no
    // item, and the seeded item carries no `flag`.
    for key in ["n1", "n2", "n3", "n4", "n5"] {
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE, "Key": {"pk": {"S": key}}}),
        )
        .await;
        assert_eq!(
            read.get("Item"),
            None,
            "case a refused NULL at {key:?}: a refused write stores nothing, got {read}"
        );
    }
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "seed"}}}),
    )
    .await;
    assert_eq!(
        read["Item"].get("flag"),
        None,
        "case a refused update: the attribute is not written, got {read}"
    );

    // The one value the member takes is stored and read back as itself, wherever it
    // sits — so the rows above are the value `false` being refused rather than the type
    // being rejected.
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": TABLE,
            "Item": {
                "pk": {"S": "ok"},
                "flag": {"NULL": true},
                "nested": {"L": [{"M": {"flag": {"NULL": true}}}]},
            },
        }),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "ok"}}}),
    )
    .await;
    assert_eq!(
        (
            read["Item"]["flag"].clone(),
            read["Item"]["nested"]["L"][0]["M"]["flag"].clone()
        ),
        (json!({"NULL": true}), json!({"NULL": true})),
        "case a NULL stating true: expected it read back as itself, got {read}"
    );
}

#[tokio::test]
async fn an_exponent_is_read_as_a_64_bit_integer_and_then_range_checked() {
    let router = seeded().await;

    // Each row: the case, the `N` text, and the clause the refusal must carry — the
    // magnitude clauses for an exponent that IS a 64-bit integer and leaves the
    // documented range, and the conversion clause only for a text whose exponent is no
    // such integer. The rows walk one past each published bound, both limits of that
    // integer and one past each of them, and a mantissa that normalises to zero (which
    // has no magnitude of its own, so the exponent it wrote is what is held to the
    // range).
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an exponent one past the documented maximum",
            format!("1E{}", MAX_EXPONENT + 1),
            OVERFLOW_CLAUSE.to_string(),
        ),
        (
            "an exponent one past the documented minimum",
            format!("1E{}", MIN_EXPONENT - 1),
            UNDERFLOW_CLAUSE.to_string(),
        ),
        (
            "an exponent at the top of the integer it is read into",
            format!("1E{MAX_SIGNED_64}"),
            OVERFLOW_CLAUSE.to_string(),
        ),
        (
            "an exponent at the bottom of the integer it is read into",
            format!("1E{MIN_SIGNED_64}"),
            UNDERFLOW_CLAUSE.to_string(),
        ),
        (
            "a zero carrying an exponent at the top of that integer",
            format!("0E{MAX_SIGNED_64}"),
            OVERFLOW_CLAUSE.to_string(),
        ),
        (
            "a zero carrying an exponent at the bottom of that integer",
            format!("0E{MIN_SIGNED_64}"),
            UNDERFLOW_CLAUSE.to_string(),
        ),
        (
            "an exponent one past the top of that integer",
            "1E9223372036854775808".to_string(),
            conversion_clause("1E9223372036854775808"),
        ),
        (
            "an exponent one past the bottom of that integer",
            "1E-9223372036854775809".to_string(),
            conversion_clause("1E-9223372036854775809"),
        ),
        (
            "an exponent that is not digits at all",
            "1Eabc".to_string(),
            conversion_clause("1Eabc"),
        ),
        (
            "an exponent marker with nothing after it",
            "1E".to_string(),
            conversion_clause("1E"),
        ),
    ];

    for (case, text, clause) in rows {
        let expected = format!("{ENVELOPE}{clause}");
        let (status, answered) = call(
            &router,
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "number"}, "v": {"N": text.as_str()}},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.clone()),
            "case {case:?} ({text}): expected {expected:?}, got {status} with {answered}"
        );
    }

    // Nothing a refused number carried was stored.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "number"}}}),
    )
    .await;
    assert_eq!(
        read.get("Item"),
        None,
        "case a refused number: no item is written, got {read}"
    );

    // Both published bounds themselves are STORED and read back, so the refusals above
    // are the bound being crossed rather than a rejection of every exponent.
    for (case, text, canonical) in [
        (
            "the documented maximum exponent",
            format!("1E{MAX_EXPONENT}"),
            format!("1{}", "0".repeat(usize::try_from(MAX_EXPONENT).unwrap_or(0))),
        ),
        (
            "the documented minimum exponent",
            format!("1E{MIN_EXPONENT}"),
            format!(
                "0.{}1",
                "0".repeat(usize::try_from(-(MIN_EXPONENT + 1)).unwrap_or(0))
            ),
        ),
        (
            "a zero carrying an exponent inside the range",
            "0E125".to_string(),
            "0".to_string(),
        ),
    ] {
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "bounds"}, "v": {"N": text.as_str()}},
            }),
        )
        .await;
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE, "Key": {"pk": {"S": "bounds"}}}),
        )
        .await;
        assert_eq!(
            read["Item"]["v"]["N"].as_str(),
            Some(canonical.as_str()),
            "case {case:?} ({text}): expected it read back as {canonical:?}, got {read}"
        );
    }
}
