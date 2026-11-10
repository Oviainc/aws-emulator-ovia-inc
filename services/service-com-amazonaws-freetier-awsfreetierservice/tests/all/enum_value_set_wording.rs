// SPDX-License-Identifier: CC0-1.0

//! What a refusal of a value outside an enumerated type SAYS, and that this
//! service has exactly ONE rendering of it.
//!
//! The rule: a request carrying a value an enumerated type does not declare is
//! refused `400 ValidationException` whose `message` is the AWS validation
//! clause — the value as submitted, the member's dotted path, and every value
//! that type admits, listed in the order the SERVICE states that set in rather
//! than the order the model declares its members in. The order is a property of
//! the enum, so it does not vary with the operation that carried the value, with
//! the member that carried it, with which value was submitted, or with the LAYER
//! that found the violation.
//!
//! That last part is what this suite adds to the per-operation suites
//! (`paid_plan_vetting.rs`, `activity_plane_refusals.rs`), which drive the rule
//! through the framework's constraint walk alone: two layers can produce this
//! refusal — that walk in front of every handler, and the handler's own
//! re-check, which keeps the refusal this operation's 400 whatever the boundary
//! does with a value the enum does not declare. Each row below asserts the wire
//! message AND the message the handler layer states for the same violation
//! (`state::enum_value_refusal`, `state::enum_element_refusal`) against one
//! expectation written out here, so a second rendering of a value set cannot
//! appear in either layer.
//!
//! Grounding: the enums and the members that target them come from
//! `smithy_query(query="shape",shape="AccountPlanType,LanguageCode,ActivityStatus")`
//! and `smithy_query(query="operation",operation="UpgradeAccountPlan,GetAccountActivity,ListAccountActivities")`;
//! the value sets are the generated `ops::types::*_VALUES` constants, and each
//! published order is first pinned to be a permutation of its declared set, so a
//! published order can only reorder a set and never change what it holds.
//!
//! The rows span the class the rule generalises over: the enumerated type, the
//! member and the operation carrying it, how the submitted value misses the set
//! (an unknown word, wrong case, padding, the empty string, a value of another
//! enum of this service), and — for the list member — the length of the list and
//! the position of the element that missed. A last suite covers the other place
//! a value set is NAMED: the sentences this service words itself
//! (`GetFreeTierUsage`'s filter refusals), which resolve the order through the
//! same one place (`state::listed_enum_value_set`), so a set this service
//! publishes no order for is still listed as the model declares it.

use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::ops::types::{
    ACCOUNT_PLAN_TYPE_VALUES, ACTIVITY_STATUS_VALUES, DIMENSION_VALUES, LANGUAGE_CODE_VALUES,
    MATCH_OPTION_VALUES,
};
use service_com_amazonaws_freetier_awsfreetierservice::state::{
    enum_element_refusal, enum_value_refusal, listed_enum_value_set, ACCOUNT_PLAN_TYPE_VALUE_SET,
    ACTIVITY_STATUS_VALUE_SET, LANGUAGE_CODE_VALUE_SET,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::free_tier::{harness, ok, text};
use crate::validation_message_rules::refusal_message;

/// One row of the scalar-member table: the member a clause names, the values its
/// enum declares, that enum's set as a refusal lists it, the operations carrying
/// the member, and the values submitted at it.
type MemberRow<'row> = (
    &'row str,
    &'row [&'row str],
    &'row str,
    &'row [&'row str],
    &'row [&'row str],
);

/// The size one refusal must stay under whatever the request carried: an upper
/// bound on the service's own clause ceiling, not that ceiling itself (which is
/// `services/common`'s and is not published). What it pins is that the bound
/// exists — the same cap holds for a request of any size.
const CLAUSE_SIZE_CAP: usize = 32 * 1_024;

/// The AWS clause for a value outside an enumerated type, written out here
/// rather than read off the service so the expectation and the implementation do
/// not share one source: the count envelope, the value as submitted, the member
/// path, and the admitted set in brackets.
fn expected_clause(member: &str, submitted: &str, value_set: &str) -> String {
    format!(
        "1 validation error detected: Value '{submitted}' at '{member}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{value_set}]"
    )
}

/// The same clause for a value outside the enumerated type a LIST's elements
/// target, which AWS states at the list member: the whole list as submitted, and
/// the element's requirements nested.
fn expected_element_clause(member: &str, submitted: &[&str], value_set: &str) -> String {
    format!(
        "1 validation error detected: Value '[{}]' at '{member}' failed to satisfy constraint: \
         Member must satisfy constraint: [Member must satisfy enum value set: [{value_set}]]",
        submitted.join(", ")
    )
}

/// The published order of one enumerated type as a refusal lists it, after
/// pinning it to hold exactly the values the model declares.
fn value_set(enumerated: &str, published: &[&str], declared: &[&str]) -> String {
    let mut sorted_published = published.to_vec();
    sorted_published.sort_unstable();
    let mut sorted_declared = declared.to_vec();
    sorted_declared.sort_unstable();
    assert_eq!(
        sorted_published, sorted_declared,
        "case {enumerated}: the published order must hold exactly the values the model \
         declares, got {published:?} against {declared:?}"
    );
    published.join(", ")
}

