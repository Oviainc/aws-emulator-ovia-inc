// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeleteIdMappingWorkflow`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `DELETE /idmappingworkflows/{workflowName}`, through the harness the
//! sibling `schema_mapping_workflow_lifecycle` suite owns — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: `DeleteIdMappingWorkflow` REMOVES the ID mapping workflow
//! stored under the requested name, so every reader of that resource stops serving it
//! (`GetIdMappingWorkflow` answers `ResourceNotFoundException`, HTTP 404,
//! `ListIdMappingWorkflows` stops listing it, and `ListTagsForResource` and `GetPolicy`
//! on its ARN answer the same 404), the name becomes creatable again as a fresh record,
//! and the room it held under the enforced "ID mapping workflows" quota comes back; a
//! name that is not stored is refused with `ResourceNotFoundException` (HTTP 404) and
//! changes nothing, which is also what a repeated delete answers.
//!
//! Each table row differs along what the rule generalises over rather than repeating
//! one recorded request: the whole modelled `IdMappingType` catalog (`RULE_BASED`,
//! `PROVIDER`), the `inputSourceConfig` size over the modelled `length 1..20` minimum,
//! a middle value and the maximum, which optional members the record carries
//! (`description`, `outputSourceConfig`, `incrementalRunConfig`, `roleArn`, `tags`) over
//! absent, present and empty, and the `workflowName` over the character classes the
//! modelled `EntityName` pattern `^[a-zA-Z_0-9-]*$` admits at its `length` minimum of 1
//! and maximum of 255. The recorded create/get/delete round trip's delete is ONE row of
//! these.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::id_mapping_workflow_lifecycle::{
    create_id_mapping_workflow, get_id_mapping_workflow, kms_arn, provider_service_arn, rule_based,
    ATTRIBUTE_MATCHING_MODELS, ID_MAPPING_TYPES, ID_NAMESPACE_TYPES, RECORD_MATCHING_MODELS,
    RULE_DEFINITION_TYPES,
};
use crate::id_mapping_workflow_listing::{list_id_mapping_workflows, page_names};
use crate::resource_policy::{get_policy, put_policy};
use crate::resource_tagging::list_tags;
use crate::schema_mapping_workflow_lifecycle::{
    assert_error_shape as assert_error, expected_arn, glue_table_arn, role_arn, string_member,
    timestamp_member, Emulator, Reply,
};

/// `DELETE /idmappingworkflows/{workflowName}` — `DeleteIdMappingWorkflow`'s modelled
/// HTTP binding
/// (`smithy_query(query="operation",operation="DeleteIdMappingWorkflow")`).
///
/// `pub(crate)` so the sibling suite that pins the TEXT of this delete's not-found
/// refusal drives the same path instead of cloning the helper.
pub(crate) async fn delete_id_mapping_workflow(emulator: &Emulator, workflow_name: &str) -> Reply {
    emulator
        .call("DELETE", &format!("/idmappingworkflows/{workflow_name}"), None)
        .await
}

/// `count` entries for `IdMappingWorkflowInputSourceConfig`, each a distinct source:
/// the ARN form alternates between the two the modelled `InputSourceARN` pattern admits
/// here (a Glue table and an Entity Resolution ID namespace) and the `type` over the
/// whole `IdNamespaceType` catalog.
fn input_sources(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                let namespace_type = ID_NAMESPACE_TYPES[index % ID_NAMESPACE_TYPES.len()];
                if index % 2 == 0 {
                    json!({
                        "inputSourceARN": glue_table_arn(&format!("table_{index}")),
                        "schemaName": format!("schema_{index}"),
                        "type": namespace_type,
                    })
                } else {
                    json!({
                        "inputSourceARN": expected_arn("idnamespace", &format!("ns_{index}")),
                        "type": namespace_type,
                    })
                }
            })
            .collect(),
    )
}

/// A provider-based `IdMappingTechniques`, the other branch of the modelled
/// `IdMappingType` catalog.
fn provider_based() -> Value {
    json!({
        "idMappingType": ID_MAPPING_TYPES[0],
        "providerProperties": {
            "providerServiceArn": provider_service_arn("testprovider", "assignment"),
        },
    })
}

/// One ID mapping workflow to create and then delete.
struct DeleteCase {
    case: &'static str,
    name: String,
    /// The `CreateIdMappingWorkflow` body for this row.
    create: Value,
}

