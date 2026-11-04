// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateIdMappingWorkflow`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `PUT /idmappingworkflows/{workflowName}`, through the harness the
//! `schema_mapping_workflow_lifecycle` suite owns and the create/read helpers the
//! sibling `id_mapping_workflow_lifecycle` suite owns — so what is asserted is what an
//! AWS SDK client would see.
//!
//! The rule under test: `UpdateIdMappingWorkflow` REPLACES the configuration members of
//! the ID mapping workflow already persisted under the requested name and re-stamps its
//! `updatedAt`, while the resource's IDENTITY — the `workflowName` key, the
//! `workflowArn` minted at create time, the `createdAt` stamp — and its tags, none of
//! which the request carries, survive unchanged; because the request carries the WHOLE
//! configuration ("identical to CreateIdMappingWorkflow, except it uses an HTTP PUT
//! request"), an optional member it omits is cleared rather than kept, and a name
//! nothing was created under is refused with `ResourceNotFoundException` (HTTP 404) and
//! persists nothing.
//!
//! Each table row differs along what the rule generalises over rather than repeating one
//! recorded request: the `idMappingTechniques.idMappingType` before and after the update
//! over the whole modelled catalog, the rule-based enums over their whole catalogs, the
//! `inputSourceConfig` length over the modelled `length 1..20` minimum, a middle value
//! and the maximum, which optional members each configuration carries (`description`,
//! `outputSourceConfig`, `incrementalRunConfig`, `roleArn`) over absent, present and
//! empty in BOTH directions, and the `workflowName` over the modelled `EntityName`
//! length bounds.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::id_mapping_workflow_lifecycle::{
    create_id_mapping_workflow, get_id_mapping_workflow, kms_arn, provider_service_arn, rule_based,
    ATTRIBUTE_MATCHING_MODELS, ID_MAPPING_TYPES, ID_NAMESPACE_TYPES, RECORD_MATCHING_MODELS,
    RULE_DEFINITION_TYPES,
};
use crate::schema_mapping_workflow_lifecycle::{
    assert_error_shape, expected_arn, glue_table_arn, role_arn, string_member, timestamp_member,
    Emulator, Reply,
};

/// Long enough that the millisecond-resolution `updatedAt` stamp of a later write is
/// strictly greater than an earlier one, so a re-stamp is observable on the wire.
///
/// Slept with `std::thread::sleep` rather than `tokio::time::sleep`, because the
/// workspace's `tokio` carries only the `macros`/`rt`/`rt-multi-thread` features and
/// `tokio::time` is gated behind `time`.
const A_LATER_MILLISECOND: Duration = Duration::from_millis(5);

/// `UpdateIdMappingWorkflow`'s `ResourceNotFoundException$message`, pinned here as a
/// literal so this suite checks the wire text independently of the constant the service
/// crate declares.
const ID_MAPPING_WORKFLOW_NOT_FOUND: &str = "IdMappingWorkflow Not Found";

/// The members `UpdateIdMappingWorkflow` and `GetIdMappingWorkflow` both model, which
/// must therefore be identical: both are served from the one stored record
/// (`smithy_query(query="operation",operation="UpdateIdMappingWorkflow,GetIdMappingWorkflow")`).
const SHARED_MEMBERS: &[&str] = &[
    "workflowName",
    "workflowArn",
    "description",
    "inputSourceConfig",
    "outputSourceConfig",
    "idMappingTechniques",
    "incrementalRunConfig",
    "roleArn",
];

/// `PUT /idmappingworkflows/{workflowName}` — `UpdateIdMappingWorkflow`'s modelled HTTP
/// binding, with the name as the `@httpLabel` and the new configuration as the body.
async fn update_id_mapping_workflow(
    emulator: &Emulator,
    workflow_name: &str,
    request: &Value,
) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/idmappingworkflows/{workflow_name}"),
            Some(request),
        )
        .await
}

