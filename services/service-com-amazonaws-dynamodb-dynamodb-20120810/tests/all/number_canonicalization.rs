// SPDX-License-Identifier: CC0-1.0

//! A stored `N` is a NUMBER, so what a read reports is that number's one canonical
//! text — never the lexeme the writer happened to spell it with.
//!
//! "Numbers are sent across the network to DynamoDB as strings, to maximize
//! compatibility across languages and libraries. However, DynamoDB treats them as
//! number type attributes" (`AttributeValue$N`, and the same sentence on
//! `AttributeValue$NS`). The rule this suite pins therefore covers every place a
//! number can sit — a top-level attribute, a key attribute, a member of an `NS`, and a
//! number nested in an `L` or an `M` — and it is checked over a TABLE of spellings
//! (signs, exponent forms, missing integral or fractional parts, padding zeros, the
//! documented precision and exponent bounds) rather than one recorded example:
//!
//! * `every_spelling_of_a_number_is_read_back_canonically` — the table, each row
//!   written in all four positions at once.
//! * `generated_spellings_of_one_value_all_read_back_as_that_value` — the invariant
//!   over a generator that ENUMERATES the spellings of a value (the decimal point
//!   moved across a bounded exponent range, each form with and without a sign, an
//!   uppercase exponent marker, and padding zeros): every spelling reads back as the
//!   same text, and all of them address ONE item.
//! * `a_number_outside_the_documented_range_or_a_set_with_a_duplicate_is_refused` —
//!   the refusals: a text that denotes no number, each published bound crossed by one,
//!   and a set that holds one value twice once its members are canonical.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;

use crate::emulator::{call, emulator, error_code};

use smithy_server_runtime::MultiServiceRouter;

/// The documented precision and exponent range of a DynamoDB number: "Number: ... can
/// have up to 38 digits of precision. Exponent range is 10^-130 to 10^125" ("Service,
/// Account, and Table Quotas" in the Amazon DynamoDB Developer Guide). The table below
/// is tested ON each of these bounds, and the refusal table one step past each.
const MAX_SIGNIFICANT_DIGITS: usize = 38;
const MIN_EXPONENT: i32 = -130;
const MAX_EXPONENT: i32 = 125;

/// 38 significant digits — the precision bound, which is accepted as written.
const THIRTY_EIGHT_DIGITS: &str = "12345678901234567890123456789012345678";

/// 39 significant digits, one past that bound.
const THIRTY_NINE_DIGITS: &str = "123456789012345678901234567890123456789";

/// One row: the lexeme a client sends, and the one text every later read reports.
///
/// The rows differ along what the rule generalises over: the sign (absent, `+`, `-`),
/// the exponent (absent, `e`, `E`, signed either way), where the decimal point falls
/// (absent, leading, trailing, interior), padding zeros (leading, trailing, both), the
/// spelling of zero, and the magnitude (from the smallest documented exponent to the
/// largest, and the full 38 digits of precision).
fn canonical_rows() -> Vec<(&'static str, String, String)> {
    let mut rows: Vec<(&'static str, String, String)> = [
        ("a bare integer", "7", "7"),
        ("an explicit plus sign", "+7", "7"),
        ("a negative integer", "-7", "-7"),
        ("a signed decimal", "+1.5", "1.5"),
        ("a trailing fractional zero", "1.50", "1.5"),
        ("positive zero", "+0", "0"),
        ("negative zero", "-0", "0"),
        ("zero with a fraction", "+0.0", "0"),
        ("zero spelled with padding", "0000", "0"),
        ("zero with an exponent", "0e10", "0"),
        ("negative zero with a fraction", "-0.000", "0"),
        ("a signed lowercase exponent", "+1e2", "100"),
        ("a signed exponent value", "1e+2", "100"),
        ("an uppercase exponent", "1E2", "100"),
        ("a decimal mantissa with an exponent", "1.5E+3", "1500"),
        ("a signed value with no integral part", "+.5", "0.5"),
        ("no integral part", ".5", "0.5"),
        ("a trailing decimal point", "5.", "5"),
        ("a trailing decimal point with an exponent", "1.e4", "10000"),
        ("leading zeros", "00042", "42"),
        ("a negative exponent", "+1e-2", "0.01"),
        ("an exponent wider than the mantissa", "1.23E10", "12300000000"),
        ("padding on both sides of the point", "0100.50", "100.5"),
        ("a negative value with trailing zeros", "-0.500", "-0.5"),
        ("a negative decimal with padding", "-012.3400", "-12.34"),
        ("a fraction that an exponent turns into an integer", "0.0025e4", "25"),
        ("an integer that an exponent turns into a fraction", "250e-4", "0.025"),
    ]
    .into_iter()
    .map(|(case, submitted, canonical)| (case, submitted.to_string(), canonical.to_string()))
    .collect();
    // ON the published bounds: the largest and the smallest documented magnitude, and
    // the full documented precision. The canonical text of a power of ten is plain
    // decimal, so these are long — which is the point: the form carries no exponent.
    rows.push((
        "the largest documented exponent",
        format!("1e{MAX_EXPONENT}"),
        format!("1{}", "0".repeat(MAX_EXPONENT.unsigned_abs() as usize)),
    ));
    rows.push((
        "the smallest documented exponent",
        format!("1e{MIN_EXPONENT}"),
        format!(
            "0.{}1",
            "0".repeat(MIN_EXPONENT.unsigned_abs() as usize - 1)
        ),
    ));
    rows.push((
        "the full documented precision",
        THIRTY_EIGHT_DIGITS.to_string(),
        THIRTY_EIGHT_DIGITS.to_string(),
    ));
    rows
}

