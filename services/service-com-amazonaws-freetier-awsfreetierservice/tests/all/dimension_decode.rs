// SPDX-License-Identifier: CC0-1.0

//! One decoding of a `GetFreeTierUsage` `Dimensions` comparison, seen from both
//! sides of the operation's two-phase split.
//!
//! RULE: a comparison is decoded once — `MatchOptions` names exactly one option,
//! and on a dimension whose value is a magnitude each of `Values` is the number
//! that value reads as, ignoring the whitespace around it — and that one decode
//! decides both whether the request is answered at all and which records it
//! selects: a comparison the decode cannot read is refused with the modelled
//! `ValidationException` (HTTP 400) at every level of the expression, never
//! answered with a page that silently dropped it.
//!
//! Grounding: `DimensionValues$MatchOptions` — "You can specify only one of
//! these values in the array" — and `DimensionValues$Values` — "the results all
//! match at least one of the specified values"
//! (`smithy_query(query="shape",shape="DimensionValues")`); `USAGE_PERCENTAGE`
//! is the one `Dimension` whose record value is a number
//! (`smithy_query(query="shape",shape="Dimension")`), so it is the one that
//! compares by magnitude.
//!
//! The rows differ along what the rule generalises over: the magnitude compared
//! against (zero, each magnitude a record actually holds, one above every
//! record, a bound no percentage reaches, a negative), the spelling that names
//! it (plain, padded with spaces on either side or both, scientific), the
//! match option, how many options the array holds, which of several values is
//! unreadable, and how deeply the comparison is nested. The harness is the one
//! `free_tier.rs` builds.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::free_tier::{call, error_code, harness, ok, text};

/// The `Dimension` whose record value is a magnitude, and the two match options
/// that compare magnitudes.
const USAGE_PERCENTAGE: &str = "USAGE_PERCENTAGE";
const EQUALS: &str = "EQUALS";
const GREATER_THAN_OR_EQUAL: &str = "GREATER_THAN_OR_EQUAL";

/// A `Dimension` whose record value is text, for the rows about the option
/// array rather than about magnitudes.
const SERVICE: &str = "SERVICE";

/// A usage record's identity, for comparing sets of records.
fn record_key(record: &Value) -> String {
    format!(
        "{}|{}|{}",
        text(record, "service"),
        text(record, "usageType"),
        text(record, "region")
    )
}

/// Every usage record the account reports, unfiltered.
async fn all_records(router: &MultiServiceRouter) -> Vec<Value> {
    ok(router, "GetFreeTierUsage", &json!({"maxResults": 1000}))
        .await["freeTierUsages"]
        .as_array()
        .expect("freeTierUsages is a list")
        .clone()
}

/// The share of its limit a reported record has used, as a percentage — the
/// value `USAGE_PERCENTAGE` compares, recomputed from the members the response
/// carries so the expectation is derived and not transcribed.
fn percentage(record: &Value) -> f64 {
    let actual = record["actualUsageAmount"].as_f64().unwrap_or_default();
    let limit = record["limit"].as_f64().unwrap_or_default();
    if limit == 0.0 {
        0.0
    } else {
        actual / limit * 100.0
    }
}

/// The records a filter selects, by identity.
async fn selected(router: &MultiServiceRouter, filter: &Value) -> BTreeSet<String> {
    ok(
        router,
        "GetFreeTierUsage",
        &json!({"maxResults": 1000, "filter": filter}),
    )
    .await["freeTierUsages"]
        .as_array()
        .expect("freeTierUsages is a list")
        .iter()
        .map(record_key)
        .collect()
}

/// One `Dimensions` comparison as a filter expression.
fn comparison(key: &str, options: &[&str], values: &[String]) -> Value {
    json!({"Dimensions": {"Key": key, "Values": values, "MatchOptions": options}})
}

/// Assert that one filter is refused with the modelled `ValidationException`
/// (HTTP 400) rather than answered.
async fn refuses(router: &MultiServiceRouter, case: &str, filter: &Value) {
    let (status, payload) = call(
        router,
        "GetFreeTierUsage",
        &json!({"maxResults": 1000, "filter": filter}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?} ({filter}): expected HTTP 400, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "case {case:?} ({filter}): expected a ValidationException, got {payload}"
    );
}

/// The modelled bounds of one filter value: `Value` is 1..20 characters and
/// matches a pattern that admits the space but no other whitespace and no
/// leading `+` (`smithy_query(query="shape",shape="Value")`). Every probe below
/// stays inside them, so each row is a value the service accepts and then has
/// to read.
const MAX_VALUE_LENGTH: usize = 20;

