// SPDX-License-Identifier: CC0-1.0

//! WHERE the order of an enum-constraint clause's bracketed set comes from, driven
//! end-to-end through the same `MultiServiceRouter` the emulator boots.
//!
//! One rule: **the bracketed set of an enum clause states the MEMBERSHIP the model declares
//! in the order this service lists that enum's values, which is a property of the ENUM** —
//! so it is the same wherever that enum is reached and whatever else the request got wrong.
//! The service's own listing is used for an enum it is observed to publish one for, and the
//! model's declaration order for every enum it does not.
//!
//! `tests/all/enum_value_set_order.rs` pins the clause per member; these rows pin what makes
//! the order a fact about the enum rather than about one surface, which is what says
//! something about the NEXT enum:
//!
//! * the SAME enum reached through several operations states one ordered set (`SortOrder`
//!   through four list operations, `CreationTime` through two single-valued sort enums);
//! * the set does not vary with where the value travels — a body member, a query-string
//!   parameter, a URI LABEL, or a member of a structure nested in a list;
//! * nor with the enum's size (one, two, three, four, six, seven and ten values), nor with
//!   the value the request offends with;
//! * a refusal reporting SEVERAL enum members states each one's own enum's listing, so one
//!   clause's order never leaks into another's;
//! * an enum reached as a list ELEMENT states the same ordered set inside the nested
//!   element-constraint set.
//!
//! Every expected set is cited from the model (`smithy_query(query="shape",shape=...)`), and
//! each row also asserts the invariant that holds whatever the order is: the values stated
//! are exactly the model's, none added and none dropped.

use std::collections::BTreeSet;

use serde_json::{json, Value};

// The readers a refusal is read through, single-homed in the suite that introduced them
// (`tests/all/enum_value_set_order.rs`) rather than copied here.
use crate::enum_value_set_order::{bracketed, expected_message, message};
use crate::support::{emulator, label, Emulator, Reply};

// ---------------------------------------------------------------------------------------
// The value sets, from the model
// ---------------------------------------------------------------------------------------

/// `DataRetentionMode` (`smithy_query(query="shape",shape="DataRetentionMode")` declares
/// `default, none, aws_review, provider_data_share, inherit`) — the one enum of this service
/// whose LISTING is published, which is the order below and no order the model carries.
const DATA_RETENTION_MODE: &[&str] = &[
    "default",
    "provider_data_share",
    "none",
    "aws_review",
    "inherit",
];

/// `SortOrder` (`query="shape"`, shape `SortOrder`), two values, with no published listing:
/// the model's declaration order.
const SORT_ORDER: &[&str] = &["Ascending", "Descending"];

/// `SortJobsBy` and `SortModelsBy` (`query="shape"`, shapes `SortJobsBy`, `SortModelsBy`):
/// two distinct enums the model declares over the same single value.
const CREATION_TIME: &[&str] = &["CreationTime"];

/// `EvaluationJobStatus` (`query="shape"`, shape `EvaluationJobStatus`), six values.
const EVALUATION_JOB_STATUS: &[&str] = &[
    "InProgress",
    "Completed",
    "Failed",
    "Stopping",
    "Stopped",
    "Deleting",
];

/// `ModelInvocationJobStatus` (`query="shape"`, shape `ModelInvocationJobStatus`), ten
/// values — the longest set an input member of this service carries.
const MODEL_INVOCATION_JOB_STATUS: &[&str] = &[
    "Submitted",
    "InProgress",
    "Completed",
    "Failed",
    "Stopping",
    "Stopped",
    "PartiallyCompleted",
    "Expired",
    "Validating",
    "Scheduled",
];

/// `ModelCopyJobStatus` (`query="shape"`, shape `ModelCopyJobStatus`), three values.
const MODEL_COPY_JOB_STATUS: &[&str] = &["InProgress", "Completed", "Failed"];

/// `CustomModelDeploymentStatus` (`query="shape"`, shape `CustomModelDeploymentStatus`):
/// three values that READ like another enum's and are declared in another order
/// (`ModelStatus` below), so a listing resolved by membership must not confuse the two.
const CUSTOM_MODEL_DEPLOYMENT_STATUS: &[&str] = &["Creating", "Active", "Failed"];

/// `ModelStatus` (`query="shape"`, shape `ModelStatus`), the same three values as
/// `CustomModelDeploymentStatus` in the model's own other order.
const MODEL_STATUS: &[&str] = &["Active", "Creating", "Failed"];

