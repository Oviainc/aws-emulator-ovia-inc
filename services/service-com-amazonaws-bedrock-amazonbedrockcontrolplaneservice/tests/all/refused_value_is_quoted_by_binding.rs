// SPDX-License-Identifier: CC0-1.0

//! WHICH part of a refused request a `ValidationException` sentence carries, and HOW the
//! `@pattern` it advertises is written — driven end-to-end over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! Two rules, each asserted over a TABLE spanning what it generalises over rather than
//! over one request:
//!
//! 1. **A constraint clause quotes the value the member held exactly where the model binds
//!    that member to the request URI** — a `@httpLabel` path segment or a `@httpQuery`
//!    parameter, the tokens the service reads off the request line — **and states the
//!    position alone for every member it reads out of the request BODY**. The rows differ
//!    along the two things the rule generalises over: the member's BINDING (a query
//!    parameter, a path label, a top-level body member, a member nested in a body
//!    structure, an element of a body collection) and the CONSTRAINT CLASS that failed (a
//!    `@pattern`, an enum's value set, a `@range` bound, a `@length` bound), over nine
//!    operations — the subject is decided by the first and never by the second. The
//!    identifier SHAPES this service is recorded reporting by position alone whatever their
//!    binding are the rule of `tests/all/refused_subject_follows_the_member_shape.rs`, and
//!    no row below reaches one.
//! 2. **A reported `@pattern` is the expression the WHOLE value must match**: the model's
//!    expression with the `^` / `$` at its ends stripped where it anchors the match, and
//!    between the unconstrained runs a Smithy `@pattern`'s partial match admits — spelled
//!    `.*` — where the model leaves it unanchored. The rows span an unanchored expression,
//!    one anchored at both ends and one the model anchors INSIDE each of its alternatives.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",...)` / `query="shape"`), never from a recorded
//! response body; the expressions are the constants
//! `tests/all/validation_message_wording.rs` reads off the model, not a second copy of
//! them. The requests go through `crate::support` — the ONE harness every suite in this
//! crate drives the emulator with.

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, ACCOUNT, REGION};
use crate::validation_message_wording::{
    pattern_requirement, GUARDRAIL_VERSION, TAGGABLE_RESOURCE_ARN,
};

/// `ModelSourceIdentifier`'s `@pattern` as the MODEL declares it: unanchored, so Smithy
/// matches it anywhere in the value (`smithy_query(query="shape",
/// shape="ModelSourceIdentifier")`).
const MODEL_SOURCE_IDENTIFIER_DECLARED: &str =
    r"arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*";

/// The same expression as a requirement QUOTES it: what the whole value must match, which
/// for an unanchored expression is the declared one between the unconstrained runs its
/// partial match admits — and the trailing run the expression already ends with is not
/// doubled.
const MODEL_SOURCE_IDENTIFIER_QUOTED: &str =
    r".*arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*";

/// `CustomModelName`'s `@pattern`, unanchored — the shape `ListCustomModels$nameContains`
/// targets in the QUERY string (`query="operation"`, `ListCustomModels`). Model:
/// `^([0-9a-zA-Z][_-]?){1,63}$`.
const CUSTOM_MODEL_NAME: &str = r"([0-9a-zA-Z][_-]?){1,63}";

/// `EvaluationJobIdentifier`'s `@pattern`, unanchored — the shape
/// `GetEvaluationJob$jobIdentifier` targets in a URI LABEL (`query="operation"`,
/// `GetEvaluationJob`). Model:
/// `^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})$`.
const EVALUATION_JOB_IDENTIFIER: &str =
    r"(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})";

/// A well-formed `AutomatedReasoningPolicyArn` for the URI-label rows: the policy need not
/// exist, since the constraint walk refuses the request before any handler reads state.
const POLICY_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:automated-reasoning-policy/abcdef123456";

/// Where the service READ the value a clause reports, which is what decides the clause's
/// subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadFrom {
    /// A `@httpLabel` path segment or a `@httpQuery` parameter: the clause quotes the
    /// token.
    Uri,
    /// The request body, at any depth: the clause states the position alone.
    Body,
}

/// One row: a request breaking exactly one constraint, the member the clause is reported
/// at, where the service read that member, the value the request carried there and the
/// requirement the clause must state.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    member: &'static str,
    read_from: ReadFrom,
    /// The value the request carried at that member, as the message would quote it.
    carried: String,
    requirement: String,
}