/// Spellings that all name `magnitude`: the shortest form that round-trips, the
/// same padded with spaces before, after and on both sides, and the scientific
/// form — keeping only those a `Value` can hold at all.
fn spellings(magnitude: f64) -> Vec<String> {
    let plain = format!("{magnitude}");
    let scientific = format!("{magnitude:e}");
    let candidates = [
        plain.clone(),
        format!(" {plain}"),
        format!("{plain} "),
        format!("  {plain} "),
        scientific.clone(),
        format!(" {scientific} "),
    ];
    let spellings: Vec<String> = candidates
        .into_iter()
        .filter(|spelling| (1..=MAX_VALUE_LENGTH).contains(&spelling.len()))
        .collect();
    assert!(
        spellings.len() > 1,
        "case {magnitude}: the probe must have several spellings inside the modelled length, \
         got {spellings:?}"
    );
    for spelling in &spellings {
        let read = spelling.trim().parse::<f64>().unwrap_or_else(|error| {
            panic!("case {spelling:?}: the probe must name a magnitude: {error}")
        });
        assert!(
            read.to_bits() == magnitude.to_bits(),
            "case {spelling:?}: the probe must name {magnitude}, got {read}"
        );
    }
    spellings
}

/// RULE, from the evaluating side: every spelling of one magnitude selects
/// exactly the records the comparison holds for, because the magnitude a value
/// names is read once and the records are then compared against that number.
#[tokio::test]
async fn one_magnitude_selects_the_same_records_however_it_is_spelled() {
    let (router, _engine) = harness();
    let records = all_records(&router).await;
    assert!(
        records.len() > 1,
        "case setup: the account must hold several usage records, got {}",
        records.len()
    );

    // The magnitudes to compare against: zero, every magnitude a record
    // actually holds, one just above the largest, the percentage bound no
    // record reaches, and a negative.
    let mut magnitudes: Vec<f64> = vec![0.0, 100.0, -12.5];
    magnitudes.extend(records.iter().map(percentage));
    let highest = magnitudes
        .iter()
        .copied()
        .fold(f64::MIN, |highest, magnitude| highest.max(magnitude));
    magnitudes.push(highest + 1.0);

    for magnitude in magnitudes {
        for option in [EQUALS, GREATER_THAN_OR_EQUAL] {
            // The rule's own expectation, over the records the service reports:
            // a record matches when its percentage compares to the magnitude
            // under this option.
            let expected: BTreeSet<String> = records
                .iter()
                .filter(|record| {
                    let value = percentage(record);
                    if option == GREATER_THAN_OR_EQUAL {
                        value >= magnitude
                    } else {
                        (value - magnitude).abs() < f64::EPSILON
                    }
                })
                .map(record_key)
                .collect();

            for spelling in spellings(magnitude) {
                let case = format!("{USAGE_PERCENTAGE} {option} {spelling:?}");
                let filter = comparison(
                    USAGE_PERCENTAGE,
                    &[option],
                    std::slice::from_ref(&spelling),
                );
                let got = selected(&router, &filter).await;
                assert_eq!(
                    got, expected,
                    "case {case:?}: expected {expected:?}, got {got:?}"
                );

                // The same magnitude beside a magnitude no record reaches: a
                // record matches at least one of `Values`, and under either
                // option nothing matches the unreachable one, so the selection
                // is unchanged.
                let widened = comparison(
                    USAGE_PERCENTAGE,
                    &[option],
                    &[spelling, format!("{}", highest + 1_000.0)],
                );
                let got = selected(&router, &widened).await;
                assert_eq!(
                    got, expected,
                    "case {case:?} beside an unreachable magnitude: expected {expected:?}, \
                     got {got:?}"
                );
            }
        }
    }
}

