// SPDX-License-Identifier: CC0-1.0

//! The BODY of a filter-`name` refusal, over EVERY operation of this service
//! that narrows a listing with a filter list.
//!
//! The rule under test: a refusal of a request value is this service's single
//! validation report — "`<n>` validation error(s) detected: Value at
//! '`<member>`' failed to satisfy constraint: `<requirement>`" — which names the
//! member that failed and the set that member's value had to come from, and
//! echoes the rejected value nowhere. Two consequences are asserted here,
//! because a refusal built by hand per operation satisfies neither: the report
//! enumerates exactly the value set the model declares for THAT operation's
//! filter name, and the same filter list refused by two operations that select
//! over the same resources reads identically on both.
//!
//! The cases span the ways a filter name can fall outside its set — a name no
//! operation of the service accepts, a name another operation's filter set
//! accepts, a valid name in the wrong case, and an empty one — and the position
//! of the offending filter in the list, which the report has to name.
//!
//! A filter `name` is typed on its value set, so the generated request walk
//! refuses a name outside it before the handler runs, and the body asserted here
//! is that walk's report, which names the offending filter by its position. Each
//! handler keeps its own check behind that, so a filter is never silently
//! ignored; it reports through the same reporter, for the list member rather
//! than one position of it.
//!
//! The `nextToken` refusals of the same operations are asserted in
//! [`crate::page_token_refusals`], and their refused time ranges in
//! [`crate::automation_events`] and [`crate::automation_event_summaries`].

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    AUTOMATION_EVENT_FILTER_NAME_VALUES, AUTOMATION_RULE_FILTER_NAME_VALUES,
    RECOMMENDED_ACTION_FILTER_NAME_VALUES as GENERATED_RECOMMENDED_ACTION_FILTER_NAMES,
};

use crate::automation_rule_lifecycle::{call, call_ok, emulator, error_code};
use crate::error_payloads::error_message;

// The value sets below are the ones the model declares, IN DECLARATION ORDER —
// which is the order a refusal enumerates them in, so the generated constants
// (sorted) cannot be used for the message itself.
// `the_value_sets_under_test_are_the_model_s` checks each one against the
// crate's generated, model-derived constant, so a newly modelled filter name
// fails here rather than being silently skipped by the cases below.

/// The `AutomationEventFilterName` value set, in the order the model declares it
/// (`smithy_query(query="shape",shape="AutomationEventFilterName")`).
const EVENT_FILTER_NAMES: &[&str] = &["AccountId", "ResourceType", "EventType", "EventStatus"];

/// The `RecommendedActionFilterName` value set, in model order
/// (`smithy_query(query="shape",shape="RecommendedActionFilterName")`).
const RECOMMENDED_ACTION_FILTER_NAMES: &[&str] = &[
    "ResourceType",
    "RecommendedActionType",
    "ResourceId",
    "LookBackPeriodInDays",
    "CurrentResourceDetailsEbsVolumeType",
    "ResourceTagsKey",
    "ResourceTagsValue",
    "AccountId",
    "RestartNeeded",
];

/// The `AutomationRuleFilterName` value set, in model order
/// (`smithy_query(query="shape",shape="AutomationRuleFilterName")`).
const RULE_FILTER_NAMES: &[&str] = &[
    "Name",
    "RecommendedActionType",
    "Status",
    "RuleType",
    "OrganizationConfigurationRuleApplyOrder",
    "AccountId",
];

/// Every operation whose input carries a filter list, with the value set its
/// filter `name` is typed as. The operations are the ones whose input holds a
/// filter list member (`smithy_query(query="digest")`).
const FILTERING_OPERATIONS: &[(&str, &[&str])] = &[
    ("ListAutomationEvents", EVENT_FILTER_NAMES),
    ("ListAutomationEventSummaries", EVENT_FILTER_NAMES),
    ("ListRecommendedActions", RECOMMENDED_ACTION_FILTER_NAMES),
    (
        "ListRecommendedActionSummaries",
        RECOMMENDED_ACTION_FILTER_NAMES,
    ),
    ("ListAutomationRules", RULE_FILTER_NAMES),
];

/// Filter names to send, as `(case, name, distinctive)`.
///
/// A row is used for an operation only when the name is NOT in that operation's
/// value set, so `AccountId`-style rows that one family accepts are exercised
/// against the families that do not. `distinctive` says the name is a string the
/// report could only contain by echoing it — an empty string is contained by
/// every message, so an echo check on it would pass for the wrong reason.
const FILTER_NAMES_TO_SEND: &[(&str, &str, bool)] = &[
    ("no-operation-accepts-it", "Nickname", true),
    // Accepted by the automation-rule listing, by neither event listing.
    ("another-filter-set-accepts-it", "Name", true),
    // Accepted by both event listings, by no other family.
    ("an-event-filter-name", "EventType", true),
    // Accepted by both recommended-action listings only.
    ("a-recommended-action-filter-name", "ResourceId", true),
    // A value set is case-sensitive, so the lowercase spelling of a name every
    // family accepts is outside every family's set.
    ("the-wrong-case", "accountid", true),
    ("empty", "", false),
];

/// The whole message a filter-name refusal must carry: the service's validation
/// report for the one member that failed, naming the member and the set.
fn expected_message(position: usize, accepted: &[&str]) -> String {
    format!(
        "1 validation error detected: Value at 'filters.{position}.member.name' failed to \
         satisfy constraint: Member must satisfy enum value set: [{}]",
        accepted.join(", ")
    )
}

