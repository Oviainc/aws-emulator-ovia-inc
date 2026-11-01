// SPDX-License-Identifier: CC0-1.0

//! What a boundary refusal of a value outside a modelled enum SAYS, and in particular
//! the ORDER it lists that enum's values in.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the sibling `schema_mapping_workflow_lifecycle` suite owns — so what is
//! asserted is the body an AWS SDK client would parse.
//!
//! The rule under test: a value outside a modelled enum is refused with HTTP 400
//! `ValidationException` whose message is the one validation envelope — `1 validation
//! error detected: Value '<the value the request carried>' at '<the member path>'
//! failed to satisfy constraint: Member must satisfy enum value set: [<the values the
//! enum admits>]` — and the ORDER that value set is listed in is a property of the
//! ENUM, not of the operation or of the member that carried the value: the service
//! states the `SOURCE`/`TARGET` set with `TARGET` first, and every other enum in the
//! order the value set reaches the refusal in. Only the rejected value and the member
//! path come from the request.
//!
//! Each table row differs along what the rule generalises over rather than repeating
//! one recorded request: the enum (four of them, of two, three and two values), the
//! operation the member belongs to, the depth of the member (a top-level member, one
//! inside a structure, one inside a structure inside a LIST), and the rejected value
//! over the forms a wrong value takes — an invented name, the right value in the wrong
//! case, and the name of a value another enum of this service admits. The recorded
//! request (`CreateIdNamespace` with `type` outside `IdNamespaceType`) is one row.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::schema_mapping_workflow_lifecycle::{
    assert_error_shape, glue_table_arn, role_arn, string_member, Emulator, Reply,
};

/// The order this service states the `SOURCE`/`TARGET` value set in. RECORDED from AWS
/// on a `CreateIdNamespace` carrying a `type` outside `IdNamespaceType`, whose value
/// set the model gives as `SOURCE` and `TARGET`
/// (`smithy_query(query="shape",shape="IdNamespaceType")`).
const SOURCE_TARGET_SET: &[&str] = &["TARGET", "SOURCE"];

/// The value sets this service states in the order they reach the refusal, which is the
/// order the MODEL declares that enum's members in, each taken from the model:
/// `ResolutionType` (`smithy_query(query="shape",shape="ResolutionType")` →
/// `RULE_MATCHING, ML_MATCHING, PROVIDER`), `AttributeMatchingModel` (`ONE_TO_ONE,
/// MANY_TO_MANY`) and `IdMappingType` (`PROVIDER, RULE_BASED`).
const RESOLUTION_TYPE_SET: &[&str] = &["RULE_MATCHING", "ML_MATCHING", "PROVIDER"];
const ATTRIBUTE_MATCHING_MODEL_SET: &[&str] = &["ONE_TO_ONE", "MANY_TO_MANY"];
const ID_MAPPING_TYPE_SET: &[&str] = &["PROVIDER", "RULE_BASED"];

/// The schema mapping the workflow rows reference.
const SCHEMA_NAME: &str = "people-enum";

/// One refused request: the operation to send it to, the member whose value is outside
/// its enum, and the set the refusal must state.
struct EnumCase {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Value,
    /// The member path the envelope names, as the validation envelope spells one.
    member_path: &'static str,
    /// The value the request carried, which the envelope echoes.
    value: &'static str,
    /// The admitted values, in the order this service states them.
    stated_set: &'static [&'static str],
}

/// A `CreateIdNamespace` body whose `type` is `value`.
fn id_namespace(name: &str, value: &str) -> Value {
    json!({ "idNamespaceName": name, "type": value })
}

/// A `CreateIdMappingWorkflow` body whose ONE input source's `type` is `value` — the
/// same `IdNamespaceType` enum as `CreateIdNamespace`'s top-level `type`, reached
/// through a structure inside a list, in another operation.
fn id_mapping_workflow(name: &str, source_type: &str) -> Value {
    json!({
        "workflowName": name,
        "inputSourceConfig": [{
            "inputSourceARN": glue_table_arn(name),
            "schemaName": SCHEMA_NAME,
            "type": source_type,
        }],
        "idMappingTechniques": {
            "idMappingType": "RULE_BASED",
            "ruleBasedProperties": {
                "ruleDefinitionType": "SOURCE",
                "attributeMatchingModel": "ONE_TO_ONE",
                "recordMatchingModel": "ONE_SOURCE_TO_ONE_TARGET",
                "rules": [{ "ruleName": "by email", "matchingKeys": ["email"] }],
            },
        },
        "roleArn": role_arn(),
    })
}

