// SPDX-License-Identifier: CC0-1.0

//! The ONE path that decides a filter `name`, over every operation of this
//! service that narrows a listing with a filter list.
//!
//! The rule under test: a filter structure's `name` member is typed on the value
//! set its filter-name shape declares, so the single generated request walk
//! decides every one of these requests — refusing a name outside that set with
//! one counted validation clause per offending filter, each naming that filter by
//! its position in the list and enumerating exactly the values the shape
//! declares, and admitting every name the shape does declare through to the
//! listing it narrows. No operation re-decides it, so the answer cannot differ
//! between operations typed on the same set, nor between a value set a request is
//! refused against and the one a refusal enumerates.
//!
//! The accepted sets here are the crate's GENERATED, model-derived value
//! constants, read back rather than spelled out, so a set this suite tests
//! against cannot drift from the model. The constants are sorted and the report
//! enumerates the model's declaration order, so what is compared is the SET;
//! [`crate::filter_name_refusals`] owns the message's exact text and its order.
//!
//! The cases span the five filtering operations, every way a name can fall
//! outside its set (unrelated, differing from a declared name only in case, in
//! padding or in separators, and empty), the position of the offending filter in
//! the list, and whether one filter or several are offending at once.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    AUTOMATION_EVENT_FILTER_NAME_VALUES, AUTOMATION_RULE_FILTER_NAME_VALUES,
    RECOMMENDED_ACTION_FILTER_NAME_VALUES,
};

use crate::automation_rule_lifecycle::{
    call, call_ok, emulator, error_code, minimal_rule, string_member,
};
use crate::error_payloads::error_message;

/// Every operation whose input carries a filter list, with the output list it
/// answers with and the generated constant for the value set its filter `name`
/// is typed as — the operations with a filter-list input member
/// (`smithy_query(query="digest")`).
///
/// One row per operation that used to re-check the name itself, so the shared
/// path is driven through each of them.
const FILTERING_OPERATIONS: &[(&str, &str, &[&str])] = &[
    (
        "ListAutomationRules",
        "automationRules",
        AUTOMATION_RULE_FILTER_NAME_VALUES,
    ),
    (
        "ListAutomationEvents",
        "automationEvents",
        AUTOMATION_EVENT_FILTER_NAME_VALUES,
    ),
    (
        "ListAutomationEventSummaries",
        "automationEventSummaries",
        AUTOMATION_EVENT_FILTER_NAME_VALUES,
    ),
    (
        "ListRecommendedActions",
        "recommendedActions",
        RECOMMENDED_ACTION_FILTER_NAME_VALUES,
    ),
    (
        "ListRecommendedActionSummaries",
        "recommendedActionSummaries",
        RECOMMENDED_ACTION_FILTER_NAME_VALUES,
    ),
];

/// Names no filter-name shape of this service declares, as `(case, name)`: one
/// unrelated to any declared name, three that differ from the declared
/// `AccountId` only in case, padding or separators (a value set is matched
/// exactly, so none of them is in it), and the empty string.
///
/// Every set under test declares `AccountId`, so each row is outside all of
/// them.
const NAMES_OUTSIDE_EVERY_SET: &[(&str, &str)] = &[
    ("unrelated", "Nickname"),
    ("lowercased", "accountid"),
    ("uppercased", "ACCOUNTID"),
    ("padded", " AccountId "),
    ("separated", "account_id"),
    ("empty", ""),
];

/// The first name `declared` holds, to pad a filter list with ahead of the
/// offending one.
fn accepted_name<'a>(declared: &[&'a str]) -> &'a str {
    declared
        .first()
        .copied()
        .unwrap_or_else(|| panic!("an operation that filters declares at least one filter name"))
}

/// A filter list holding `name` at `position` (1 based) — preceded by filters
/// the operation accepts, and followed by a second offending filter when
/// `repeated`, so a list with more than one filter outside the set is reported
/// clause by clause rather than collapsed onto one of them.
fn filters_with(name: &str, position: usize, repeated: bool, declared: &[&str]) -> Value {
    let accepted = accepted_name(declared);
    let mut filters: Vec<Value> = (1..position)
        .map(|_| json!({"name": accepted, "values": ["111111111111"]}))
        .collect();
    filters.push(json!({"name": name, "values": ["x"]}));
    if repeated {
        filters.push(json!({"name": name, "values": ["y"]}));
    }
    json!({"filters": filters})
}

/// Assert `message` is the walk's report for the filters at `positions` — one
/// counted clause each, in list order — and that every clause enumerates
/// `declared`.
fn assert_enum_report(message: &str, positions: &[usize], declared: &[&str], case: &str) {
    let count = positions.len();
    let noun = if count == 1 { "error" } else { "errors" };
    let prefix = format!("{count} validation {noun} detected: ");
    let body = message.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected a report opening {prefix:?}, got {message:?}")
    });
    let clauses: Vec<&str> = body.split("; ").collect();
    assert_eq!(
        clauses.len(),
        count,
        "case {case:?}: expected one clause per offending filter, got {clauses:?}"
    );

    let mut expected: Vec<&str> = declared.to_vec();
    expected.sort_unstable();
    for (clause, position) in clauses.iter().zip(positions) {
        let clause_prefix = format!(
            "Value at 'filters.{position}.member.name' failed to satisfy constraint: Member must \
             satisfy enum value set: ["
        );
        let listed = clause
            .strip_prefix(&clause_prefix)
            .and_then(|rest| rest.strip_suffix(']'))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a clause of the form {clause_prefix:?}…], got \
                     {clause:?}"
                )
            });
        let mut listed: Vec<&str> = listed.split(", ").collect();
        listed.sort_unstable();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the clause for filter {position} to enumerate {expected:?}, \
             got {listed:?} from {clause:?}"
        );
    }
}

