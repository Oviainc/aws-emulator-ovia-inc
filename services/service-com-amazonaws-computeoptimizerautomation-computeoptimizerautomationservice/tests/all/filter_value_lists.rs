// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the floor a LISTING FILTER's value list is held to.
//!
//! The rule under test: a filter's `values` are the alternatives it selects on
//! and every filter a listing request carries has to hold, so a filter carrying
//! no value states a condition nothing can satisfy; such a request is refused
//! with `InvalidParameterValueException` (400), under that filter's own member
//! path and its position in the list, rather than answered with an empty page
//! that reads as an authoritative result set. A filter carrying at least one
//! value IS acted on, whether or not anything matches it.
//!
//! The rows span what the rule generalises over: every operation of this service
//! whose input carries a filter list, every filter name the model declares for
//! that operation (read back from the crate's generated value constants, so a
//! newly modelled name is covered here the day it is modelled), the POSITION of
//! the offending filter in the list, and the shape of the rest of the list — the
//! sizes of the value lists the other filters carry.
//!
//! Every case drives the service over the wire through the harness
//! [`super::automation_rule_lifecycle`] owns, so a refusal that is not computed
//! on the request fails here. The filter-`name` half of the same guard is
//! asserted in [`crate::filter_name_refusals`], and the floor the service's other
//! caller-supplied value lists are held to in [`crate::empty_value_lists`].

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    RecommendedAction, AUTOMATION_EVENT_FILTER_NAME_VALUES, AUTOMATION_RULE_FILTER_NAME_VALUES,
    RECOMMENDED_ACTION_FILTER_NAME_VALUES,
};

use crate::automation_events::{now_secs, seed, stored_event};
use crate::automation_rule_lifecycle::{
    call, call_ok, emulator_with_engine, error_code, member, minimal_rule,
};
use crate::error_payloads::error_message;
use crate::recommended_actions::{record, stored};

/// Every operation whose input carries a filter list, with the value set its
/// filter `name` is typed as — the crate's generated, model-derived constant for
/// that shape. The operations are the ones whose input holds a filter list
/// member (`smithy_query(query="digest")`).
const FILTERING_OPERATIONS: &[(&str, &[&str])] = &[
    ("ListAutomationRules", AUTOMATION_RULE_FILTER_NAME_VALUES),
    ("ListAutomationEvents", AUTOMATION_EVENT_FILTER_NAME_VALUES),
    (
        "ListAutomationEventSummaries",
        AUTOMATION_EVENT_FILTER_NAME_VALUES,
    ),
    (
        "ListRecommendedActions",
        RECOMMENDED_ACTION_FILTER_NAME_VALUES,
    ),
    (
        "ListRecommendedActionSummaries",
        RECOMMENDED_ACTION_FILTER_NAME_VALUES,
    ),
];

/// The two modelled `RecommendedActionType` values
/// (`smithy_query(query="shape",shape="RecommendedActionType")`), which are also
/// the `EventType` values.
const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// The account every emulated caller presents.
const CALLER_ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// Another account, modelled `^[0-9]{12}$`, so an event fixture can span more
/// than one owner.
const OTHER_ACCOUNT_ID: &str = "222222222222";

/// The whole message a refusal of an empty filter value list must carry: the
/// service's validation report for the one member that failed, naming that
/// member by its position in the filter list (counted from one, as the
/// boundary's own filter-`name` refusal counts it) and echoing no value.
fn expected_message(position: usize) -> String {
    format!(
        "1 validation error detected: Value at 'filters.{position}.member.values' failed to \
         satisfy constraint: Member must have length greater than or equal to 1"
    )
}

/// A filter list of `length` filters for `operation`, in which the one at
/// `position` (1 based) carries NO value and each other carries `filler_values`
/// values of a name the operation accepts — so the offending filter can sit
/// before, among or after well-formed ones.
fn filters_with_empty_at(
    position: usize,
    length: usize,
    name: &str,
    accepted: &[&str],
    filler_values: usize,
) -> Value {
    let filler_name = accepted.first().unwrap_or_else(|| {
        panic!("an operation that filters declares at least one accepted filter name")
    });
    // `FilterValue` is `length(1..255)`, pattern `^[a-zA-Z0-9\-_\.\*\?\s]+$`
    // (`smithy_query(query="shape",shape="FilterValue")`), so the filler values
    // are well-formed and only their COUNT varies.
    let filler: Vec<String> = (0..filler_values).map(|index| format!("v{index}")).collect();
    let filters: Vec<Value> = (1..=length)
        .map(|index| {
            if index == position {
                json!({"name": name, "values": []})
            } else {
                json!({"name": filler_name, "values": filler})
            }
        })
        .collect();
    json!({"filters": filters})
}

