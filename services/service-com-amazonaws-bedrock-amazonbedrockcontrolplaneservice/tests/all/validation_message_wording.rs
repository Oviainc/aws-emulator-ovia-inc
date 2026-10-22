// SPDX-License-Identifier: CC0-1.0

//! The wording of this service's `ValidationException` message, driven end-to-end over
//! the same `MultiServiceRouter` the emulator boots.
//!
//! Two rules are under test, each asserted over a SET of inputs rather than over one
//! recorded request:
//!
//! 1. **A reported `@pattern` is the expression WITHOUT the `^` / `$` anchors the model
//!    spells it with**, and only those: a `^` inside a character class (`[^:]+`) and a
//!    pattern the model does not anchor at its ends are reported verbatim. The rows span
//!    five modelled patterns across six operations and three member names.
//! 2. **A violation on an ELEMENT of a collection is reported against the collection
//!    MEMBER** — no `<index>.member` step — with that element shape's WHOLE constraint set
//!    nested inside `Member must satisfy constraint: [ … ]`. The rows span two collection
//!    members over different element shapes, one and several offending elements, an
//!    element that breaks two of its shape's constraints at once, and — as the contrast —
//!    a violation on the collection's OWN `@length`, which is not nested.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation")` / `query="shape"`), never from a recorded response
//! body. The requests go through `crate::support` — the ONE harness every suite in this
//! crate drives the emulator with — rather than a second copy of it.

use serde_json::{json, Value};

use crate::support::{self, emulator, label, Emulator};

// ---------------------------------------------------------------------------------------
// The modelled patterns these rows quote, each UNANCHORED — the form a message states.
// ---------------------------------------------------------------------------------------

/// `GuardrailIdentifier`'s `@pattern`, unanchored. Model (`query="shape"`,
/// shape `GuardrailIdentifier`):
/// `^(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))$`.
/// The `^` inside `(-[^:]+)?` is an ordinary character of a class and stays.
const GUARDRAIL_IDENTIFIER: &str =
    r"(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))";

/// `GuardrailVersion`'s `@pattern`, unanchored. Model: `^(([1-9][0-9]{0,7})|(DRAFT))$`.
const GUARDRAIL_VERSION: &str = r"(([1-9][0-9]{0,7})|(DRAFT))";

/// `GuardrailNumericalVersion`'s `@pattern`, unanchored. Model: `^[1-9][0-9]{0,7}$`.
const GUARDRAIL_NUMERICAL_VERSION: &str = r"[1-9][0-9]{0,7}";

/// `EvaluationJobIdentifier`'s `@pattern`, unanchored. Model:
/// `^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})$`.
const EVALUATION_JOB_IDENTIFIER: &str =
    r"(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})";

/// `AdvancedPromptOptimizationJobIdentifier`'s `@pattern`, unanchored. Model:
/// `^((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})$`.
const APO_JOB_IDENTIFIER: &str = r"((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})";

/// `TaggableResourcesArn`'s `@pattern`, which the model anchors INSIDE each of its
/// alternatives rather than at the ends (it opens `(^[a-zA-Z0-9]…` and closes `…)`), so
/// the reported expression is the model's verbatim: there are no end anchors to strip.
/// Model: `query="shape"`, shape `TaggableResourcesArn`.
const TAGGABLE_RESOURCE_ARN: &str = r"(^[a-zA-Z0-9][a-zA-Z0-9\-]*$)|(^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported)/[a-z0-9]{12}$)|(^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:([0-9]{12}|)((:(fine-tuning-job|model-customization-job|custom-model)/[a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}([a-z0-9-]{1,63}[.]){0,2}[a-z0-9-]{1,63}([:][a-z0-9-]{1,63}){0,2}(/[a-z0-9]{12})$)|(:guardrail/[a-z0-9]+$)|(:automated-reasoning-policy/[a-zA-Z0-9]+(:[a-zA-Z0-9]+)?$)|(:(inference-profile|application-inference-profile)/[a-zA-Z0-9-:.]+$)|(:(provisioned-model|model-invocation-job|model-evaluation-job|evaluation-job|model-import-job|imported-model|async-invoke|provisioned-model-v2|provisioned-model-reservation|prompt-router|custom-model-deployment)/[a-z0-9]{12}$)))";