/// RULE, from the checking side: a comparison the decode cannot read is
/// refused, at every level of the expression — the option array has to hold
/// exactly one option, and every value of a magnitude comparison has to name a
/// magnitude.
#[tokio::test]
async fn a_comparison_the_decode_cannot_read_is_refused_at_every_level() {
    let (router, _engine) = harness();

    // Rows differ in what the decode cannot read: how many options the array
    // holds (none, two, three — on a text dimension and on the numeric one),
    // and which of the values does not name a magnitude (the only one, the
    // first of several, the last of several, and forms that look numeric).
    let mut rows: Vec<(String, Value)> = Vec::new();
    for key in [SERVICE, USAGE_PERCENTAGE] {
        let values = if key == USAGE_PERCENTAGE {
            vec!["50".to_string()]
        } else {
            vec!["AWSLambda".to_string()]
        };
        for options in [
            vec![],
            vec![EQUALS, EQUALS],
            vec![EQUALS, GREATER_THAN_OR_EQUAL],
            vec![EQUALS, GREATER_THAN_OR_EQUAL, "CONTAINS"],
        ] {
            rows.push((
                format!("{key} with {} match options", options.len()),
                comparison(key, &options, &values),
            ));
        }
    }
    for values in [
        vec!["half"],
        vec![" "],
        vec!["."],
        vec!["1,5"],
        vec!["0x32"],
        vec!["50 50"],
        vec!["fifty", "50"],
        vec!["50", "fifty"],
        vec!["0", "25", "nearly all"],
    ] {
        let owned: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
        for value in &owned {
            assert!(
                (1..=MAX_VALUE_LENGTH).contains(&value.len()),
                "case {value:?}: the probe must be a value the model admits, got {} characters",
                value.len()
            );
        }
        rows.push((
            format!("{USAGE_PERCENTAGE} compared against {values:?}"),
            comparison(USAGE_PERCENTAGE, &[EQUALS], &owned),
        ));
    }

    let readable = comparison(SERVICE, &[EQUALS], &["AWSLambda".to_string()]);
    for (case, filter) in rows {
        refuses(&router, &case, &filter).await;

        // The same comparison one level down: the decode is the same decode
        // wherever the comparison sits, so each nesting is refused too.
        for (place, nested) in [
            (
                "inside And",
                json!({"And": [readable.clone(), filter.clone()]}),
            ),
            ("inside Or", json!({"Or": [filter.clone()]})),
            ("inside Not", json!({"Not": filter.clone()})),
            (
                "inside Not of Or",
                json!({"Not": {"Or": [filter.clone()]}}),
            ),
        ] {
            refuses(&router, &format!("{case} {place}"), &nested).await;
        }
    }
}

/// Spellings the float syntax reads but that name no magnitude: the non-finite
/// words `nan`, `inf` and `infinity`, in any case and with an optional sign
/// (`f64`'s `FromStr` grammar), and the exponents past the largest magnitude a
/// double holds (about `1.8e308`), which read as infinity rather than as the
/// number written. Each row also stays inside the modelled `Value` bounds —
/// 1..20 characters, and a pattern that admits letters, the space, `-` and `.`
/// but no leading `+` (`smithy_query(query="shape",shape="Value")`) — so each
/// is a value the service accepts and then has to read.
fn spellings_naming_no_magnitude() -> Vec<String> {
    let rows: Vec<String> = [
        "NaN",
        "nan",
        "NAN",
        "naN",
        "-NaN",
        "-nan",
        " NaN ",
        "inf",
        "Inf",
        "INF",
        "-inf",
        "infinity",
        "Infinity",
        "INFINITY",
        "-infinity",
        " -inf ",
        "1e400",
        "1E400",
        "-1e400",
        "1e309",
    ]
    .iter()
    .map(|spelling| (*spelling).to_string())
    .collect();
    for row in &rows {
        assert!(
            (1..=MAX_VALUE_LENGTH).contains(&row.len()),
            "case {row:?}: the probe must be a value the model admits, got {} characters",
            row.len()
        );
        let read = row.trim().parse::<f64>().unwrap_or_else(|error| {
            panic!("case {row:?}: the probe must be one the float syntax reads at all: {error}")
        });
        assert!(
            !read.is_finite(),
            "case {row:?}: the probe must name no finite magnitude, got {read}"
        );
    }
    rows
}