/// A filter carrying no value is refused by EVERY operation that filters, for
/// every filter name that operation declares, wherever in the list it sits and
/// whatever the rest of the list looks like — with the same status, the same
/// code and the service's validation report naming that filter's position.
#[tokio::test]
async fn a_filter_naming_no_value_is_refused_by_every_listing_that_filters() {
    let (router, _engine) = emulator_with_engine();

    for (operation, accepted) in FILTERING_OPERATIONS {
        for name in *accepted {
            // (the offending filter's position, how long the list is, how many
            // values each other filter carries): the first position and later
            // ones, the offending filter last in the list and ahead of
            // well-formed ones, and value counts of one and three — so the
            // report is pinned to naming the offending filter rather than the
            // first or the last entry, whatever the rest of the list looks like.
            for (position, length, filler_values) in
                [(1usize, 1usize, 1usize), (2, 2, 1), (3, 3, 3), (1, 3, 1), (2, 3, 2)]
            {
                let case =
                    format!("{operation}-{name}-at-{position}-of-{length}-with-{filler_values}");
                let request =
                    filters_with_empty_at(position, length, name, accepted, filler_values);
                let (status, body) = call(&router, operation, &request).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: a filter that names no value is a client error, got {status} \
                     and {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterValueException",
                    "case {case:?}: expected the parameter-value error, got {body}"
                );
                let expected = expected_message(position);
                let actual = error_message(&body);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }
}

/// A refused listing reports no page at all, and leaves what the next caller
/// sees exactly as it was: the same listing without the empty filter still
/// reports every item the store holds.
#[tokio::test]
async fn a_refused_listing_reports_nothing_and_changes_nothing() {
    let (router, engine) = emulator_with_engine();
    for name in ["alpha-rule", "beta-rule"] {
        call_ok(&router, "CreateAutomationRule", &minimal_rule(name), "setup").await;
    }
    record(
        &engine,
        vec![action(0, UPGRADE), action(1, SNAPSHOT_AND_DELETE)],
    );

    // (case, operation, the items member of its response, how many items the
    // store holds for it)
    let rows: [(&str, &str, &str, usize); 2] = [
        ("rules", "ListAutomationRules", "automationRules", 2),
        (
            "recommended-actions",
            "ListRecommendedActions",
            "recommendedActions",
            2,
        ),
    ];

    for (case, operation, items, held) in rows {
        let (status, body) = call(
            &router,
            operation,
            &json!({"filters": [{"name": "AccountId", "values": []}]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal, got {status} and {body}"
        );
        let reported = member(&body, items);
        assert!(
            reported.is_null(),
            "case {case:?}: a refused listing must report no page, got {reported}"
        );

        let served = call_ok(&router, operation, &json!({}), case).await;
        let count = member(&served, items)
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            count, held,
            "case {case:?}: the refusal must leave the listing as it was, expected {held} items, \
             got {count} in {served}"
        );
    }
}

/// One recorded recommended action of `recommended_action_type`, owned by the
/// calling account, built on the crate's one recommended-action fixture builder.
fn action(index: usize, recommended_action_type: &str) -> RecommendedAction {
    stored(
        index,
        &format!("vol-{index:04}"),
        CALLER_ACCOUNT_ID,
        "us-east-1",
        14,
        recommended_action_type,
        Some("gp2"),
        false,
        &[("env", "prod")],
    )
}

/// How many items `operation` selected: the length of the page it reports, or —
/// for a summarising listing, whose response groups the selected items — the
/// counts its groups total.
fn selected_count(operation: &str, body: &Value) -> i64 {
    // (the items member of the response, the count a summary group totals under
    // `total`, where the response summarises)
    let (items, counted) = match operation {
        "ListAutomationRules" => ("automationRules", None),
        "ListAutomationEvents" => ("automationEvents", None),
        "ListRecommendedActions" => ("recommendedActions", None),
        "ListAutomationEventSummaries" => {
            ("automationEventSummaries", Some("automationEventCount"))
        }
        "ListRecommendedActionSummaries" => {
            ("recommendedActionSummaries", Some("recommendedActionCount"))
        }
        other => panic!("{other} is not one of this service's filtering listings"),
    };
    let listed = member(body, items);
    if listed.is_null() {
        return 0;
    }
    let listed = listed
        .as_array()
        .unwrap_or_else(|| panic!("{items} is a list, got {body}"));
    match counted {
        None => listed.len() as i64,
        Some(count) => listed
            .iter()
            .map(|summary| {
                member(member(summary, "total"), count)
                    .as_i64()
                    .unwrap_or_else(|| panic!("a summary totals its {count}, got {summary}"))
            })
            .sum(),
    }
}

/// A filter carrying at least one value is ACTED ON — the listing is served and
/// selects exactly the items whose field is among the values — whether the
/// values select every item, some of them or none at all.
///
/// The rows differ along what the rule generalises over: how many values the
/// filter carries, whether a value selects anything, a value repeated, how many
/// filters the list carries, the dimension selected on, and whether the listing
/// reports items or summarises them.
#[tokio::test]
async fn a_filter_naming_at_least_one_value_is_acted_on() {
    let (router, engine) = emulator_with_engine();
    // Three rules, differing in the two dimensions a rule filter selects on
    // here: the rule's name and its status.
    for (name, status) in [
        ("alpha-rule", "Active"),
        ("beta-rule", "Inactive"),
        ("gamma-rule", "Active"),
    ] {
        let mut request = minimal_rule(name);
        request["status"] = json!(status);
        call_ok(&router, "CreateAutomationRule", &request, "setup").await;
    }
    // Three events, differing in owner, type and status, all inside the
    // retrieval window.
    let base = now_secs() - 3600;
    seed(
        &engine,
        &[
            stored_event(0, CALLER_ACCOUNT_ID, UPGRADE, "Complete", base),
            stored_event(1, OTHER_ACCOUNT_ID, UPGRADE, "Failed", base + 60),
            stored_event(
                2,
                CALLER_ACCOUNT_ID,
                SNAPSHOT_AND_DELETE,
                "Complete",
                base + 120,
            ),
        ],
    );
    // Three recommended actions, two of one type and one of the other.
    record(
        &engine,
        vec![
            action(0, UPGRADE),
            action(1, SNAPSHOT_AND_DELETE),
            action(2, UPGRADE),
        ],
    );

    // (case, operation, filters, how many of the store's items the filters
    // select)
    let rows: Vec<(&str, &str, Value, i64)> = vec![
        (
            "one-value-selecting-one-rule",
            "ListAutomationRules",
            json!([{"name": "Name", "values": ["alpha-rule"]}]),
            1,
        ),
        (
            "two-values-selecting-two-rules",
            "ListAutomationRules",
            json!([{"name": "Name", "values": ["alpha-rule", "beta-rule"]}]),
            2,
        ),
        (
            "three-values-of-which-two-select",
            "ListAutomationRules",
            json!([{"name": "Name", "values": ["alpha-rule", "zeta-rule", "gamma-rule"]}]),
            2,
        ),
        (
            "the-same-value-twice",
            "ListAutomationRules",
            json!([{"name": "Name", "values": ["alpha-rule", "alpha-rule"]}]),
            1,
        ),
        (
            "one-value-selecting-no-rule",
            "ListAutomationRules",
            json!([{"name": "Name", "values": ["zeta-rule"]}]),
            0,
        ),
        (
            "a-status-shared-by-two-rules",
            "ListAutomationRules",
            json!([{"name": "Status", "values": ["Active"]}]),
            2,
        ),
        (
            "two-filters-that-both-hold-of-one-rule",
            "ListAutomationRules",
            json!([
                {"name": "Status", "values": ["Active"]},
                {"name": "Name", "values": ["alpha-rule", "beta-rule"]},
            ]),
            1,
        ),
        (
            "two-filters-of-which-one-selects-nothing",
            "ListAutomationRules",
            json!([
                {"name": "Status", "values": ["Active"]},
                {"name": "Name", "values": ["beta-rule"]},
            ]),
            0,
        ),
        (
            "events-of-one-account",
            "ListAutomationEvents",
            json!([{"name": "AccountId", "values": [CALLER_ACCOUNT_ID]}]),
            2,
        ),
        (
            "events-in-either-status",
            "ListAutomationEvents",
            json!([{"name": "EventStatus", "values": ["Complete", "Failed"]}]),
            3,
        ),
        (
            "events-of-an-account-with-none",
            "ListAutomationEvents",
            json!([{"name": "AccountId", "values": ["999999999999"]}]),
            0,
        ),
        (
            "summarised-events-of-one-account",
            "ListAutomationEventSummaries",
            json!([{"name": "AccountId", "values": [CALLER_ACCOUNT_ID]}]),
            2,
        ),
        (
            "summarised-events-of-an-account-with-none",
            "ListAutomationEventSummaries",
            json!([{"name": "AccountId", "values": ["999999999999"]}]),
            0,
        ),
        (
            "actions-of-one-type",
            "ListRecommendedActions",
            json!([{"name": "RecommendedActionType", "values": [UPGRADE]}]),
            2,
        ),
        (
            "actions-of-either-type",
            "ListRecommendedActions",
            json!([{"name": "RecommendedActionType", "values": [UPGRADE, SNAPSHOT_AND_DELETE]}]),
            3,
        ),
        (
            "actions-of-a-resource-the-store-has-none-of",
            "ListRecommendedActions",
            json!([{"name": "ResourceId", "values": ["vol-9999"]}]),
            0,
        ),
        (
            "summarised-actions-of-one-type",
            "ListRecommendedActionSummaries",
            json!([{"name": "RecommendedActionType", "values": [UPGRADE]}]),
            2,
        ),
        (
            "summarised-actions-of-a-resource-the-store-has-none-of",
            "ListRecommendedActionSummaries",
            json!([{"name": "ResourceId", "values": ["vol-9999"]}]),
            0,
        ),
    ];

    for (case, operation, filters, expected) in &rows {
        let body = call_ok(&router, operation, &json!({"filters": filters}), case).await;
        let actual = selected_count(operation, &body);
        assert_eq!(
            actual, *expected,
            "case {case:?}/{operation}: expected {expected} items selected, got {actual} in \
             {body}"
        );
    }
}
