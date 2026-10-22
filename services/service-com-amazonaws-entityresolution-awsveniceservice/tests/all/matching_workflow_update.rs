// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateMatchingWorkflow`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `PUT /matchingworkflows/{workflowName}`, through the harness the sibling
//! `schema_mapping_workflow_lifecycle` suite owns — so what is asserted is what an AWS
//! SDK client would see.
//!
//! The rule under test: `UpdateMatchingWorkflow` REPLACES the configuration members of
//! the matching workflow already persisted under the requested name and re-stamps its
//! `updatedAt`, while the resource's IDENTITY — the `workflowName` key, the
//! `workflowArn` minted at create time, the `createdAt` stamp — and its tags, none of
//! which the request carries, survive unchanged; a name that is not stored is refused
//! with `ResourceNotFoundException` (HTTP 404) and persists nothing. Because the
//! request carries the WHOLE configuration, an optional member it omits is cleared,
//! not kept. Alongside it, the one cross-member rule the two writers document — "For
//! workflows where `resolutionType` is `PROVIDER`, incremental processing is not
//! supported" — is refused with `ValidationException` (HTTP 400) by both
//! `CreateMatchingWorkflow` and `UpdateMatchingWorkflow`.
//!
//! Each table spans the class the rule generalises over rather than one recorded
//! request: the `resolutionTechniques.resolutionType` before and after the update over
//! the whole modelled `ResolutionType` catalog, the `inputSourceConfig` length over the
//! modelled `length 1..20` minimum, a middle value and the maximum, which optional
//! members each configuration carries (`description`, `incrementalRunConfig`,
//! `applyNormalization`, `kmsArn`, `matchPurpose`, `enableRealTimeMatching`,
//! `enableTransitiveMatching`) over absent and present in BOTH directions, the shape of
//! `resolutionTechniques` (rule-based, rule-condition, provider), and the
//! `workflowName` over the modelled `EntityName` length maximum.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use crate::schema_mapping_workflow_lifecycle::{
    glue_table_arn, role_arn, string_member, timestamp_member, Emulator, Reply,
};

/// The members `UpdateMatchingWorkflow` carries on BOTH its input and its output, i.e.
/// the workflow's configuration: exactly what the update replaces
/// (`smithy_query(query="operation",operation="UpdateMatchingWorkflow")`). `workflowName`
/// is excluded because it is the `@httpLabel` that names the resource rather than part
/// of the configuration.
const CONFIG_MEMBERS: &[&str] = &[
    "description",
    "inputSourceConfig",
    "outputSourceConfig",
    "resolutionTechniques",
    "incrementalRunConfig",
    "roleArn",
];

/// The whole modelled `ResolutionType` catalog
/// (`smithy_query(query="shape",shape="ResolutionTechniques")` →
/// `ML_MATCHING | PROVIDER | RULE_MATCHING`).
const RESOLUTION_TYPES: &[&str] = &["ML_MATCHING", "PROVIDER", "RULE_MATCHING"];

/// `GetMatchingWorkflow`'s and `UpdateMatchingWorkflow`'s
/// `ResourceNotFoundException$message`, pinned here as a literal so this suite checks
/// the wire text independently of the constants the service crate declares.
const MATCHING_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";

/// `count` schema-mapping names under `prefix`, distinct across the table so one
/// shared emulator can serve every case.
fn schema_names(prefix: &str, count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{prefix}_{index}")).collect()
}

/// An `inputSourceConfig` referencing `schemas` — `InputSource$schemaName` is a
/// workflow's reference to a schema mapping — carrying `applyNormalization` only when
/// `normalize` is `Some`, so the table spans that optional member too.
fn input_sources(schemas: &[String], normalize: Option<bool>) -> Value {
    Value::Array(
        schemas
            .iter()
            .map(|schema| {
                let mut source = json!({
                    "inputSourceARN": glue_table_arn(schema),
                    "schemaName": schema,
                });
                if let Some(flag) = normalize {
                    source["applyNormalization"] = json!(flag);
                }
                source
            })
            .collect(),
    )
}

