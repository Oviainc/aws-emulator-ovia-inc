// SPDX-License-Identifier: CC0-1.0

//! WHICH refusals of this service carry the value they refused, driven end-to-end over the
//! same `MultiServiceRouter` the emulator boots.
//!
//! The rule under test: **whether a constraint clause quotes the value a member held is a
//! property of the SHAPE reached at that member, not of the member's name, the operation,
//! the constraint class or the binding that carried it.** This service states the refusals
//! of its advanced-prompt-optimization job identifier and of its automated-reasoning policy
//! test-case id by the member's position alone —
//! `N validation error detected: Value at '<member>' failed to satisfy constraint:
//! <the modelled requirement>` — and keeps the value for the other identifier shapes it
//! reaches the same way (`src/service/validation.rs`, rule 2). The model declares those
//! shapes exactly as it declares the ones that quote — a pattern-constrained string in a
//! `@httpLabel` — so the catalogue is recorded service metadata, and these rows are what
//! pins both sides of it.
//!
//! The rows span what the rule generalises over rather than one request: the shape
//! (two of them, against three that keep their value), the member path reaching it
//! (`jobIdentifier`, `testCaseId`, the `jobIdentifiers` collection whose ELEMENT is one of
//! those shapes), the operation and method (GET, POST and a body POST over seven
//! operations), and the FORM of the value refused (a full resource ARN, a bare identifier,
//! the wrong case, the wrong length, several list elements at once).
//!
//! [`a_shape_outside_the_catalogue_keeps_the_value_it_refused`] is the contrast: the SAME
//! member name over another identifier shape, another label-bound identifier shape, and a
//! query-bound member still quote what they refused — so what these rows pin is the shape
//! and not a sentence that quotes nothing anywhere.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation")` / `query="shape"`), never from a recorded response
//! body; the expressions are the constants `tests/all/validation_message_wording.rs` reads
//! off the model, not a second copy of them. The requests go through `crate::support` — the
//! ONE harness every suite in this crate drives the emulator with.

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator};
use crate::validation_message_wording::{
    apo_element_set, pattern_requirement, APO_JOB_IDENTIFIER, TEST_CASE_ID,
};

/// `GuardrailIdentifier`'s `@pattern`, unanchored (`query="shape"`, shape
/// `GuardrailIdentifier`) — a shape OUTSIDE the catalogue, reached as a `@httpLabel` exactly
/// as the catalogued ones are.
const GUARDRAIL_IDENTIFIER: &str =
    r"(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))";

/// `EvaluationJobIdentifier`'s `@pattern`, unanchored (`query="shape"`, shape
/// `EvaluationJobIdentifier`) — the other shape the model declares the member NAME
/// `jobIdentifier` over.
const EVALUATION_JOB_IDENTIFIER: &str =
    r"(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})";

/// A well-formed `AutomatedReasoningPolicyArn` for the rows whose subject is another label:
/// the policy need not exist, since the constraint walk refuses the request before any
/// handler reads state (`query="shape"`, shape `AutomatedReasoningPolicyArn`).
const POLICY_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:automated-reasoning-policy/abcdef123456";

/// A well-formed `AutomatedReasoningPolicyBuildWorkflowId`, for the same reason
/// (`query="shape"`: `@pattern`
/// `^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$`).
const BUILD_WORKFLOW_ID: &str = "abcdef12-3456-4789-8abc-abcdef123456";

/// One row: a request breaking exactly one constraint of one member, and the requirement
/// its clause must state.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path the clause is reported at.
    member: &'static str,
    /// The text the request carried there.
    carried: String,
    /// The requirement the clause must state.
    requirement: String,
}

/// The whole message a row of the catalogued shapes must be refused with: AWS's
/// one-violation envelope over the member's position and the requirement, with no value.
fn position_only(row: &Row) -> String {
    format!(
        "1 validation error detected: Value at '{}' failed to satisfy constraint: {}",
        row.member, row.requirement
    )
}

/// The whole message a row outside the catalogue must be refused with: the same envelope
/// with the value it refused.
fn with_value(row: &Row) -> String {
    format!(
        "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: {}",
        row.carried, row.member, row.requirement
    )
}

/// `GET /advanced-prompt-optimization-jobs/{jobIdentifier}`, as the model binds it.
fn get_job_path(identifier: &str) -> String {
    format!("/advanced-prompt-optimization-jobs/{}", label(identifier))
}