/// The id of one stored activity, so the detail reader has a well-formed id to
/// carry the `languageCode` under test.
async fn one_activity_id(router: &MultiServiceRouter) -> String {
    let listing = ok(router, "ListAccountActivities", &json!({"maxResults": 1})).await;
    let rows = listing["activities"]
        .as_array()
        .unwrap_or_else(|| panic!("case harness: activities must be a list, got {listing}"));
    let first = rows
        .first()
        .unwrap_or_else(|| panic!("case harness: the account holds activities, got {listing}"));
    text(first, "activityId")
}

/// RULE: a value outside an enumerated type is refused with the AWS clause
/// naming that type's set in the order the service states it — the same message
/// over the wire and from the handler layer's own check, for every member of
/// every operation that targets one of these enums.
#[tokio::test]
async fn a_value_outside_an_enumerated_type_is_refused_in_one_wording() {
    let router = harness().0;
    let activity_id = one_activity_id(&router).await;

    let plan_types = value_set(
        "AccountPlanType",
        &ACCOUNT_PLAN_TYPE_VALUE_SET,
        ACCOUNT_PLAN_TYPE_VALUES,
    );
    let languages = value_set("LanguageCode", &LANGUAGE_CODE_VALUE_SET, LANGUAGE_CODE_VALUES);

    // One [`MemberRow`] per enumerated type reached through a scalar member. The
    // submitted values differ in HOW they miss the set, and include none of the
    // values the per-operation suites already drive alone.
    let rows: [MemberRow; 2] = [
        (
            "accountPlanType",
            ACCOUNT_PLAN_TYPE_VALUES,
            plan_types.as_str(),
            &["UpgradeAccountPlan"],
            &["PREMIUM", "ENTERPRISE", "Free", "paid ", "", "IN_PROGRESS"],
        ),
        (
            "languageCode",
            LANGUAGE_CODE_VALUES,
            languages.as_str(),
            &["GetAccountActivity", "ListAccountActivities"],
            &["nl-NL", "EN-GB", "ja", "ja_JP", "", "PAID"],
        ),
    ];

    for (member, declared, value_set, operations, submitted_values) in rows {
        for submitted in submitted_values {
            assert!(
                !declared.contains(submitted),
                "case {member} {submitted:?}: the probe must not be a value the model declares"
            );
            let expected = expected_clause(member, submitted, value_set);

            // The layer in front of every handler: the framework's constraint
            // walk, worded by this service.
            for operation in operations {
                let body = request(operation, member, &activity_id, submitted);
                let case = format!("{operation} {member} {submitted:?}");
                let actual = refusal_message(&router, &case, operation, &body).await;
                assert_eq!(
                    actual, expected,
                    "case {case}: expected {expected:?}, got {actual:?}"
                );
            }

            // The handler's own re-check of the same modelled constraint, which
            // must state that same message rather than a second rendering of the
            // value set.
            let stated = enum_value_refusal(member, submitted, declared);
            assert_eq!(
                stated.as_deref(),
                Some(expected.as_str()),
                "case {member} {submitted:?}: the handler layer must state the clause the \
                 boundary states, got {stated:?}"
            );
        }

        // The control: every value the enum declares is admitted, so the check
        // above refuses nothing the model allows.
        for admitted in declared {
            let stated = enum_value_refusal(member, admitted, declared);
            assert!(
                stated.is_none(),
                "case {member} {admitted:?}: a value the model declares must not be refused, \
                 got {stated:?}"
            );
        }
    }
}

/// RULE, at a LIST member whose elements target an enumerated type: the clause is
/// stated once at the list, quoting the whole list the caller sent and nesting
/// the element's requirements, with the set in the same published order — over
/// the wire and from the handler layer alike.
///
/// The rows differ along the length of the list and the position of the element
/// that missed the set, which is what the clause's list rendering generalises
/// over.
#[tokio::test]
async fn a_list_element_outside_its_enumerated_type_is_refused_in_one_wording() {
    let router = harness().0;
    let statuses = value_set(
        "ActivityStatus",
        &ACTIVITY_STATUS_VALUE_SET,
        ACTIVITY_STATUS_VALUES,
    );
    let member = "filterActivityStatuses";

    for submitted in [
        vec!["PREMIUM"],
        vec!["COMPLETED", "Expiring"],
        vec!["not_started", "COMPLETED", "EXPIRING"],
        vec!["IN_PROGRESS", "COMPLETED", "", "EXPIRING"],
    ] {
        let expected = expected_element_clause(member, &submitted, statuses.as_str());
        let case = format!("{member} {submitted:?}");

        let actual = refusal_message(
            &router,
            &case,
            "ListAccountActivities",
            &json!({"filterActivityStatuses": submitted}),
        )
        .await;
        assert_eq!(
            actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );

        let owned: Vec<String> = submitted.iter().map(|value| (*value).to_string()).collect();
        let stated = enum_element_refusal(member, &owned, ACTIVITY_STATUS_VALUES);
        assert_eq!(
            stated.as_deref(),
            Some(expected.as_str()),
            "case {case}: the handler layer must state the clause the boundary states, got \
             {stated:?}"
        );
    }

    // The control: a filter naming only declared statuses is served, so the
    // check above refuses no list the model allows.
    let owned: Vec<String> = ACTIVITY_STATUS_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    let stated = enum_element_refusal(member, &owned, ACTIVITY_STATUS_VALUES);
    assert!(
        stated.is_none(),
        "case {member} every declared status: a list of values the model declares must not be \
         refused, got {stated:?}"
    );
    let served = ok(
        &router,
        "ListAccountActivities",
        &json!({"filterActivityStatuses": ACTIVITY_STATUS_VALUES}),
    )
    .await;
    assert!(
        served["activities"].is_array(),
        "case {member} every declared status: the listing must be served, got {served}"
    );
}