/// One workflow, the configuration it is created with, and the configuration an
/// `UpdateMatchingWorkflow` call replaces that with.
struct UpdateCase {
    case: &'static str,
    workflow_name: String,
    /// Every schema mapping either configuration references, created first so the
    /// workflow's `inputSourceConfig` names records that exist.
    schemas: Vec<String>,
    create: Value,
    /// The `UpdateMatchingWorkflow` body: `workflowName` is the `@httpLabel`, so it is
    /// not a body member.
    update: Value,
}

/// The `UpdateMatchingWorkflow` table.
fn update_cases() -> Vec<UpdateCase> {
    let rules = schema_names("rules", 3);
    let wide = schema_names("wide", 20);
    let to_provider = schema_names("toprov", 2);
    let from_provider = schema_names("fromprov", 2);
    let output = schema_names("output", 1);
    let longest = schema_names("longest", 1);
    // The modelled `EntityName` length maximum of 255.
    let longest_name = "w".repeat(255);
    // The modelled `Description` length maximum of 255.
    let longest_description = "d".repeat(255);

    vec![
        UpdateCase {
            case: "rule matching narrowed to the modelled minimum of one input source, \
                   every optional member dropped",
            workflow_name: "wf-rules".to_string(),
            schemas: rules.clone(),
            create: json!({
                "workflowName": "wf-rules",
                "description": "the original rule-based workflow",
                "inputSourceConfig": input_sources(&rules, Some(true)),
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/rules",
                    "output": [{ "name": "id" }, { "name": "name", "hashed": true }],
                    "applyNormalization": true,
                }],
                "resolutionTechniques": {
                    "resolutionType": "RULE_MATCHING",
                    "ruleBasedProperties": {
                        "rules": [{ "ruleName": "byName", "matchingKeys": ["name"] }],
                        "attributeMatchingModel": "ONE_TO_ONE",
                    },
                },
                "incrementalRunConfig": { "incrementalRunType": "IMMEDIATE" },
                "roleArn": role_arn(),
                "tags": { "owner": "identity" },
            }),
            update: json!({
                "inputSourceConfig": input_sources(&rules[..1], None),
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/rules-v2",
                    "output": [{ "name": "id" }],
                }],
                "resolutionTechniques": {
                    "resolutionType": "RULE_MATCHING",
                    "ruleBasedProperties": {
                        "rules": [
                            { "ruleName": "byEmail", "matchingKeys": ["email"] },
                            { "ruleName": "byPhone", "matchingKeys": ["phone", "zip"] },
                        ],
                        "attributeMatchingModel": "MANY_TO_MANY",
                        "matchPurpose": "INDEXING",
                    },
                },
                "roleArn": role_arn(),
            }),
        },
        UpdateCase {
            case: "ML matching widened from one to the modelled maximum of 20 input \
                   sources, with a description and a new role added",
            workflow_name: "WF_ml".to_string(),
            schemas: wide.clone(),
            create: json!({
                "workflowName": "WF_ml",
                "inputSourceConfig": input_sources(&wide[..1], None),
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/ml",
                    "output": [{ "name": "id" }],
                }],
                "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                "roleArn": role_arn(),
            }),
            update: json!({
                "description": "widened to every source",
                "inputSourceConfig": input_sources(&wide, Some(false)),
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/ml",
                    "output": [{ "name": "id" }, { "name": "email", "hashed": true }],
                }],
                "resolutionTechniques": {
                    "resolutionType": "ML_MATCHING",
                    "enableRealTimeMatching": true,
                },
                "roleArn": "arn:aws:iam::000000000000:role/entity-resolution-v2",
            }),
        },
        UpdateCase {
            case: "resolutionType switched from RULE_MATCHING to PROVIDER, which drops \
                   the incrementalRunConfig the create carried",
            workflow_name: "wf-to-provider".to_string(),
            schemas: to_provider.clone(),
            create: json!({
                "workflowName": "wf-to-provider",
                "description": "rule based, incremental",
                "inputSourceConfig": input_sources(&to_provider, None),
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/switch",
                    "output": [{ "name": "id" }],
                }],
                "resolutionTechniques": {
                    "resolutionType": "RULE_MATCHING",
                    "ruleBasedProperties": {
                        "rules": [{ "ruleName": "byId", "matchingKeys": ["id"] }],
                        "attributeMatchingModel": "MANY_TO_MANY",
                        "matchPurpose": "IDENTIFIER_GENERATION",
                    },
                },
                "incrementalRunConfig": { "incrementalRunType": "IMMEDIATE" },
                "roleArn": role_arn(),
                "tags": {},
            }),
            update: json!({
                "description": "provider based",
                "inputSourceConfig": input_sources(&to_provider[..1], Some(true)),
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/switch",
                    "KMSArn": "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555",
                    "output": [{ "name": "id" }],
                }],
                "resolutionTechniques": {
                    "resolutionType": "PROVIDER",
                    "enableRealTimeMatching": false,
                    "providerProperties": {
                        "providerServiceArn":
                            "arn:aws:entityresolution:us-east-1::providerservice/testprovider/assignment",
                        "intermediateSourceConfiguration": {
                            "intermediateS3Path": "s3://er-intermediate/switch",
                        },
                    },
                },
                "roleArn": role_arn(),
            }),
        },
        UpdateCase {
            case: "resolutionType switched from PROVIDER to ML_MATCHING, which is what \
                   lets an incrementalRunConfig be added",
            workflow_name: "wf-from-provider0".to_string(),
            schemas: from_provider.clone(),
            create: json!({
                "workflowName": "wf-from-provider0",
                "inputSourceConfig": input_sources(&from_provider[..1], None),
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/fromprovider",
                    "output": [{ "name": "id" }],
                }],
                "resolutionTechniques": {
                    "resolutionType": "PROVIDER",
                    "providerProperties": {
                        "providerServiceArn":
                            "arn:aws:entityresolution:us-east-1::providerservice/testprovider/assignment",
                    },
                },
                "roleArn": role_arn(),
                "tags": { "env": "test", "team": "identity" },
            }),
            update: json!({
                "inputSourceConfig": input_sources(&from_provider, None),
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/fromprovider",
                    "output": [{ "name": "id" }],
                    "applyNormalization": false,
                }],
                "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                "incrementalRunConfig": { "incrementalRunType": "IMMEDIATE" },
                "roleArn": role_arn(),
            }),
        },
        UpdateCase {
            case: "only the single output source is rewritten, gaining a Customer \
                   Profiles integration",
            workflow_name: "wf3".to_string(),
            schemas: output.clone(),
            create: json!({
                "workflowName": "wf3",
                "description": "output rewrite",
                "inputSourceConfig": input_sources(&output, None),
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/v1",
                    "output": [{ "name": "id" }],
                }],
                "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                "roleArn": role_arn(),
            }),
            update: json!({
                "description": "output rewrite",
                "inputSourceConfig": input_sources(&output, None),
                "outputSourceConfig": [{
                    "output": [
                        { "name": "id" },
                        { "name": "first", "hashed": false },
                        { "name": "last", "hashed": true },
                    ],
                    "applyNormalization": true,
                    "customerProfilesIntegrationConfig": {
                        "domainArn": "arn:aws:profile:us-east-1:000000000000:domains/crm",
                        "objectTypeArn":
                            "arn:aws:profile:us-east-1:000000000000:domains/crm/object-types/people",
                    },
                }],
                "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                "roleArn": role_arn(),
            }),
        },
        UpdateCase {
            case: "a workflowName at the modelled length maximum of 255, updated to \
                   rule-condition properties and a description at its own maximum",
            workflow_name: longest_name.clone(),
            schemas: longest.clone(),
            create: json!({
                "workflowName": longest_name,
                "inputSourceConfig": input_sources(&longest, None),
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/longest",
                    "output": [{ "name": "id" }],
                }],
                "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                "roleArn": role_arn(),
            }),
            update: json!({
                "description": longest_description,
                "inputSourceConfig": input_sources(&longest, None),
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/longest",
                    "output": [{ "name": "id" }],
                }],
                "resolutionTechniques": {
                    "resolutionType": "RULE_MATCHING",
                    "ruleConditionProperties": {
                        "rules": [{
                            "ruleName": "transitive",
                            "condition": "{\"operator\":\"AND\"}",
                        }],
                        "matchingConfig": { "enableTransitiveMatching": true },
                    },
                },
                "roleArn": role_arn(),
            }),
        },
    ]
}