/// One ID mapping workflow configuration, held as its PIECES rather than as a literal
/// body: the request is assembled from the same values the assertions expect, so the two
/// cannot drift, and an optional member a case omits really is absent from the JSON.
struct Config {
    /// `IdMappingWorkflowInputSourceConfig`, modelled `length 1..20` and @required.
    input_source_config: Value,
    /// `IdMappingTechniques`, @required.
    id_mapping_techniques: Value,
    /// Optional on both the writer and the reader, so absent stays absent.
    description: Option<String>,
    /// Optional with NO modelled default (`length 1..1`).
    output_source_config: Option<Value>,
    /// Optional with no modelled default.
    incremental_run_config: Option<Value>,
    /// What the request sends for `roleArn`. The member is modelled with `default: ""`,
    /// so `None` means "the caller sent none" and the service must answer the empty
    /// string for it.
    role_arn: Option<String>,
    /// `tags` is carried by `CreateIdMappingWorkflow` only: `UpdateIdMappingWorkflow`
    /// models no tags at all, so they are the create's and `GetIdMappingWorkflow`'s.
    tags: Option<Value>,
}

impl Config {
    /// The configuration members shared by the create and the update bodies.
    fn members(&self) -> Map<String, Value> {
        let mut body = Map::new();
        body.insert(
            "inputSourceConfig".to_string(),
            self.input_source_config.clone(),
        );
        body.insert(
            "idMappingTechniques".to_string(),
            self.id_mapping_techniques.clone(),
        );
        if let Some(description) = &self.description {
            body.insert("description".to_string(), json!(description));
        }
        if let Some(output_source_config) = &self.output_source_config {
            body.insert(
                "outputSourceConfig".to_string(),
                output_source_config.clone(),
            );
        }
        if let Some(incremental_run_config) = &self.incremental_run_config {
            body.insert(
                "incrementalRunConfig".to_string(),
                incremental_run_config.clone(),
            );
        }
        if let Some(role_arn) = &self.role_arn {
            body.insert("roleArn".to_string(), json!(role_arn));
        }
        body
    }

    /// The `CreateIdMappingWorkflow` body: the configuration plus the `workflowName`
    /// member and the tags, neither of which the update carries.
    fn create_body(&self, workflow_name: &str) -> Value {
        let mut body = self.members();
        body.insert("workflowName".to_string(), json!(workflow_name));
        if let Some(tags) = &self.tags {
            body.insert("tags".to_string(), tags.clone());
        }
        Value::Object(body)
    }

    /// The `UpdateIdMappingWorkflow` body: `workflowName` is the `@httpLabel`, so it is
    /// not a body member, and the operation models no `tags`.
    fn update_body(&self) -> Value {
        Value::Object(self.members())
    }

    /// The `roleArn` the service must answer: what the request carried, or the modelled
    /// `default: ""` when it carried none.
    fn served_role_arn(&self) -> &str {
        self.role_arn.as_deref().unwrap_or("")
    }
}

/// One workflow, the configuration it is created with, and the configuration an
/// `UpdateIdMappingWorkflow` call replaces that with.
struct UpdateCase {
    case: &'static str,
    workflow_name: String,
    create: Config,
    update: Config,
}

/// `count` `IdMappingWorkflowInputSource` entries, alternating over the two
/// `InputSourceARN` forms this member admits (a Glue table and an Entity Resolution ID
/// namespace) and over the whole `IdNamespaceType` catalog, so the LENGTH is what a
/// longer list changes.
fn input_sources(prefix: &str, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                let namespace_type = ID_NAMESPACE_TYPES[index % ID_NAMESPACE_TYPES.len()];
                if index % 2 == 0 {
                    json!({
                        "inputSourceARN": glue_table_arn(&format!("{prefix}_{index}")),
                        "schemaName": format!("{prefix}_{index}"),
                        "type": namespace_type,
                    })
                } else {
                    json!({
                        "inputSourceARN": expected_arn("idnamespace", &format!("{prefix}ns{index}")),
                        "type": namespace_type,
                    })
                }
            })
            .collect(),
    )
}