/// RULE, at the bound on what one clause may quote: a value far past the ceiling a
/// clause carries is quoted as a prefix plus the marker naming the whole value's
/// byte count, so the refusal's size is bounded by the SERVICE and not by the
/// request — in either layer that produces the refusal.
///
/// The rows differ along how far past the ceiling the value sits, and the cap they
/// are held to is the SAME at both sizes, so a message that grew with the request
/// fails here. The ceiling itself is `services/common`'s and is not published, so
/// nothing below names it: a value under it is quoted whole and pinned against the
/// other layer by the table above, and these rows assert what holds at any size —
/// a bounded message that still names the value set and how much was sent.
#[tokio::test]
async fn an_oversized_value_is_quoted_under_a_bounded_ceiling() {
    let router = harness().0;
    let member = "accountPlanType";
    let value_set = value_set(
        "AccountPlanType",
        &ACCOUNT_PLAN_TYPE_VALUE_SET,
        ACCOUNT_PLAN_TYPE_VALUES,
    );
    for length in [64 * 1_024, 1_024 * 1_024] {
        let submitted = "Z".repeat(length);
        let wire = refusal_message(
            &router,
            &format!("{member} of {length} bytes over the wire"),
            "UpgradeAccountPlan",
            &json!({"accountPlanType": submitted}),
        )
        .await;
        let stated = enum_value_refusal(member, &submitted, ACCOUNT_PLAN_TYPE_VALUES)
            .unwrap_or_else(|| panic!("case {member} of {length} bytes: it must be refused"));
        for (layer, message) in [("the boundary", &wire), ("the handler layer", &stated)] {
            let case = format!("{member} of {length} bytes from {layer}");
            assert!(
                message.ends_with(&format!("enum value set: [{value_set}]")),
                "case {case}: the clause must still name the published value set, got \
                 {message:?}"
            );
            assert!(
                message.contains(&format!("... [{length} bytes]")),
                "case {case}: an elided clause must name how much was sent, got {message:?}"
            );
            // The same cap holds at every size above, so the message is bounded by
            // the service: a clause interpolating the value would grow with it.
            assert!(
                message.len() < CLAUSE_SIZE_CAP,
                "case {case}: the refusal must be bounded by the service, got {} bytes",
                message.len()
            );
        }
    }
}

/// RULE, at the other place a refusal NAMES an enum's value set: the sentences
/// this service words itself list the set through the same order resolution, so
/// no site iterates the model's declaration order on its own — and a set this
/// service publishes no order for is listed exactly as the model declares it.
///
/// The rows are every enumerated type this crate names in a sentence of its own:
/// the one with a published order, and the two reached through `GetFreeTierUsage`'s
/// filter, which have none.
#[test]
fn every_listing_of_a_value_set_resolves_one_order() {
    // (the enumerated type, the values the model declares, the order a sentence
    // lists them in) — `MATCH_OPTION_VALUES` and `DIMENSION_VALUES` are listed as
    // declared because this service has shown no other order for them.
    let rows: [(&str, &[&str], String); 3] = [
        (
            "AccountPlanType",
            ACCOUNT_PLAN_TYPE_VALUES,
            ACCOUNT_PLAN_TYPE_VALUE_SET.join(", "),
        ),
        (
            "MatchOption",
            MATCH_OPTION_VALUES,
            MATCH_OPTION_VALUES.join(", "),
        ),
        ("Dimension", DIMENSION_VALUES, DIMENSION_VALUES.join(", ")),
    ];

    for (enumerated, declared, expected) in rows {
        let listed = listed_enum_value_set(declared);
        assert_eq!(
            listed, expected,
            "case {enumerated}: expected the set listed {expected:?}, got {listed:?}"
        );
    }
}

/// One request carrying `submitted` at `member`, for the operation under test.
fn request(operation: &str, member: &str, activity_id: &str, submitted: &str) -> Value {
    let mut body = match operation {
        "GetAccountActivity" => json!({"activityId": activity_id}),
        "UpgradeAccountPlan" => json!({}),
        _ => json!({"maxResults": 1}),
    };
    body[member] = json!(submitted);
    body
}
