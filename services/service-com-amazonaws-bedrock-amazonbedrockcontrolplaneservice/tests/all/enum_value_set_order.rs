// SPDX-License-Identifier: CC0-1.0

//! The ORDER an enum-constraint refusal lists an enum's admitted values in, driven
//! end-to-end through the same `MultiServiceRouter` the emulator boots.
//!
//! One rule, asserted over a SET of enum-constrained members rather than over the one a
//! report cited: **the bracketed set of an enum clause states the MEMBERSHIP the model
//! declares, in the order this service lists that enum's values** — its own listing where
//! this service publishes one, and the model's declaration order for every enum where it
//! does not. The rows differ along what the rule generalises over: whether the enum has a
//! published listing, which operation and member carry it, whether the value travels in the
//! BODY, in the QUERY string or inside a structure nested in a list, how many values the
//! enum publishes (one, two, three, four, five and six) and which value the request
//! offends with — the set a clause states is a fact about the enum, so it cannot vary with
//! the request.
//!
//! Every row's expected set is cited from the model
//! (`smithy_query(query="shape",shape=...)`), and each row also asserts the invariant that
//! holds whatever the order is: the values stated are exactly the model's, no value added
//! and none dropped.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::support::{emulator, Emulator};

// ---------------------------------------------------------------------------------------
// The value sets this service's enums declare, from the model
// ---------------------------------------------------------------------------------------

/// `DataRetentionMode`, declared `default, none, aws_review, provider_data_share, inherit`
/// (`smithy_query(query="shape",shape="DataRetentionMode")`) and LISTED by this service in
/// the order below — the one its refusals state, which no order of the model reproduces.
const DATA_RETENTION_MODE: &[&str] = &[
    "default",
    "provider_data_share",
    "none",
    "aws_review",
    "inherit",
];

/// The same five values as the model declares them, which is what a clause's membership
/// must match however it is ordered.
const DATA_RETENTION_MODE_DECLARED: &[&str] = &[
    "default",
    "none",
    "aws_review",
    "provider_data_share",
    "inherit",
];

/// `ModelModality` (`smithy_query(query="shape",shape="ModelModality")`), an enum with no
/// published listing: the clause states the model's declaration order.
const MODEL_MODALITY: &[&str] = &["TEXT", "IMAGE", "EMBEDDING"];

/// `InferenceType` (`query="shape"`, shape `InferenceType`).
const INFERENCE_TYPE: &[&str] = &["ON_DEMAND", "PROVISIONED"];

/// `ModelCustomization` (`query="shape"`, shape `ModelCustomization`).
const MODEL_CUSTOMIZATION: &[&str] = &["FINE_TUNING", "CONTINUED_PRE_TRAINING", "DISTILLATION"];

/// `SortOrder` (`query="shape"`, shape `SortOrder`).
const SORT_ORDER: &[&str] = &["Ascending", "Descending"];

/// `SortByProvisionedModels` (`query="shape"`, shape `SortByProvisionedModels`): one value.
const SORT_BY_PROVISIONED_MODELS: &[&str] = &["CreationTime"];

/// `ProvisionedModelStatus` (`query="shape"`, shape `ProvisionedModelStatus`).
const PROVISIONED_MODEL_STATUS: &[&str] = &["Creating", "InService", "Updating", "Failed"];

/// `GuardrailContentFilterType` (`query="shape"`, shape `GuardrailContentFilterType`),
/// reached through a structure nested in a list rather than at the top level of the input.
const GUARDRAIL_CONTENT_FILTER_TYPE: &[&str] = &[
    "SEXUAL",
    "VIOLENCE",
    "HATE",
    "INSULTS",
    "MISCONDUCT",
    "PROMPT_ATTACK",
];

// ---------------------------------------------------------------------------------------
// The clause a refusal states
// ---------------------------------------------------------------------------------------

/// The whole message a request with ONE enum-constrained member outside its set is refused
/// with: the AWS envelope over the admitted set, with the value the member held quoted only
/// where the model binds that member to the request URI (`src/service/validation.rs`,
/// rule 2) and the position stated alone for one carried in the body.
///
/// `pub(crate)` because `tests/all/enum_value_set_listing.rs` — the suite for the ORDER the
/// set is listed in — reads a refusal through these same three readers rather than through a
/// second copy of them.
pub(crate) fn expected_message(quoted: Option<&str>, member: &str, values: &[&str]) -> String {
    let subject = match quoted {
        Some(value) => format!("Value '{value}'"),
        None => "Value".to_string(),
    };
    format!(
        "1 validation error detected: {subject} at '{member}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        values.join(", ")
    )
}

/// The `message` a refused answer carries.
pub(crate) fn message(case: &str, reply: &crate::support::Reply) -> String {
    reply
        .get("message")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {:?}", reply.body))
        .to_string()
}

/// The values a clause brackets, read back out of the message it stated.
///
/// Up to the bracket that CLOSES the set rather than off the end of the message, so a set
/// stated inside a collection's nested element-constraint set (`… constraint: [… enum value
/// set: [ … ]]`) is read the same way as one stated on its own. Every row that states the
/// set at the end of the message asserts the whole message besides.
pub(crate) fn bracketed(case: &str, message: &str) -> Vec<String> {
    let opened = message
        .split_once("enum value set: [")
        .unwrap_or_else(|| panic!("case {case:?}: the message states no enum value set: {message:?}"))
        .1;
    let set = opened
        .split_once(']')
        .unwrap_or_else(|| panic!("case {case:?}: the value set is not closed: {message:?}"))
        .0;
    set.split(", ").map(str::to_string).collect()
}

