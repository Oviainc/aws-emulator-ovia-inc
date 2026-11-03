// SPDX-License-Identifier: CC0-1.0

//! What this service's refusals SAY, driven over the wire through the same
//! multi-service router the emulator boots.
//!
//! A refusal's `message` is part of its contract: a client branches on it, logs it and
//! re-prompts from it, so a sentence this emulator wrote itself is a defect even when the
//! status and the error code are right. Two layers produce one, and each has its own rule:
//!
//! 1. a condition a HANDLER checks (the root-operator arity of a `filter`, the number of
//!    match options, a `nextToken` the service did not issue) is stated as ONE fixed
//!    sentence for the whole condition: nothing of the request appears in it, so every
//!    input that breaks the rule reads the same. WHICH sentence, and whether it is wrapped
//!    in the envelope this service's filter-shape layer states a condition in
//!    (`crate::state::stated_validation`), is a property of the operation that states it;
//!    an unissued `nextToken` is the condition two operations reach and state differently;
//! 2. a constraint the MODEL declares is refused in front of the handler and stated in the
//!    AWS validation envelope — one clause per failing member, with the value as
//!    submitted, the member's dotted path and the requirement — and a constraint broken by
//!    a LIST's elements is stated ONCE at the list member, with the whole list as the value
//!    and every constraint the element shape declares nested.
//!
//! Every assertion below is about those rules over a TABLE of inputs rather than one
//! recorded call: the rows differ along the arity, the operator set, the nesting depth, the
//! token, the match-option count, the member path, the constraint class and the number of
//! failing elements. Each refusal table carries its positive control, so a passing row
//! cannot mean "every request is refused".

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_freetier_awsfreetierservice::ops::types::{
    ACTIVITY_STATUS_VALUES, MATCH_OPTION_VALUES,
};
use service_com_amazonaws_freetier_awsfreetierservice::state::{
    stated_validation, ACCOUNT_PLAN_TYPE_VALUE_SET, ACTIVITIES_INVALID_NEXT_TOKEN,
    INVALID_NEXT_TOKEN,
};

use crate::free_tier::{call, error_code, harness, ok};

/// The modelled `Value` constraints, from
/// `smithy_query(query="shape", shape="Values,Value")`: the list's member targets `Value`,
/// a string of 1..20 characters matching `^[ a-zA-Z0-9\-\:\.\_\/\,\$\(\)]*$`.
const MAX_VALUE_LENGTH: usize = 20;

/// The requirements a clause lists for one element of `Values`: every constraint that
/// member's element shape declares — the length maximum, the length minimum and the
/// pattern, which is stated without the anchors the model writes the whole-value match
/// with.
const VALUES_ELEMENT_CONSTRAINTS: &str = "Member must satisfy constraint: [Member must have \
     length less than or equal to 20, Member must have length greater than or equal to 1, \
     Member must satisfy regular expression pattern: \
     [ a-zA-Z0-9\\-\\:\\.\\_\\/\\,\\$\\(\\)]*]";

/// The `message` of an error body.
fn message(payload: &Value) -> String {
    payload["message"].as_str().unwrap_or_default().to_string()
}

/// Assert that one request is refused `400 ValidationException` and report its message.
///
/// Shared with the suites that assert ONE of these rules over a wider table (see
/// `tests/all/match_option_cardinality.rs`), so every suite reads a refusal's message the
/// same way.
pub(crate) async fn refusal_message(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> String {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400 from {operation}, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "case {case}: expected a ValidationException from {operation}, got {payload}"
    );
    message(&payload)
}

/// One `Dimensions` comparison as a filter expression.
fn comparison(key: &str, values: &[&str], match_options: &[&str]) -> Value {
    json!({"Dimensions": {"Key": key, "Values": values, "MatchOptions": match_options}})
}

/// A well-formed single-root filter, for a positive control and as the other operand of a
/// filter that sets two roots.
fn one_root() -> Value {
    comparison("REGION", &["us-east-1"], &["EQUALS"])
}