/// Create every schema mapping `schemas` names, so the workflow's `inputSourceConfig`
/// references records that exist.
async fn seed_schemas(emulator: &Emulator, case: &str, schemas: &[String]) {
    for schema in schemas {
        let reply = emulator
            .create_schema_mapping(&json!({
                "schemaName": schema,
                "mappedInputFields": [
                    { "fieldName": "id", "type": "UNIQUE_ID" },
                    { "fieldName": "name", "type": "NAME" },
                ],
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the referenced schema mapping {schema:?} must be created, \
             got {reply:?}"
        );
    }
}

/// Assert every configuration member of `actual` is what `expected` carries — which
/// also pins that a member `expected` OMITS is absent from `actual`.
fn assert_configuration(case: &str, from: &str, expected: &Value, actual: &Value) {
    for member in CONFIG_MEMBERS {
        assert_eq!(
            actual.get(member),
            expected.get(member),
            "case {case:?}: {from} must answer {member} exactly as the update carried \
             it, got {actual:?}"
        );
    }
}

/// Assert `reply` is the modelled client failure `code` with HTTP `status`.
fn assert_error(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {reply:?}"
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some(code),
        "case {case:?}: expected {code}, got {reply:?}"
    );
}

/// Long enough that the millisecond-resolution `updatedAt` stamp of a later write is
/// strictly greater than an earlier one, so a re-stamp is observable on the wire.
///
/// Slept with `std::thread::sleep` rather than `tokio::time::sleep`, because the
/// workspace's `tokio` carries only the `macros`/`rt`/`rt-multi-thread` features and
/// `tokio::time` is gated behind `time`: a test-only pause is not a reason to widen a
/// runtime dependency's feature set. Each test has its own runtime and no concurrent
/// task, so blocking it for five milliseconds starves nothing.
const A_LATER_MILLISECOND: Duration = Duration::from_millis(5);

#[tokio::test]
async fn an_update_replaces_the_stored_configuration_and_keeps_the_resources_identity() {
    let emulator = Emulator::new();

    for case in update_cases() {
        let label = case.case;
        let name = &case.workflow_name;
        seed_schemas(&emulator, label, &case.schemas).await;

        let created = emulator.create_matching_workflow(&case.create).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateMatchingWorkflow, got {created:?}"
        );
        let before = emulator.get_matching_workflow(name).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: expected HTTP 200 from GetMatchingWorkflow, got {before:?}"
        );

        sleep(A_LATER_MILLISECOND);
        let updated = emulator.update_matching_workflow(name, &case.update).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateMatchingWorkflow, got {updated:?}"
        );
        assert_eq!(
            string_member(&updated.body, "workflowName"),
            *name,
            "case {label:?}: the update must answer the workflow it addressed, got {:?}",
            updated.body
        );
        assert_configuration(label, "UpdateMatchingWorkflow", &case.update, &updated.body);

        // The round trip: the new configuration is what the reader serves.
        let after = emulator.get_matching_workflow(name).await;
        assert_eq!(
            after.status, 200,
            "case {label:?}: expected HTTP 200 from GetMatchingWorkflow, got {after:?}"
        );
        assert_configuration(label, "GetMatchingWorkflow", &case.update, &after.body);

        // Identity and tags are not members of the update request, so they survive it.
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
            before.body.get("tags"),
            "case {label:?}: the update carries no tags, so it must not change them, \
             got {after:?}"
        );

        // `updatedAt` is re-stamped by the write, and never precedes `createdAt`.
        let was_updated_at = timestamp_member(&before.body, "updatedAt");
        let now_updated_at = timestamp_member(&after.body, "updatedAt");
        assert!(
            now_updated_at > was_updated_at,
            "case {label:?}: expected updatedAt to advance past {was_updated_at}, \
             got {now_updated_at}"
        );
        assert!(
            now_updated_at >= created_at,
            "case {label:?}: expected updatedAt {now_updated_at} not to precede \
             createdAt {created_at}"
        );
    }
}

