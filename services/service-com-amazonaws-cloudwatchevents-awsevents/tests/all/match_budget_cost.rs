// SPDX-License-Identifier: CC0-1.0

//! Tests for what a match COSTS, not only what it answers.
//!
//! The rule under test: matching is driven by caller-supplied data on both sides, so every
//! node the walk VISITS is charged against the one per-request budget
//! ([`MAX_MATCH_STEPS_PER_REQUEST`]) — including a visit that answers "no match" because the
//! observed value is of the wrong type for the filter. An event field holding a list is
//! walked element by element, so N elements visited cost at least N steps and can never buy
//! more work than the budget sold: a budget smaller than the walk needs stops the walk, which
//! reports no match.
//!
//! The table spans what the rule generalises over rather than one pattern: every filter form
//! whose answer depends on the observed value's TYPE (`prefix`, `suffix`, `equals-ignore-case`,
//! `wildcard`, `cidr`, `numeric`, a nested object, a plain scalar and an operator this matcher
//! does not know), each matched against an event field holding arrays of several sizes whose
//! elements are of the type that filter refuses.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchevents_awsevents::state::event_pattern::{
    matches_within, MatchBudget, MAX_MATCH_STEPS_PER_REQUEST,
};

/// Array sizes to walk: one element, and two sizes far enough apart that a cost charged per
/// element is unmistakable next to a cost charged once per field.
const SIZES: [usize; 3] = [1, 32, 512];

/// The filter forms whose answer turns on the observed value's type, each paired with the
/// element type its array is filled with so the filter refuses every element.
fn filters() -> Vec<(&'static str, Value, Value)> {
    vec![
        ("prefix against numbers", json!({"prefix": "order-"}), json!(17)),
        ("suffix against numbers", json!({"suffix": "-done"}), json!(17)),
        (
            "prefix with equals-ignore-case against numbers",
            json!({"prefix": {"equals-ignore-case": "Order-"}}),
            json!(17),
        ),
        (
            "equals-ignore-case against booleans",
            json!({"equals-ignore-case": "ORDER"}),
            json!(true),
        ),
        ("wildcard against numbers", json!({"wildcard": "order-*"}), json!(17)),
        ("cidr against numbers", json!({"cidr": "10.0.0.0/24"}), json!(17)),
        ("numeric against strings", json!({"numeric": [">", 0]}), json!("not-a-number")),
        (
            "an operator this matcher does not know",
            json!({"never-an-operator": "x"}),
            json!("value"),
        ),
        ("a nested object against numbers", json!({"inner": ["x"]}), json!(17)),
        ("a scalar against numbers", json!("order-1"), json!(17)),
    ]
}

/// An event whose one field holds `size` copies of `element`.
fn event_with(field: &str, element: &Value, size: usize) -> Value {
    json!({field: Value::Array(std::iter::repeat_n(element.clone(), size).collect())})
}

/// What one match of `pattern` against `event` costs, out of the full per-request budget,
/// plus whether it matched.
fn cost(pattern: &Value, event: &Value) -> (u64, bool) {
    let mut budget = MatchBudget::new(MAX_MATCH_STEPS_PER_REQUEST);
    let matched = matches_within(pattern, event, &mut budget);
    (budget.spent(), matched)
}

/// Every element the walk visits is charged, so the cost of a field holding a list grows with
/// the list — a filter that answers on the element's TYPE is no cheaper than one that reads it.
#[tokio::test]
async fn every_element_visited_is_charged() {
    for (case, filter, element) in filters() {
        let pattern = json!({"detail-type": [filter]});
        let mut previous: Option<(usize, u64)> = None;
        for size in SIZES {
            let event = event_with("detail-type", &element, size);
            let (spent, matched) = cost(&pattern, &event);
            assert!(
                !matched,
                "case {case:?} at {size} element(s): the element type is one this filter \
                 refuses, so the event must not match"
            );
            let least = u64::try_from(size).expect("the size fits in a u64");
            assert!(
                spent >= least,
                "case {case:?} at {size} element(s): every element visited costs a step, so \
                 the match must charge at least {least}, got {spent}"
            );
            if let Some((smaller, cheaper)) = previous {
                assert!(
                    spent > cheaper,
                    "case {case:?}: walking {size} elements must cost more than walking \
                     {smaller} ({cheaper}), got {spent}"
                );
            }
            previous = Some((size, spent));
        }
    }
}