/// RULE 1, the root-operator arity: an `Expression` that does not set exactly one of its
/// root operators is refused with ONE sentence, whichever operators it set and however
/// many — the rule is the same rule in every case, so the reason a client reads does not
/// vary with the request.
#[tokio::test]
async fn a_filter_that_does_not_set_one_root_operator_states_one_sentence() {
    let router = harness().0;
    let expected = "[Message = Only one root operator is allowed for each expression: And, Or, \
                    Not, Dimensions]";

    let branch = one_root();
    let other = comparison("REGION", &["us-west-2"], &["EQUALS"]);
    // What each root operator carries, so a row can set any subset of them.
    let roots: [(&str, Value); 4] = [
        ("Or", json!([other.clone()])),
        ("And", json!([branch.clone()])),
        ("Not", branch.clone()),
        (
            "Dimensions",
            json!({"Key": "SERVICE", "Values": ["AWSLambda"], "MatchOptions": ["EQUALS"]}),
        ),
    ];

    // Every subset of the four roots EXCEPT the four that set exactly one: the empty
    // expression and every pair, triple and the whole set.
    let mut rows: Vec<(String, Value)> = Vec::new();
    for mask in 0..16_u8 {
        let set: Vec<&(&str, Value)> = roots
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, root)| root)
            .collect();
        if set.len() == 1 {
            continue;
        }
        let mut filter = serde_json::Map::new();
        for (name, operand) in &set {
            filter.insert((*name).to_string(), operand.clone());
        }
        let names: Vec<&str> = set.iter().map(|(name, _)| *name).collect();
        rows.push((
            format!("a filter setting {names:?}"),
            Value::Object(filter),
        ));
    }
    assert_eq!(
        rows.len(),
        12,
        "case the arity table: expected every subset but the four single ones, got {}",
        rows.len()
    );

    for (case, filter) in rows {
        // The same expression at every position of the tree: the rule governs each
        // level, so a nested one is refused with the same sentence.
        let places: [(&str, Value); 4] = [
            ("at the root", filter.clone()),
            ("inside And", json!({"And": [filter.clone()]})),
            ("inside Or", json!({"Or": [branch.clone(), filter.clone()]})),
            ("inside Not", json!({"Not": {"Or": [filter.clone()]}})),
        ];
        for (place, nested) in places {
            let case = format!("{case} {place}");
            let actual = refusal_message(
                &router,
                &case,
                "GetFreeTierUsage",
                &json!({"maxResults": 10, "filter": nested}),
            )
            .await;
            assert_eq!(
                actual, expected,
                "case {case}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    // The positive control: each root operator ON ITS OWN is served.
    for (name, operand) in &roots {
        let filter = json!({ name.to_string(): operand.clone() });
        let answered = ok(
            &router,
            "GetFreeTierUsage",
            &json!({"maxResults": 10, "filter": filter}),
        )
        .await;
        assert!(
            answered["freeTierUsages"].is_array(),
            "case the single root operator {name}: expected a served page, got {answered}"
        );
    }
}

/// RULE 1, the match-option count: `MatchOptions` carries exactly one option
/// ("You can specify only one of these values in the array"), and every other count is
/// refused with ONE sentence that states neither the count submitted nor the options.
#[tokio::test]
async fn a_match_options_count_other_than_one_states_one_sentence() {
    let router = harness().0;
    let expected = "[Message = Exactly one match option must be provided]";

    // The rows differ along the count (none, two, three, all of them) and along WHICH
    // options are listed, including one option repeated.
    let rows: Vec<(String, Vec<&str>)> = vec![
        ("no option at all".to_string(), vec![]),
        ("two options".to_string(), vec!["EQUALS", "STARTS_WITH"]),
        (
            "two options, the other way round".to_string(),
            vec!["STARTS_WITH", "EQUALS"],
        ),
        (
            "one option repeated".to_string(),
            vec!["CONTAINS", "CONTAINS"],
        ),
        (
            "three options".to_string(),
            vec!["EQUALS", "CONTAINS", "ENDS_WITH"],
        ),
        (
            "every option the model declares".to_string(),
            MATCH_OPTION_VALUES.to_vec(),
        ),
    ];

    for (case, options) in rows {
        let filter = comparison("REGION", &["us-east-1"], &options);
        let actual = refusal_message(
            &router,
            &case,
            "GetFreeTierUsage",
            &json!({"maxResults": 10, "filter": filter}),
        )
        .await;
        assert_eq!(
            actual, expected,
            "case {case} ({options:?}): expected {expected:?}, got {actual:?}"
        );
    }

    // The positive control: exactly one option is served, for every option a text
    // dimension compares by (`GREATER_THAN_OR_EQUAL` compares magnitudes, so it is
    // driven on the one numeric dimension instead).
    for option in MATCH_OPTION_VALUES {
        let (key, value) = if *option == "GREATER_THAN_OR_EQUAL" {
            ("USAGE_PERCENTAGE", "0")
        } else {
            ("REGION", "us-east-1")
        };
        let filter = comparison(key, &[value], &[option]);
        let answered = ok(
            &router,
            "GetFreeTierUsage",
            &json!({"maxResults": 10, "filter": filter}),
        )
        .await;
        assert!(
            answered["freeTierUsages"].is_array(),
            "case the single match option {option}: expected a served page, got {answered}"
        );
    }
}

/// RULE 1, an unissued `nextToken`: each paginated operation refuses a token it did not
/// issue with ONE sentence of its own, which names neither the operation asked nor the
/// token sent — so every way of failing to be a token of that listing reads the same,
/// including a token the OTHER listing issued.
#[tokio::test]
async fn a_next_token_the_service_did_not_issue_states_one_sentence() {
    let router = harness().0;

    // A token each paginated operation really issued, so a row can send one operation's
    // token to the other.
    let usage_token = ok(&router, "GetFreeTierUsage", &json!({"maxResults": 1})).await
        ["nextToken"]
        .as_str()
        .expect("a page that leaves rows unreported issues a token")
        .to_string();
    let activities_token = ok(&router, "ListAccountActivities", &json!({"maxResults": 1})).await
        ["nextToken"]
        .as_str()
        .expect("a page that leaves rows unreported issues a token")
        .to_string();

    // The rows differ along what the token IS: prose, something that is not the encoding
    // at all, the encoding of another listing, and a prefix of a real token.
    let rows: Vec<(String, String)> = vec![
        (
            "a token that was never one".to_string(),
            "not-a-real-pagination-token".to_string(),
        ),
        ("a single character".to_string(), "x".to_string()),
        (
            "hex that decodes to something else".to_string(),
            ::hex::encode("ftv0:activities:0"),
        ),
        (
            "a position with no listing named".to_string(),
            ::hex::encode("12"),
        ),
        (
            "a prefix of a token this listing issued".to_string(),
            usage_token[..usage_token.len() - 2].to_string(),
        ),
    ];

    // Both paginated operations, each with the sentence IT states and with the token it
    // did NOT issue among the rows.
    let operations = [
        (
            "GetFreeTierUsage",
            activities_token.clone(),
            stated_validation(INVALID_NEXT_TOKEN),
        ),
        (
            "ListAccountActivities",
            usage_token.clone(),
            ACTIVITIES_INVALID_NEXT_TOKEN.to_string(),
        ),
    ];
    for (operation, foreign, expected) in &operations {
        for (case, token) in &rows {
            let case = format!("{operation} refusing {case}");
            let actual =
                refusal_message(&router, &case, operation, &json!({"nextToken": token})).await;
            assert_eq!(
                &actual, expected,
                "case {case} ({token:?}): expected {expected:?}, got {actual:?}"
            );
        }
        // The token of the OTHER listing, which is a token — just not this one's.
        let case = format!("{operation} refusing the other listing's token");
        let actual =
            refusal_message(&router, &case, operation, &json!({"nextToken": foreign})).await;
        assert_eq!(
            &actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );
    }

    // The positive control: the token each operation issued resumes its own listing.
    let resumed = ok(
        &router,
        "GetFreeTierUsage",
        &json!({"maxResults": 1, "nextToken": usage_token}),
    )
    .await;
    assert!(
        resumed["freeTierUsages"].is_array(),
        "case a token this listing issued: expected the next page, got {resumed}"
    );
    let resumed = ok(
        &router,
        "ListAccountActivities",
        &json!({"maxResults": 1, "nextToken": activities_token}),
    )
    .await;
    assert!(
        resumed["activities"].is_array(),
        "case a token this listing issued: expected the next page, got {resumed}"
    );
}

/// RULE 2, a constraint broken by a LIST's elements: the clause is stated once at the LIST
/// member — never at an indexed element — with the value that member holds (the whole list
/// the caller sent, bracketed) and with EVERY constraint the element shape declares nested,
/// whichever one the request broke.
#[tokio::test]
async fn a_list_elements_constraint_failure_is_stated_at_the_list_member() {
    let router = harness().0;

    let overlong = "x".repeat(MAX_VALUE_LENGTH + 1);
    // The rows differ along the constraint class broken (the element pattern, its length
    // maximum, its length minimum), the number and position of the failing elements, and
    // the member PATH the list is reached at — the same `Values` list nested under a
    // logical operator is reported at its own path.
    let rows: Vec<(String, Value, Vec<String>, &str)> = vec![
        (
            "one element outside the element pattern".to_string(),
            comparison("SERVICE", &["Amazon!EC2"], &["CONTAINS"]),
            vec!["Amazon!EC2".to_string()],
            "filter.dimensions.values",
        ),
        (
            "a bad element after a good one".to_string(),
            comparison("SERVICE", &["AWSLambda", "Amazon!EC2"], &["EQUALS"]),
            vec!["AWSLambda".to_string(), "Amazon!EC2".to_string()],
            "filter.dimensions.values",
        ),
        (
            "an element one character past the length maximum".to_string(),
            comparison("SERVICE", &[&overlong], &["EQUALS"]),
            vec![overlong.clone()],
            "filter.dimensions.values",
        ),
        (
            "an element below the length minimum".to_string(),
            comparison("SERVICE", &[""], &["EQUALS"]),
            vec![String::new()],
            "filter.dimensions.values",
        ),
        (
            "two bad elements, each breaking a different constraint".to_string(),
            comparison("SERVICE", &["Amazon!EC2", &overlong], &["EQUALS"]),
            vec!["Amazon!EC2".to_string(), overlong.clone()],
            "filter.dimensions.values",
        ),
        (
            "the same list nested under And".to_string(),
            json!({"And": [comparison("SERVICE", &["Amazon!EC2"], &["CONTAINS"])]}),
            vec!["Amazon!EC2".to_string()],
            "filter.and.1.member.dimensions.values",
        ),
        (
            "the same list nested under Not inside Or".to_string(),
            json!({"Or": [{"Not": comparison("SERVICE", &["Amazon!EC2"], &["CONTAINS"])}]}),
            vec!["Amazon!EC2".to_string()],
            "filter.or.1.member.not.dimensions.values",
        ),
    ];

    for (case, filter, submitted, path) in rows {
        let expected = format!(
            "1 validation error detected: Value '[{}]' at '{path}' failed to satisfy \
             constraint: {VALUES_ELEMENT_CONSTRAINTS}",
            submitted.join(", ")
        );
        let actual = refusal_message(
            &router,
            &case,
            "GetFreeTierUsage",
            &json!({"maxResults": 10, "filter": filter}),
        )
        .await;
        assert_eq!(
            actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );
    }

    // The positive control, at the published bound and one inside it: a value of exactly
    // the maximum length, and one of the minimum, are served.
    for value in [
        "x".repeat(MAX_VALUE_LENGTH),
        "x".to_string(),
        "Amazon-EC2".to_string(),
    ] {
        let filter = comparison("SERVICE", &[&value], &["CONTAINS"]);
        let answered = ok(
            &router,
            "GetFreeTierUsage",
            &json!({"maxResults": 10, "filter": filter}),
        )
        .await;
        assert!(
            answered["freeTierUsages"].is_array(),
            "case the well-formed value {value:?}: expected a served page, got {answered}"
        );
    }
}

/// The same rule on ANOTHER list member of another operation, whose element shape declares
/// a different constraint class: the clause is stated at `filterActivityStatuses` with the
/// whole list as the value and the element's value set nested.
///
/// The set is compared as a SET: which values it holds is the model's, while the order they
/// are listed in is the order that model declares them, and this suite reads the model
/// through a sorted object.
#[tokio::test]
async fn a_list_of_enumerated_elements_is_stated_at_the_list_member_too() {
    let router = harness().0;

    // The rows differ along the number of elements and the position of the bad one.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("one value outside the set", vec!["ABANDONED"]),
        (
            "a bad value after a good one",
            vec!["COMPLETED", "ABANDONED"],
        ),
        (
            "a bad value between good ones",
            vec!["COMPLETED", "ABANDONED", "EXPIRING"],
        ),
        ("two bad values", vec!["ABANDONED", "FORGOTTEN"]),
    ];

    for (case, statuses) in rows {
        let actual = refusal_message(
            &router,
            case,
            "ListAccountActivities",
            &json!({"filterActivityStatuses": statuses}),
        )
        .await;
        let opening = format!(
            "1 validation error detected: Value '[{}]' at 'filterActivityStatuses' failed to \
             satisfy constraint: Member must satisfy constraint: [Member must satisfy enum \
             value set: [",
            statuses.join(", ")
        );
        let listed = actual
            .strip_prefix(&opening)
            .and_then(|rest| rest.strip_suffix("]]"))
            .unwrap_or_default();
        assert!(
            !listed.is_empty(),
            "case {case}: expected a clause opening {opening:?}, got {actual:?}"
        );
        let mut listed: Vec<&str> = listed.split(", ").collect();
        listed.sort_unstable();
        let mut declared: Vec<&str> = ACTIVITY_STATUS_VALUES.to_vec();
        declared.sort_unstable();
        assert_eq!(
            listed, declared,
            "case {case}: expected the value set the model declares, {declared:?}, got \
             {listed:?} in {actual:?}"
        );
    }

    // The positive control: every value the set declares is served.
    for status in ACTIVITY_STATUS_VALUES {
        let answered = ok(
            &router,
            "ListAccountActivities",
            &json!({"filterActivityStatuses": [status]}),
        )
        .await;
        assert!(
            answered["activities"].is_array(),
            "case the declared status {status}: expected a served page, got {answered}"
        );
    }
}

/// RULE 2 on a member that is NOT a list element: the clause states the value as
/// submitted, the member's dotted path and the one requirement it failed — a `@pattern`
/// without the anchors the model writes the whole-value match with, and a COLLECTION's own
/// `@length` with no value at all, because a list has no scalar rendering to quote.
#[tokio::test]
async fn a_members_own_constraint_is_stated_with_its_value_path_and_requirement() {
    let router = harness().0;

    let overlong_id = "0".repeat(33);
    // (case, operation, body, the message stated)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a @pattern on a scalar member",
            "GetAccountActivity",
            json!({"activityId": "0000000000000000000000000000000-"}),
            "1 validation error detected: Value '0000000000000000000000000000000-' at \
             'activityId' failed to satisfy constraint: Member must satisfy regular \
             expression pattern: [a-zA-Z0-9]+"
                .to_string(),
        ),
        (
            "a @length maximum on a scalar member",
            "GetAccountActivity",
            json!({"activityId": overlong_id}),
            format!(
                "1 validation error detected: Value '{overlong_id}' at 'activityId' failed to \
                 satisfy constraint: Member must have length less than or equal to 32"
            ),
        ),
        (
            "a @length minimum on a scalar member",
            "GetAccountActivity",
            json!({"activityId": "short"}),
            "1 validation error detected: Value 'short' at 'activityId' failed to satisfy \
             constraint: Member must have length greater than or equal to 32"
                .to_string(),
        ),
        (
            "a @range minimum",
            "GetFreeTierUsage",
            json!({"maxResults": 0}),
            "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
             constraint: Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "a @range maximum",
            "ListAccountActivities",
            json!({"maxResults": 5000}),
            "1 validation error detected: Value '5000' at 'maxResults' failed to satisfy \
             constraint: Member must have value less than or equal to 1000"
                .to_string(),
        ),
        (
            "an absent @required member",
            "UpgradeAccountPlan",
            json!({}),
            "1 validation error detected: Value null at 'accountPlanType' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "a value outside an enumerated member",
            "UpgradeAccountPlan",
            json!({"accountPlanType": "GOLD"}),
            format!(
                "1 validation error detected: Value 'GOLD' at 'accountPlanType' failed to \
                 satisfy constraint: Member must satisfy enum value set: [{}]",
                ACCOUNT_PLAN_TYPE_VALUE_SET.join(", ")
            ),
        ),
        (
            "a LIST's own @length, stated with no value",
            "GetFreeTierUsage",
            json!({"filter": {"Dimensions":
                {"Key": "SERVICE", "Values": [], "MatchOptions": ["EQUALS"]}}}),
            "1 validation error detected: Value at 'filter.dimensions.values' failed to \
             satisfy constraint: Member must have length greater than or equal to 1"
                .to_string(),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let actual = refusal_message(&router, case, operation, &body).await;
        assert_eq!(
            actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );
    }
}