/// The `DeleteIdMappingWorkflow` table.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "rule-based, the modelled inputSourceConfig minimum of one source, no \
                   optional member at all, a single-letter name at the modelled \
                   EntityName length minimum of 1",
            name: "a".to_string(),
            create: json!({
                "workflowName": "a",
                "inputSourceConfig": input_sources(1),
                "idMappingTechniques": rule_based(
                    RULE_DEFINITION_TYPES[0],
                    ATTRIBUTE_MATCHING_MODELS[0],
                    RECORD_MATCHING_MODELS[0],
                    Some(json!([{ "ruleName": "by email", "matchingKeys": ["email"] }])),
                ),
            }),
        },
        DeleteCase {
            case: "provider-based with every optional member present and two tags",
            name: "crm_people-1".to_string(),
            create: json!({
                "workflowName": "crm_people-1",
                "inputSourceConfig": input_sources(2),
                "idMappingTechniques": provider_based(),
                "description": "people mapped from the CRM",
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/crm",
                    "KMSArn": kms_arn(),
                }],
                "incrementalRunConfig": { "incrementalRunType": "ON_DEMAND" },
                "roleArn": role_arn(),
                "tags": { "team": "identity", "env": "test" },
            }),
        },
        DeleteCase {
            case: "rule-based TARGET / ONE_TO_ONE / MANY_SOURCE_TO_ONE_TARGET with the \
                   empty description, empty tags and the explicitly empty roleArn the \
                   modelled default admits",
            name: "DEVICES0".to_string(),
            create: json!({
                "workflowName": "DEVICES0",
                "inputSourceConfig": input_sources(3),
                "idMappingTechniques": rule_based(
                    RULE_DEFINITION_TYPES[1],
                    ATTRIBUTE_MATCHING_MODELS[1],
                    RECORD_MATCHING_MODELS[1],
                    None,
                ),
                "description": "",
                "roleArn": "",
                "tags": {},
            }),
        },
        DeleteCase {
            case: "the modelled inputSourceConfig maximum of 20 sources",
            name: "widest".to_string(),
            create: json!({
                "workflowName": "widest",
                "inputSourceConfig": input_sources(20),
                "idMappingTechniques": rule_based(
                    RULE_DEFINITION_TYPES[0],
                    ATTRIBUTE_MATCHING_MODELS[1],
                    RECORD_MATCHING_MODELS[0],
                    Some(json!([{ "ruleName": "wide", "matchingKeys": ["id"] }])),
                ),
                "description": "twenty sources",
                "roleArn": role_arn(),
                "tags": { "sources": "20" },
            }),
        },
        DeleteCase {
            case: "a digits-only name and provider-based techniques",
            name: "0".to_string(),
            create: json!({
                "workflowName": "0",
                "inputSourceConfig": input_sources(1),
                "idMappingTechniques": provider_based(),
                "outputSourceConfig": [{ "outputS3Path": "s3://er-output/digits" }],
            }),
        },
        DeleteCase {
            case: "a name at the modelled EntityName length maximum of 255, with the \
                   description at its maximum of 255",
            name: "w".repeat(255),
            create: json!({
                "workflowName": "w".repeat(255),
                "inputSourceConfig": input_sources(4),
                "idMappingTechniques": rule_based(
                    RULE_DEFINITION_TYPES[1],
                    ATTRIBUTE_MATCHING_MODELS[0],
                    RECORD_MATCHING_MODELS[1],
                    Some(json!([{ "ruleName": "longest", "matchingKeys": ["email"] }])),
                ),
                "description": "d".repeat(255),
                "incrementalRunConfig": { "incrementalRunType": "ON_DEMAND" },
                "tags": { "length": "255" },
            }),
        },
    ]
}

/// Create the workflow `case` describes, and answer the created record.
async fn seed(emulator: &Emulator, case: &DeleteCase) -> Reply {
    let created = create_id_mapping_workflow(emulator, &case.create).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateIdMappingWorkflow, got {created:?}",
        case.case
    );
    created
}

/// The `message` of a successful delete: modelled @required, so it is always there.
fn success_message(case: &str, deleted: &Reply) -> String {
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected HTTP 200 from DeleteIdMappingWorkflow, got {deleted:?}"
    );
    let message = string_member(&deleted.body, "message");
    assert!(
        !message.is_empty(),
        "case {case:?}: `message` is modelled @required, got {:?}",
        deleted.body
    );
    message
}

