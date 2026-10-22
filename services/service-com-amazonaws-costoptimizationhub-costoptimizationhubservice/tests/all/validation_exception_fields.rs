// SPDX-License-Identifier: CC0-1.0

//! The structured `ValidationException` this service answers a refused request
//! with, driven end-to-end over the wire (`awsJson1_0`) through the same
//! multi-service router the emulator boots.
//!
//! The rule under test: a request Cost Optimization Hub refuses because a member
//! is outside a constraint its model declares is answered with HTTP 400
//! `ValidationException` carrying `reason` = `FieldValidationFailed` and one
//! `fields` entry per violation, each naming the member it is attributed to and
//! the constraint that member must satisfy
//! (`smithy_query(query="shape", shape="ValidationException,ValidationExceptionDetail,ValidationExceptionReason")`).
//!
//! It is asserted over a table that spans the constraint classes the model uses
//! (`@pattern`, `@range`, `@length`, enum membership, `@required`), the positions
//! a violation can hold (a scalar member, a member of a nested structure, an
//! element of a nested list), the number of violations in one request, and the
//! operations that declare them — including one whose handler is still
//! scaffolded, because validation precedes the handler.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;

use crate::cost_optimization_hub::{call, router};

/// The model's enum value sets, as the generated catalogue holds them — the same
/// model data the constraint walk checks against, so the clause a refusal states
/// is compared with the model rather than with a list copied into this file.
use cost_optimization_hub::ops::types::shared_types::{
    ENROLLMENT_STATUS_VALUES, GRANULARITY_TYPE_VALUES, IMPLEMENTATION_EFFORT_VALUES,
    SAVINGS_ESTIMATION_MODE_VALUES,
};

/// The reason a member outside its declared constraint is reported under: one of
/// the two values `ValidationExceptionReason` holds (the other is `Other`).
const FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// The wire error code (`__type`) of an error body, without its namespace.
fn error_code(payload: &Value) -> String {
    payload["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `fields` list of a `ValidationException` body as `(fieldName, message)`
/// pairs, sorted so a case does not depend on the order the walk found the
/// violations in.
fn fields(payload: &Value) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = payload["fields"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["fieldName"].as_str().unwrap_or_default().to_string(),
                entry["message"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    entries.sort();
    entries
}

/// The clause a violation found on an ELEMENT of the named member states: the
/// element's own requirement, bracketed, because the member reported is the one
/// that holds the element.
fn element_clause(requirement: &str) -> String {
    format!("Member must satisfy constraint: [{requirement}]")
}

/// `AccountId`'s requirement, stated without the anchors the model spells it with
/// (`smithy_query(query="shape", shape="AccountId")`: `@pattern ^[0-9]{12}$`).
const ACCOUNT_ID_CLAUSE: &str = "Member must satisfy regular expression pattern: [0-9]{12}";

/// What one `fields` entry must say.
#[derive(Debug)]
enum Clause {
    /// Exactly this requirement.
    Exact(String),
    /// Enum membership over the model's value set, held at the named member
    /// itself (`on_element` false) or at an element of it (`on_element` true).
    ///
    /// The set is compared as a SET: which values the clause lists is the
    /// model's, but the order it lists them in is the model's declaration order
    /// and not a rule of the refusal.
    EnumSet {
        values: &'static [&'static str],
        on_element: bool,
    },
}

impl Clause {
    /// Whether `actual` is the clause this expectation describes.
    fn states(&self, actual: &str) -> bool {
        match self {
            Self::Exact(expected) => actual == expected,
            Self::EnumSet { values, on_element } => {
                let inner = if *on_element {
                    actual
                        .strip_prefix("Member must satisfy constraint: [")
                        .and_then(|inner| inner.strip_suffix(']'))
                } else {
                    Some(actual)
                };
                let listed = inner
                    .and_then(|inner| inner.strip_prefix("Member must satisfy enum value set: ["))
                    .and_then(|listed| listed.strip_suffix(']'));
                let Some(listed) = listed else {
                    return false;
                };
                let mut got: Vec<&str> = listed.split(", ").collect();
                got.sort_unstable();
                let mut wanted: Vec<&str> = values.to_vec();
                wanted.sort_unstable();
                got == wanted
            }
        }
    }
}

/// One `fields` entry, as a case writes its expectation: an exact requirement.
fn entry(field_name: &str, requirement: impl Into<String>) -> (&str, Clause) {
    (field_name, Clause::Exact(requirement.into()))
}

/// An `accountIds` filter one past its published maximum length
/// (`AccountIdList @length(min: 1, max: 100)`), whose last element is also
/// outside `AccountId`'s pattern — so the one member fails two distinct rules.
fn over_long_account_ids() -> Vec<String> {
    let mut ids: Vec<String> = (0..100).map(|n| format!("{n:012}")).collect();
    ids.push("12345".to_string());
    ids
}

/// One `fields` entry stating enum membership at the named member.
fn enum_entry<'a>(field_name: &'a str, values: &'static [&'static str]) -> (&'a str, Clause) {
    (
        field_name,
        Clause::EnumSet {
            values,
            on_element: false,
        },
    )
}

