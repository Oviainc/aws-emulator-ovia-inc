// SPDX-License-Identifier: CC0-1.0

//! Two rules, each asserted over the class it generalises over.
//!
//! * A legacy `ScanFilter`/`QueryFilter` comparison evaluates over the attribute an item
//!   HOLDS, so an item that carries no value for the name satisfies no comparison — the
//!   documented exception being `NE` — and a comparison over a binary value compares
//!   BYTES, "each byte of the binary data as unsigned" (`Condition$AttributeValueList`),
//!   so `CONTAINS` finds any run of bytes, `0x00` among them.
//! * A value a PartiQL statement carries — written out as a literal or bound through a
//!   `Parameters` entry — is held to the same documented value rules a stored one is:
//!   the numeric domain (38 significant digits inside the documented exponent range, a
//!   zero included) and the set rules.
//!
//! Every row states the case and the value it got, and nothing here reads one recorded
//! request: the rows span the comparison vocabulary, the three set types and both ends
//! of the numeric range.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table the legacy rows scan: one partition key, and items that differ in whether
/// they carry the filtered attribute at all.
const TABLE: &str = "rows";

/// The documented ceiling on `N` precision, and one past it.
const THIRTY_EIGHT_DIGITS: &str = "12345678901234567890123456789012345678";
const THIRTY_NINE_DIGITS: &str = "123456789012345678901234567890123456789";

/// An emulator holding `TABLE` with four items: two carrying `status`, one carrying it
/// with another value, and one carrying none at all.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    for (pk, status) in [
        ("has-match", Some("alpha")),
        ("has-other", Some("beta")),
        ("has-empty-ish", Some("al")),
        ("no-attr", None),
    ] {
        let mut item = json!({"pk": {"S": pk}});
        if let Some(status) = status {
            item["status"] = json!({"S": status});
        }
        ok(
            &router,
            "PutItem",
            json!({"TableName": TABLE, "Item": item}),
        )
        .await;
    }
    router
}