/// A budget smaller than the walk needs stops the walk: it is never overspent, and the
/// unfinished walk reports no match.
#[tokio::test]
async fn a_walk_never_spends_more_than_its_budget() {
    // Small budgets, including one that buys nothing beyond the pattern's own field.
    for allowance in [1_u64, 5, 50] {
        for (case, filter, element) in filters() {
            let pattern = json!({"detail-type": [filter]});
            let event = event_with("detail-type", &element, 4_096);
            let mut budget = MatchBudget::new(allowance);
            let matched = matches_within(&pattern, &event, &mut budget);
            assert!(
                !matched,
                "case {case:?} with a budget of {allowance}: a walk that cannot finish \
                 reports no match"
            );
            assert!(
                budget.spent() <= allowance,
                "case {case:?} with a budget of {allowance}: the walk must not spend more \
                 than it was given, got {}",
                budget.spent()
            );
            assert!(
                budget.exhausted(),
                "case {case:?} with a budget of {allowance}: a walk this budget cannot buy \
                 must leave it spent, got {} of {allowance} spent",
                budget.spent()
            );
        }
    }
}

/// The one filter that is about the field's PRESENCE rather than its value answers without
/// walking the list at all, so its cost does not grow with the list: the rule is that every
/// element VISITED is charged, not that every element is visited.
#[tokio::test]
async fn a_presence_filter_answers_without_walking_the_list() {
    for (case, filter, expected) in [
        ("exists true on a field the event carries", json!({"exists": true}), true),
        ("exists false on a field the event carries", json!({"exists": false}), false),
    ] {
        let pattern = json!({"detail-type": [filter]});
        let mut costs: Vec<u64> = Vec::new();
        for size in SIZES {
            let event = event_with("detail-type", &json!("present"), size);
            let (spent, matched) = cost(&pattern, &event);
            assert_eq!(
                matched, expected,
                "case {case:?} at {size} element(s): expected a match of {expected}, got \
                 {matched}"
            );
            costs.push(spent);
        }
        assert!(
            costs.iter().all(|spent| *spent == costs[0]),
            "case {case:?}: a presence filter walks no element, so its cost does not grow \
             with the list, got {costs:?}"
        );
    }
}

/// The charging above does not change what matching ANSWERS: an element the filter does admit
/// still matches, however many elements the list holds, and within the request budget.
#[tokio::test]
async fn an_admitted_element_still_matches() {
    for size in SIZES {
        // One matching element at the END of the list, so every element before it is walked.
        let mut values: Vec<Value> =
            std::iter::repeat_n(json!("other-event"), size.saturating_sub(1)).collect();
        values.push(json!("order-created"));
        let event = json!({"detail-type": values});
        for (case, filter) in [
            ("a prefix the last element carries", json!({"prefix": "order-"})),
            ("a suffix the last element carries", json!({"suffix": "-created"})),
            (
                "a case-insensitive equal of the last element",
                json!({"equals-ignore-case": "ORDER-CREATED"}),
            ),
            ("a wildcard the last element satisfies", json!({"wildcard": "order-*"})),
            ("the last element itself", json!("order-created")),
        ] {
            let pattern = json!({"detail-type": [filter]});
            let (spent, matched) = cost(&pattern, &event);
            assert!(
                matched,
                "case {case:?} at {size} element(s): the list carries an element this \
                 pattern admits, so the event matches (cost {spent})"
            );
            assert!(
                spent <= MAX_MATCH_STEPS_PER_REQUEST,
                "case {case:?} at {size} element(s): the match must stay inside the request \
                 budget, got {spent}"
            );
        }
    }
}