#[tokio::test]
async fn updating_a_workflow_that_does_not_exist_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();

    for case in update_cases() {
        let label = case.case;
        let name = &case.workflow_name;

        let refused = emulator.update_matching_workflow(name, &case.update).await;
        assert_error(label, &refused, 404, "ResourceNotFoundException");
        assert_eq!(
            string_member(&refused.body, "message"),
            MATCHING_WORKFLOW_NOT_FOUND,
            "case {label:?}: expected the message {MATCHING_WORKFLOW_NOT_FOUND:?}, \
             got {:?}",
            refused.body
        );

        // The refusal wrote nothing: an update is not a create.
        let read = emulator.get_matching_workflow(name).await;
        assert_error(label, &read, 404, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn an_update_touches_only_the_workflow_it_names() {
    let emulator = Emulator::new();
    let cases = update_cases();

    for case in &cases {
        seed_schemas(&emulator, case.case, &case.schemas).await;
        let created = emulator.create_matching_workflow(&case.create).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 from CreateMatchingWorkflow, got {created:?}",
            case.case
        );
    }

    for case in &cases {
        let label = case.case;
        // What every OTHER workflow reads as, immediately before this update.
        let mut untouched = Vec::new();
        for other in cases.iter().filter(|other| {
            other.workflow_name != case.workflow_name
        }) {
            let read = emulator.get_matching_workflow(&other.workflow_name).await;
            untouched.push((other.workflow_name.clone(), read.body));
        }

        sleep(A_LATER_MILLISECOND);
        let updated = emulator
            .update_matching_workflow(&case.workflow_name, &case.update)
            .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateMatchingWorkflow, got {updated:?}"
        );

        for (name, expected) in untouched {
            let read = emulator.get_matching_workflow(&name).await;
            assert_eq!(
                read.body, expected,
                "case {label:?}: updating it must leave {name:?} byte-identical, \
                 got {:?}",
                read.body
            );
        }
    }
}