/// One row: a request whose single offending member is outside its enum, and the ordered
/// set the clause must state.
struct Row {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
    /// The member path the clause is reported at.
    member: &'static str,
    /// The value the request carries, when the clause quotes it: `Some` for a member the
    /// model binds to the request URI, `None` for one the service reads out of the body.
    quoted: Option<&'static str>,
    /// The admitted values in the order the clause must list them.
    listed: &'static [&'static str],
    /// The same values as the MODEL declares them, which the clause's membership must
    /// match whatever order it states them in.
    declared: &'static [&'static str],
}

/// A guardrail body whose only fault is the content filter's `type`: the three members
/// `CreateGuardrail` requires are present and valid, and so are the filter's other required
/// members (`smithy_query(query="operation",operation="CreateGuardrail")`,
/// `query="shape",shape="GuardrailContentFilterConfig"`).
fn guardrail_with_filter_type(filter_type: &str) -> Value {
    json!({
        "name": "enum-order",
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

/// An enum clause lists the admitted values in the order this service lists them: its
/// published listing for the one enum that has one, and the model's declaration order for
/// every other enum.
#[tokio::test]
async fn an_enum_clause_lists_the_admitted_values_in_the_order_the_service_lists_them() {
    let emulator: Emulator = emulator();
    let rows = vec![
        Row {
            case: "PutAccountDataRetention mode, a published listing, in the body",
            method: "PUT",
            path: "/data-retention",
            body: Some(json!({ "mode": "thirty-seven-years" })),
            member: "mode",
            quoted: None,
            listed: DATA_RETENTION_MODE,
            declared: DATA_RETENTION_MODE_DECLARED,
        },
        Row {
            case: "PutAccountDataRetention mode, another offending value",
            method: "PUT",
            path: "/data-retention",
            body: Some(json!({ "mode": "DEFAULT" })),
            member: "mode",
            quoted: None,
            listed: DATA_RETENTION_MODE,
            declared: DATA_RETENTION_MODE_DECLARED,
        },
        Row {
            case: "PutAccountDataRetention mode, an offending value that is itself a set \
                   member of another enum",
            method: "PUT",
            path: "/data-retention",
            body: Some(json!({ "mode": "TEXT" })),
            member: "mode",
            quoted: None,
            listed: DATA_RETENTION_MODE,
            declared: DATA_RETENTION_MODE_DECLARED,
        },
        Row {
            case: "ListFoundationModels byOutputModality, three values, in the query string",
            method: "GET",
            path: "/foundation-models?byOutputModality=AUDIO",
            body: None,
            member: "byOutputModality",
            quoted: Some("AUDIO"),
            listed: MODEL_MODALITY,
            declared: MODEL_MODALITY,
        },
        Row {
            case: "ListFoundationModels byInferenceType, two values",
            method: "GET",
            path: "/foundation-models?byInferenceType=ONDEMAND",
            body: None,
            member: "byInferenceType",
            quoted: Some("ONDEMAND"),
            listed: INFERENCE_TYPE,
            declared: INFERENCE_TYPE,
        },
        Row {
            case: "ListFoundationModels byCustomizationType, three values",
            method: "GET",
            path: "/foundation-models?byCustomizationType=PRE_TRAINING",
            body: None,
            member: "byCustomizationType",
            quoted: Some("PRE_TRAINING"),
            listed: MODEL_CUSTOMIZATION,
            declared: MODEL_CUSTOMIZATION,
        },
        Row {
            case: "ListProvisionedModelThroughputs sortOrder, two values, another operation",
            method: "GET",
            path: "/provisioned-model-throughputs?sortOrder=sideways",
            body: None,
            member: "sortOrder",
            quoted: Some("sideways"),
            listed: SORT_ORDER,
            declared: SORT_ORDER,
        },
        Row {
            case: "ListProvisionedModelThroughputs sortBy, a single-valued enum",
            method: "GET",
            path: "/provisioned-model-throughputs?sortBy=Name",
            body: None,
            member: "sortBy",
            quoted: Some("Name"),
            listed: SORT_BY_PROVISIONED_MODELS,
            declared: SORT_BY_PROVISIONED_MODELS,
        },
        Row {
            case: "ListProvisionedModelThroughputs statusEquals, four values",
            method: "GET",
            path: "/provisioned-model-throughputs?statusEquals=Deleting",
            body: None,
            member: "statusEquals",
            quoted: Some("Deleting"),
            listed: PROVISIONED_MODEL_STATUS,
            declared: PROVISIONED_MODEL_STATUS,
        },
        Row {
            case: "CreateGuardrail filter type, six values, inside a structure nested in a \
                   list",
            method: "POST",
            path: "/guardrails",
            body: Some(guardrail_with_filter_type("NOT_A_CATEGORY")),
            member: "contentPolicyConfig.filtersConfig.1.member.type",
            quoted: None,
            listed: GUARDRAIL_CONTENT_FILTER_TYPE,
            declared: GUARDRAIL_CONTENT_FILTER_TYPE,
        },
    ];

    for row in rows {
        let case = row.case;
        let reply = emulator.call(row.method, row.path, row.body.clone()).await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
        let stated = message(case, &reply);
        let expected = expected_message(row.quoted, row.member, row.listed);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        // Whatever the order, the membership is the model's: the clause neither adds a
        // value the model does not declare nor drops one it does.
        let bracketed: BTreeSet<String> = bracketed(case, &stated).into_iter().collect();
        let declared: BTreeSet<String> = row.declared.iter().map(|v| (*v).to_string()).collect();
        assert_eq!(
            bracketed, declared,
            "case {case:?}: the set stated must hold exactly the values the model declares, \
             got {stated:?}"
        );
    }
}