/// A provider-based `IdMappingTechniques`, i.e. the `PROVIDER` entry of the modelled
/// `IdMappingType` catalog (`ID_MAPPING_TYPES`).
fn provider_based(service: &str, maximum_matches: u64) -> Value {
    json!({
        "idMappingType": ID_MAPPING_TYPES[0],
        "providerProperties": {
            "providerServiceArn": provider_service_arn("testprovider", service),
            "providerConfiguration": {
                "targetIdentifier": "unique_id",
                "attributes": ["email", "phone"],
                "maximumMatches": maximum_matches,
            },
            "intermediateSourceConfiguration": {
                "intermediateS3Path": format!("s3://er-intermediate/{service}"),
            },
        },
    })
}

/// The table. Rows span the catalogs, bounds and optional-member directions the module
/// doc describes.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "rule-based to rule-based across the whole rule-enum catalogs, at the \
                   modelled minimum of one input source, with no optional member on \
                   either side",
            workflow_name: "customers".to_string(),
            create: Config {
                input_source_config: input_sources("customers", 1),
                id_mapping_techniques: rule_based(
                    RULE_DEFINITION_TYPES[0],
                    ATTRIBUTE_MATCHING_MODELS[0],
                    RECORD_MATCHING_MODELS[0],
                    Some(json!([{ "ruleName": "by email", "matchingKeys": ["email"] }])),
                ),
                description: None,
                output_source_config: None,
                incremental_run_config: None,
                role_arn: None,
                tags: Some(json!({ "team": "identity" })),
            },
            update: Config {
                input_source_config: input_sources("renamed", 1),
                id_mapping_techniques: rule_based(
                    RULE_DEFINITION_TYPES[1],
                    ATTRIBUTE_MATCHING_MODELS[1],
                    RECORD_MATCHING_MODELS[1],
                    Some(json!([
                        { "ruleName": "by phone", "matchingKeys": ["phone"] },
                        { "ruleName": "by address", "matchingKeys": ["address", "zip"] },
                    ])),
                ),
                description: None,
                output_source_config: None,
                incremental_run_config: None,
                role_arn: None,
                tags: None,
            },
        },
        UpdateCase {
            case: "rule-based to provider-based, every optional member absent before the \
                   update and present after it",
            workflow_name: "crm_people-1".to_string(),
            create: Config {
                input_source_config: input_sources("crm", 2),
                id_mapping_techniques: rule_based(
                    RULE_DEFINITION_TYPES[1],
                    ATTRIBUTE_MATCHING_MODELS[0],
                    RECORD_MATCHING_MODELS[1],
                    None,
                ),
                description: None,
                output_source_config: None,
                incremental_run_config: None,
                role_arn: None,
                tags: Some(json!({ "team": "identity", "env": "test" })),
            },
            update: Config {
                input_source_config: input_sources("crm_after", 3),
                id_mapping_techniques: provider_based("assignment", 7),
                description: Some("people mapped from the CRM".to_string()),
                output_source_config: Some(json!([{
                    "outputS3Path": "s3://er-output/crm",
                    "KMSArn": kms_arn(),
                }])),
                incremental_run_config: Some(json!({ "incrementalRunType": "ON_DEMAND" })),
                role_arn: Some(role_arn()),
                tags: None,
            },
        },
        UpdateCase {
            case: "provider-based to rule-based, every optional member present before the \
                   update and omitted after it, so the update CLEARS them",
            workflow_name: "DEVICES0".to_string(),
            create: Config {
                input_source_config: input_sources("devices", 4),
                id_mapping_techniques: provider_based("matching", 1),
                description: Some("devices, before".to_string()),
                output_source_config: Some(json!([{
                    "outputS3Path": "s3://er-output/devices",
                    "KMSArn": kms_arn(),
                }])),
                incremental_run_config: Some(json!({ "incrementalRunType": "ON_DEMAND" })),
                role_arn: Some(role_arn()),
                tags: Some(json!({})),
            },
            update: Config {
                input_source_config: input_sources("devices_after", 1),
                id_mapping_techniques: rule_based(
                    RULE_DEFINITION_TYPES[0],
                    ATTRIBUTE_MATCHING_MODELS[1],
                    RECORD_MATCHING_MODELS[0],
                    Some(json!([{ "ruleName": "by serial", "matchingKeys": ["serial"] }])),
                ),
                description: None,
                output_source_config: None,
                incremental_run_config: None,
                role_arn: None,
                tags: None,
            },
        },
        UpdateCase {
            case: "the modelled minimum of one input source widened to the modelled \
                   maximum of twenty",
            workflow_name: "widest".to_string(),
            create: Config {
                input_source_config: input_sources("narrow", 1),
                id_mapping_techniques: rule_based(
                    RULE_DEFINITION_TYPES[0],
                    ATTRIBUTE_MATCHING_MODELS[0],
                    RECORD_MATCHING_MODELS[0],
                    None,
                ),
                description: Some("one source".to_string()),
                output_source_config: None,
                incremental_run_config: None,
                role_arn: Some(role_arn()),
                tags: Some(json!({ "sources": "1" })),
            },
            update: Config {
                input_source_config: input_sources("wide", 20),
                id_mapping_techniques: rule_based(
                    RULE_DEFINITION_TYPES[1],
                    ATTRIBUTE_MATCHING_MODELS[0],
                    RECORD_MATCHING_MODELS[0],
                    Some(Value::Array(
                        (0..25)
                            .map(|index| {
                                json!({
                                    "ruleName": format!("rule {index}"),
                                    "matchingKeys": [format!("key_{index}"), "email"],
                                })
                            })
                            .collect(),
                    )),
                ),
                description: Some("twenty sources".to_string()),
                output_source_config: None,
                incremental_run_config: None,
                role_arn: Some(role_arn()),
                tags: None,
            },
        },
        UpdateCase {
            case: "the modelled maximum of twenty input sources narrowed to a middle \
                   seven, with the empty description, the empty incrementalRunConfig and \
                   the explicitly empty roleArn the modelled default admits",
            workflow_name: "a".to_string(),
            create: Config {
                input_source_config: input_sources("full", 20),
                id_mapping_techniques: provider_based("assignment", 10),
                description: Some("twenty, before".to_string()),
                output_source_config: Some(json!([{ "outputS3Path": "s3://er-output/full" }])),
                incremental_run_config: Some(json!({ "incrementalRunType": "ON_DEMAND" })),
                role_arn: Some(role_arn()),
                tags: Some(json!({ "sources": "20" })),
            },
            update: Config {
                input_source_config: input_sources("middle", 7),
                id_mapping_techniques: rule_based(
                    RULE_DEFINITION_TYPES[1],
                    ATTRIBUTE_MATCHING_MODELS[1],
                    RECORD_MATCHING_MODELS[1],
                    None,
                ),
                description: Some(String::new()),
                output_source_config: Some(json!([{ "outputS3Path": "s3://er-output/middle" }])),
                incremental_run_config: Some(json!({})),
                role_arn: Some(String::new()),
                tags: None,
            },
        },
        UpdateCase {
            case: "a workflowName at the modelled EntityName length maximum of 255, with \
                   the description at its maximum of 255",
            workflow_name: "w".repeat(255),
            create: Config {
                input_source_config: input_sources("longest", 1),
                id_mapping_techniques: rule_based(
                    RULE_DEFINITION_TYPES[0],
                    ATTRIBUTE_MATCHING_MODELS[1],
                    RECORD_MATCHING_MODELS[1],
                    None,
                ),
                description: Some("c".repeat(255)),
                output_source_config: None,
                incremental_run_config: None,
                role_arn: Some(String::new()),
                tags: Some(json!({ "length": "255" })),
            },
            update: Config {
                input_source_config: input_sources("longest_after", 2),
                id_mapping_techniques: provider_based("matching", 255),
                description: Some("d".repeat(255)),
                output_source_config: Some(json!([{
                    "outputS3Path": "s3://er-output/longest",
                    "KMSArn": kms_arn(),
                }])),
                incremental_run_config: Some(json!({ "incrementalRunType": "ON_DEMAND" })),
                role_arn: Some(role_arn()),
                tags: None,
            },
        },
    ]
}