/// A `CreateIdMappingWorkflow` body whose `idMappingTechniques.idMappingType` is
/// `value`: a member inside a structure, of a different enum.
fn id_mapping_workflow_with_type(name: &str, mapping_type: &str) -> Value {
    json!({
        "workflowName": name,
        "inputSourceConfig": [{
            "inputSourceARN": glue_table_arn(name),
            "schemaName": SCHEMA_NAME,
            "type": "SOURCE",
        }],
        "idMappingTechniques": { "idMappingType": mapping_type },
        "roleArn": role_arn(),
    })
}

/// A `CreateMatchingWorkflow` body whose `resolutionTechniques` carries
/// `resolution_type`, and whose `ruleBasedProperties` carries `matching_model` when one
/// is given.
fn matching_workflow(name: &str, resolution_type: &str, matching_model: Option<&str>) -> Value {
    let mut techniques = json!({ "resolutionType": resolution_type });
    if let Some(model) = matching_model {
        techniques["ruleBasedProperties"] = json!({
            "rules": [{ "ruleName": "by email", "matchingKeys": ["email"] }],
            "attributeMatchingModel": model,
        });
    }
    json!({
        "workflowName": name,
        "inputSourceConfig": [
            { "inputSourceARN": glue_table_arn(name), "schemaName": SCHEMA_NAME },
        ],
        "outputSourceConfig": [{
            "outputS3Path": "s3://er-output/enum",
            "output": [{ "name": "id" }],
        }],
        "resolutionTechniques": techniques,
        "roleArn": role_arn(),
    })
}

fn enum_cases() -> Vec<EnumCase> {
    vec![
        EnumCase {
            case: "CreateIdNamespace type, an invented value",
            method: "POST",
            path: "/idnamespaces".to_string(),
            body: id_namespace("ns-invented", "NOT_A_REAL_TYPE"),
            member_path: "type",
            value: "NOT_A_REAL_TYPE",
            stated_set: SOURCE_TARGET_SET,
        },
        EnumCase {
            case: "CreateIdNamespace type, an admitted value in the wrong case",
            method: "POST",
            path: "/idnamespaces".to_string(),
            body: id_namespace("ns-case", "source"),
            member_path: "type",
            value: "source",
            stated_set: SOURCE_TARGET_SET,
        },
        EnumCase {
            case: "CreateIdNamespace type, a value another enum of this service admits",
            method: "POST",
            path: "/idnamespaces".to_string(),
            body: id_namespace("ns-other", "RULE_BASED"),
            member_path: "type",
            value: "RULE_BASED",
            stated_set: SOURCE_TARGET_SET,
        },
        EnumCase {
            case: "CreateIdMappingWorkflow inputSourceConfig type, the SAME enum in a list",
            method: "POST",
            path: "/idmappingworkflows".to_string(),
            body: id_mapping_workflow("wf-source-type", "BOTH"),
            member_path: "inputSourceConfig.1.member.type",
            value: "BOTH",
            stated_set: SOURCE_TARGET_SET,
        },
        EnumCase {
            case: "CreateMatchingWorkflow resolutionType, a member inside a structure",
            method: "POST",
            path: "/matchingworkflows".to_string(),
            body: matching_workflow("wf-resolution", "FUZZY_MATCHING", None),
            member_path: "resolutionTechniques.resolutionType",
            value: "FUZZY_MATCHING",
            stated_set: RESOLUTION_TYPE_SET,
        },
        EnumCase {
            case: "CreateMatchingWorkflow attributeMatchingModel, two structures deep",
            method: "POST",
            path: "/matchingworkflows".to_string(),
            body: matching_workflow("wf-model", "RULE_MATCHING", Some("ONE_TO_MANY")),
            member_path: "resolutionTechniques.ruleBasedProperties.attributeMatchingModel",
            value: "ONE_TO_MANY",
            stated_set: ATTRIBUTE_MATCHING_MODEL_SET,
        },
        EnumCase {
            case: "CreateIdMappingWorkflow idMappingType, another enum again",
            method: "POST",
            path: "/idmappingworkflows".to_string(),
            body: id_mapping_workflow_with_type("wf-mapping-type", "ML_BASED"),
            member_path: "idMappingTechniques.idMappingType",
            value: "ML_BASED",
            stated_set: ID_MAPPING_TYPE_SET,
        },
    ]
}