/// The whole message a row must be refused with: AWS's one-violation envelope over the
/// subject the member's binding decides, its position and the requirement.
fn expected_message(row: &Row) -> String {
    let subject = match row.read_from {
        ReadFrom::Uri => format!("Value '{}'", row.carried),
        ReadFrom::Body => "Value".to_string(),
    };
    format!(
        "1 validation error detected: {subject} at '{}' failed to satisfy constraint: {}",
        row.member, row.requirement
    )
}

/// A guardrail body whose only fault is the content filter's `type`, which is reached
/// through a structure nested in a list (`query="shape"`,
/// shape `GuardrailContentFilterConfig`).
fn guardrail_with_filter_type(filter_type: &str) -> Value {
    json!({
        "name": "binding-rule",
        "blockedInputMessaging": "blocked in",
        "blockedOutputsMessaging": "blocked out",
        "contentPolicyConfig": {
            "filtersConfig": [{
                "type": filter_type,
                "inputStrength": "HIGH",
                "outputStrength": "HIGH",
            }],
        },
    })
}

/// The element constraint set `TagKeyList`'s member declares, as a collection clause nests
/// it: `@length 1..128` and `TagKey`'s `@pattern` (`query="shape"`, shape `TagKeyList`).
fn tag_key_element_set() -> String {
    format!(
        "Member must satisfy constraint: [Member must have length less than or equal to 128, \
         Member must have length greater than or equal to 1, {}]",
        pattern_requirement(r"[a-zA-Z0-9\s._:/=+@-]*")
    )
}