/// One refusal row: the attributes to write beside a valid key, and why they are
/// refused.
fn refusal_rows() -> Vec<(&'static str, Value)> {
    let mut rows: Vec<(&'static str, Value)> = vec![
        ("a top-level N that is not a number", json!({"v": {"N": "abc"}})),
        ("an empty N", json!({"v": {"N": ""}})),
        ("an N that is only a sign", json!({"v": {"N": "-"}})),
        ("an N with no exponent digits", json!({"v": {"N": "1e"}})),
        ("an N with two decimal points", json!({"v": {"N": "1.2.3"}})),
        (
            "an N nested in a list that is not a number",
            json!({"v": {"L": [{"N": "1"}, {"N": "oops"}]}}),
        ),
        (
            "an N nested in a map that is not a number",
            json!({"v": {"M": {"n": {"N": "1e4e5"}}}}),
        ),
        (
            "an NS member that is not a number",
            json!({"v": {"NS": ["1", "x"]}}),
        ),
        (
            "an NS holding two spellings of one number",
            json!({"v": {"NS": ["1", "1.0"]}}),
        ),
        (
            "an NS holding one member twice",
            json!({"v": {"NS": ["2", "2"]}}),
        ),
        (
            "an NS whose duplicate is nested in a list",
            json!({"v": {"L": [{"NS": ["+3", "3e0"]}]}}),
        ),
        (
            "an SS holding one member twice",
            json!({"v": {"SS": ["a", "a"]}}),
        ),
        (
            "a BS holding one member twice",
            json!({"v": {"BS": ["AQI=", "AQI="]}}),
        ),
    ];
    // ONE PAST each published bound: 39 significant digits, and an exponent either
    // side of the documented range.
    rows.push((
        "one digit more than the documented precision",
        json!({"v": {"N": THIRTY_NINE_DIGITS}}),
    ));
    rows.push((
        "an exponent one past the documented maximum",
        json!({"v": {"N": format!("1e{}", MAX_EXPONENT + 1)}}),
    ));
    rows.push((
        "an exponent one past the documented minimum",
        json!({"v": {"N": format!("1e{}", MIN_EXPONENT - 1)}}),
    ));
    rows
}

/// Create a table keyed on a string partition key and a NUMBER sort key, and wait out
/// the `CREATING` -> `ACTIVE` transition the clock owes it.
async fn create_numbers_table(router: &MultiServiceRouter, name: &str) {
    let (status, created) = call(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "N"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case create {name}: expected HTTP 200, got {status} with {created}"
    );
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    let (status, described) = call(router, "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        (status, described["Table"]["TableStatus"].clone()),
        (200, json!("ACTIVE")),
        "case create {name}: the table becomes ACTIVE once the transition is due, got {described}"
    );
}

/// The `ItemCount` a table reports, which is derived from the items it holds.
async fn item_count(router: &MultiServiceRouter, name: &str) -> i64 {
    let (status, described) = call(router, "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        status, 200,
        "case describe {name}: expected HTTP 200, got {status} with {described}"
    );
    described["Table"]["ItemCount"].as_i64().unwrap_or(-1)
}