/// Assert the configuration members of `body` are the ones `want` asked for, and that
/// the identity members name the resource `name`.
fn assert_configuration(label: &str, source: &str, name: &str, want: &Config, body: &Value) {
    assert_eq!(
        string_member(body, "workflowName"),
        name,
        "case {label:?}: {source} must answer the stored workflowName, got {body:?}"
    );
    assert_eq!(
        string_member(body, "workflowArn"),
        expected_arn("idmappingworkflow", name),
        "case {label:?}: {source} must answer the ARN minted at create time, got {body:?}"
    );
    assert_eq!(
        body.get("description").and_then(Value::as_str),
        want.description.as_deref(),
        "case {label:?}: {source} must answer the stored description, got {body:?}"
    );
    assert_eq!(
        body.get("inputSourceConfig"),
        Some(&want.input_source_config),
        "case {label:?}: {source} must answer the stored inputSourceConfig, got {body:?}"
    );
    assert_eq!(
        body.get("outputSourceConfig"),
        want.output_source_config.as_ref(),
        "case {label:?}: {source} must answer the stored outputSourceConfig, got {body:?}"
    );
    assert_eq!(
        body.get("idMappingTechniques"),
        Some(&want.id_mapping_techniques),
        "case {label:?}: {source} must answer the stored idMappingTechniques, got {body:?}"
    );
    assert_eq!(
        body.get("incrementalRunConfig"),
        want.incremental_run_config.as_ref(),
        "case {label:?}: {source} must answer the stored incrementalRunConfig, got {body:?}"
    );
    assert_eq!(
        body.get("roleArn").and_then(Value::as_str).unwrap_or(""),
        want.served_role_arn(),
        "case {label:?}: {source} must answer the stored roleArn, got {body:?}"
    );
}