/// `ApplicationType` (`query="shape"`, shape `ApplicationType`).
const APPLICATION_TYPE: &[&str] = &["ModelEvaluation", "RagEvaluation"];

/// `ModelCustomization` (`query="shape"`, shape `ModelCustomization`).
const MODEL_CUSTOMIZATION: &[&str] = &["FINE_TUNING", "CONTINUED_PRE_TRAINING", "DISTILLATION"];

/// `AutomatedReasoningPolicyBuildWorkflowType` (`query="shape"`, shape
/// `AutomatedReasoningPolicyBuildWorkflowType`), seven values, reached through a URI LABEL.
const BUILD_WORKFLOW_TYPE: &[&str] = &[
    "INGEST_CONTENT",
    "REFINE_POLICY",
    "IMPORT_POLICY",
    "GENERATE_FIDELITY_REPORT",
    "GENERATE_POLICY_SCENARIOS",
    "RESOLVE_POLICY_AMBIGUITIES",
    "ITERATIVELY_REFINE_POLICY",
];

/// `GuardrailFilterStrength` (`query="shape"`, shape `GuardrailFilterStrength`), four
/// values, reached through a structure nested in a list.
const GUARDRAIL_FILTER_STRENGTH: &[&str] = &["NONE", "LOW", "MEDIUM", "HIGH"];

/// `GuardrailModality` (`query="shape"`, shape `GuardrailModality`), the ELEMENT shape of
/// `GuardrailContentFilterConfig.inputModalities` (`query="shape"`, shape
/// `GuardrailContentFilterConfig`).
const GUARDRAIL_MODALITY: &[&str] = &["TEXT", "IMAGE"];

/// A well-formed `AutomatedReasoningPolicyArn` for the URI-label row: the policy need not
/// exist, since the constraint walk refuses the request before any handler reads state.
const POLICY_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:automated-reasoning-policy/abcdef123456";

// ---------------------------------------------------------------------------------------
// Reading a refusal
// ---------------------------------------------------------------------------------------

/// The `message` a refusal carries, asserting it IS a refusal first.
fn refusal_message(case: &str, reply: &Reply) -> String {
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected 400 ValidationException, got {} {:?}",
        reply.status,
        reply.body
    );
    message(case, reply)
}

/// The clause a refusal states for `member`, however many clauses it joined.
fn clause_for(quoted: Option<&str>, member: &str, listed: &[&str]) -> String {
    let subject = match quoted {
        Some(value) => format!("Value '{value}'"),
        None => "Value".to_string(),
    };
    format!(
        "{subject} at '{member}' failed to satisfy constraint: Member must satisfy enum value \
         set: [{}]",
        listed.join(", ")
    )
}

/// That the set a clause stated holds exactly the values the model declares.
fn assert_membership(case: &str, message: &str, declared: &[&str]) {
    let stated: BTreeSet<String> = bracketed(case, message).into_iter().collect();
    let declared: BTreeSet<String> = declared.iter().map(|value| (*value).to_string()).collect();
    assert_eq!(
        stated, declared,
        "case {case:?}: the set stated must hold exactly the values the model declares, got \
         {message:?}"
    );
}

/// One row: a request whose single offending member is outside its enum.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path the clause is reported at.
    member: &'static str,
    /// The value the request carries, when the clause quotes it: `Some` for a member the
    /// model binds to the request URI, `None` for one the service reads out of the body.
    quoted: Option<&'static str>,
    /// The admitted values in the order the clause must list them.
    listed: &'static [&'static str],
}

/// A guardrail body whose only fault is the content filter's `inputStrength`: the three
/// members `CreateGuardrail` requires are present and valid, and so are the filter's other
/// required members (`smithy_query(query="shape",shape="GuardrailContentFilterConfig")`).
fn guardrail_with_input_strength(strength: &str) -> Value {
    json!({
        "name": "enum-listing",
        "blockedInputMessaging": "blocked in",
        "blockedOutputsMessaging": "blocked out",
        "contentPolicyConfig": {
            "filtersConfig": [{
                "type": "HATE",
                "inputStrength": strength,
                "outputStrength": "HIGH",
            }],
        },
    })
}

/// A guardrail body whose only fault is one ELEMENT of the filter's `inputModalities`, a
/// list of `GuardrailModality`.
fn guardrail_with_input_modalities(modalities: Value) -> Value {
    json!({
        "name": "enum-listing",
        "blockedInputMessaging": "blocked in",
        "blockedOutputsMessaging": "blocked out",
        "contentPolicyConfig": {
            "filtersConfig": [{
                "type": "HATE",
                "inputStrength": "HIGH",
                "outputStrength": "HIGH",
                "inputModalities": modalities,
            }],
        },
    })
}