/// `TagKey`'s `@pattern`, unanchored. Model (`query="shape"`, shape `TagKeyList`): the
/// list's member targets `TagKey`, `@length 1..128`, `@pattern
/// ^[a-zA-Z0-9\s._:/=+@-]*$`.
const TAG_KEY: &str = r"[a-zA-Z0-9\s._:/=+@-]*";

/// The requirement clause a `@pattern` violation states, over the expression as the
/// message quotes it (UNANCHORED).
fn pattern_requirement(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// The element constraint set of `AdvancedPromptOptimizationJobIdentifiers`' member:
/// `@length(max: 1011)` — no declared minimum, so the minimum stated is `0` — and the
/// identifier `@pattern` (`query="shape"`,
/// shape `AdvancedPromptOptimizationJobIdentifiers`).
fn apo_element_set() -> String {
    format!(
        "Member must have length less than or equal to 1011, \
         Member must have length greater than or equal to 0, \
         {}",
        pattern_requirement(APO_JOB_IDENTIFIER)
    )
}

/// The element constraint set of `EvaluationJobIdentifiers`' member: the same `@length`
/// and the evaluation-job `@pattern` (`query="shape"`, shape `EvaluationJobIdentifiers`).
fn evaluation_element_set() -> String {
    format!(
        "Member must have length less than or equal to 1011, \
         Member must have length greater than or equal to 0, \
         {}",
        pattern_requirement(EVALUATION_JOB_IDENTIFIER)
    )
}

/// The element constraint set of `TagKeyList`'s member: `@length 1..128` — a DECLARED
/// minimum, so `1` and not `0` — and `TagKey`'s `@pattern`.
fn tag_key_element_set() -> String {
    format!(
        "Member must have length less than or equal to 128, \
         Member must have length greater than or equal to 1, \
         {}",
        pattern_requirement(TAG_KEY)
    )
}

/// One row of a table that drives a request and expects one refusal clause.
///
/// A named struct rather than a tuple: the rows carry a request (method, path, body) and
/// the clause it must produce, and a seven-field tuple reads as nothing at the call site.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path the clause must be reported at.
    member: &'static str,
    /// The requirement the clause must state.
    requirement: String,
    /// The value the clause must echo, when the row pins it.
    value: Option<&'static str>,
}

/// One emulator answer: the status, the wire error code and the `message` the body states.
///
/// A projection of the shared harness's [`crate::support::Reply`], not a second harness:
/// `support::emulator()` is the ONE dispatch path every suite in this crate drives, and
/// this suite only ever reads the refusal's `message` out of the body it returns.
#[derive(Debug)]
struct Refusal {
    status: u16,
    error: Option<String>,
    message: String,
}

impl From<support::Reply> for Refusal {
    fn from(reply: support::Reply) -> Self {
        Self {
            status: reply.status,
            error: reply.error.clone(),
            message: reply
                .get("message")
                .as_str()
                .unwrap_or_default()
                .to_string(),
        }
    }
}