/// One row of the cross-member rule: a `resolutionType` and whether the request
/// carries an `incrementalRunConfig`.
struct IncrementalCase {
    case: String,
    resolution_type: &'static str,
    incremental_run_config: Option<Value>,
}

/// The table: the WHOLE modelled `ResolutionType` catalog crossed with
/// `incrementalRunConfig` absent and present. Only `PROVIDER` + present is refused.
fn incremental_cases() -> Vec<IncrementalCase> {
    RESOLUTION_TYPES
        .iter()
        .flat_map(|resolution_type| {
            [
                None,
                // `IncrementalRunType`'s only modelled value
                // (`smithy_query(query="shape",shape="IncrementalRunConfig")`).
                Some(json!({ "incrementalRunType": "IMMEDIATE" })),
            ]
            .into_iter()
            .map(|incremental_run_config| IncrementalCase {
                case: format!(
                    "{resolution_type} with incrementalRunConfig {}",
                    if incremental_run_config.is_some() {
                        "present"
                    } else {
                        "absent"
                    }
                ),
                resolution_type,
                incremental_run_config,
            })
            .collect::<Vec<_>>()
        })
        .collect()
}

/// A matching-workflow configuration for `case`, as a body for either writer. The
/// `workflow_name` is included only when the caller needs it (create carries it in the
/// body; update carries it as the `@httpLabel`).
fn incremental_body(case: &IncrementalCase, workflow_name: Option<&str>) -> Value {
    let mut body = json!({
        "inputSourceConfig": [{
            "inputSourceARN": glue_table_arn("incremental"),
            "schemaName": "incremental",
        }],
        "outputSourceConfig": [{
            "outputS3Path": "s3://er-output/incremental",
            "output": [{ "name": "id" }],
        }],
        "resolutionTechniques": { "resolutionType": case.resolution_type },
        "roleArn": role_arn(),
    });
    if let Some(name) = workflow_name {
        body["workflowName"] = json!(name);
    }
    if let Some(incremental) = &case.incremental_run_config {
        body["incrementalRunConfig"] = incremental.clone();
    }
    body
}