#[tokio::test]
async fn every_spelling_of_a_number_is_read_back_canonically() {
    let router = emulator();
    create_numbers_table(&router, "numbers").await;

    for (case, submitted, canonical) in canonical_rows() {
        // ONE write puts the same lexeme in all four places the rule covers: a
        // top-level attribute, the numeric sort key, an N nested two levels down in
        // an L -> M, and a member of an NS.
        let (status, written) = call(
            &router,
            "PutItem",
            json!({
                "TableName": "numbers",
                "Item": {
                    "pk": {"S": "p"},
                    "sk": {"N": submitted.as_str()},
                    "v": {"N": submitted.as_str()},
                    "nested": {"L": [{"M": {"n": {"N": submitted.as_str()}}}]},
                    "ns": {"NS": [submitted.as_str()]},
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case} ({submitted}): expected HTTP 200 from PutItem, got {status} with {written}"
        );

        // The CANONICAL spelling of the key addresses the item the submitted spelling
        // wrote: a key's identity is its number, not its text.
        let (status, fetched) = call(
            &router,
            "GetItem",
            json!({
                "TableName": "numbers",
                "Key": {"pk": {"S": "p"}, "sk": {"N": canonical.as_str()}},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case} ({submitted}): expected HTTP 200 from GetItem, got {status} with {fetched}"
        );
        let item = &fetched["Item"];
        for (path, actual) in [
            ("$.Item.v.N", item["v"]["N"].as_str()),
            ("$.Item.sk.N", item["sk"]["N"].as_str()),
            (
                "$.Item.nested.L[0].M.n.N",
                item["nested"]["L"][0]["M"]["n"]["N"].as_str(),
            ),
            ("$.Item.ns.NS[0]", item["ns"]["NS"][0].as_str()),
        ] {
            assert_eq!(
                actual,
                Some(canonical.as_str()),
                "case {case} ({submitted}) at {path}: expected {canonical:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn generated_spellings_of_one_value_all_read_back_as_that_value() {
    let router = emulator();
    create_numbers_table(&router, "generated").await;

    // The invariant: for every spelling the generator writes, a read reports the
    // value's canonical text, and every spelling of one value addresses ONE item. The
    // generator ENUMERATES its bounded space (it does not sample it), so the cases are
    // the same on every run and there is no seed to report; the case list is in the
    // assertion message instead.
    for value in BASE_VALUES {
        for spelling in spellings(value) {
            let case = format!("{value} spelled {spelling}");
            let (status, written) = call(
                &router,
                "PutItem",
                json!({
                    "TableName": "generated",
                    "Item": {
                        "pk": {"S": "p"},
                        "sk": {"N": spelling.as_str()},
                        "v": {"N": spelling.as_str()},
                        "ns": {"NS": [spelling.as_str()]},
                    },
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200 from PutItem, got {status} with {written}"
            );
            let (status, fetched) = call(
                &router,
                "GetItem",
                json!({
                    "TableName": "generated",
                    "Key": {"pk": {"S": "p"}, "sk": {"N": *value}},
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200 from GetItem, got {status} with {fetched}"
            );
            for (path, actual) in [
                ("$.Item.v.N", fetched["Item"]["v"]["N"].as_str()),
                ("$.Item.sk.N", fetched["Item"]["sk"]["N"].as_str()),
                ("$.Item.ns.NS[0]", fetched["Item"]["ns"]["NS"][0].as_str()),
            ] {
                assert_eq!(
                    actual,
                    Some(*value),
                    "case {case} at {path}: expected {value:?}, got {actual:?}"
                );
            }
        }
    }

    // Every spelling of one value wrote the SAME item, so the table holds exactly one
    // item per distinct value — not one per spelling.
    let count = item_count(&router, "generated").await;
    let expected = i64::try_from(BASE_VALUES.len()).unwrap_or(-1);
    assert_eq!(
        count, expected,
        "case generated spellings: every spelling of a value addresses one item, so \
         expected ItemCount {expected}, got {count}"
    );
}

#[tokio::test]
async fn a_number_outside_the_documented_range_or_a_set_with_a_duplicate_is_refused() {
    let router = emulator();
    create_numbers_table(&router, "refusals").await;

    for (case, attributes) in refusal_rows() {
        let mut item = json!({"pk": {"S": "p"}, "sk": {"N": "1"}});
        for (name, value) in attributes.as_object().expect("the row is a map") {
            item[name] = value.clone();
        }
        let (status, refused) = call(
            &router,
            "PutItem",
            json!({"TableName": "refusals", "Item": item}),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} with {refused}"
        );
    }

    // Nothing a refusal carried was stored: the key every refused write named holds no
    // item at all.
    let (status, fetched) = call(
        &router,
        "GetItem",
        json!({
            "TableName": "refusals",
            "Key": {"pk": {"S": "p"}, "sk": {"N": "1"}},
        }),
    )
    .await;
    assert_eq!(
        (status, fetched.get("Item")),
        (200, None),
        "case refused writes: a refused write stores nothing, got {status} with {fetched}"
    );

    // The bounds themselves are ACCEPTED, so the refusals above are the bound being
    // crossed rather than a rejection of everything large.
    let accepted = [
        ("the documented precision", THIRTY_EIGHT_DIGITS.to_string()),
        ("the largest documented exponent", format!("1e{MAX_EXPONENT}")),
        ("the smallest documented exponent", format!("1e{MIN_EXPONENT}")),
    ];
    assert_eq!(
        THIRTY_EIGHT_DIGITS.len(),
        MAX_SIGNIFICANT_DIGITS,
        "case the documented precision: the row must carry exactly \
         {MAX_SIGNIFICANT_DIGITS} digits, got {}",
        THIRTY_EIGHT_DIGITS.len()
    );
    for (case, value) in accepted {
        let (status, written) = call(
            &router,
            "PutItem",
            json!({
                "TableName": "refusals",
                "Item": {"pk": {"S": "p"}, "sk": {"N": "2"}, "v": {"N": value.as_str()}},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case} ({value}): a value ON the bound is accepted, got {status} with {written}"
        );
    }
}

/// The values the generator below spells every way it can: zero, a positive and a
/// negative integer, a value below one, a multiple of ten and a negative decimal.
/// Each is written in its own canonical form, which is what every spelling of it must
/// read back as.
const BASE_VALUES: &[&str] = &["0", "7", "-7", "0.5", "1500", "-12.34", "0.01"];

/// How far the generator moves the decimal point either way. Small and fixed: the
/// space is enumerated in full, so this is the bound on the case count (one per shift,
/// exponent marker, sign form and padding).
const MAX_POINT_SHIFT: i32 = 3;

/// Every spelling of `canonical` this generator can write.
///
/// `canonical` is plain decimal, so it is `digits * 10^-scale`; a spelling moves the
/// decimal point by `shift` places and compensates with an exponent of `shift`, which
/// is the identity every accepted lexeme of one value is built from. On top of that it
/// varies the exponent marker's case and sign, the mantissa's own sign, and padding
/// zeros on either side of the point — none of which change the value.
fn spellings(canonical: &str) -> Vec<String> {
    let (negative, magnitude) = match canonical.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, canonical),
    };
    let (integral, fraction) = match magnitude.split_once('.') {
        Some((integral, fraction)) => (integral, fraction),
        None => (magnitude, ""),
    };
    let digits = format!("{integral}{fraction}");
    let scale = i32::try_from(fraction.len()).unwrap_or(0);

    let mut out = Vec::new();
    for shift in -MAX_POINT_SHIFT..=MAX_POINT_SHIFT {
        let mantissa = place_point(&digits, scale + shift);
        let mut exponents = vec![format!("e{shift}"), format!("E{shift}")];
        if shift >= 0 {
            // A positive exponent may carry an explicit sign, and a zero one may be
            // left off entirely.
            exponents.push(format!("e+{shift}"));
            if shift == 0 {
                exponents.push(String::new());
            }
        }
        for exponent in exponents {
            for padded in [mantissa.clone(), pad(&mantissa)] {
                let sign = if negative { "-" } else { "" };
                out.push(format!("{sign}{padded}{exponent}"));
                if !negative {
                    out.push(format!("+{padded}{exponent}"));
                }
            }
        }
    }
    out
}

/// `digits * 10^-scale` written in plain decimal: the point `scale` places from the
/// right, padded with zeros when it falls outside the digits.
fn place_point(digits: &str, scale: i32) -> String {
    let Ok(scale) = usize::try_from(scale) else {
        // A negative scale multiplies up: the point moves right past the digits.
        let zeros = usize::try_from(scale.unsigned_abs()).unwrap_or(0);
        return format!("{digits}{}", "0".repeat(zeros));
    };
    if scale == 0 {
        digits.to_string()
    } else if scale >= digits.len() {
        format!("0.{}{digits}", "0".repeat(scale - digits.len()))
    } else {
        let at = digits.len() - scale;
        format!("{}.{}", &digits[..at], &digits[at..])
    }
}

/// The same value with padding zeros: two on the left of the integral part, and two
/// past the end of the fraction (a decimal point is added when there is none, so the
/// zeros never change the magnitude).
fn pad(mantissa: &str) -> String {
    if mantissa.contains('.') {
        format!("00{mantissa}00")
    } else {
        format!("00{mantissa}.00")
    }
}