/// Drive one request through the shared harness and read its refusal.
async fn refuse(
    emulator: &Emulator,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Refusal {
    emulator.call(method, path, body).await.into()
}

/// Every `@pattern` this service reports is the expression without the `^` / `$` the model
/// anchors it with — over five modelled patterns, six operations and three member names.
///
/// The rejected value differs per row along what the rule generalises over: which pattern
/// the member declares, which operation and member carry it, and whether the model anchors
/// the expression at its ends at all.
#[tokio::test]
async fn a_reported_pattern_is_the_expression_without_the_models_anchors() {
    let emulator = emulator();
    let pattern_row = |case, method, path: String, body, value: &'static str, member, expression| Row {
        case,
        method,
        path,
        body,
        member,
        requirement: pattern_requirement(expression),
        value: Some(value),
    };
    let rows = vec![
        pattern_row(
            "GetGuardrail guardrailIdentifier outside GuardrailIdentifier",
            "GET",
            "/guardrails/Not_An_Id".to_string(),
            None,
            "Not_An_Id",
            "guardrailIdentifier",
            GUARDRAIL_IDENTIFIER,
        ),
        pattern_row(
            "GetGuardrail guardrailVersion outside GuardrailVersion",
            "GET",
            "/guardrails/abc123?guardrailVersion=0".to_string(),
            None,
            "0",
            "guardrailVersion",
            GUARDRAIL_VERSION,
        ),
        pattern_row(
            "DeleteGuardrail guardrailVersion outside GuardrailNumericalVersion",
            "DELETE",
            "/guardrails/abc123?guardrailVersion=DRAFT".to_string(),
            None,
            "DRAFT",
            "guardrailVersion",
            GUARDRAIL_NUMERICAL_VERSION,
        ),
        pattern_row(
            "GetEvaluationJob jobIdentifier outside EvaluationJobIdentifier",
            "GET",
            format!(
                "/evaluation-jobs/{}",
                label("arn:aws:bedrock:us-east-1:000000000000:evaluation-job/nonexistent")
            ),
            None,
            "arn:aws:bedrock:us-east-1:000000000000:evaluation-job/nonexistent",
            "jobIdentifier",
            EVALUATION_JOB_IDENTIFIER,
        ),
        pattern_row(
            "StopEvaluationJob jobIdentifier outside EvaluationJobIdentifier",
            "POST",
            format!("/evaluation-job/{}/stop", label("not-a-job")),
            None,
            "not-a-job",
            "jobIdentifier",
            EVALUATION_JOB_IDENTIFIER,
        ),
        pattern_row(
            "GetAdvancedPromptOptimizationJob jobIdentifier outside its pattern",
            "GET",
            format!(
                "/advanced-prompt-optimization-jobs/{}",
                label("arn:aws:bedrock:us-east-1:000000000000:advanced-prompt-optimization-job/nope")
            ),
            None,
            "arn:aws:bedrock:us-east-1:000000000000:advanced-prompt-optimization-job/nope",
            "jobIdentifier",
            APO_JOB_IDENTIFIER,
        ),
        pattern_row(
            "StopAdvancedPromptOptimizationJob jobIdentifier outside its pattern",
            "POST",
            format!("/advanced-prompt-optimization-jobs/{}/stop", label("UPPERCASE123")),
            None,
            "UPPERCASE123",
            "jobIdentifier",
            APO_JOB_IDENTIFIER,
        ),
        pattern_row(
            "UntagResource resourceARN outside a pattern the model does not anchor at its ends",
            "POST",
            "/untagResource".to_string(),
            Some(json!({ "resourceARN": "not a taggable arn!!", "tagKeys": ["Team"] })),
            "not a taggable arn!!",
            "resourceARN",
            TAGGABLE_RESOURCE_ARN,
        ),
    ];

    for row in rows {
        let reply = refuse(&emulator, row.method, &row.path, row.body).await;
        let case = row.case;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.message
        );
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: {}",
            row.value.unwrap_or_default(),
            row.member,
            row.requirement
        );
        assert_eq!(
            reply.message, expected,
            "case {case}: expected {expected:?}, got {:?}",
            reply.message
        );
    }
}

/// A violation on a collection ELEMENT is reported against the collection MEMBER, with the
/// element shape's whole constraint set nested — over two collection members with
/// different element shapes, and lists differing in how many elements offend.
#[tokio::test]
async fn an_element_violation_is_reported_at_its_collection_with_the_elements_constraint_set() {
    let emulator = emulator();
    let evaluation_arn =
        |suffix: &str| format!("arn:aws:bedrock:us-east-1:000000000000:evaluation-job/{suffix}");
    // (case, path, body, the value the clause states, the member, the element set)
    let rows: Vec<(&str, &str, Value, String, &str, String)> = vec![
        (
            "BatchDeleteEvaluationJob, one offending element",
            "/evaluation-jobs/batch-delete",
            json!({ "jobIdentifiers": [evaluation_arn("nonexistent")] }),
            format!("[{}]", evaluation_arn("nonexistent")),
            "jobIdentifiers",
            evaluation_element_set(),
        ),
        (
            "BatchDeleteEvaluationJob, two offending elements",
            "/evaluation-jobs/batch-delete",
            json!({ "jobIdentifiers": [evaluation_arn("first"), evaluation_arn("second")] }),
            format!(
                "[{}, {}]",
                evaluation_arn("first"),
                evaluation_arn("second")
            ),
            "jobIdentifiers",
            evaluation_element_set(),
        ),
        (
            "BatchDeleteAdvancedPromptOptimizationJob, one offending element",
            "/advanced-prompt-optimization-job/batch-delete",
            json!({ "jobIdentifiers": ["NOT-AN-ID"] }),
            "[NOT-AN-ID]".to_string(),
            "jobIdentifiers",
            apo_element_set(),
        ),
        (
            "BatchDeleteAdvancedPromptOptimizationJob, three offending elements",
            "/advanced-prompt-optimization-job/batch-delete",
            json!({ "jobIdentifiers": ["A", "BB", "CCC"] }),
            "[A, BB, CCC]".to_string(),
            "jobIdentifiers",
            apo_element_set(),
        ),
        (
            "UntagResource, an element below the element shape's minimum length",
            "/untagResource",
            json!({ "resourceARN": "abcdefghij0123456789", "tagKeys": [""] }),
            "[]".to_string(),
            "tagKeys",
            tag_key_element_set(),
        ),
        (
            "UntagResource, an element outside the element shape's pattern",
            "/untagResource",
            json!({ "resourceARN": "abcdefghij0123456789", "tagKeys": ["no*stars"] }),
            "[no*stars]".to_string(),
            "tagKeys",
            tag_key_element_set(),
        ),
    ];

    for (case, path, body, value, member, element_set) in rows {
        let reply = refuse(&emulator, "POST", path, Some(body)).await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.message
        );
        let expected = format!(
            "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
             constraint: Member must satisfy constraint: [{element_set}]"
        );
        assert_eq!(
            reply.message, expected,
            "case {case}: expected {expected:?}, got {:?}",
            reply.message
        );
        assert!(
            !reply.message.contains(".member"),
            "case {case}: the clause must name the collection member, not an indexed \
             element position, got {:?}",
            reply.message
        );
    }
}

