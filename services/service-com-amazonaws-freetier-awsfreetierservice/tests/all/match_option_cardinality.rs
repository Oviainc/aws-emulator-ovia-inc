// SPDX-License-Identifier: CC0-1.0

//! The `MatchOptions` cardinality refusal of `GetFreeTierUsage`, driven end to end over
//! the same multi-service router the emulator boots.
//!
//! `DimensionValues$MatchOptions` is documented as "The match options that you can use to
//! filter your results. You can specify only one of these values in the array."
//! (`smithy_query(query="doc", shape="DimensionValues", member="MatchOptions")`), so a
//! comparison that carries any other number of options breaks a constraint this service's
//! own validation layer checks. Such a refusal is stated as ONE fixed sentence for the
//! whole condition in that layer's envelope: the condition is "the array does not hold
//! exactly one option", which is the same condition whatever the array held and wherever
//! the comparison sat, so nothing of the request — no count, no member path, no echo of
//! the options — belongs in what the client reads.
//!
//! The rows below differ along the number of options (none, two, three, every option the
//! model declares, and on up past it), along WHICH options are listed and in what order,
//! along the dimension the comparison names (every value the model's `Dimension` enum
//! declares) and along the position the comparison occupies in the expression tree (the
//! root and four nested places). Each table carries its positive control, so a passing row
//! cannot mean "every filter is refused".

use serde_json::{json, Value};

use service_com_amazonaws_freetier_awsfreetierservice::ops::types::{
    DIMENSION_VALUES, MATCH_OPTION_VALUES,
};

use crate::free_tier::{harness, ok};
use crate::validation_message_rules::refusal_message;

/// What the service states when `MatchOptions` does not carry exactly one option. Fixed
/// text in its entirety, the `[Message = ` wrapper and the closing `]` included.
const EXPECTED: &str = "[Message = Exactly one match option must be provided]";

/// The one `Dimension` whose values are magnitudes rather than text
/// (`shared_types::DIMENSION_VALUES` lists every dimension), so the positive control
/// compares it by the options a magnitude is comparable by.
const NUMERIC_DIMENSION: &str = "USAGE_PERCENTAGE";

/// The `MatchOption` that compares magnitudes (`shared_types::MATCH_OPTION_VALUES` lists
/// every option), so it applies to [`NUMERIC_DIMENSION`] alone.
const MAGNITUDE_OPTION: &str = "GREATER_THAN_OR_EQUAL";

/// A value every dimension accepts: inside the modelled `Value` length and pattern, and a
/// magnitude where the dimension compares magnitudes.
const NUMERIC_VALUE: &str = "0";
/// See [`NUMERIC_VALUE`]; the text dimensions' well-formed value.
const TEXT_VALUE: &str = "us-east-1";

/// One `Dimensions` comparison as a filter expression.
fn comparison(key: &str, values: &[&str], match_options: &[&str]) -> Value {
    json!({"Dimensions": {"Key": key, "Values": values, "MatchOptions": match_options}})
}

/// A well-formed comparison, as the other operand of an expression whose bad comparison
/// sits beside a good one.
fn well_formed() -> Value {
    comparison("REGION", &[TEXT_VALUE], &["EQUALS"])
}

/// The option sets that break the rule: every count but one, differing in which options
/// are listed, in their order, and in whether one is repeated.
fn offending_option_sets() -> Vec<(String, Vec<&'static str>)> {
    let mut rows: Vec<(String, Vec<&'static str>)> = vec![
        ("no option at all".to_string(), Vec::new()),
        (
            "two options".to_string(),
            vec!["EQUALS", "STARTS_WITH"],
        ),
        (
            "the same two options the other way round".to_string(),
            vec!["STARTS_WITH", "EQUALS"],
        ),
        (
            "one option listed twice".to_string(),
            vec!["CONTAINS", "CONTAINS"],
        ),
        (
            "three options".to_string(),
            vec!["ENDS_WITH", MAGNITUDE_OPTION, "EQUALS"],
        ),
        (
            "every option the model declares".to_string(),
            MATCH_OPTION_VALUES.to_vec(),
        ),
    ];
    // Past the number of options that exist, by repeating one: the rule is about the
    // count, which has no upper bound of its own.
    for count in [MATCH_OPTION_VALUES.len() + 1, 12] {
        rows.push((
            format!("{count} options"),
            vec!["EQUALS"; count],
        ));
    }
    rows
}