/// The requirement a `jobIdentifiers` element violation states: the element shape's whole
/// constraint set, nested in the collection clause
/// (`tests/all/validation_message_wording.rs` is the rule for the set itself).
fn job_identifiers_requirement() -> String {
    format!("Member must satisfy constraint: [{}]", apo_element_set())
}

/// The rows reaching a shape this service states by position alone: two shapes, three
/// member paths, seven operations and five forms of a refused identifier.
fn catalogued_rows() -> Vec<Row> {
    let apo_requirement = pattern_requirement(APO_JOB_IDENTIFIER);
    let test_case_requirement = pattern_requirement(TEST_CASE_ID);
    vec![
        Row {
            // `query="operation"`, `GetAdvancedPromptOptimizationJob`: `jobIdentifier` is a
            // `@httpLabel` over `AdvancedPromptOptimizationJobIdentifier`. A full job ARN
            // whose resource id is not twelve characters of `[a-z0-9]`.
            case: "GetAdvancedPromptOptimizationJob with a job ARN outside its pattern",
            method: "GET",
            path: get_job_path(
                "arn:aws:bedrock:us-west-2:000000000000:advanced-prompt-optimization-job/not-an-id",
            ),
            body: None,
            member: "jobIdentifier",
            carried:
                "arn:aws:bedrock:us-west-2:000000000000:advanced-prompt-optimization-job/not-an-id"
                    .to_string(),
            requirement: apo_requirement.clone(),
        },
        Row {
            // The same member, a BARE identifier of the wrong case: the pattern admits the
            // id without the ARN prefix, so the form refused here is another one.
            case: "GetAdvancedPromptOptimizationJob with a bare identifier in upper case",
            method: "GET",
            path: get_job_path("ABCDEF123456"),
            body: None,
            member: "jobIdentifier",
            carried: "ABCDEF123456".to_string(),
            requirement: apo_requirement.clone(),
        },
        Row {
            // The same member on ANOTHER operation, reached by POST
            // (`query="operation"`, `StopAdvancedPromptOptimizationJob`:
            // POST /advanced-prompt-optimization-jobs/{jobIdentifier}/stop).
            case: "StopAdvancedPromptOptimizationJob with a bare identifier of the wrong \
                   length",
            method: "POST",
            path: format!(
                "/advanced-prompt-optimization-jobs/{}/stop",
                label("abc123")
            ),
            body: None,
            member: "jobIdentifier",
            carried: "abc123".to_string(),
            requirement: apo_requirement,
        },
        Row {
            // `query="operation"`, `GetAutomatedReasoningPolicyTestCase`: `testCaseId` is a
            // `@httpLabel` over `AutomatedReasoningPolicyTestCaseId`. Twelve characters, so
            // `@length(max: 12)` holds and only the character class is broken.
            case: "GetAutomatedReasoningPolicyTestCase with a lower-case test-case id",
            method: "GET",
            path: format!(
                "/automated-reasoning-policies/{}/test-cases/{}",
                label(POLICY_ARN),
                label("aaaaaaaaaaaa")
            ),
            body: None,
            member: "testCaseId",
            carried: "aaaaaaaaaaaa".to_string(),
            requirement: test_case_requirement.clone(),
        },
        Row {
            // The same shape on another operation, reached through a DEEPER URI space
            // (`query="operation"`, `GetAutomatedReasoningPolicyTestResult`: three labels,
            // the other two well formed here so the test-case id is the only fault).
            case: "GetAutomatedReasoningPolicyTestResult with a test-case id carrying a \
                   separator",
            method: "GET",
            path: format!(
                "/automated-reasoning-policies/{}/build-workflows/{}/test-cases/{}/test-results",
                label(POLICY_ARN),
                label(BUILD_WORKFLOW_ID),
                label("ABCDEF-23456")
            ),
            body: None,
            member: "testCaseId",
            carried: "ABCDEF-23456".to_string(),
            requirement: test_case_requirement,
        },
        Row {
            // `query="operation"`, `BatchDeleteAdvancedPromptOptimizationJob`:
            // `jobIdentifiers` is a required BODY list whose ELEMENT is the catalogued
            // shape, so the clause is reported at the collection with the element shape's
            // whole constraint set and quotes nothing.
            case: "BatchDeleteAdvancedPromptOptimizationJob with one offending identifier",
            method: "POST",
            path: "/advanced-prompt-optimization-job/batch-delete".to_string(),
            body: Some(json!({
                "jobIdentifiers": [
                    "arn:aws:bedrock:us-west-2:000000000000:advanced-prompt-optimization-job/NOPE",
                ],
            })),
            member: "jobIdentifiers",
            carried: "arn:aws:bedrock:us-west-2:000000000000:advanced-prompt-optimization-job/NOPE"
                .to_string(),
            requirement: job_identifiers_requirement(),
        },
        Row {
            // The same collection with SEVERAL offending elements: the clause is one, and
            // no element of the list reaches the sentence.
            case: "BatchDeleteAdvancedPromptOptimizationJob with three offending identifiers",
            method: "POST",
            path: "/advanced-prompt-optimization-job/batch-delete".to_string(),
            body: Some(json!({
                "jobIdentifiers": ["Uppercase123", "short", "dashes-in-here"],
            })),
            member: "jobIdentifiers",
            carried: "dashes-in-here".to_string(),
            requirement: job_identifiers_requirement(),
        },
    ]
}