/// One element breaking SEVERAL of its shape's constraints is ONE error: the clause states
/// what the member must satisfy, not how many of its rules a value broke.
#[tokio::test]
async fn an_element_breaking_two_constraints_states_one_clause() {
    let emulator = emulator();
    // 1012 characters: one past `@length(max: 1011)`, and outside the identifier
    // `@pattern` as well, so the walk reports both constraints on the one element.
    let oversize = "z".repeat(1012);
    let reply = refuse(
        &emulator,
        "POST",
        "/advanced-prompt-optimization-job/batch-delete",
        Some(json!({ "jobIdentifiers": [oversize] })),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (400, Some("ValidationException")),
        "case an element past its maximum length: expected 400 ValidationException, got {} {:?}",
        reply.status,
        reply.message
    );
    assert!(
        reply
            .message
            .starts_with("1 validation error detected: Value '["),
        "case an element past its maximum length: two broken constraints of one element \
         shape are one error, got {:?}",
        reply.message
    );
    let expected_tail = format!(
        "at 'jobIdentifiers' failed to satisfy constraint: Member must satisfy constraint: \
         [{}]",
        apo_element_set()
    );
    assert!(
        reply.message.ends_with(&expected_tail),
        "case an element past its maximum length: expected the clause to end {expected_tail:?}, \
         got {:?}",
        reply.message
    );
    assert_eq!(
        reply.message.matches("Member must satisfy constraint: [").count(),
        1,
        "case an element past its maximum length: the element set is stated once, got {:?}",
        reply.message
    );
}

/// A collection only PARTLY invalid still names the member and the element shape's whole
/// constraint set.
///
/// The value is where this diverges, deliberately and recorded in `Clause::render`: the
/// structured refusal the service words carries the OFFENDING elements, not the collection
/// they came from, so the clause echoes the collection formed from those — AWS echoes the
/// whole list. Inventing the elements the refusal never carried would echo bytes the caller
/// never sent; what a caller acts on (which member, and what every element of it must
/// satisfy) is exact.
#[tokio::test]
async fn a_partly_invalid_collection_names_the_member_and_its_element_constraint_set() {
    let emulator = emulator();
    // `[a-z0-9]{12}` is inside `AdvancedPromptOptimizationJobIdentifier`'s pattern, so the
    // first element is accepted and only the second offends.
    let reply = refuse(
        &emulator,
        "POST",
        "/advanced-prompt-optimization-job/batch-delete",
        Some(json!({ "jobIdentifiers": ["abc123def456", "NOT-AN-ID"] })),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (400, Some("ValidationException")),
        "case one valid and one invalid element: expected 400 ValidationException, got {} {:?}",
        reply.status,
        reply.message
    );
    let expected = format!(
        "1 validation error detected: Value '[NOT-AN-ID]' at 'jobIdentifiers' failed to \
         satisfy constraint: Member must satisfy constraint: [{}]",
        apo_element_set()
    );
    assert_eq!(
        reply.message, expected,
        "case one valid and one invalid element: expected {expected:?}, got {:?}",
        reply.message
    );
}