/// A `ValidationException` for this rule must say which member cannot be carried and
/// which `resolutionType` value forbids it, so the caller can fix the request from it.
fn assert_names_the_conflict(case: &str, reply: &Reply) {
    let message = string_member(&reply.body, "message");
    for needle in ["incrementalRunConfig", "PROVIDER"] {
        assert!(
            message.contains(needle),
            "case {case:?}: the refusal must name {needle:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_incremental_run_config_is_refused_for_the_provider_resolution_type() {
    for case in incremental_cases() {
        let label = case.case.as_str();
        let refused = case.resolution_type == "PROVIDER" && case.incremental_run_config.is_some();
        let emulator = Emulator::new();
        seed_schemas(&emulator, label, &["incremental".to_string()]).await;

        // The CREATE path.
        let created = emulator
            .create_matching_workflow(&incremental_body(&case, Some("wf-incremental")))
            .await;
        if refused {
            assert_error(label, &created, 400, "ValidationException");
            assert_names_the_conflict(label, &created);
            let read = emulator.get_matching_workflow("wf-incremental").await;
            assert_error(label, &read, 404, "ResourceNotFoundException");
        } else {
            assert_eq!(
                created.status, 200,
                "case {label:?}: expected HTTP 200 from CreateMatchingWorkflow, got {created:?}"
            );
        }

        // The UPDATE path, over a workflow whose stored configuration is accepted by
        // the rule, so only the update under test can be refused.
        let baseline = IncrementalCase {
            case: format!("{label}: the baseline"),
            resolution_type: "ML_MATCHING",
            incremental_run_config: None,
        };
        let seeded = emulator
            .create_matching_workflow(&incremental_body(&baseline, Some("wf-baseline")))
            .await;
        assert_eq!(
            seeded.status, 200,
            "case {label:?}: expected HTTP 200 from CreateMatchingWorkflow, got {seeded:?}"
        );
        let before = emulator.get_matching_workflow("wf-baseline").await;

        sleep(A_LATER_MILLISECOND);
        let updated = emulator
            .update_matching_workflow("wf-baseline", &incremental_body(&case, None))
            .await;
        if refused {
            assert_error(label, &updated, 400, "ValidationException");
            assert_names_the_conflict(label, &updated);
            // A refused update writes nothing at all — not even the `updatedAt` stamp.
            let after = emulator.get_matching_workflow("wf-baseline").await;
            assert_eq!(
                after.body, before.body,
                "case {label:?}: a refused update must leave the workflow unchanged, \
                 got {:?}",
                after.body
            );
        } else {
            assert_eq!(
                updated.status, 200,
                "case {label:?}: expected HTTP 200 from UpdateMatchingWorkflow, got {updated:?}"
            );
            let after = emulator.get_matching_workflow("wf-baseline").await;
            assert_configuration(
                label,
                "GetMatchingWorkflow",
                &incremental_body(&case, None),
                &after.body,
            );
        }
    }
}

#[tokio::test]
async fn applying_the_same_update_twice_leaves_the_same_configuration() {
    let emulator = Emulator::new();

    for case in update_cases() {
        let label = case.case;
        let name = &case.workflow_name;
        seed_schemas(&emulator, label, &case.schemas).await;
        let created = emulator.create_matching_workflow(&case.create).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateMatchingWorkflow, got {created:?}"
        );

        let first = emulator.update_matching_workflow(name, &case.update).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateMatchingWorkflow, got {first:?}"
        );
        let once = emulator.get_matching_workflow(name).await;

        sleep(A_LATER_MILLISECOND);
        let second = emulator.update_matching_workflow(name, &case.update).await;
        assert_eq!(
            second.status, 200,
            "case {label:?}: expected HTTP 200 from a repeated UpdateMatchingWorkflow, \
             got {second:?}"
        );
        assert_eq!(
            second.body, first.body,
            "case {label:?}: the repeated update must answer identically, got {:?}",
            second.body
        );

        let twice = emulator.get_matching_workflow(name).await;
        assert_configuration(label, "GetMatchingWorkflow", &case.update, &twice.body);
        for member in ["workflowName", "workflowArn"] {
            assert_eq!(
                string_member(&twice.body, member),
                string_member(&once.body, member),
                "case {label:?}: {member} must survive a repeated update, got {:?}",
                twice.body
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
        assert!(
            timestamp_member(&twice.body, "updatedAt") >= timestamp_member(&once.body, "updatedAt"),
            "case {label:?}: expected updatedAt not to go backwards, got {:?}",
            twice.body
        );
    }
}
