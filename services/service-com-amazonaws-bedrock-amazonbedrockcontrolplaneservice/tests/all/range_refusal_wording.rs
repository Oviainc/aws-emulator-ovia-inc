// SPDX-License-Identifier: CC0-1.0

//! How this service words a request refused for a number outside a member's modelled
//! `@range`, driven end-to-end over the same `MultiServiceRouter` the emulator boots.
//!
//! The rule under test: a value outside a `@range` bound is refused with HTTP 400
//! `ValidationException` whose message names the member's dotted path and the BOUND the
//! model declares — `1 validation error detected: <subject> at '<member path>' failed to
//! satisfy constraint: Member must have value {less,greater} than or equal to <bound>`
//! ([`a_range_violation_states_the_member_and_the_bound_with_the_subject_its_binding_decides`]).
//! Whether that subject quotes the number sent is the MEMBER's binding and not the range
//! class's (`src/service/validation.rs`, rule 2): a member the service reads out of the
//! request body states its position alone, one the model binds to the query string quotes
//! the token it refused. It holds for every `@range`-constrained
//! member of every operation, so the rows below span:
//!
//! * both bounds of a range (a minimum and a maximum), each broken by one step and by far
//!   more than one step;
//! * both numeric types the model constrains this way — an `integer` member and a `double`
//!   member whose rejected value is fractional;
//! * a range declared with a maximum and a minimum beside one declared with a minimum
//!   alone;
//! * members bound to a query string and members carried in the body, over six operations
//!   and three member names, two of which share a member name across operations.
//!
//! The CONTRAST row is in [`the_subject_of_a_clause_follows_the_members_binding_not_its_class`]: a
//! `@pattern` violation on a query-bound member in the same request keeps the value it
//! refused, so what the body rows remove is that member's content and not the envelope's
//! subject everywhere.
//!
//! Every bound below is quoted from this service's Smithy model, cited per row
//! (`smithy_query(query="operation",operation="…")` /
//! `query="shape",shape="MaxResults,PositiveInteger,AutomatedReasoningCheckTranslationConfidence"`),
//! never from a recorded response body. The requests go through `crate::support` — the ONE
//! harness every suite in this crate drives the emulator with.

use serde_json::{json, Value};

use crate::automated_reasoning_policy_test_cases::create_policy;
use crate::support::{emulator, label, Emulator, Reply};

// ---------------------------------------------------------------------------------------
// The bounds these rows quote, each read off the model.
// ---------------------------------------------------------------------------------------

/// `MaxResults` is `@range(min: 1, max: 1000)` (`query="shape"`, shape `MaxResults`) — the
/// shape every `maxResults` member of this service's list operations targets.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// `PositiveInteger` is `@range(min: 1)` with NO maximum (`query="shape"`, shape
/// `PositiveInteger`) — the shape `CreateProvisionedModelThroughput$modelUnits` targets.
const MODEL_UNITS_MIN: i64 = 1;

/// `AutomatedReasoningCheckTranslationConfidence` is a `double` `@range(min: 0, max: 1)`
/// (`query="shape"`, shape `AutomatedReasoningCheckTranslationConfidence`) — the shape
/// `confidenceThreshold` targets on both the create and the update of a test case.
const CONFIDENCE_MIN: f64 = 0.0;
const CONFIDENCE_MAX: f64 = 1.0;

/// The envelope AWS states ONE violation under.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause a `@range` violation states: the member path and the requirement, with the
/// subject the member's binding decides — the refused number quoted for a member the model
/// binds to the request URI, the position alone for one carried in the body.
fn range_clause(quoted: Option<&str>, member: &str, requirement: &str) -> String {
    let subject = match quoted {
        Some(value) => format!("Value '{value}'"),
        None => "Value".to_string(),
    };
    format!("{subject} at '{member}' failed to satisfy constraint: {requirement}")
}

/// The requirement a `@range` maximum states.
fn at_most(bound: &str) -> String {
    format!("Member must have value less than or equal to {bound}")
}

/// The requirement a `@range` minimum states.
fn at_least(bound: &str) -> String {
    format!("Member must have value greater than or equal to {bound}")
}