/// The contrast rows: a violation on a member ITSELF is NOT nested in the element-set
/// clause, whatever the member's type — the collection's own `@length`, a scalar's
/// `@length`, a `@range` and an absent `@required` member alike.
#[tokio::test]
async fn a_violation_on_the_member_itself_is_not_nested() {
    let emulator = emulator();
    let valid_guardrail = |extra: Value| {
        let mut body = json!({
            "name": "valid",
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
        });
        for (key, value) in extra.as_object().expect("an object") {
            body[key] = value.clone();
        }
        body
    };
    let rows = vec![
        Row {
            // `EvaluationJobIdentifiers` is `@length(min: 1, max: 25)`: the EMPTY list
            // breaks the collection's own minimum, not any element's constraint.
            case: "BatchDeleteEvaluationJob with an empty jobIdentifiers list",
            method: "POST",
            path: "/evaluation-jobs/batch-delete".to_string(),
            body: Some(json!({ "jobIdentifiers": [] })),
            member: "jobIdentifiers",
            requirement: "Member must have length greater than or equal to 1".to_string(),
            value: None,
        },
        Row {
            // `GuardrailName` is `@length(min: 1, max: 50)`.
            case: "CreateGuardrail with a name one past its maximum length",
            method: "POST",
            path: "/guardrails".to_string(),
            body: Some(valid_guardrail(json!({ "name": "a".repeat(51) }))),
            member: "name",
            requirement: "Member must have length less than or equal to 50".to_string(),
            value: None,
        },
        Row {
            // `ListGuardrails$maxResults` is `@range(min: 1, max: 1000)`.
            case: "ListGuardrails with maxResults below its range",
            method: "GET",
            path: "/guardrails?maxResults=0".to_string(),
            body: None,
            member: "maxResults",
            requirement: "Member must have value greater than or equal to 1".to_string(),
            value: None,
        },
        Row {
            case: "CreateGuardrail without its required name",
            method: "POST",
            path: "/guardrails".to_string(),
            body: Some(json!({
                "blockedInputMessaging": "in",
                "blockedOutputsMessaging": "out",
            })),
            member: "name",
            requirement: "Member must not be null".to_string(),
            value: None,
        },
    ];

    for row in rows {
        let reply = refuse(&emulator, row.method, &row.path, row.body).await;
        let case = row.case;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.message
        );
        let expected_tail = format!(
            "at '{}' failed to satisfy constraint: {}",
            row.member, row.requirement
        );
        assert!(
            reply.message.ends_with(&expected_tail),
            "case {case}: expected the clause to end {expected_tail:?}, got {:?}",
            reply.message
        );
        assert!(
            !reply.message.contains("Member must satisfy constraint: ["),
            "case {case}: a violation on the member itself must not be nested in the \
             element-set clause, got {:?}",
            reply.message
        );
    }
}

/// Two members of one request each state their own clause, and the envelope counts them.
#[tokio::test]
async fn two_offending_members_state_two_clauses() {
    let emulator = emulator();
    let reply = refuse(
        &emulator,
        "POST",
        "/untagResource",
        Some(json!({ "resourceARN": "short", "tagKeys": ["no*stars"] })),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (400, Some("ValidationException")),
        "case a bad resourceARN beside a bad tag key: expected 400 ValidationException, got {} {:?}",
        reply.status,
        reply.message
    );
    // `TaggableResourcesArn` is `@length(min: 20, max: 1011)`, and Smithy's `@pattern` is
    // a FIND, so `short` breaks the minimum length while still matching the ARN pattern's
    // first alternative; the tag key breaks its element shape's pattern. Two members, two
    // clauses.
    assert!(
        reply.message.starts_with("2 validation errors detected: "),
        "case a bad resourceARN beside a bad tag key: the envelope counts every distinct \
         clause, got {:?}",
        reply.message
    );
    for fragment in [
        "at 'resourceARN' failed to satisfy constraint: Member must have length greater than \
         or equal to 20",
        &format!(
            "at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [{}]",
            tag_key_element_set()
        ),
    ] {
        assert!(
            reply.message.contains(fragment),
            "case a bad resourceARN beside a bad tag key: expected the envelope to state \
             {fragment:?}, got {:?}",
            reply.message
        );
    }
}