/// RULE, from the checking side, over the values the float syntax reads yet
/// that name no magnitude: a comparison on the numeric dimension is refused
/// unless every one of `Values` names a magnitude, i.e. one finite number, so a
/// non-finite spelling is refused exactly as `"half"` is — never accepted and
/// then answered with a page whose contents the spelling alone decides (empty
/// under either option, or every record under `GREATER_THAN_OR_EQUAL`).
#[tokio::test]
async fn a_value_naming_no_finite_magnitude_is_refused_like_any_unreadable_value() {
    let (router, _engine) = harness();
    let records = all_records(&router).await;
    assert!(
        !records.is_empty(),
        "case setup: the account must hold usage records, so an accepted comparison would have \
         rows to select"
    );

    let readable = comparison(SERVICE, &[EQUALS], &["AWSLambda".to_string()]);
    for spelling in spellings_naming_no_magnitude() {
        for option in [EQUALS, GREATER_THAN_OR_EQUAL] {
            let case = format!("{USAGE_PERCENTAGE} {option} {spelling:?}");
            let filter = comparison(
                USAGE_PERCENTAGE,
                &[option],
                std::slice::from_ref(&spelling),
            );
            refuses(&router, &case, &filter).await;

            // Beside values that DO name magnitudes, in each position: one
            // value the decode cannot read makes the whole comparison
            // unevaluable, wherever in the array it sits.
            for beside in [
                vec![spelling.clone(), "50".to_string()],
                vec!["50".to_string(), spelling.clone()],
                vec!["0".to_string(), spelling.clone(), "100".to_string()],
            ] {
                refuses(
                    &router,
                    &format!("{case} beside {beside:?}"),
                    &comparison(USAGE_PERCENTAGE, &[option], &beside),
                )
                .await;
            }

            // The same comparison nested: the decode is the same decode
            // wherever the comparison sits, including under a negation, where
            // an accepted non-finite value would otherwise select every record.
            for (place, nested) in [
                (
                    "inside And",
                    json!({"And": [readable.clone(), filter.clone()]}),
                ),
                ("inside Or", json!({"Or": [filter.clone()]})),
                ("inside Not", json!({"Not": filter.clone()})),
            ] {
                refuses(&router, &format!("{case} {place}"), &nested).await;
            }
        }
    }
}

/// RULE, at the boundary of the refusal above: a value that names a FINITE
/// magnitude is still read and still selects by comparison, however extreme the
/// magnitude is — so the refusal is the rule's own edge (the largest and
/// smallest magnitudes a double holds, and an exponent that underflows to zero)
/// and not a refusal of large numbers.
#[tokio::test]
async fn a_finite_magnitude_is_still_read_however_extreme() {
    let (router, _engine) = harness();
    let records = all_records(&router).await;

    // Rows at the edge of the double range, each the finite neighbour of a row
    // the refusal above covers: the largest and smallest magnitudes that fit in
    // 20 characters, an exponent small enough to underflow to zero, and a
    // plain magnitude for contrast.
    for spelling in ["1e308", "-1e308", "1e-400", "0", "50"] {
        let magnitude = spelling
            .trim()
            .parse::<f64>()
            .expect("the probe names a number");
        assert!(
            magnitude.is_finite(),
            "case {spelling:?}: the probe must name a finite magnitude, got {magnitude}"
        );
        for option in [EQUALS, GREATER_THAN_OR_EQUAL] {
            let expected: BTreeSet<String> = records
                .iter()
                .filter(|record| {
                    let value = percentage(record);
                    if option == GREATER_THAN_OR_EQUAL {
                        value >= magnitude
                    } else {
                        (value - magnitude).abs() < f64::EPSILON
                    }
                })
                .map(record_key)
                .collect();
            let filter = comparison(USAGE_PERCENTAGE, &[option], &[spelling.to_string()]);
            let got = selected(&router, &filter).await;
            assert_eq!(
                got, expected,
                "case {USAGE_PERCENTAGE} {option} {spelling:?}: expected {expected:?}, got {got:?}"
            );
        }
    }
}

/// RULE, at its edge: reading a value as a magnitude is what ignores the
/// whitespace around it, so a text dimension compares the value exactly as
/// written — the padding that makes no difference to a magnitude makes all the
/// difference to text.
#[tokio::test]
async fn whitespace_is_ignored_only_where_a_value_names_a_magnitude() {
    let (router, _engine) = harness();
    let records = all_records(&router).await;
    let services: BTreeSet<String> = records.iter().map(|record| text(record, "service")).collect();
    let service = services
        .first()
        .expect("the account holds at least one usage record")
        .clone();

    let exact = comparison(SERVICE, &[EQUALS], std::slice::from_ref(&service));
    let got = selected(&router, &exact).await;
    assert!(
        !got.is_empty(),
        "case {service:?} exactly: the probe must select the records of that service, got {got:?}"
    );

    for padded in [
        format!(" {service}"),
        format!("{service} "),
        format!(" {service} "),
    ] {
        assert!(
            padded.len() <= MAX_VALUE_LENGTH,
            "case {padded:?}: the probe must be a value the model admits, got {} characters",
            padded.len()
        );
        let filter = comparison(SERVICE, &[EQUALS], std::slice::from_ref(&padded));
        let got = selected(&router, &filter).await;
        assert!(
            got.is_empty(),
            "case {padded:?}: a text dimension compares the value as written, so no record \
             matches, got {got:?}"
        );
    }
}