// ---------------------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------------------

/// The order a clause lists an enum's values in is the order this service lists that ENUM's
/// values: one ordered set per enum, whichever operation, member and wire position reaches
/// it, and whatever value the request offends with.
#[tokio::test]
async fn an_enum_clause_lists_one_ordered_set_per_enum_wherever_it_is_reached() {
    let emulator: Emulator = emulator();
    let rows = vec![
        Row {
            case: "PutAccountDataRetention mode: the one published listing, in the body",
            method: "PUT",
            path: "/data-retention".to_string(),
            body: Some(json!({ "mode": "keep-everything" })),
            member: "mode",
            quoted: None,
            listed: DATA_RETENTION_MODE,
        },
        Row {
            case: "ListEvaluationJobs sortOrder: SortOrder in the query string",
            method: "GET",
            path: "/evaluation-jobs?sortOrder=sideways".to_string(),
            body: None,
            member: "sortOrder",
            quoted: Some("sideways"),
            listed: SORT_ORDER,
        },
        Row {
            case: "ListModelCopyJobs sortOrder: the same enum, another operation",
            method: "GET",
            path: "/model-copy-jobs?sortOrder=Asc".to_string(),
            body: None,
            member: "sortOrder",
            quoted: Some("Asc"),
            listed: SORT_ORDER,
        },
        Row {
            case: "ListCustomModels sortOrder: the same enum, a value differing only in case",
            method: "GET",
            path: "/custom-models?sortOrder=ascending".to_string(),
            body: None,
            member: "sortOrder",
            quoted: Some("ascending"),
            listed: SORT_ORDER,
        },
        Row {
            case: "ListAdvancedPromptOptimizationJobs sortOrder: the same enum again",
            method: "GET",
            path: "/advanced-prompt-optimization-jobs?sortOrder=DESC".to_string(),
            body: None,
            member: "sortOrder",
            quoted: Some("DESC"),
            listed: SORT_ORDER,
        },
        Row {
            case: "ListEvaluationJobs sortBy: SortJobsBy, a single-valued enum",
            method: "GET",
            path: "/evaluation-jobs?sortBy=Name".to_string(),
            body: None,
            member: "sortBy",
            quoted: Some("Name"),
            listed: CREATION_TIME,
        },
        Row {
            case: "ListImportedModels sortBy: SortModelsBy, another enum over that value",
            method: "GET",
            path: "/imported-models?sortBy=Name".to_string(),
            body: None,
            member: "sortBy",
            quoted: Some("Name"),
            listed: CREATION_TIME,
        },
        Row {
            case: "ListEvaluationJobs statusEquals: six values",
            method: "GET",
            path: "/evaluation-jobs?statusEquals=Pending".to_string(),
            body: None,
            member: "statusEquals",
            quoted: Some("Pending"),
            listed: EVALUATION_JOB_STATUS,
        },
        Row {
            case: "ListModelInvocationJobs statusEquals: ten values",
            method: "GET",
            path: "/model-invocation-jobs?statusEquals=Pending".to_string(),
            body: None,
            member: "statusEquals",
            quoted: Some("Pending"),
            listed: MODEL_INVOCATION_JOB_STATUS,
        },
        Row {
            case: "ListModelCopyJobs statusEquals: three values",
            method: "GET",
            path: "/model-copy-jobs?statusEquals=Pending".to_string(),
            body: None,
            member: "statusEquals",
            quoted: Some("Pending"),
            listed: MODEL_COPY_JOB_STATUS,
        },
        Row {
            case: "ListCustomModelDeployments statusEquals: three values another enum of \
                   this service also declares",
            method: "GET",
            path: "/model-customization/custom-model-deployments?statusEquals=Deleting"
                .to_string(),
            body: None,
            member: "statusEquals",
            quoted: Some("Deleting"),
            listed: CUSTOM_MODEL_DEPLOYMENT_STATUS,
        },
        Row {
            case: "ListCustomModels modelStatus: that same membership, the other declaration \
                   order",
            method: "GET",
            path: "/custom-models?modelStatus=Deleting".to_string(),
            body: None,
            member: "modelStatus",
            quoted: Some("Deleting"),
            listed: MODEL_STATUS,
        },
        Row {
            case: "ListEvaluationJobs applicationTypeEquals: two values",
            method: "GET",
            path: "/evaluation-jobs?applicationTypeEquals=Model".to_string(),
            body: None,
            member: "applicationTypeEquals",
            quoted: Some("Model"),
            listed: APPLICATION_TYPE,
        },
        Row {
            case: "ListFoundationModels byCustomizationType: three values",
            method: "GET",
            path: "/foundation-models?byCustomizationType=PRE_TRAINING".to_string(),
            body: None,
            member: "byCustomizationType",
            quoted: Some("PRE_TRAINING"),
            listed: MODEL_CUSTOMIZATION,
        },
        Row {
            case: "StartAutomatedReasoningPolicyBuildWorkflow buildWorkflowType: seven \
                   values, bound to a URI label",
            method: "POST",
            path: format!(
                "/automated-reasoning-policies/{}/build-workflows/NOT_A_WORKFLOW_TYPE/start",
                label(POLICY_ARN)
            ),
            body: Some(json!({})),
            member: "buildWorkflowType",
            quoted: Some("NOT_A_WORKFLOW_TYPE"),
            listed: BUILD_WORKFLOW_TYPE,
        },
        Row {
            case: "CreateGuardrail filter inputStrength: four values, inside a structure \
                   nested in a list",
            method: "POST",
            path: "/guardrails".to_string(),
            body: Some(guardrail_with_input_strength("VERY_HIGH")),
            member: "contentPolicyConfig.filtersConfig.1.member.inputStrength",
            quoted: None,
            listed: GUARDRAIL_FILTER_STRENGTH,
        },
    ];

    for row in rows {
        let case = row.case;
        let reply = emulator.call(row.method, &row.path, row.body.clone()).await;
        let stated = refusal_message(case, &reply);
        let expected = expected_message(row.quoted, row.member, row.listed);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        assert_membership(case, &stated, row.listed);
    }
}