/// The `workflowName`s the listing currently serves.
async fn listed(emulator: &Emulator) -> BTreeSet<String> {
    let page = list_id_mapping_workflows(emulator, "").await;
    assert_eq!(page.status, 200, "the listing must answer, got {page:?}");
    page_names(&page).into_iter().collect()
}

#[tokio::test]
async fn a_deleted_workflow_is_gone_from_every_reader_and_a_repeated_delete_is_not_found() {
    let emulator = Emulator::new();

    for case in delete_cases() {
        let label = case.case;
        let name = &case.name;
        seed(&emulator, &case).await;

        // It is there before the delete, so what the readers answer afterwards is the
        // delete's doing.
        let before = get_id_mapping_workflow(&emulator, name).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: expected HTTP 200 from GetIdMappingWorkflow, got {before:?}"
        );
        assert!(
            listed(&emulator).await.contains(name),
            "case {label:?}: the created workflow must be listed before the delete"
        );

        let deleted = delete_id_mapping_workflow(&emulator, name).await;
        success_message(label, &deleted);

        let read = get_id_mapping_workflow(&emulator, name).await;
        assert_error(label, &read, 404, "ResourceNotFoundException");
        assert!(
            !listed(&emulator).await.contains(name),
            "case {label:?}: a deleted workflow must leave the listing"
        );

        // Only the HTTP 404 and the `ResourceNotFoundException` code are requirements
        // for the repeat: AWS documents no text for this refusal, so the message the
        // service answers is an example and is deliberately not asserted.
        let again = delete_id_mapping_workflow(&emulator, name).await;
        assert_error(label, &again, 404, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn deleting_a_workflow_that_was_never_created_is_not_found_and_creates_nothing() {
    let emulator = Emulator::new();

    for case in delete_cases() {
        let label = case.case;
        let name = &case.name;

        let refused = delete_id_mapping_workflow(&emulator, name).await;
        assert_error(label, &refused, 404, "ResourceNotFoundException");

        // A refused delete is not a create: the name is still absent from both readers.
        let read = get_id_mapping_workflow(&emulator, name).await;
        assert_error(label, &read, 404, "ResourceNotFoundException");
        assert!(
            !listed(&emulator).await.contains(name),
            "case {label:?}: a refused delete must create nothing"
        );

        // ...and once the workflow exists, the SAME request succeeds, so the 404 is
        // decided by what the store holds and not by the request's shape.
        seed(&emulator, &case).await;
        let deleted = delete_id_mapping_workflow(&emulator, name).await;
        success_message(label, &deleted);
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_id_mapping_workflow_it_names() {
    let emulator = Emulator::new();
    let cases = delete_cases();

    for case in &cases {
        seed(&emulator, case).await;
    }

    let mut still_there: Vec<&DeleteCase> = cases.iter().collect();
    while let Some(case) = still_there.pop() {
        let label = case.case;
        let untouched: Vec<(String, Value)> = {
            let mut records = Vec::new();
            for other in &still_there {
                let read = get_id_mapping_workflow(&emulator, &other.name).await;
                assert_eq!(
                    read.status, 200,
                    "case {label:?}: {:?} must still be there, got {read:?}",
                    other.name
                );
                records.push((other.name.clone(), read.body));
            }
            records
        };

        let deleted = delete_id_mapping_workflow(&emulator, &case.name).await;
        success_message(label, &deleted);

        for (name, body) in untouched {
            let read = get_id_mapping_workflow(&emulator, &name).await;
            assert_eq!(
                read.body, body,
                "case {label:?}: deleting {:?} must leave {name:?} exactly as it was, got \
                 {:?} after {body:?}",
                case.name, read.body
            );
        }
    }
}

#[tokio::test]
async fn a_matching_workflow_of_the_same_name_survives_the_delete() {
    let emulator = Emulator::new();
    // Every matching workflow below references this schema mapping, and a
    // matching-workflow write resolves that reference, so it is created once up front.
    emulator.seed_schema_mapping("customers").await;

    for case in delete_cases() {
        let label = case.case;
        let name = &case.name;
        seed(&emulator, &case).await;

        // The two kinds are separate namespaces, so a matching workflow may carry the
        // same name — and this delete is about the ID mapping one only.
        let matching = json!({
            "workflowName": name,
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
        let created_matching = emulator.create_matching_workflow(&matching).await;
        assert_eq!(
            created_matching.status, 200,
            "case {label:?}: expected HTTP 200 from CreateMatchingWorkflow, got \
             {created_matching:?}"
        );
        let matching_before = emulator.get_matching_workflow(name).await;

        let deleted = delete_id_mapping_workflow(&emulator, name).await;
        success_message(label, &deleted);

        let matching_after = emulator.get_matching_workflow(name).await;
        assert_eq!(
            matching_after.body, matching_before.body,
            "case {label:?}: the matching workflow of the same name must be untouched, got \
             {:?} after {:?}",
            matching_after.body, matching_before.body
        );
        assert_error(
            label,
            &get_id_mapping_workflow(&emulator, name).await,
            404,
            "ResourceNotFoundException",
        );
    }
}

#[tokio::test]
async fn the_success_message_carries_nothing_of_the_record_that_was_removed() {
    let emulator = Emulator::new();
    let mut seen = BTreeSet::new();

    for case in delete_cases() {
        let label = case.case;
        seed(&emulator, &case).await;
        let deleted = delete_id_mapping_workflow(&emulator, &case.name).await;
        seen.insert(success_message(label, &deleted));
    }

    assert_eq!(
        seen.len(),
        1,
        "the success message must not depend on the workflow that was deleted, saw {seen:?}"
    );
}

#[tokio::test]
async fn a_deleted_name_is_creatable_again_as_a_fresh_record() {
    let emulator = Emulator::new();

    for case in delete_cases() {
        let label = case.case;
        let name = &case.name;
        seed(&emulator, &case).await;
        // `CreateIdMappingWorkflowOutput` models no stamps, so the first incarnation's
        // `createdAt` is read through the reader that does model it.
        let first = get_id_mapping_workflow(&emulator, name).await;
        let first_created_at = timestamp_member(&first.body, "createdAt");
        let deleted = delete_id_mapping_workflow(&emulator, name).await;
        success_message(label, &deleted);

        // The name is free again, so the create that would have conflicted now goes
        // through — and the record it makes is its OWN: the deleted workflow's
        // description, output config, incremental run config, role and tags are not
        // inherited.
        let minimal = json!({
            "workflowName": name,
            "inputSourceConfig": input_sources(1),
            "idMappingTechniques": provider_based(),
        });
        let recreated = create_id_mapping_workflow(&emulator, &minimal).await;
        assert_eq!(
            recreated.status, 200,
            "case {label:?}: a deleted name must be creatable again, got {recreated:?}"
        );
        let read = get_id_mapping_workflow(&emulator, name).await;
        for member in [
            "description",
            "outputSourceConfig",
            "incrementalRunConfig",
            "tags",
        ] {
            assert!(
                read.body.get(member).is_none(),
                "case {label:?}: the re-created workflow must not inherit {member}, got {:?}",
                read.body
            );
        }
        assert_eq!(
            read.body.get("roleArn").and_then(Value::as_str).unwrap_or(""),
            "",
            "case {label:?}: the re-created workflow carries the modelled default roleArn, \
             got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("idMappingTechniques"),
            Some(&provider_based()),
            "case {label:?}: the re-created workflow must carry its OWN techniques, got {:?}",
            read.body
        );
        assert!(
            timestamp_member(&read.body, "createdAt") >= first_created_at,
            "case {label:?}: the re-created workflow is a later incarnation, got {:?} after \
             a createdAt of {first_created_at}",
            read.body
        );
    }
}

#[tokio::test]
async fn the_room_a_delete_releases_comes_back_under_the_enforced_quota() {
    let emulator = Emulator::new();

    // The bound this emulator enforces for the published, adjustable per-account,
    // per-Region "ID mapping workflows" quota (`crate::state::MAX_ID_MAPPING_WORKFLOWS`),
    // pinned here as a literal so the suite checks the behaviour and not the constant.
    const MAX_ID_MAPPING_WORKFLOWS: usize = 100;

    let quota_workflow = |name: String| {
        json!({
            "workflowName": name,
            "inputSourceConfig": input_sources(1),
            "idMappingTechniques": provider_based(),
        })
    };

    for index in 0..MAX_ID_MAPPING_WORKFLOWS {
        let created =
            create_id_mapping_workflow(&emulator, &quota_workflow(format!("quota-{index}"))).await;
        assert_eq!(
            created.status, 200,
            "workflow {index} is inside the quota, so it must be created, got {created:?}"
        );
    }

    let refused =
        create_id_mapping_workflow(&emulator, &quota_workflow("one-too-many".to_string())).await;
    assert_error("a create at the quota", &refused, 402, "ExceedsLimitException");

    // Deleting one releases exactly one slot: the next create goes through, and the one
    // after it is refused again.
    let deleted = delete_id_mapping_workflow(&emulator, "quota-7").await;
    success_message("a delete that releases quota room", &deleted);

    let admitted =
        create_id_mapping_workflow(&emulator, &quota_workflow("one-too-many".to_string())).await;
    assert_eq!(
        admitted.status, 200,
        "the room the delete released must admit one create, got {admitted:?}"
    );
    let refused_again =
        create_id_mapping_workflow(&emulator, &quota_workflow("two-too-many".to_string())).await;
    assert_error(
        "a create past the refilled quota",
        &refused_again,
        402,
        "ExceedsLimitException",
    );
}

#[tokio::test]
async fn the_delete_label_is_a_name_so_an_arn_names_no_workflow() {
    let emulator = Emulator::new();

    for case in delete_cases() {
        let label = case.case;
        let created = seed(&emulator, &case).await;

        // `DeleteIdMappingWorkflow`'s `workflowName` is an `EntityName`
        // (`^[a-zA-Z_0-9-]*$`), which admits neither `:` nor `/`, so an ARN is refused
        // at the boundary with the modelled `ValidationException` (HTTP 400).
        let arn = string_member(&created.body, "workflowArn");
        let refused = delete_id_mapping_workflow(&emulator, &arn.replace('/', "%2F")).await;
        assert_error(label, &refused, 400, "ValidationException");

        // The refused call removed nothing: the workflow still reads as it was created.
        let read = get_id_mapping_workflow(&emulator, &case.name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the refused delete must leave the stored record alone, got \
             {read:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_workflow_takes_its_tags_and_its_resource_policy_with_it() {
    let emulator = Emulator::new();
    // A minimal, well-formed policy document; this suite is about the delete, not about
    // what a document may contain (see the `resource_policy` suite for that).
    let document = r#"{"Version":"2012-10-17","Statement":[]}"#;

    for case in delete_cases() {
        let label = case.case;
        let created = seed(&emulator, &case).await;
        let arn = string_member(&created.body, "workflowArn");

        let written = put_policy(&emulator, &arn, document, None).await;
        assert_eq!(
            written.status, 200,
            "case {label:?}: expected HTTP 200 from PutPolicy, got {written:?}"
        );
        let token = string_member(&written.body, "token");
        let tags_before = list_tags(&emulator, &arn).await;
        assert_eq!(
            tags_before.status, 200,
            "case {label:?}: expected HTTP 200 from ListTagsForResource, got {tags_before:?}"
        );
        let policy_before = get_policy(&emulator, &arn).await;
        assert_eq!(
            policy_before.status, 200,
            "case {label:?}: expected HTTP 200 from GetPolicy, got {policy_before:?}"
        );

        let deleted = delete_id_mapping_workflow(&emulator, &case.name).await;
        success_message(label, &deleted);

        // Tags live on the record, and a resource-based policy belongs to the resource
        // it is attached to: with the workflow gone, neither is served.
        assert_error(
            label,
            &list_tags(&emulator, &arn).await,
            404,
            "ResourceNotFoundException",
        );
        assert_error(
            label,
            &get_policy(&emulator, &arn).await,
            404,
            "ResourceNotFoundException",
        );

        // A workflow created again under the same name is a NEW resource with the same
        // ARN, so it carries no policy and the deleted one's revision token is not the
        // current one.
        let recreated = create_id_mapping_workflow(
            &emulator,
            &json!({
                "workflowName": case.name,
                "inputSourceConfig": input_sources(1),
                "idMappingTechniques": provider_based(),
            }),
        )
        .await;
        assert_eq!(
            recreated.status, 200,
            "case {label:?}: expected HTTP 200 re-creating the workflow, got {recreated:?}"
        );
        assert_eq!(
            string_member(&recreated.body, "workflowArn"),
            expected_arn("idmappingworkflow", &case.name),
            "case {label:?}: the re-created workflow has the same ARN, got {:?}",
            recreated.body
        );
        assert_error(
            label,
            &get_policy(&emulator, &arn).await,
            404,
            "ResourceNotFoundException",
        );
        assert_error(
            label,
            &put_policy(&emulator, &arn, document, Some(&token)).await,
            400,
            "ConflictException",
        );
    }
}