/// One `fields` entry stating enum membership at an ELEMENT of the named member.
fn enum_element_entry<'a>(
    field_name: &'a str,
    values: &'static [&'static str],
) -> (&'a str, Clause) {
    (
        field_name,
        Clause::EnumSet {
            values,
            on_element: true,
        },
    )
}

/// One row of the refusal table: the case name, the operation called, the request
/// sent, and the `fields` entries the answer must carry.
type RefusalCase<'a> = (&'a str, &'a str, Value, Vec<(&'a str, Clause)>);

/// Every refused request is 400 `ValidationException` with
/// `reason = FieldValidationFailed` and one `fields` entry per violation, naming
/// the member it is attributed to and what that member must satisfy.
#[tokio::test]
async fn a_constraint_refusal_names_every_member_and_its_constraint() {
    let router = router();

    // (case, operation, request, the fields entries the answer must carry)
    let cases: Vec<RefusalCase> = vec![
        (
            "pattern on a scalar member",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "accountId": "12345"}),
            vec![entry("accountId", ACCOUNT_ID_CLAUSE)],
        ),
        (
            // Same class, a value the issue did not cite: too LONG rather than
            // too short, and with the defaulted member omitted.
            "pattern on a scalar member, thirteen digits",
            "ListEnrollmentStatuses",
            json!({"accountId": "9876543210987"}),
            vec![entry("accountId", ACCOUNT_ID_CLAUSE)],
        ),
        (
            // `MaxResults` is `@range(min: 0, max: 100)`, so each published bound
            // is accepted (see `every_request_inside_its_constraints_is_served`)
            // and one past it is refused.
            "range above the published maximum",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "maxResults": 101}),
            vec![entry(
                "maxResults",
                "Member must have value less than or equal to 100",
            )],
        ),
        (
            "range below the published minimum",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "maxResults": -1}),
            vec![entry(
                "maxResults",
                "Member must have value greater than or equal to 0",
            )],
        ),
        (
            "enum membership on a required member",
            "UpdateEnrollmentStatus",
            json!({"status": "Enabled"}),
            vec![enum_entry("status", ENROLLMENT_STATUS_VALUES)],
        ),
        (
            "a required member absent altogether",
            "UpdateEnrollmentStatus",
            json!({}),
            vec![entry("status", "Member must not be null")],
        ),
        (
            "a required member of an implemented operation absent",
            "GetRecommendation",
            json!({}),
            vec![entry("recommendationId", "Member must not be null")],
        ),
        (
            // The violation is on an ELEMENT of a list nested in a structure, so
            // it is attributed to the list member that holds it.
            "pattern on an element of a nested list",
            "ListRecommendations",
            json!({"filter": {"accountIds": ["123456789012", "12345"]}}),
            vec![entry(
                "accountIds",
                element_clause(ACCOUNT_ID_CLAUSE),
            )],
        ),
        (
            "enum membership on an element of a nested list",
            "ListRecommendations",
            json!({"filter": {"implementationEfforts": ["Gigantic"]}}),
            vec![enum_element_entry(
                "implementationEfforts",
                IMPLEMENTATION_EFFORT_VALUES,
            )],
        ),
        (
            // The length of the nested list itself, not of an element: attributed
            // to the same member, stated unbracketed.
            "length of a nested list",
            "ListRecommendations",
            json!({"filter": {"accountIds": []}}),
            vec![entry(
                "accountIds",
                "Member must have length greater than or equal to 1",
            )],
        ),
        (
            // `fields` is the list of INVALID FIELDS, so a member whose values
            // failed the same rule twice is named once and states that rule once.
            "two elements of one nested list",
            "ListRecommendations",
            json!({"filter": {"accountIds": ["12345", "abc"]}}),
            vec![entry("accountIds", element_clause(ACCOUNT_ID_CLAUSE))],
        ),
        (
            // One member, two DISTINCT requirements — its elements' pattern and
            // its own length — stated as the bracketed list of both, in the order
            // the walk reached them.
            "a nested list that fails its own bound and its elements'",
            "ListRecommendations",
            json!({"filter": {"accountIds": over_long_account_ids()}}),
            vec![entry(
                "accountIds",
                element_clause(&format!(
                    "{ACCOUNT_ID_CLAUSE}, Member must have length less than or equal to 100"
                )),
            )],
        ),
        (
            "two members at once",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "accountId": "12345", "maxResults": 101}),
            vec![
                entry("accountId", ACCOUNT_ID_CLAUSE),
                entry(
                    "maxResults",
                    "Member must have value less than or equal to 100",
                ),
            ],
        ),
        (
            // Validation precedes the handler, so an operation this slice still
            // answers 501 for refuses a bad request the same way.
            "length on a scaffolded operation's member",
            "ListRecommendationSummaries",
            json!({"groupBy": "ActionType", "metrics": []}),
            vec![entry(
                "metrics",
                "Member must have length greater than or equal to 1",
            )],
        ),
        (
            "enum membership on a scaffolded operation's member",
            "ListEfficiencyMetrics",
            json!({
                "granularity": "Hourly",
                "timePeriod": {"start": "2026-01-01", "end": "2026-02-01"},
            }),
            vec![enum_entry("granularity", GRANULARITY_TYPE_VALUES)],
        ),
        (
            "enum membership on a write",
            "UpdatePreferences",
            json!({"savingsEstimationMode": "Nope"}),
            vec![enum_entry(
                "savingsEstimationMode",
                SAVINGS_ESTIMATION_MODE_VALUES,
            )],
        ),
    ];

    for (case, operation, request, expected) in &cases {
        let (status, payload) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {code:?} in {payload}"
        );
        let reason = payload["reason"].as_str().unwrap_or_default();
        assert_eq!(
            reason, FIELD_VALIDATION_FAILED,
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {reason:?} in {payload}"
        );
        let mut wanted: Vec<&(&str, Clause)> = expected.iter().collect();
        wanted.sort_by_key(|(field_name, _)| *field_name);
        let actual = fields(&payload);
        assert_eq!(
            actual.len(),
            wanted.len(),
            "case {case:?}: expected {} field entries ({wanted:?}), got {actual:?} in {payload}",
            wanted.len()
        );
        for ((field_name, message), (wanted_name, wanted_clause)) in actual.iter().zip(&wanted) {
            assert_eq!(
                field_name, wanted_name,
                "case {case:?}: expected a field entry for {wanted_name:?}, \
                 got {field_name:?} in {payload}"
            );
            assert!(
                wanted_clause.states(message),
                "case {case:?}: field {field_name:?}: expected {wanted_clause:?}, \
                 got {message:?}"
            );
        }
        // The required `message` is still there: a summary, with the per-member
        // diagnosis in `fields`.
        assert!(
            payload["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a non-empty message, got {payload}"
        );
    }
}