/// The rows: the same two dimensions crossed — five bindings against four constraint
/// classes, over nine operations.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // `query="operation"`, `ListCustomModels`: `modelStatus` is `@httpQuery` over
            // the enum `ModelStatus` (`query="shape"`, shape `ModelStatus`).
            case: "ListCustomModels modelStatus: an enum in the query string",
            method: "GET",
            path: "/custom-models?modelStatus=not-a-real-status".to_string(),
            body: None,
            member: "modelStatus",
            read_from: ReadFrom::Uri,
            carried: "not-a-real-status".to_string(),
            requirement: "Member must satisfy enum value set: [Active, Creating, Failed]"
                .to_string(),
        },
        Row {
            // The same operation's `maxResults`, `@httpQuery` over `MaxResults`
            // `@range(min: 1, max: 1000)`: another class, the same binding.
            case: "ListCustomModels maxResults: a range bound in the query string",
            method: "GET",
            path: "/custom-models?maxResults=0".to_string(),
            body: None,
            member: "maxResults",
            read_from: ReadFrom::Uri,
            carried: "0".to_string(),
            requirement: "Member must have value greater than or equal to 1".to_string(),
        },
        Row {
            // `query="operation"`, `ListCustomModels`: `nameContains` is `@httpQuery` over
            // `CustomModelName`, which declares a `@pattern`.
            case: "ListCustomModels nameContains: a pattern in the query string",
            method: "GET",
            path: "/custom-models?nameContains=not%20a%20model%20name!".to_string(),
            body: None,
            member: "nameContains",
            read_from: ReadFrom::Uri,
            carried: "not a model name!".to_string(),
            requirement: pattern_requirement(CUSTOM_MODEL_NAME),
        },
        Row {
            // `query="operation"`, `GetGuardrail`: `guardrailVersion` is `@httpQuery` over
            // `GuardrailVersion`; `abc123` is inside `guardrailIdentifier`'s own pattern,
            // so the version is the only thing the request breaks.
            case: "GetGuardrail guardrailVersion: a pattern in the query string",
            method: "GET",
            path: "/guardrails/abc123?guardrailVersion=0".to_string(),
            body: None,
            member: "guardrailVersion",
            read_from: ReadFrom::Uri,
            carried: "0".to_string(),
            requirement: pattern_requirement(GUARDRAIL_VERSION),
        },
        Row {
            // `query="operation"`, `GetEvaluationJob`: `jobIdentifier` is `@httpLabel` over
            // `EvaluationJobIdentifier` — a path segment, read off the request line like a
            // query parameter.
            case: "GetEvaluationJob jobIdentifier: a pattern in a URI label",
            method: "GET",
            path: format!("/evaluation-jobs/{}", label("not-a-job-identifier")),
            body: None,
            member: "jobIdentifier",
            read_from: ReadFrom::Uri,
            carried: "not-a-job-identifier".to_string(),
            requirement: pattern_requirement(EVALUATION_JOB_IDENTIFIER),
        },
        Row {
            // `query="operation"`, `StartAutomatedReasoningPolicyBuildWorkflow`:
            // `buildWorkflowType` is `@httpLabel` over the enum
            // `AutomatedReasoningPolicyBuildWorkflowType` — an enum in a label.
            case: "StartAutomatedReasoningPolicyBuildWorkflow buildWorkflowType: an enum in \
                   a URI label",
            method: "POST",
            path: format!(
                "/automated-reasoning-policies/{}/build-workflows/NOT_A_WORKFLOW_TYPE/start",
                label(POLICY_ARN)
            ),
            body: Some(json!({})),
            member: "buildWorkflowType",
            read_from: ReadFrom::Uri,
            carried: "NOT_A_WORKFLOW_TYPE".to_string(),
            requirement: "Member must satisfy enum value set: [INGEST_CONTENT, REFINE_POLICY, \
                          IMPORT_POLICY, GENERATE_FIDELITY_REPORT, GENERATE_POLICY_SCENARIOS, \
                          RESOLVE_POLICY_AMBIGUITIES, ITERATIVELY_REFINE_POLICY]"
                .to_string(),
        },
        Row {
            // `query="operation"`, `RegisterMarketplaceModelEndpoint`:
            // `modelSourceIdentifier` is a required BODY member over
            // `ModelSourceIdentifier`, while `endpointIdentifier` is the `@httpLabel` — the
            // label here carries a well-formed SageMaker endpoint ARN, so the body member is
            // the only thing the request breaks.
            case: "RegisterMarketplaceModelEndpoint modelSourceIdentifier: a pattern in the \
                   body",
            method: "POST",
            path: format!(
                "/marketplace-model/endpoints/{}/registration",
                label(&format!(
                    "arn:aws:sagemaker:{REGION}:{ACCOUNT}:endpoint/registered-endpoint"
                ))
            ),
            body: Some(json!({ "modelSourceIdentifier": "not-an-arn-at-all" })),
            member: "modelSourceIdentifier",
            read_from: ReadFrom::Body,
            carried: "not-an-arn-at-all".to_string(),
            requirement: pattern_requirement(MODEL_SOURCE_IDENTIFIER_QUOTED),
        },
        Row {
            // `query="operation"`, `PutAccountDataRetention`: `mode` is a BODY member over
            // the enum `DataRetentionMode`, listed by this service in the order
            // `tests/all/enum_value_set_order.rs` is the rule for.
            case: "PutAccountDataRetention mode: an enum in the body",
            method: "PUT",
            path: "/data-retention".to_string(),
            body: Some(json!({ "mode": "retain-everything" })),
            member: "mode",
            read_from: ReadFrom::Body,
            carried: "retain-everything".to_string(),
            requirement: "Member must satisfy enum value set: [default, provider_data_share, \
                          none, aws_review, inherit]"
                .to_string(),
        },
        Row {
            // `query="operation"`, `CreateProvisionedModelThroughput`: `modelUnits` is a
            // BODY member over `PositiveInteger` `@range(min: 1)`.
            case: "CreateProvisionedModelThroughput modelUnits: a range bound in the body",
            method: "POST",
            path: "/provisioned-model-throughput".to_string(),
            body: Some(json!({
                "provisionedModelName": "bindingrule",
                "modelId": "amazon.titan-text-lite-v1",
                "modelUnits": 0,
            })),
            member: "modelUnits",
            read_from: ReadFrom::Body,
            carried: "0".to_string(),
            requirement: "Member must have value greater than or equal to 1".to_string(),
        },
        Row {
            // `query="operation"`, `CreateGuardrail`: `name` is a BODY member over
            // `GuardrailName` `@length(min: 1, max: 50)`.
            case: "CreateGuardrail name: a length bound in the body",
            method: "POST",
            path: "/guardrails".to_string(),
            body: Some(json!({
                "name": "g".repeat(51),
                "blockedInputMessaging": "blocked in",
                "blockedOutputsMessaging": "blocked out",
            })),
            member: "name",
            read_from: ReadFrom::Body,
            carried: "g".repeat(51),
            requirement: "Member must have length less than or equal to 50".to_string(),
        },
        Row {
            // The same operation, with the offending member NESTED inside a structure in a
            // list: a nested member is read out of the body too.
            case: "CreateGuardrail content filter type: an enum nested in the body",
            method: "POST",
            path: "/guardrails".to_string(),
            body: Some(guardrail_with_filter_type("NOT_A_CATEGORY")),
            member: "contentPolicyConfig.filtersConfig.1.member.type",
            read_from: ReadFrom::Body,
            carried: "NOT_A_CATEGORY".to_string(),
            requirement: "Member must satisfy enum value set: [SEXUAL, VIOLENCE, HATE, \
                          INSULTS, MISCONDUCT, PROMPT_ATTACK]"
                .to_string(),
        },
        Row {
            // `query="operation"`, `UntagResource`: the offending value is an ELEMENT of the
            // body collection `tagKeys`, reported at the collection member with its element
            // shape's whole constraint set. `resourceARN` is 20 characters and inside
            // `TaggableResourcesArn`'s first alternative, so the tag key is the only fault.
            case: "UntagResource tagKeys: an element of a collection in the body",
            method: "POST",
            path: "/untagResource".to_string(),
            body: Some(json!({
                "resourceARN": "abcdefghij0123456789",
                "tagKeys": ["no*stars"],
            })),
            member: "tagKeys",
            read_from: ReadFrom::Body,
            carried: "[no*stars]".to_string(),
            requirement: tag_key_element_set(),
        },
    ]
}