/// A request for `operation` whose filter list holds `name` at `position` (1
/// based), preceded where `position` is not the first by a filter the operation
/// does accept.
fn request_with(name: &str, position: usize, accepted: &[&str]) -> Value {
    let accepted_name = accepted.first().unwrap_or_else(|| {
        panic!("an operation that filters declares at least one accepted filter name")
    });
    let mut filters: Vec<Value> = (1..position)
        .map(|_| json!({"name": accepted_name, "values": ["x"]}))
        .collect();
    filters.push(json!({"name": name, "values": ["x"]}));
    json!({"filters": filters})
}

/// The value sets the cases below are built from are the model's, read back from
/// the crate's generated value constants: a filter name the model gains or loses
/// fails HERE rather than leaving a stale set in an assertion.
#[test]
fn the_value_sets_under_test_are_the_model_s() {
    // (the set under test, the generated constant for the same shape)
    let sets: [(&str, &[&str], &[&str]); 3] = [
        (
            "AutomationEventFilterName",
            EVENT_FILTER_NAMES,
            AUTOMATION_EVENT_FILTER_NAME_VALUES,
        ),
        (
            "RecommendedActionFilterName",
            RECOMMENDED_ACTION_FILTER_NAMES,
            GENERATED_RECOMMENDED_ACTION_FILTER_NAMES,
        ),
        (
            "AutomationRuleFilterName",
            RULE_FILTER_NAMES,
            AUTOMATION_RULE_FILTER_NAME_VALUES,
        ),
    ];

    for (shape, declared, generated) in sets {
        // The generated constant is sorted and this suite's copy is in model
        // order, so the sets are compared as sets.
        let mut declared_sorted = declared.to_vec();
        declared_sorted.sort_unstable();
        let mut generated_sorted = generated.to_vec();
        generated_sorted.sort_unstable();
        assert_eq!(
            declared_sorted, generated_sorted,
            "case {shape:?}: the suite's value set must be the model's, got {declared:?} against \
             {generated:?}"
        );
    }
}

/// A filter name outside the value set its operation declares is refused with
/// the service's validation report — which names the member by its position in
/// the list and enumerates that operation's accepted set — and the rejected name
/// appears nowhere in the body.
#[tokio::test]
async fn a_filter_name_outside_its_value_set_is_refused_without_being_echoed() {
    let router = emulator();

    for (operation, accepted) in FILTERING_OPERATIONS {
        for (name_case, name, distinctive) in FILTER_NAMES_TO_SEND {
            if accepted.contains(name) {
                continue;
            }
            // The first list position and a later one, so the report is pinned
            // to naming the offending filter rather than always the first.
            for position in [1usize, 2] {
                let case = format!("{operation}-{name_case}-at-{position}");
                let request = request_with(name, position, accepted);
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
                let message = error_message(&body);
                let expected = expected_message(position, accepted);
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected message {expected:?}, got {message:?}"
                );
                if *distinctive {
                    assert!(
                        !message.contains(*name),
                        "case {case:?}: the report must not echo the rejected name, got \
                         {message:?}"
                    );
                }
            }
        }
    }
}

/// Two operations that narrow the SAME resources with the same filter list give
/// the same refusal for it: the same error code and the same message, so the
/// wording does not depend on which of the pair was called.
#[tokio::test]
async fn paired_listings_refuse_the_same_filter_list_the_same_way() {
    let router = emulator();

    // (case, the two operations over the same resources, the filter list)
    let pairs: Vec<(&str, &str, &str, Value)> = vec![
        (
            "events-unknown-name",
            "ListAutomationEvents",
            "ListAutomationEventSummaries",
            json!({"filters": [{"name": "Nickname", "values": ["x"]}]}),
        ),
        (
            "events-name-of-another-filter-set",
            "ListAutomationEvents",
            "ListAutomationEventSummaries",
            json!({"filters": [{"name": "Name", "values": ["x"]}]}),
        ),
        (
            "events-unknown-name-after-an-accepted-one",
            "ListAutomationEvents",
            "ListAutomationEventSummaries",
            json!({"filters": [
                {"name": "AccountId", "values": ["111111111111"]},
                {"name": "accountid", "values": ["111111111111"]},
            ]}),
        ),
        (
            "recommended-actions-unknown-name",
            "ListRecommendedActions",
            "ListRecommendedActionSummaries",
            json!({"filters": [{"name": "Nickname", "values": ["x"]}]}),
        ),
    ];

    for (case, first, second, request) in pairs {
        let (first_status, first_body) = call(&router, first, &request).await;
        let (second_status, second_body) = call(&router, second, &request).await;
        assert_eq!(
            first_status, second_status,
            "case {case:?}: both operations must refuse with the same status, got \
             {first_status} from {first} and {second_status} from {second}"
        );
        assert_eq!(
            error_code(&first_body),
            error_code(&second_body),
            "case {case:?}: both operations must refuse with the same error code, got \
             {first_body} and {second_body}"
        );
        assert_eq!(
            error_message(&first_body),
            error_message(&second_body),
            "case {case:?}: both operations must word the refusal the same way, got {:?} from \
             {first} and {:?} from {second}",
            error_message(&first_body),
            error_message(&second_body)
        );
    }
}

/// Positive control: every name a value set declares is accepted by the
/// operations typed on that set, so the refusals above are about the name being
/// outside the set and not about carrying a filter list at all.
#[tokio::test]
async fn every_declared_filter_name_is_accepted() {
    let router = emulator();

    for (operation, accepted) in FILTERING_OPERATIONS {
        for name in *accepted {
            let case = format!("{operation}-{name}");
            // A value that selects nothing is enough: the call has to be served,
            // and what each name selects on is asserted by the suite that owns
            // that listing.
            call_ok(
                &router,
                operation,
                &json!({"filters": [{"name": name, "values": ["111111111111"]}]}),
                &case,
            )
            .await;
        }
    }
}