/// A filter name outside its value set is refused by the one shared walk, for
/// every filtering operation: HTTP 400 `InvalidParameterValueException` whose
/// body is the validation report naming each offending filter by its position
/// and enumerating that shape's declared values.
#[tokio::test]
async fn a_filter_name_outside_its_set_is_refused_by_the_shared_request_walk() {
    let router = emulator();

    for (operation, _, declared) in FILTERING_OPERATIONS {
        for (name_case, name) in NAMES_OUTSIDE_EVERY_SET {
            // The first list position and later ones, so the report is pinned to
            // naming the offending filter rather than always the first, and with
            // one or two offenders, so each is reported rather than only one.
            for position in [1usize, 2, 3] {
                for repeated in [false, true] {
                    let case =
                        format!("{operation}-{name_case}-at-{position}-repeated-{repeated}");
                    let positions: Vec<usize> = if repeated {
                        vec![position, position + 1]
                    } else {
                        vec![position]
                    };
                    let request = filters_with(name, position, repeated, declared);
                    let (status, body) = call(&router, operation, &request).await;
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected HTTP 400, got {status} and {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        "InvalidParameterValueException",
                        "case {case:?}: expected error code InvalidParameterValueException, got \
                         {body}"
                    );
                    assert_enum_report(&error_message(&body), &positions, declared, &case);
                }
            }
        }
    }
}

/// Every operation's refusal of a filter name is worded by the SAME reporter: a
/// filter name no shape of this service declares, sent at the same position to
/// each filtering operation, differs between two operations only where their
/// declared sets differ.
#[tokio::test]
async fn one_reporter_words_every_operation_s_filter_name_refusal() {
    let router = emulator();

    for (name_case, name) in NAMES_OUTSIDE_EVERY_SET {
        // (operation, the set it declares, the message it answered with)
        let mut answers: Vec<(&str, &[&str], String)> = Vec::new();
        for (operation, _, declared) in FILTERING_OPERATIONS {
            let request = filters_with(name, 1, false, declared);
            let (status, body) = call(&router, operation, &request).await;
            let case = format!("{name_case}-{operation}");
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} and {body}"
            );
            answers.push((operation, declared, error_message(&body)));
        }
        for (operation, declared, message) in &answers {
            for (other, other_declared, other_message) in &answers {
                let case = format!("{name_case}-{operation}-against-{other}");
                let same_set = {
                    let (mut one, mut two) = (declared.to_vec(), other_declared.to_vec());
                    one.sort_unstable();
                    two.sort_unstable();
                    one == two
                };
                assert_eq!(
                    message == other_message,
                    same_set,
                    "case {case:?}: two refusals read alike exactly when the sets match, got \
                     {message:?} and {other_message:?} for {declared:?} and {other_declared:?}"
                );
            }
        }
    }
}

/// A name the set DOES declare is admitted by the same walk and reaches the
/// listing it narrows: every declared name is served, and the one that selects
/// on a stored rule's own member returns exactly the rules it names.
#[tokio::test]
async fn a_declared_filter_name_reaches_the_listing_it_narrows() {
    let router = emulator();

    for (operation, items, declared) in FILTERING_OPERATIONS {
        for name in *declared {
            let case = format!("{operation}-{name}");
            // A value that selects nothing is enough here: the call has to be
            // served and answer with its list. What each name selects on is
            // asserted by the suite that owns that listing, and below for the
            // rule listing.
            let answer = call_ok(
                &router,
                operation,
                &json!({"filters": [{"name": name, "values": ["111111111111"]}]}),
                &case,
            )
            .await;
            let listed = answer.get(items).and_then(Value::as_array);
            assert!(
                listed.is_some_and(|rows| rows.is_empty()),
                "case {case:?}: expected an empty {items} for a value nothing carries, got \
                 {answer}"
            );
        }
    }

    // The selection itself, through the operation whose own copy of the value
    // set is gone: two rules differing only in name, each selected by name.
    let names = ["boundary-rule-one", "boundary-rule-two"];
    for name in names {
        call_ok(&router, "CreateAutomationRule", &minimal_rule(name), name).await;
    }
    for name in names {
        let case = format!("select-{name}");
        let listed = call_ok(
            &router,
            "ListAutomationRules",
            &json!({"filters": [{"name": "Name", "values": [name]}]}),
            &case,
        )
        .await;
        let rules = listed
            .get("automationRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let selected: Vec<String> = rules.iter().map(|rule| string_member(rule, "name")).collect();
        assert_eq!(
            selected,
            vec![name.to_string()],
            "case {case:?}: expected the filter to select exactly that rule, got {selected:?}"
        );
    }
}