/// The partition keys a scan reported, in the order it reported them.
fn reported(answered: &Value) -> Vec<String> {
    answered["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["pk"]["S"].as_str().map(ToString::to_string))
        .collect()
}

/// One legacy comparison, as a `ScanFilter` entry over `status`.
fn scan_filter(operator: &str, operands: Value) -> Value {
    let mut condition = json!({"ComparisonOperator": operator});
    if !operands.is_null() {
        condition["AttributeValueList"] = operands;
    }
    json!({"status": condition})
}

#[tokio::test]
async fn a_legacy_comparison_selects_only_items_that_hold_the_attribute() {
    let router = seeded().await;

    // Each row: the comparison, its operands, and the items it selects. The rows span
    // the vocabulary `Condition$ComparisonOperator` declares over one attribute — the
    // equalities, the orderings, the two substring forms and the two existence forms —
    // and what each row asserts is whether the item carrying NO `status` is in the
    // answer: only `NE` and `NULL` select it.
    let rows: Vec<(&str, &str, Value, Vec<&str>)> = vec![
        (
            "EQ over a value one item holds",
            "EQ",
            json!([{"S": "alpha"}]),
            vec!["has-match"],
        ),
        (
            "NE, which the item holding no value satisfies too",
            "NE",
            json!([{"S": "alpha"}]),
            vec!["has-empty-ish", "has-other", "no-attr"],
        ),
        (
            "CONTAINS over a substring",
            "CONTAINS",
            json!([{"S": "al"}]),
            vec!["has-empty-ish", "has-match"],
        ),
        (
            "NOT_CONTAINS, which excludes the item holding no value",
            "NOT_CONTAINS",
            json!([{"S": "al"}]),
            vec!["has-other"],
        ),
        (
            "BEGINS_WITH, which excludes it as well",
            "BEGINS_WITH",
            json!([{"S": "al"}]),
            vec!["has-empty-ish", "has-match"],
        ),
        (
            "GT, which has nothing to compare for it",
            "GT",
            json!([{"S": "alpha"}]),
            vec!["has-other"],
        ),
        (
            "LT",
            "LT",
            json!([{"S": "alpha"}]),
            vec!["has-empty-ish"],
        ),
        (
            "BETWEEN",
            "BETWEEN",
            json!([{"S": "al"}, {"S": "alpha"}]),
            vec!["has-empty-ish", "has-match"],
        ),
        (
            "IN",
            "IN",
            json!([{"S": "alpha"}, {"S": "beta"}]),
            vec!["has-match", "has-other"],
        ),
        (
            "NOT_NULL, which is the attribute being there",
            "NOT_NULL",
            Value::Null,
            vec!["has-empty-ish", "has-match", "has-other"],
        ),
        (
            "NULL, which is the attribute being absent",
            "NULL",
            Value::Null,
            vec!["no-attr"],
        ),
    ];

    for (case, operator, operands, expected) in rows {
        let answered = ok(
            &router,
            "Scan",
            json!({"TableName": TABLE, "ScanFilter": scan_filter(operator, operands)}),
        )
        .await;
        let mut got = reported(&answered);
        got.sort();
        let mut want: Vec<String> = expected.iter().map(ToString::to_string).collect();
        want.sort();
        assert_eq!(
            got, want,
            "case {case:?}: expected the items {want:?}, got {got:?} from {answered}"
        );
    }
}

#[tokio::test]
async fn a_legacy_filter_over_a_binary_value_compares_unsigned_bytes() {
    let router = emulator();
    create_active_table(&router, "bytes", &[("pk", "HASH", "S")]).await;
    // Each stored value is a run of bytes, named by what it holds: the rows differ in
    // whether the needle appears in them, and the bytes span the signed/unsigned
    // boundary (0x80) and the zero byte a text encoding could not carry.
    for (pk, value) in [
        ("zero-inside", vec![1u8, 0, 2]),
        ("high-bytes", vec![0x80u8, 0xFF]),
        ("plain", vec![b'a', b'b', b'c']),
    ] {
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": "bytes",
                "Item": {"pk": {"S": pk},
                         "v": {"B": ::aws_smithy_types::base64::encode(&value)}},
            }),
        )
        .await;
    }

    // Each row: the needle, and the stored values that hold it as a contiguous run.
    let rows: Vec<(&str, Vec<u8>, Vec<&str>)> = vec![
        ("a zero byte", vec![0], vec!["zero-inside"]),
        ("a run spanning the zero byte", vec![1, 0], vec!["zero-inside"]),
        ("a byte past the signed range", vec![0x80], vec!["high-bytes"]),
        ("the highest byte", vec![0xFF], vec!["high-bytes"]),
        ("a run of printable bytes", vec![b'b', b'c'], vec!["plain"]),
        ("a run no value holds", vec![0, 1], vec![]),
    ];

    for (case, needle, expected) in rows {
        let answered = ok(
            &router,
            "Scan",
            json!({
                "TableName": "bytes",
                "ScanFilter": {"v": {
                    "ComparisonOperator": "CONTAINS",
                    "AttributeValueList": [
                        {"B": ::aws_smithy_types::base64::encode(&needle)},
                    ],
                }},
            }),
        )
        .await;
        let mut got = reported(&answered);
        got.sort();
        let mut want: Vec<String> = expected.iter().map(ToString::to_string).collect();
        want.sort();
        assert_eq!(
            got, want,
            "case {case:?}: expected the items {want:?}, got {got:?} from {answered}"
        );
    }
}