/// One row: a request that breaks exactly one `@range`, and the clause it must produce.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path the clause must be reported at.
    member: &'static str,
    /// The number the request carries, when the clause quotes it: `Some` for a member the
    /// model binds to the request URI, `None` for one carried in the body.
    quoted: Option<&'static str>,
    /// The requirement the clause must state.
    requirement: String,
}

/// A test-case create body carrying `threshold`, with every required member valid so the
/// threshold is the only thing the request breaks
/// (`query="operation"`, `CreateAutomatedReasoningPolicyTestCase`: `guardContent` and
/// `expectedAggregatedFindingsResult` are `@required`).
fn test_case_body(threshold: Value) -> Value {
    json!({
        "guardContent": "All returns are accepted within 30 days.",
        "expectedAggregatedFindingsResult": "INVALID",
        "confidenceThreshold": threshold,
    })
}

/// `POST /automated-reasoning-policies/{policyArn}/test-cases`, as the model binds it.
fn test_cases_path(policy_arn: &str) -> String {
    format!(
        "/automated-reasoning-policies/{}/test-cases",
        label(policy_arn)
    )
}

async fn call(emu: &Emulator, method: &str, path: &str, body: Option<Value>) -> Reply {
    emu.call(method, path, body).await
}

/// The `message` a reply states, or the empty string when it carries none.
fn message(reply: &Reply) -> String {
    reply
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A number outside a member's `@range` is refused with the member and the bound, and with
/// no echo of the number that was sent — over every row of the table.
#[tokio::test]
async fn a_range_violation_states_the_member_and_the_bound_with_the_subject_its_binding_decides() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "range-wording-policy").await;
    // A test case under that policy, so the UPDATE row addresses a `testCaseId` the
    // model's `^[0-9A-Z]{12}$` admits rather than a made-up one.
    let created = call(
        &emu,
        "POST",
        &test_cases_path(&policy_arn),
        Some(test_case_body(json!(0.5))),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"a test case to update\": the create must answer 200, got {} {:?}",
        created.status, created.body
    );
    let test_case_id = created.text("testCaseId");

    let rows = vec![
        Row {
            // The `double` member, one step past its maximum.
            case: "CreateAutomatedReasoningPolicyTestCase confidenceThreshold above its \
                   maximum",
            method: "POST",
            path: test_cases_path(&policy_arn),
            body: Some(test_case_body(json!(1.5))),
            member: "confidenceThreshold",
            quoted: None,
            requirement: at_most("1"),
        },
        Row {
            // The same member below its minimum: the rule is the class's, not the
            // maximum's.
            case: "CreateAutomatedReasoningPolicyTestCase confidenceThreshold below its \
                   minimum",
            method: "POST",
            path: test_cases_path(&policy_arn),
            body: Some(test_case_body(json!(-0.25))),
            member: "confidenceThreshold",
            quoted: None,
            requirement: at_least("0"),
        },
        Row {
            // A fractional value a hair past the maximum, so what the clause omits is not
            // only a value with a short decimal form.
            case: "CreateAutomatedReasoningPolicyTestCase confidenceThreshold a hair past \
                   its maximum",
            method: "POST",
            path: test_cases_path(&policy_arn),
            body: Some(test_case_body(json!(1.000_000_1))),
            member: "confidenceThreshold",
            quoted: None,
            requirement: at_most("1"),
        },
        Row {
            // The same member name on ANOTHER operation, reached with another method
            // (`query="operation"`, `UpdateAutomatedReasoningPolicyTestCase`: PATCH, with
            // `lastUpdatedAt` and `guardContent` `@required`).
            case: "UpdateAutomatedReasoningPolicyTestCase confidenceThreshold above its \
                   maximum",
            method: "PATCH",
            path: format!(
                "/automated-reasoning-policies/{}/test-cases/{}",
                label(&policy_arn),
                label(&test_case_id)
            ),
            body: Some(json!({
                "guardContent": "All returns are accepted within 30 days.",
                "expectedAggregatedFindingsResult": "VALID",
                // Epoch seconds: a JSON number is read as epoch seconds whatever format
                // the member declares, so the row carries the timestamp in a form that
                // reaches the constraint walk rather than asserting the wire form (the
                // modelled `date-time` string is read too — see
                // `types/update_automated_reasoning_policy_test_case_types.rs`).
                "lastUpdatedAt": 1_767_225_600_i64,
                "confidenceThreshold": 2,
            })),
            member: "confidenceThreshold",
            quoted: None,
            requirement: at_most("1"),
        },
        Row {
            // An `integer` member bound to a QUERY string, below its minimum: a token the
            // service read off the request line, so the clause quotes it.
            case: "ListGuardrails maxResults below its minimum",
            method: "GET",
            path: format!("/guardrails?maxResults={}", MAX_RESULTS_MIN - 1),
            body: None,
            member: "maxResults",
            quoted: Some("0"),
            requirement: at_least(&MAX_RESULTS_MIN.to_string()),
        },
        Row {
            case: "ListGuardrails maxResults one past its maximum",
            method: "GET",
            path: format!("/guardrails?maxResults={}", MAX_RESULTS_MAX + 1),
            body: None,
            member: "maxResults",
            quoted: Some("1001"),
            requirement: at_most(&MAX_RESULTS_MAX.to_string()),
        },
        Row {
            // The same bound on another operation, broken by far more than one step.
            case: "ListCustomModels maxResults far past its maximum",
            method: "GET",
            path: format!("/custom-models?maxResults={}", MAX_RESULTS_MAX * 100),
            body: None,
            member: "maxResults",
            quoted: Some("100000"),
            requirement: at_most(&MAX_RESULTS_MAX.to_string()),
        },
        Row {
            case: "ListAutomatedReasoningPolicies maxResults below its minimum",
            method: "GET",
            path: "/automated-reasoning-policies?maxResults=0".to_string(),
            body: None,
            member: "maxResults",
            quoted: Some("0"),
            requirement: at_least(&MAX_RESULTS_MIN.to_string()),
        },
        Row {
            // A range declared with a MINIMUM ALONE, on a body member.
            case: "CreateProvisionedModelThroughput modelUnits below its minimum",
            method: "POST",
            path: "/provisioned-model-throughput".to_string(),
            body: Some(json!({
                "provisionedModelName": "rangewording",
                "modelId": "amazon.titan-text-lite-v1",
                "modelUnits": MODEL_UNITS_MIN - 1,
            })),
            member: "modelUnits",
            quoted: None,
            requirement: at_least(&MODEL_UNITS_MIN.to_string()),
        },
        Row {
            case: "CreateProvisionedModelThroughput modelUnits far below its minimum",
            method: "POST",
            path: "/provisioned-model-throughput".to_string(),
            body: Some(json!({
                "provisionedModelName": "rangewording",
                "modelId": "amazon.titan-text-lite-v1",
                "modelUnits": -4096,
            })),
            member: "modelUnits",
            quoted: None,
            requirement: at_least(&MODEL_UNITS_MIN.to_string()),
        },
    ];

    for row in rows {
        let case = row.case;
        let reply = call(&emu, row.method, &row.path, row.body).await;
        let stated = message(&reply);
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {} {stated:?}",
            reply.status
        );
        let expected = one_error(&range_clause(row.quoted, row.member, &row.requirement));
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        if row.quoted.is_none() {
            assert!(
                !stated.contains("Value '"),
                "case {case:?}: a clause on a member read out of the body quotes no value, \
                 got {stated:?}"
            );
        }
    }
}