/// The whole message the rule says a refusal of `value` at `member_path` carries, with
/// `stated_set` listed in the order the service states it.
fn expected_message(value: &str, member_path: &str, stated_set: &[&str]) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member_path}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{}]",
        stated_set.join(", ")
    )
}

/// The value set a message states, i.e. what is inside the brackets of its
/// `enum value set: [...]` clause.
fn stated_value_set(message: &str) -> Option<&str> {
    let (_, tail) = message.split_once("enum value set: [")?;
    tail.strip_suffix(']')
}

/// Seed the schema mapping the workflow rows reference, so no row is refused for a
/// missing reference before its enum is looked at.
async fn seed(emulator: &Emulator) {
    let reply = emulator
        .call(
            "POST",
            "/schemas",
            Some(&json!({
                "schemaName": SCHEMA_NAME,
                "mappedInputFields": [
                    { "fieldName": "id", "type": "UNIQUE_ID" },
                    { "fieldName": "email", "type": "EMAIL_ADDRESS", "matchKey": "email" },
                ],
            })),
        )
        .await;
    assert!(
        reply.status == 200 || reply.error_code.as_deref() == Some("ConflictException"),
        "seeding the schema mapping failed: {reply:?}"
    );
}

async fn refuse(emulator: &Emulator, case: &EnumCase) -> Reply {
    emulator
        .call(case.method, &case.path, Some(&case.body))
        .await
}

#[tokio::test]
async fn a_value_outside_an_enum_is_refused_naming_the_member_the_value_and_the_set() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in enum_cases() {
        let label = case.case;
        let refused = refuse(&emulator, &case).await;
        assert_error_shape(label, &refused, 400, "ValidationException");
        let message = string_member(&refused.body, "message");
        assert_eq!(
            message,
            expected_message(case.value, case.member_path, case.stated_set),
            "case {label:?}: got {message:?}"
        );
    }
}

#[tokio::test]
async fn one_enum_is_stated_in_one_order_wherever_it_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // Group the rows by the set they must state: every row of one set must answer the
    // SAME bracketed list, whichever operation, member or depth it came from, and no
    // two different enums may answer the same list.
    let mut sets: BTreeSet<(String, String)> = BTreeSet::new();
    for case in enum_cases() {
        let label = case.case;
        let refused = refuse(&emulator, &case).await;
        let message = string_member(&refused.body, "message");
        let stated = stated_value_set(&message)
            .unwrap_or_else(|| panic!("case {label:?}: no enum value set in {message:?}"));
        assert_eq!(
            stated,
            case.stated_set.join(", "),
            "case {label:?}: expected the value set {:?}, got {stated:?}",
            case.stated_set
        );
        sets.insert((case.stated_set.join(", "), stated.to_string()));
    }

    // One row per DISTINCT enum in the table: the four sets above, each stated once.
    assert_eq!(
        sets.len(),
        4,
        "each enum of the table must state exactly one value set, saw {sets:?}"
    );
}

#[tokio::test]
async fn the_stated_set_holds_exactly_the_values_the_enum_admits() {
    // The order is the service's; WHICH values are listed is the model's. Reordering
    // must therefore neither add nor drop a value, which is what this pins against the
    // generated value-set constants the model produced.
    for (enum_name, stated, modelled) in [
        (
            "IdNamespaceType",
            SOURCE_TARGET_SET,
            service_com_amazonaws_entityresolution_awsveniceservice::ops::types::ID_NAMESPACE_TYPE_VALUES,
        ),
        (
            "ResolutionType",
            RESOLUTION_TYPE_SET,
            service_com_amazonaws_entityresolution_awsveniceservice::ops::types::RESOLUTION_TYPE_VALUES,
        ),
        (
            "AttributeMatchingModel",
            ATTRIBUTE_MATCHING_MODEL_SET,
            service_com_amazonaws_entityresolution_awsveniceservice::ops::types::ATTRIBUTE_MATCHING_MODEL_VALUES,
        ),
        (
            "IdMappingType",
            ID_MAPPING_TYPE_SET,
            service_com_amazonaws_entityresolution_awsveniceservice::ops::types::ID_MAPPING_TYPE_VALUES,
        ),
    ] {
        let stated: BTreeSet<&str> = stated.iter().copied().collect();
        let modelled: BTreeSet<&str> = modelled.iter().copied().collect();
        assert_eq!(
            stated, modelled,
            "{enum_name}: the stated set must hold exactly the modelled values, got \
             {stated:?} against {modelled:?}"
        );
    }
}