#[tokio::test]
async fn update_id_mapping_workflow_replaces_the_stored_configuration_and_keeps_the_identity() {
    let emulator = Emulator::new();

    for case in update_cases() {
        let label = case.case;
        let name = &case.workflow_name;

        let created = create_id_mapping_workflow(&emulator, &case.create.create_body(name)).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateIdMappingWorkflow, got {created:?}"
        );
        let before = get_id_mapping_workflow(&emulator, name).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: expected HTTP 200 from GetIdMappingWorkflow, got {before:?}"
        );
        assert_configuration(
            label,
            "GetIdMappingWorkflow before the update",
            name,
            &case.create,
            &before.body,
        );

        sleep(A_LATER_MILLISECOND);
        let updated = update_id_mapping_workflow(&emulator, name, &case.update.update_body()).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateIdMappingWorkflow, got {updated:?}"
        );
        assert_configuration(
            label,
            "UpdateIdMappingWorkflow",
            name,
            &case.update,
            &updated.body,
        );
        // `UpdateIdMappingWorkflowOutput` models neither the stamps nor the tags, so
        // neither may appear on the update response.
        for absent in ["createdAt", "updatedAt", "tags"] {
            assert!(
                updated.body.get(absent).is_none(),
                "case {label:?}: UpdateIdMappingWorkflowOutput models no {absent}, got {:?}",
                updated.body
            );
        }

        // The round trip: the reader clones what the update wrote.
        let after = get_id_mapping_workflow(&emulator, name).await;
        assert_eq!(
            after.status, 200,
            "case {label:?}: expected HTTP 200 from GetIdMappingWorkflow, got {after:?}"
        );
        assert_configuration(
            label,
            "GetIdMappingWorkflow after the update",
            name,
            &case.update,
            &after.body,
        );
        for member in SHARED_MEMBERS {
            assert_eq!(
                updated.body.get(member),
                after.body.get(member),
                "case {label:?}: member {member:?} differs between the update and the read: \
                 {:?} vs {:?}",
                updated.body,
                after.body
            );
        }

        // The identity the request does not carry survives: the ARN is not re-minted,
        // `createdAt` is not re-stamped, and the tags the create set are untouched.
        assert_eq!(
            string_member(&after.body, "workflowArn"),
            string_member(&before.body, "workflowArn"),
            "case {label:?}: the update must not re-mint workflowArn, got {after:?}"
        );
        let created_at = timestamp_member(&before.body, "createdAt");
        assert!(
            (timestamp_member(&after.body, "createdAt") - created_at).abs() < f64::EPSILON,
            "case {label:?}: expected createdAt to stay {created_at}, got {:?}",
            after.body
        );
        assert_eq!(
            after.body.get("tags"),
            case.create.tags.as_ref(),
            "case {label:?}: the update carries no tags, so it must not change them, got {:?}",
            after.body
        );

        // `updatedAt` is re-stamped by the write, and never precedes `createdAt`.
        let was_updated_at = timestamp_member(&before.body, "updatedAt");
        let now_updated_at = timestamp_member(&after.body, "updatedAt");
        assert!(
            now_updated_at > was_updated_at,
            "case {label:?}: expected updatedAt to advance past {was_updated_at}, got \
             {now_updated_at}"
        );
        assert!(
            now_updated_at >= created_at,
            "case {label:?}: expected updatedAt {now_updated_at} not to precede createdAt \
             {created_at}"
        );

        // A read changes nothing a later caller sees.
        let again = get_id_mapping_workflow(&emulator, name).await;
        assert_eq!(
            again.body, after.body,
            "case {label:?}: two consecutive reads must answer identically, got {again:?}"
        );
    }
}