/// A refusal reporting SEVERAL enum members states each member's own enum's listing: the
/// order one clause lists is a fact about its enum, so it does not spread to the next
/// clause.
#[tokio::test]
async fn every_enum_clause_of_one_refusal_states_its_own_enums_listing() {
    let emulator: Emulator = emulator();
    // Three enum-constrained members of ONE operation, each outside its own set
    // (`smithy_query(query="operation",operation="ListEvaluationJobs")`).
    let case = "ListEvaluationJobs with statusEquals, applicationTypeEquals and sortOrder all \
                outside their sets";
    let reply = emulator
        .call(
            "GET",
            "/evaluation-jobs?statusEquals=Pending&applicationTypeEquals=Model&sortOrder=sideways",
            None,
        )
        .await;
    let stated = refusal_message(case, &reply);
    assert!(
        stated.starts_with("3 validation errors detected: "),
        "case {case:?}: expected the envelope over three clauses, got {stated:?}"
    );
    for (member, quoted, listed) in [
        ("statusEquals", "Pending", EVALUATION_JOB_STATUS),
        ("applicationTypeEquals", "Model", APPLICATION_TYPE),
        ("sortOrder", "sideways", SORT_ORDER),
    ] {
        let clause = clause_for(Some(quoted), member, listed);
        assert!(
            stated.contains(&clause),
            "case {case:?}: expected the clause {clause:?}, got {stated:?}"
        );
    }
}

/// An enum reached as a list ELEMENT states the same ordered set, inside the element
/// constraint set the clause nests — the set is the element shape's, so the listing rule
/// holds there too.
#[tokio::test]
async fn an_enum_reached_as_a_list_element_states_the_same_ordered_set() {
    let emulator: Emulator = emulator();
    // (case, the list the request carries)
    let rows: Vec<(&str, Value)> = vec![
        ("one element outside GuardrailModality", json!(["AUDIO"])),
        (
            "an offending element beside an admitted one",
            json!(["TEXT", "AUDIO"]),
        ),
        ("both elements offending", json!(["AUDIO", "VIDEO"])),
    ];
    for (case, modalities) in rows {
        let reply = emulator
            .call(
                "POST",
                "/guardrails",
                Some(guardrail_with_input_modalities(modalities)),
            )
            .await;
        let stated = refusal_message(case, &reply);
        let member = "contentPolicyConfig.filtersConfig.1.member.inputModalities";
        assert!(
            stated.contains(&format!("at '{member}' failed to satisfy constraint:")),
            "case {case:?}: expected the clause reported at {member:?}, got {stated:?}"
        );
        let listed = bracketed(case, &stated);
        let expected: Vec<String> = GUARDRAIL_MODALITY
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the element's set {expected:?}, got {stated:?}"
        );
        assert_membership(case, &stated, GUARDRAIL_MODALITY);
    }
}