#[tokio::test]
async fn a_begins_with_operand_that_has_no_prefix_is_refused() {
    let router = seeded().await;

    // "AttributeValueList can contain only one AttributeValue of type String or Binary
    // (not a Number or a set type)" (`Condition$ComparisonOperator`, BEGINS_WITH): the
    // rows span the types that have no prefix.
    for (case, operand) in [
        ("a number", json!({"N": "1"})),
        ("a boolean", json!({"BOOL": true})),
        ("the null type", json!({"NULL": true})),
        ("a string set", json!({"SS": ["a"]})),
        ("a list", json!({"L": [{"S": "a"}]})),
        ("a map", json!({"M": {"a": {"S": "b"}}})),
    ] {
        let (status, answered) = call(
            &router,
            "Scan",
            json!({
                "TableName": TABLE,
                "ScanFilter": scan_filter("BEGINS_WITH", json!([operand])),
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case BEGINS_WITH given {case}: expected 400 ValidationException, got {status} with \
             {answered}"
        );
        assert!(
            message(&answered).starts_with(
                "One or more parameter values were invalid: ComparisonOperator BEGINS_WITH is \
                 not valid for "
            ),
            "case BEGINS_WITH given {case}: expected the operand-type sentence, got {:?}",
            message(&answered)
        );
    }

    // The CONTROL: the two types that do have a prefix are served.
    for (case, operand) in [
        ("a string", json!({"S": "al"})),
        ("a binary value", json!({"B": "YQ=="})),
    ] {
        let answered = ok(
            &router,
            "Scan",
            json!({
                "TableName": TABLE,
                "ScanFilter": scan_filter("BEGINS_WITH", json!([operand])),
            }),
        )
        .await;
        assert!(
            answered["Items"].is_array(),
            "case BEGINS_WITH given {case}: expected a served scan, got {answered}"
        );
    }
}

#[tokio::test]
async fn an_in_comparison_is_held_to_a_hundred_operands_on_both_surfaces() {
    let router = seeded().await;

    // "The IN operator can have up to 100 comparison values" (the comparator and
    // function reference for `ConditionExpression`/`FilterExpression`), the same ceiling
    // the legacy `IN` comparison's `AttributeValueList` is held to. The rows are the
    // published bound, one past it and one well past it.
    for (case, count, refused) in [
        ("the published ceiling", 100usize, false),
        ("one operand past it", 101, true),
        ("well past it", 150, true),
    ] {
        let placeholders: Vec<String> = (0..count).map(|at| format!(":v{at}")).collect();
        let values: serde_json::Map<String, Value> = placeholders
            .iter()
            .enumerate()
            .map(|(at, placeholder)| (placeholder.clone(), json!({"S": format!("v{at}")})))
            .collect();
        let (status, answered) = call(
            &router,
            "Scan",
            json!({
                "TableName": TABLE,
                "FilterExpression": format!("#s IN ({})", placeholders.join(", ")),
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": Value::Object(values),
            }),
        )
        .await;
        if !refused {
            assert_eq!(
                status, 200,
                "case an expression IN at {case}: expected a served scan, got {status} with \
                 {answered}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case an expression IN {case}: expected 400 ValidationException, got {status} with \
             {answered}"
        );
        assert_eq!(
            message(&answered),
            format!(
                "Invalid FilterExpression: The IN operator is provided with too many operands; \
                 number of operands: {count}"
            ),
            "case an expression IN {case}: expected the operand-count sentence naming {count}, \
             got {}",
            message(&answered)
        );

        // The legacy comparison carries the same ceiling, in its own member's words.
        let operands: Vec<Value> = (0..count).map(|at| json!({"S": format!("v{at}")})).collect();
        let (status, answered) = call(
            &router,
            "Scan",
            json!({
                "TableName": TABLE,
                "ScanFilter": scan_filter("IN", Value::Array(operands)),
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case a legacy IN {case}: expected 400 ValidationException, got {status} with \
             {answered}"
        );
        assert_eq!(
            message(&answered),
            "One or more parameter values were invalid: Invalid number of argument(s) for the IN \
             ComparisonOperator",
            "case a legacy IN {case}: expected the argument-count sentence, got {}",
            message(&answered)
        );
    }
}

#[tokio::test]
async fn a_statement_parameter_is_held_to_the_rules_for_its_type() {
    let router = seeded().await;

    // Each row: the case, the `Parameters` entry, and the clause the refusal states.
    // The rows span the numeric domain (the text, the precision, both magnitude bounds,
    // and a mantissa that normalises to zero) and the set rules. The clause is the same
    // one the classic item API states for that rule; the statement surface states it on
    // its own, the way it states every other refusal about a statement.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a text that is no number",
            json!({"N": "abc"}),
            "The parameter cannot be converted to a numeric value: abc".to_string(),
        ),
        (
            "one digit more than the documented precision",
            json!({"N": THIRTY_NINE_DIGITS}),
            "Attempting to store more than 38 significant digits in a Number".to_string(),
        ),
        (
            "a magnitude past the documented exponent maximum",
            json!({"N": "1e126"}),
            "Number overflow. Attempting to store a number with magnitude larger than supported \
             range"
                .to_string(),
        ),
        (
            "a magnitude below the documented exponent minimum",
            json!({"N": "1e-131"}),
            "Number underflow. Attempting to store a number with magnitude smaller than \
             supported range"
                .to_string(),
        ),
        (
            "a zero carrying an exponent past the documented maximum",
            json!({"N": "0e126"}),
            "Number overflow. Attempting to store a number with magnitude larger than supported \
             range"
                .to_string(),
        ),
        (
            "a string set holding one member twice",
            json!({"SS": ["a", "a"]}),
            "One or more parameter values were invalid: Input collection [\"a\", \"a\"] contains \
             duplicates."
                .to_string(),
        ),
        (
            "an empty number set",
            json!({"NS": []}),
            "One or more parameter values were invalid: An number set  may not be empty"
                .to_string(),
        ),
    ];

    for (case, parameter, expected) in rows {
        let (status, answered) = call(
            &router,
            "ExecuteStatement",
            json!({
                "Statement": format!("SELECT * FROM \"{TABLE}\" WHERE pk = ?"),
                "Parameters": [parameter],
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?} as a statement parameter: expected 400 ValidationException, got \
             {status} with {answered}"
        );
        let stated = message(&answered);
        assert_eq!(
            stated, expected,
            "case {case:?} as a statement parameter: expected {expected:?}, got {stated:?}"
        );
    }

    // The CONTROL: the documented ceiling itself, and the smallest and largest
    // magnitudes inside the documented range, are bound and served. The marker sits on a
    // NON-key attribute, so what the row asserts is the value rule and not the key-type
    // one.
    for (case, parameter) in [
        ("the documented precision", json!({"N": THIRTY_EIGHT_DIGITS})),
        ("the documented exponent maximum", json!({"N": "1e125"})),
        ("the documented exponent minimum", json!({"N": "1e-130"})),
        ("a zero inside the documented range", json!({"N": "0e125"})),
        ("a set whose members are distinct", json!({"SS": ["a", "b"]})),
    ] {
        let answered = ok(
            &router,
            "ExecuteStatement",
            json!({
                "Statement": format!("SELECT * FROM \"{TABLE}\" WHERE n = ?"),
                "Parameters": [parameter],
            }),
        )
        .await;
        assert!(
            answered["Items"].is_array(),
            "case {case:?} as a statement parameter: expected a served statement, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_statement_key_equality_is_held_to_the_declared_key_type() {
    let router = seeded().await;

    // Each row: the value pinned on the string partition key, which is a key attribute
    // the table DECLARES a type for. A value of another type addresses nothing the table
    // can hold, so the statement is refused rather than served from a scan.
    for (case, pinned) in [
        ("a number", "1"),
        ("a number with a fraction", "1.5"),
        ("a boolean", "true"),
        ("the null type", "NULL"),
    ] {
        let (status, answered) = call(
            &router,
            "ExecuteStatement",
            json!({"Statement": format!("SELECT * FROM \"{TABLE}\" WHERE pk = {pinned}")}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case a key equality on {case}: expected 400 ValidationException, got {status} with \
             {answered}"
        );
        assert_eq!(
            message(&answered),
            "Key attribute's data type should match its data type in table's schema: Key pk",
            "case a key equality on {case}: expected the key-type sentence, got {}",
            message(&answered)
        );
    }

    // The CONTROL: the declared type is served, through a literal and through a
    // parameter, and reports the item it addresses.
    for (case, body) in [
        (
            "a literal of the declared type",
            json!({"Statement": format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'has-match'")}),
        ),
        (
            "a parameter of the declared type",
            json!({
                "Statement": format!("SELECT * FROM \"{TABLE}\" WHERE pk = ?"),
                "Parameters": [{"S": "has-match"}],
            }),
        ),
    ] {
        let answered = ok(&router, "ExecuteStatement", body).await;
        assert_eq!(
            reported(&answered),
            vec!["has-match".to_string()],
            "case {case}: expected the addressed item, got {answered}"
        );
    }
}