#[tokio::test]
async fn updating_one_workflow_leaves_every_other_stored_workflow_untouched() {
    let emulator = Emulator::new();
    let cases = update_cases();

    for case in &cases {
        let created =
            create_id_mapping_workflow(&emulator, &case.create.create_body(&case.workflow_name))
                .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: seeding the store failed: {created:?}",
            case.case
        );
    }

    for case in &cases {
        let label = case.case;
        let mut untouched = Vec::new();
        for other in cases
            .iter()
            .filter(|other| other.workflow_name != case.workflow_name)
        {
            let read = get_id_mapping_workflow(&emulator, &other.workflow_name).await;
            untouched.push((other.workflow_name.clone(), read.body));
        }

        sleep(A_LATER_MILLISECOND);
        let updated = update_id_mapping_workflow(
            &emulator,
            &case.workflow_name,
            &case.update.update_body(),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateIdMappingWorkflow, got {updated:?}"
        );

        for (other_name, snapshot) in untouched {
            let read = get_id_mapping_workflow(&emulator, &other_name).await;
            assert_eq!(
                read.body, snapshot,
                "case {label:?}: updating it must not touch workflow {other_name:?}, got \
                 {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_unknown_workflow_name_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();

    // One ID mapping workflow exists, so a miss is a real lookup miss and not an empty
    // store.
    let seed = &update_cases()[0];
    let created =
        create_id_mapping_workflow(&emulator, &seed.create.create_body(&seed.workflow_name)).await;
    assert_eq!(created.status, 200, "seeding the store failed: {created:?}");
    let seeded = get_id_mapping_workflow(&emulator, &seed.workflow_name).await;

    // A MATCHING workflow is a different namespace keyed by the same kind of name, so
    // an ID mapping update addressed to its name must not find — or touch — it.
    let matching_name = "shared-name";
    emulator.seed_schema_mapping("customers").await;
    let matching = json!({
        "workflowName": matching_name,
        "inputSourceConfig": [
            { "inputSourceARN": glue_table_arn("customers"), "schemaName": "customers" },
        ],
        "outputSourceConfig": [{
            "outputS3Path": "s3://er-output/shared",
            "output": [{ "name": "id" }],
        }],
        "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
        "roleArn": role_arn(),
    });
    let seeded_matching = emulator.create_matching_workflow(&matching).await;
    assert_eq!(
        seeded_matching.status, 200,
        "seeding the matching workflow failed: {seeded_matching:?}"
    );
    let matching_before = emulator.get_matching_workflow(matching_name).await;

    // Names spanning what the modelled `EntityName` pattern `^[a-zA-Z_0-9-]*$` admits,
    // at both its length bounds — none of them created as an ID mapping workflow.
    let missing = [
        "z".to_string(),
        "MISSING".to_string(),
        "no_such-workflow-0".to_string(),
        "m".repeat(255),
        matching_name.to_string(),
    ];

    for name in &missing {
        let reply = update_id_mapping_workflow(&emulator, name, &seed.update.update_body()).await;
        assert_error_shape(name, &reply, 404, "ResourceNotFoundException");
        assert_eq!(
            reply.body.get("message").and_then(Value::as_str),
            Some(ID_MAPPING_WORKFLOW_NOT_FOUND),
            "case {name:?}: expected the operation's constant not-found message, got {reply:?}"
        );

        // A refused update creates nothing under the name it was addressed to.
        let read = get_id_mapping_workflow(&emulator, name).await;
        assert_error_shape(name, &read, 404, "ResourceNotFoundException");
    }

    // Neither the one stored ID mapping workflow nor the same-named matching workflow
    // moved.
    let still_there = get_id_mapping_workflow(&emulator, &seed.workflow_name).await;
    assert_eq!(
        still_there.body, seeded.body,
        "a refused update must leave every stored record untouched, got {still_there:?}"
    );
    let matching_after = emulator.get_matching_workflow(matching_name).await;
    assert_eq!(
        matching_after.body, matching_before.body,
        "an ID mapping update must not touch the matching workflow of the same name, got \
         {matching_after:?}"
    );
}

#[tokio::test]
async fn a_repeated_update_answers_the_same_configuration() {
    let emulator = Emulator::new();

    for case in update_cases() {
        let label = case.case;
        let name = &case.workflow_name;
        let created = create_id_mapping_workflow(&emulator, &case.create.create_body(name)).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: seeding the store failed: {created:?}"
        );

        let first = update_id_mapping_workflow(&emulator, name, &case.update.update_body()).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateIdMappingWorkflow, got {first:?}"
        );
        let once = get_id_mapping_workflow(&emulator, name).await;

        sleep(A_LATER_MILLISECOND);
        let second = update_id_mapping_workflow(&emulator, name, &case.update.update_body()).await;
        assert_eq!(
            second.status, 200,
            "case {label:?}: expected HTTP 200 from a repeated UpdateIdMappingWorkflow, got \
             {second:?}"
        );
        let twice = get_id_mapping_workflow(&emulator, name).await;

        for member in SHARED_MEMBERS {
            assert_eq!(
                twice.body.get(member),
                once.body.get(member),
                "case {label:?}: member {member:?} must survive a repeated update unchanged: \
                 {:?} vs {:?}",
                twice.body,
                once.body
            );
        }
        assert_eq!(
            twice.body.get("tags"),
            once.body.get("tags"),
            "case {label:?}: the tags must survive a repeated update, got {:?}",
            twice.body
        );
        let created_at = timestamp_member(&once.body, "createdAt");
        assert!(
            (timestamp_member(&twice.body, "createdAt") - created_at).abs() < f64::EPSILON,
            "case {label:?}: expected createdAt to stay {created_at}, got {:?}",
            twice.body
        );
        let once_updated_at = timestamp_member(&once.body, "updatedAt");
        let twice_updated_at = timestamp_member(&twice.body, "updatedAt");
        assert!(
            twice_updated_at >= once_updated_at,
            "case {label:?}: expected updatedAt not to go backwards from {once_updated_at}, \
             got {twice_updated_at}"
        );
    }
}