/// A refusal a handler raises for a rule the model cannot express fills the same
/// two members, so a client reads one shape whichever layer refused the request.
#[tokio::test]
async fn a_handler_raised_refusal_names_its_member_too() {
    let router = router();

    // (case, operation, request, the member the refusal is attributed to)
    let cases: [(&str, &str, Value, &str); 3] = [
        (
            "a nextToken this service never issued",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "nextToken": "not-a-token"}),
            "nextToken",
        ),
        (
            "a nextToken from another list",
            "ListRecommendations",
            json!({"nextToken": "6e6f7065"}),
            "nextToken",
        ),
        (
            "an orderBy dimension that names no member",
            "ListRecommendations",
            json!({"orderBy": {"dimension": "NotADimension", "order": "Asc"}}),
            "orderBy.dimension",
        ),
    ];

    for (case, operation, request, member) in &cases {
        let (status, payload) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        let reason = payload["reason"].as_str().unwrap_or_default();
        assert_eq!(
            reason, FIELD_VALIDATION_FAILED,
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {reason:?} in {payload}"
        );
        let named: Vec<String> = fields(&payload)
            .into_iter()
            .map(|(field_name, _)| field_name)
            .collect();
        assert_eq!(
            named,
            vec![(*member).to_string()],
            "case {case:?}: expected fields naming {member:?}, got {named:?} in {payload}"
        );
    }
}

/// A request inside every constraint it declares is served, not refused: each
/// published bound of a `@range`, every value of an enum, a list at its minimum
/// length, and a 12-digit account id. An operation whose handler is still
/// scaffolded answers 501 — the refusal layer added for the field list must not
/// turn a valid request into a 400.
#[tokio::test]
async fn every_request_inside_its_constraints_is_served() {
    let router = router();

    // (case, operation, request, the status the request must get)
    let mut cases: Vec<(String, &str, Value, u16)> = vec![
        (
            "maxResults at the published minimum".to_string(),
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "maxResults": 0}),
            200,
        ),
        (
            "maxResults at the published maximum".to_string(),
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "maxResults": 100}),
            200,
        ),
        (
            "a twelve-digit accountId".to_string(),
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": true, "accountId": "123456789012"}),
            200,
        ),
        (
            "a nested list at its minimum length".to_string(),
            "ListRecommendations",
            json!({"filter": {"accountIds": ["123456789012"]}}),
            200,
        ),
        (
            "an enum value on a nested list element".to_string(),
            "ListRecommendations",
            json!({"filter": {"implementationEfforts": ["VeryLow"]}}),
            200,
        ),
        (
            "a valid request to a scaffolded operation".to_string(),
            "ListRecommendationSummaries",
            json!({"groupBy": "ActionType", "metrics": ["SavingsPercentage"]}),
            501,
        ),
    ];
    // Every value the model's `EnrollmentStatus` enum holds is accepted, so the
    // refusal above is about membership and not about the member.
    for status in ENROLLMENT_STATUS_VALUES {
        cases.push((
            format!("the enrollment status {status:?}"),
            "UpdateEnrollmentStatus",
            json!({"status": status}),
            200,
        ));
    }

    for (case, operation, request, expected) in &cases {
        let (status, payload) = call(&router, operation, request).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} with {payload}"
        );
        assert!(
            payload["fields"].is_null(),
            "case {case:?}: a request inside its constraints carries no field list, got {payload}"
        );
    }
}