/// The subject of a clause is decided by the member's HTTP binding: a value the service
/// read off the request line is quoted back, and one it read out of the body is reported by
/// its position alone — whatever constraint class the value broke.
#[tokio::test]
async fn a_clause_quotes_a_uri_bound_value_and_states_a_body_member_by_position() {
    let emulator: Emulator = emulator();
    for row in rows() {
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
        let expected = expected_message(&row);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        // The invariant each row's whole-message assertion rests on, stated on its own so a
        // failure names it: nothing the request carried in its BODY reaches the sentence.
        if row.read_from == ReadFrom::Body {
            assert!(
                !stated.contains(&row.carried) && !stated.contains("Value '"),
                "case {case:?}: a clause on a member read out of the body quotes no value, \
                 got {stated:?}"
            );
        }
    }
}

/// One row of the `@pattern` table: a request whose only fault is a member outside its
/// `@pattern`, and the expression the clause must quote for it.
struct PatternRow {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    member: &'static str,
    /// The expression the clause must quote — the one the WHOLE value must match.
    expression: &'static str,
}

/// The expression a `@pattern` clause advertises is the one the WHOLE value must match:
/// `.*` stands for the unconstrained runs an unanchored expression admits, the `^` / `$` at
/// an anchored expression's ends are not part of the grammar a caller satisfies, and an
/// expression the model anchors inside its alternatives already describes the whole value,
/// so it is quoted verbatim.
#[tokio::test]
async fn a_quoted_pattern_is_the_expression_the_whole_value_must_match() {
    let emulator: Emulator = emulator();
    let rows = vec![
        PatternRow {
            case: "an expression the model leaves UNANCHORED is quoted between the runs its \
                   partial match admits",
            method: "POST",
            path: format!(
                "/marketplace-model/endpoints/{}/registration",
                label(&format!(
                    "arn:aws:sagemaker:{REGION}:{ACCOUNT}:endpoint/whole-value-endpoint"
                ))
            ),
            body: Some(json!({ "modelSourceIdentifier": "this-is-not-an-arn" })),
            member: "modelSourceIdentifier",
            expression: MODEL_SOURCE_IDENTIFIER_QUOTED,
        },
        PatternRow {
            case: "an expression the model ANCHORS at its ends is quoted without them",
            method: "GET",
            path: "/guardrails/abc123?guardrailVersion=0".to_string(),
            body: None,
            member: "guardrailVersion",
            expression: GUARDRAIL_VERSION,
        },
        PatternRow {
            case: "an expression the model anchors INSIDE each alternative is quoted verbatim",
            method: "POST",
            path: "/untagResource".to_string(),
            body: Some(json!({ "resourceARN": "not a taggable arn!!", "tagKeys": ["Team"] })),
            member: "resourceARN",
            expression: TAGGABLE_RESOURCE_ARN,
        },
    ];

    for row in rows {
        let case = row.case;
        let reply = emulator.call(row.method, &row.path, row.body).await;
        let stated = reply.get("message").as_str().unwrap_or_default().to_string();
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
        let tail = format!(
            "at '{}' failed to satisfy constraint: {}",
            row.member,
            pattern_requirement(row.expression)
        );
        assert!(
            stated.ends_with(&tail),
            "case {case:?}: expected the clause to end {tail:?}, got {stated:?}"
        );
    }

    // The unanchored expression the first row quotes is the declared one with ONE leading
    // run added: the trailing `.*` the model already spells is not doubled.
    assert_eq!(
        MODEL_SOURCE_IDENTIFIER_QUOTED,
        format!(".*{MODEL_SOURCE_IDENTIFIER_DECLARED}"),
        "case \"the quoted expression against the declared one\": the quoted expression is \
         the model's with the runs its partial match admits spelled out, got \
         {MODEL_SOURCE_IDENTIFIER_QUOTED:?}"
    );
}