/// The places a `Dimensions` comparison can sit in an `Expression`: the rule governs every
/// level of the tree, so a nested comparison is refused by the same sentence as a root one.
fn positions(bad: &Value) -> Vec<(&'static str, Value)> {
    vec![
        ("at the root", bad.clone()),
        ("inside And", json!({"And": [bad.clone()]})),
        (
            "inside Or, after a well-formed branch",
            json!({"Or": [well_formed(), bad.clone()]}),
        ),
        ("inside Not", json!({"Not": bad.clone()})),
        (
            "two levels down, inside Or inside And",
            json!({"And": [{"Or": [bad.clone()]}]}),
        ),
    ]
}

/// The refusal is the same fixed sentence for every dimension the model declares and for
/// every position the comparison occupies — and a comparison carrying exactly one option
/// is served.
#[tokio::test]
async fn the_cardinality_refusal_is_one_sentence_for_every_dimension_and_position() {
    let router = harness().0;

    // Every dimension, with the bad comparison at the root.
    for key in DIMENSION_VALUES {
        for (case, options) in offending_option_sets() {
            let case = format!("{key} with {case}");
            let filter = comparison(key, &[TEXT_VALUE], &options);
            let actual = refusal_message(
                &router,
                &case,
                "GetFreeTierUsage",
                &json!({"maxResults": 10, "filter": filter}),
            )
            .await;
            assert_eq!(
                actual, EXPECTED,
                "case {case} ({options:?}): expected {EXPECTED:?}, got {actual:?}"
            );
        }
    }

    // Every position in the expression tree, for each offending option set.
    for (case, options) in offending_option_sets() {
        let bad = comparison("SERVICE", &[TEXT_VALUE], &options);
        for (place, filter) in positions(&bad) {
            let case = format!("{case} {place}");
            let actual = refusal_message(
                &router,
                &case,
                "GetFreeTierUsage",
                &json!({"maxResults": 10, "filter": filter}),
            )
            .await;
            assert_eq!(
                actual, EXPECTED,
                "case {case} ({options:?}): expected {EXPECTED:?}, got {actual:?}"
            );
        }
    }

    // The positive control: exactly one option is served, for every dimension and every
    // option that dimension's values are comparable by.
    for key in DIMENSION_VALUES {
        let numeric = *key == NUMERIC_DIMENSION;
        let value = if numeric { NUMERIC_VALUE } else { TEXT_VALUE };
        for option in MATCH_OPTION_VALUES {
            // A magnitude is comparable by equality and by the magnitude option; text by
            // every option but the magnitude one.
            let comparable = if numeric {
                *option == MAGNITUDE_OPTION || *option == "EQUALS"
            } else {
                *option != MAGNITUDE_OPTION
            };
            if !comparable {
                continue;
            }
            let filter = comparison(key, &[value], &[option]);
            let answered = ok(
                &router,
                "GetFreeTierUsage",
                &json!({"maxResults": 10, "filter": filter}),
            )
            .await;
            assert!(
                answered["freeTierUsages"].is_array(),
                "case {key} compared by the single option {option}: expected a served page, \
                 got {answered}"
            );
        }
    }
}

/// The sentence states NOTHING of the request: every offending request reads identically,
/// and the text carries neither a count nor the member path the comparison was reached at.
#[tokio::test]
async fn the_cardinality_refusal_states_nothing_of_the_request() {
    let router = harness().0;

    let mut stated: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (case, options) in offending_option_sets() {
        let bad = comparison("REGION", &[TEXT_VALUE], &options);
        for (place, filter) in positions(&bad) {
            let case = format!("{case} {place}");
            let actual = refusal_message(
                &router,
                &case,
                "GetFreeTierUsage",
                &json!({"maxResults": 10, "filter": filter}),
            )
            .await;
            assert!(
                !actual.chars().any(|character| character.is_ascii_digit()),
                "case {case} ({options:?}): expected a sentence carrying no count, got {actual:?}"
            );
            for borrowed in ["MatchOptions", "Dimensions", "filter", "EQUALS"] {
                assert!(
                    !actual.contains(borrowed),
                    "case {case} ({options:?}): expected a sentence carrying nothing of the \
                     request, but it names {borrowed:?}: {actual:?}"
                );
            }
            stated.insert(actual);
        }
    }

    let stated: Vec<String> = stated.into_iter().collect();
    assert_eq!(
        stated,
        vec![EXPECTED.to_string()],
        "case the whole offending table: expected the one sentence {EXPECTED:?}, got {stated:?}"
    );
}