/// A clause about one of the shapes this service states by position alone names the member
/// and the modelled requirement — and nothing of what the request carried there.
#[tokio::test]
async fn a_catalogued_shape_states_the_members_position_and_quotes_no_value() {
    let emulator: Emulator = emulator();
    for row in catalogued_rows() {
        let case = row.case;
        let reply = emulator.call(row.method, &row.path, row.body.clone()).await;
        let stated = reply.get("message").as_str().unwrap_or_default().to_string();
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
        let expected = position_only(&row);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        // The invariant the whole-message assertion rests on, stated on its own so a
        // failure names it.
        assert!(
            !stated.contains(&row.carried) && !stated.contains("Value '"),
            "case {case:?}: the value the request carried at {:?} must not reach the \
             sentence, got {stated:?}",
            row.member
        );
    }
}

/// The contrast: a shape outside the catalogue keeps the value it refused, so the subject
/// follows the SHAPE and not the member name, the constraint class or a blanket rule — the
/// rows reach the same member NAME as a catalogued shape, another label-bound identifier
/// shape, and a query-bound member, over three constraint classes.
#[tokio::test]
async fn a_shape_outside_the_catalogue_keeps_the_value_it_refused() {
    let emulator: Emulator = emulator();
    let rows = vec![
        Row {
            // The member NAME a catalogued shape is also reached under
            // (`query="operation"`, `GetEvaluationJob`: `jobIdentifier` is a `@httpLabel`
            // over `EvaluationJobIdentifier`).
            case: "GetEvaluationJob jobIdentifier: the same member name, another shape",
            method: "GET",
            path: format!("/evaluation-jobs/{}", label("not-a-job-identifier")),
            body: None,
            member: "jobIdentifier",
            carried: "not-a-job-identifier".to_string(),
            requirement: pattern_requirement(EVALUATION_JOB_IDENTIFIER),
        },
        Row {
            // Another identifier shape in a `@httpLabel` (`query="operation"`,
            // `GetGuardrail`): declared the way the catalogued ones are, and quoted.
            case: "GetGuardrail guardrailIdentifier: another label-bound identifier shape",
            method: "GET",
            path: "/guardrails/Not_An_Id".to_string(),
            body: None,
            member: "guardrailIdentifier",
            carried: "Not_An_Id".to_string(),
            requirement: pattern_requirement(GUARDRAIL_IDENTIFIER),
        },
        Row {
            // A `@httpQuery` member of the SAME URI space as the catalogued job identifier
            // (`query="operation"`, `ListAdvancedPromptOptimizationJobs`: `maxResults` over
            // `MaxResults` `@range(min: 1, max: 1000)`).
            case: "ListAdvancedPromptOptimizationJobs maxResults below its minimum",
            method: "GET",
            path: "/advanced-prompt-optimization-jobs?maxResults=0".to_string(),
            body: None,
            member: "maxResults",
            carried: "0".to_string(),
            requirement: "Member must have value greater than or equal to 1".to_string(),
        },
        Row {
            // The same operation's `sortOrder`, `@httpQuery` over the enum `SortOrder`: a
            // third constraint class on a member that keeps its value.
            case: "ListAdvancedPromptOptimizationJobs sortOrder outside SortOrder",
            method: "GET",
            path: "/advanced-prompt-optimization-jobs?sortOrder=sideways".to_string(),
            body: None,
            member: "sortOrder",
            carried: "sideways".to_string(),
            requirement: "Member must satisfy enum value set: [Ascending, Descending]"
                .to_string(),
        },
    ];

    for row in rows {
        let case = row.case;
        let reply = emulator.call(row.method, &row.path, row.body.clone()).await;
        let stated = reply.get("message").as_str().unwrap_or_default().to_string();
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
        let expected = with_value(&row);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}