/// Each published bound of a `@range` is INSIDE it: the value at the minimum and the value
/// at the maximum are accepted, which is what makes the rows one past them refusals of the
/// bound rather than of every number.
#[tokio::test]
async fn each_published_bound_of_a_range_is_accepted() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "range-bound-policy").await;

    // (case, method, path, body, the status the model declares for the success)
    let rows: Vec<(String, &str, String, Option<Value>, u16)> = vec![
        (
            format!("confidenceThreshold at its minimum {CONFIDENCE_MIN}"),
            "POST",
            test_cases_path(&policy_arn),
            Some(test_case_body(json!(CONFIDENCE_MIN))),
            200,
        ),
        (
            format!("confidenceThreshold at its maximum {CONFIDENCE_MAX}"),
            "POST",
            test_cases_path(&policy_arn),
            Some(test_case_body(json!(CONFIDENCE_MAX))),
            200,
        ),
        (
            format!("maxResults at its minimum {MAX_RESULTS_MIN}"),
            "GET",
            format!("/guardrails?maxResults={MAX_RESULTS_MIN}"),
            None,
            200,
        ),
        (
            format!("maxResults at its maximum {MAX_RESULTS_MAX}"),
            "GET",
            format!("/guardrails?maxResults={MAX_RESULTS_MAX}"),
            None,
            200,
        ),
        (
            format!("modelUnits at its minimum {MODEL_UNITS_MIN}"),
            "POST",
            "/provisioned-model-throughput".to_string(),
            Some(json!({
                "provisionedModelName": "boundwording",
                "modelId": "amazon.titan-text-lite-v1",
                // A commitment, so the create is answered on its own terms rather than by
                // the no-commitment capacity quota: what this row pins is that the value
                // AT the bound is not refused for its range.
                "commitmentDuration": "OneMonth",
                "modelUnits": MODEL_UNITS_MIN,
            })),
            201,
        ),
    ];

    for (case, method, path, body, success) in rows {
        let reply = call(&emu, method, &path, body).await;
        assert_eq!(
            reply.status,
            success,
            "case {case:?}: a value AT the bound is inside the range, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

/// The envelope's subject follows the MEMBER's binding and not the violation's class: one
/// request whose two offending members are both read off the request line states two
/// clauses that each quote what they refused — a `@range` as much as a `@pattern` — and one
/// whose two offending members are both read out of the body states two clauses that quote
/// nothing.
#[tokio::test]
async fn the_subject_of_a_clause_follows_the_members_binding_not_its_class() {
    let emu = emulator();
    // `ListGuardrails$guardrailIdentifier` targets `GuardrailIdentifier`
    // (`query="operation"`, `ListGuardrails`), whose `@pattern` admits a bare id or a
    // guardrail ARN and neither of which `Not_An_Id` is; `maxResults` is past its maximum.
    // Both are `@httpQuery` members of that operation.
    let reply = call(
        &emu,
        "GET",
        &format!(
            "/guardrails?guardrailIdentifier=Not_An_Id&maxResults={}",
            MAX_RESULTS_MAX + 1
        ),
        None,
    )
    .await;
    let stated = message(&reply);
    let case = "a guardrailIdentifier outside its pattern beside a maxResults past its maximum";
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected 400 ValidationException, got {} {stated:?}",
        reply.status
    );
    assert!(
        stated.starts_with("2 validation errors detected: "),
        "case {case:?}: the envelope counts both clauses, got {stated:?}"
    );
    assert!(
        stated.contains(&range_clause(
            Some("1001"),
            "maxResults",
            &at_most(&MAX_RESULTS_MAX.to_string())
        )),
        "case {case:?}: the range clause on a query-bound member quotes the number it \
         refused, got {stated:?}"
    );
    assert!(
        stated.contains("Value 'Not_An_Id' at 'guardrailIdentifier'"),
        "case {case:?}: the pattern clause keeps the value it refused, got {stated:?}"
    );

    // The contrast: `CreateProvisionedModelThroughput` carries both offending members in
    // its BODY (`query="operation"`, `CreateProvisionedModelThroughput`:
    // `provisionedModelName` declares a `@pattern`, `modelUnits` a `@range` minimum), so
    // neither clause quotes anything.
    let reply = call(
        &emu,
        "POST",
        "/provisioned-model-throughput",
        Some(json!({
            "provisionedModelName": "not a legal name!",
            "modelId": "amazon.titan-text-lite-v1",
            "modelUnits": MODEL_UNITS_MIN - 1,
        })),
    )
    .await;
    let stated = message(&reply);
    let case = "a provisionedModelName outside its pattern beside a modelUnits below its minimum";
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected 400 ValidationException, got {} {stated:?}",
        reply.status
    );
    assert!(
        stated.starts_with("2 validation errors detected: "),
        "case {case:?}: the envelope counts both clauses, got {stated:?}"
    );
    assert!(
        stated.contains(&range_clause(
            None,
            "modelUnits",
            &at_least(&MODEL_UNITS_MIN.to_string())
        )),
        "case {case:?}: the range clause on a body member states the member and the bound \
         alone, got {stated:?}"
    );
    assert!(
        stated.contains("Value at 'provisionedModelName' failed to satisfy constraint: Member \
                         must satisfy regular expression pattern: "),
        "case {case:?}: the pattern clause on a body member states the position and the \
         expression alone, got {stated:?}"
    );
    assert!(
        !stated.contains("Value '"),
        "case {case:?}: no body content reaches the sentence, got {stated:?}"
    );
}
