// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Entity Resolution ID-mapping-workflow create/read
//! loop.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the sibling `schema_mapping_workflow_lifecycle` suite owns — so what is
//! asserted is what an AWS SDK client would see, not an internal function's return
//! value.
//!
//! The rule under test: `CreateIdMappingWorkflow` mints the resource's ARN, PERSISTS
//! the whole configuration the request carried together with its `createdAt`/
//! `updatedAt` stamps, and `GetIdMappingWorkflow` serves that stored record
//! field-for-field; the `workflowName` is the resource's unique key, so a second
//! create under the same name is refused and changes nothing, and a name nothing was
//! created under reads as not-found.
//!
//! Each table row differs along what the rule generalises over rather than repeating
//! one recorded request: the `idMappingType` over the whole modelled catalog, the
//! rule-based enums over their whole catalogs, the number of input sources over the
//! modelled `length` minimum, a middle value and the maximum, the number of rules over
//! its modelled maximum, the `workflowName` over every character class the modelled
//! pattern admits at both length bounds, and every optional member over absent,
//! present and empty.

use serde_json::{json, Map, Value};

use crate::schema_mapping_workflow_lifecycle::{
    account_id, expected_arn, glue_table_arn, role_arn, string_member, timestamp_member, Emulator,
    Reply, REGION,
};

/// `POST /idmappingworkflows` — `CreateIdMappingWorkflow`'s modelled HTTP binding
/// (`smithy_query(query="operation",operation="CreateIdMappingWorkflow")`).
pub(crate) async fn create_id_mapping_workflow(emulator: &Emulator, request: &Value) -> Reply {
    emulator
        .call("POST", "/idmappingworkflows", Some(request))
        .await
}

/// `GET /idmappingworkflows/{workflowName}` — `GetIdMappingWorkflow`'s modelled HTTP
/// binding.
pub(crate) async fn get_id_mapping_workflow(emulator: &Emulator, workflow_name: &str) -> Reply {
    emulator
        .call("GET", &format!("/idmappingworkflows/{workflow_name}"), None)
        .await
}

/// The values `IdMappingType` accepts on the wire, from
/// `smithy_query(query="shape",shape="IdMappingType")`. Every entry is exercised by
/// the table below. `pub(crate)` so the sibling ID-namespace suite spans the SAME
/// catalog from this one declaration.
pub(crate) const ID_MAPPING_TYPES: &[&str] = &["PROVIDER", "RULE_BASED"];

/// The values `IdMappingWorkflowRuleDefinitionType` accepts on the wire, from
/// `smithy_query(query="shape",shape="IdMappingRuleBasedProperties")`.
pub(crate) const RULE_DEFINITION_TYPES: &[&str] = &["SOURCE", "TARGET"];

/// The values `AttributeMatchingModel` accepts on the wire, same source.
pub(crate) const ATTRIBUTE_MATCHING_MODELS: &[&str] = &["MANY_TO_MANY", "ONE_TO_ONE"];

/// The values `RecordMatchingModel` accepts on the wire, from
/// `smithy_query(query="shape",shape="RecordMatchingModel")`.
pub(crate) const RECORD_MATCHING_MODELS: &[&str] =
    &["ONE_SOURCE_TO_ONE_TARGET", "MANY_SOURCE_TO_ONE_TARGET"];

/// The values `IdNamespaceType` accepts on `IdMappingWorkflowInputSource$type`, from
/// `smithy_query(query="shape",shape="IdMappingWorkflowInputSource")`. `pub(crate)`
/// so the sibling ID-namespace suite spans the SAME catalog from this one declaration.
pub(crate) const ID_NAMESPACE_TYPES: &[&str] = &["SOURCE", "TARGET"];

/// A `ProviderServiceArn`, which is modelled with an EMPTY account segment:
/// `^arn:(aws|…):(entityresolution):(<region>)::providerservice/<provider>/<service>$`
/// (`smithy_query(query="shape",shape="ProviderProperties")`). `pub(crate)` so the
/// sibling ID-namespace suite builds the same ARN from this one helper.
pub(crate) fn provider_service_arn(provider: &str, service: &str) -> String {
    format!("arn:aws:entityresolution:{REGION}::providerservice/{provider}/{service}")
}

/// A customer KMS key ARN for `IdMappingWorkflowOutputSource$KMSArn`, whose modelled
/// pattern is `^arn:aws:kms:.*:[0-9]+:.*$`.
pub(crate) fn kms_arn() -> String {
    format!("arn:aws:kms:{REGION}:{}:key/er-output", account_id())
}

/// One `CreateIdMappingWorkflow` request, held as its PIECES rather than as a literal
/// body: the request is assembled from the same values the assertions expect, so the
/// two cannot drift.
struct WorkflowCase {
    case: &'static str,
    workflow_name: String,
    /// `IdMappingWorkflowInputSourceConfig`, modelled `length 1..20` and @required.
    input_source_config: Value,
    /// `IdMappingTechniques`, @required.
    id_mapping_techniques: Value,
    /// `description` is optional on both the create and the read, so absent stays
    /// absent.
    description: Option<String>,
    /// `outputSourceConfig` is optional with NO modelled default (`length 1..1`).
    output_source_config: Option<Value>,
    /// `incrementalRunConfig` is optional with no modelled default.
    incremental_run_config: Option<Value>,
    /// What the request sends for `roleArn`. The member is modelled with
    /// `default: ""`, so `None` here means "the caller sent none" and the service must
    /// answer the empty string for it.
    role_arn: Option<String>,
    /// `tags` is input-only on the create; only `GetIdMappingWorkflow` models it back.
    tags: Option<Value>,
}

impl WorkflowCase {
    /// The request body, assembled so an absent optional member really is absent.
    fn request(&self) -> Value {
        let mut request = Map::new();
        request.insert("workflowName".to_string(), json!(self.workflow_name));
        request.insert(
            "inputSourceConfig".to_string(),
            self.input_source_config.clone(),
        );
        request.insert(
            "idMappingTechniques".to_string(),
            self.id_mapping_techniques.clone(),
        );
        if let Some(description) = &self.description {
            request.insert("description".to_string(), json!(description));
        }
        if let Some(output_source_config) = &self.output_source_config {
            request.insert(
                "outputSourceConfig".to_string(),
                output_source_config.clone(),
            );
        }
        if let Some(incremental_run_config) = &self.incremental_run_config {
            request.insert(
                "incrementalRunConfig".to_string(),
                incremental_run_config.clone(),
            );
        }
        if let Some(role_arn) = &self.role_arn {
            request.insert("roleArn".to_string(), json!(role_arn));
        }
        if let Some(tags) = &self.tags {
            request.insert("tags".to_string(), tags.clone());
        }
        Value::Object(request)
    }

    /// The `roleArn` the service must answer: what the request carried, or the
    /// modelled `default: ""` when it carried none.
    fn served_role_arn(&self) -> &str {
        self.role_arn.as_deref().unwrap_or("")
    }
}

/// A rule-based `IdMappingTechniques`, with each enum taken from its own catalog.
pub(crate) fn rule_based(
    rule_definition_type: &str,
    attribute_matching_model: &str,
    record_matching_model: &str,
    rules: Option<Value>,
) -> Value {
    let mut properties = Map::new();
    properties.insert(
        "ruleDefinitionType".to_string(),
        json!(rule_definition_type),
    );
    properties.insert(
        "attributeMatchingModel".to_string(),
        json!(attribute_matching_model),
    );
    properties.insert(
        "recordMatchingModel".to_string(),
        json!(record_matching_model),
    );
    if let Some(rules) = rules {
        properties.insert("rules".to_string(), rules);
    }
    json!({ "idMappingType": "RULE_BASED", "ruleBasedProperties": Value::Object(properties) })
}

/// The table. Rows span the modelled catalogs and bounds described in the module doc.
fn workflow_cases() -> Vec<WorkflowCase> {
    // The modelled `IdMappingWorkflowInputSourceConfig` maximum of 20 sources, built
    // so each entry is a distinct source rather than one row repeated: the ARN form
    // alternates between the two the modelled `InputSourceARN` pattern admits here
    // (a Glue table and an Entity Resolution ID namespace) and the `type` alternates
    // over the whole `IdNamespaceType` catalog.
    let widest: Vec<Value> = (0..20)
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
        .collect();

    // The modelled `RuleList` maximum of 25 rules, each with its own matching keys.
    let many_rules: Vec<Value> = (0..25)
        .map(|index| {
            json!({
                "ruleName": format!("rule {index}"),
                "matchingKeys": [format!("key_{index}"), "email"],
            })
        })
        .collect();

    vec![
        WorkflowCase {
            case: "rule-based, the modelled minimum of one input source, no optional members",
            workflow_name: "customers".to_string(),
            input_source_config: json!([{
                "inputSourceARN": glue_table_arn("customers"),
                "schemaName": "customers",
                "type": ID_NAMESPACE_TYPES[0],
            }]),
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
            tags: None,
        },
        WorkflowCase {
            case: "provider-based with every optional member present",
            workflow_name: "crm_people-1".to_string(),
            input_source_config: json!([
                {
                    "inputSourceARN": expected_arn("idnamespace", "crm-source"),
                    "type": ID_NAMESPACE_TYPES[0],
                },
                {
                    "inputSourceARN": expected_arn("idnamespace", "crm-target"),
                    "schemaName": "crm_people-1",
                    "type": ID_NAMESPACE_TYPES[1],
                },
            ]),
            id_mapping_techniques: json!({
                "idMappingType": ID_MAPPING_TYPES[0],
                "providerProperties": {
                    "providerServiceArn": provider_service_arn("testprovider", "assignment"),
                    "providerConfiguration": {
                        "targetIdentifier": "unique_id",
                        "attributes": ["email", "phone"],
                        "maximumMatches": 7,
                        "strict": true,
                    },
                    "intermediateSourceConfiguration": {
                        "intermediateS3Path": "s3://er-intermediate/crm",
                    },
                },
            }),
            description: Some("people mapped from the CRM".to_string()),
            output_source_config: Some(json!([{
                "outputS3Path": "s3://er-output/crm",
                "KMSArn": kms_arn(),
            }])),
            incremental_run_config: Some(json!({ "incrementalRunType": "ON_DEMAND" })),
            role_arn: Some(role_arn()),
            tags: Some(json!({ "team": "identity", "env": "test" })),
        },
        WorkflowCase {
            case: "rule-based TARGET / ONE_TO_ONE / MANY_SOURCE_TO_ONE_TARGET, with the \
                   empty description, empty tags and the explicitly empty roleArn the \
                   modelled default admits",
            workflow_name: "DEVICES0".to_string(),
            input_source_config: json!([{
                "inputSourceARN": glue_table_arn("devices"),
            }]),
            id_mapping_techniques: rule_based(
                RULE_DEFINITION_TYPES[1],
                ATTRIBUTE_MATCHING_MODELS[1],
                RECORD_MATCHING_MODELS[1],
                None,
            ),
            description: Some(String::new()),
            output_source_config: Some(json!([{ "outputS3Path": "s3://er-output/devices" }])),
            incremental_run_config: Some(json!({})),
            role_arn: Some(String::new()),
            tags: Some(json!({})),
        },
        WorkflowCase {
            case: "the modelled maximum of 20 input sources",
            workflow_name: "widest".to_string(),
            input_source_config: Value::Array(widest),
            id_mapping_techniques: rule_based(
                RULE_DEFINITION_TYPES[0],
                ATTRIBUTE_MATCHING_MODELS[1],
                RECORD_MATCHING_MODELS[0],
                Some(json!([{ "ruleName": "wide", "matchingKeys": ["id"] }])),
            ),
            description: Some("twenty sources".to_string()),
            output_source_config: None,
            incremental_run_config: None,
            role_arn: Some(role_arn()),
            tags: Some(json!({ "sources": "20" })),
        },
        WorkflowCase {
            case: "a one-character workflowName at the modelled length minimum, with the \
                   modelled maximum of 25 rules",
            workflow_name: "a".to_string(),
            input_source_config: json!([{
                "inputSourceARN": glue_table_arn("a"),
                "schemaName": "a",
                "type": ID_NAMESPACE_TYPES[1],
            }]),
            id_mapping_techniques: rule_based(
                RULE_DEFINITION_TYPES[1],
                ATTRIBUTE_MATCHING_MODELS[0],
                RECORD_MATCHING_MODELS[1],
                Some(Value::Array(many_rules)),
            ),
            description: None,
            output_source_config: None,
            incremental_run_config: None,
            role_arn: None,
            tags: None,
        },
        WorkflowCase {
            case: "workflowName at the modelled length maximum of 255 and description at \
                   its maximum of 255",
            workflow_name: "w".repeat(255),
            input_source_config: json!([{
                "inputSourceARN": glue_table_arn("widest_name"),
                "schemaName": "s".repeat(255),
            }]),
            id_mapping_techniques: rule_based(
                RULE_DEFINITION_TYPES[0],
                ATTRIBUTE_MATCHING_MODELS[1],
                RECORD_MATCHING_MODELS[1],
                None,
            ),
            description: Some("d".repeat(255)),
            output_source_config: Some(json!([{
                "outputS3Path": "s3://er-output/longest",
                "KMSArn": kms_arn(),
            }])),
            incremental_run_config: Some(json!({ "incrementalRunType": "ON_DEMAND" })),
            role_arn: Some(String::new()),
            tags: Some(json!({ "length": "255" })),
        },
    ]
}

/// The members `CreateIdMappingWorkflow` and `GetIdMappingWorkflow` both model, which
/// must therefore be identical: both are served from the one stored record.
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

/// Assert the configuration members of `body` are the ones `case` asked for.
fn assert_configuration(label: &str, source: &str, case: &WorkflowCase, body: &Value) {
    assert_eq!(
        string_member(body, "workflowName"),
        case.workflow_name,
        "case {label:?}: {source} must answer the stored workflowName, got {body:?}"
    );
    assert_eq!(
        string_member(body, "workflowArn"),
        expected_arn("idmappingworkflow", &case.workflow_name),
        "case {label:?}: {source} must answer the generated idmappingworkflow ARN, got {body:?}"
    );
    assert_eq!(
        body.get("description").and_then(Value::as_str),
        case.description.as_deref(),
        "case {label:?}: {source} must answer the stored description, got {body:?}"
    );
    assert_eq!(
        body.get("inputSourceConfig"),
        Some(&case.input_source_config),
        "case {label:?}: {source} must answer the stored inputSourceConfig, got {body:?}"
    );
    assert_eq!(
        body.get("outputSourceConfig"),
        case.output_source_config.as_ref(),
        "case {label:?}: {source} must answer the stored outputSourceConfig, got {body:?}"
    );
    assert_eq!(
        body.get("idMappingTechniques"),
        Some(&case.id_mapping_techniques),
        "case {label:?}: {source} must answer the stored idMappingTechniques, got {body:?}"
    );
    assert_eq!(
        body.get("incrementalRunConfig"),
        case.incremental_run_config.as_ref(),
        "case {label:?}: {source} must answer the stored incrementalRunConfig, got {body:?}"
    );
    assert_eq!(
        body.get("roleArn").and_then(Value::as_str).unwrap_or(""),
        case.served_role_arn(),
        "case {label:?}: {source} must answer the stored roleArn, got {body:?}"
    );
}

#[tokio::test]
async fn create_id_mapping_workflow_persists_a_record_get_id_mapping_workflow_reads_it_back() {
    let emulator = Emulator::new();

    for case in workflow_cases() {
        let label = case.case;
        let name = &case.workflow_name;

        let created = create_id_mapping_workflow(&emulator, &case.request()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateIdMappingWorkflow, got {created:?}"
        );
        assert_configuration(label, "CreateIdMappingWorkflow", &case, &created.body);
        // `CreateIdMappingWorkflowOutput` models neither the stamps nor the tags, so
        // neither may appear on the create response.
        for absent in ["createdAt", "updatedAt", "tags"] {
            assert!(
                created.body.get(absent).is_none(),
                "case {label:?}: CreateIdMappingWorkflowOutput models no {absent}, got {:?}",
                created.body
            );
        }

        // The round trip: the reader clones what the writer stored.
        let read = get_id_mapping_workflow(&emulator, name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected HTTP 200 from GetIdMappingWorkflow, got {read:?}"
        );
        assert_configuration(label, "GetIdMappingWorkflow", &case, &read.body);
        assert_eq!(
            read.body.get("tags"),
            case.tags.as_ref(),
            "case {label:?}: GetIdMappingWorkflow must answer the stored tags, got {:?}",
            read.body
        );

        // Both responses come from the one stored record, so every member the two
        // operations share is the same value.
        for member in SHARED_MEMBERS {
            assert_eq!(
                created.body.get(member),
                read.body.get(member),
                "case {label:?}: member {member:?} differs between create and read: {:?} vs {:?}",
                created.body,
                read.body
            );
        }

        // A freshly created workflow has not been updated since, and both stamps are
        // stable across reads (nothing is re-derived per call).
        let created_at = timestamp_member(&read.body, "createdAt");
        let updated_at = timestamp_member(&read.body, "updatedAt");
        assert!(
            created_at > 0.0,
            "case {label:?}: expected a real createdAt, got {created_at}"
        );
        assert_eq!(
            created_at, updated_at,
            "case {label:?}: a workflow never updated must carry equal stamps, got \
             {created_at} and {updated_at}"
        );

        // A read changes nothing a later caller sees.
        let again = get_id_mapping_workflow(&emulator, name).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: two consecutive reads must answer identically, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_duplicate_workflow_name_conflicts_and_leaves_the_stored_record_untouched() {
    let emulator = Emulator::new();

    for case in workflow_cases() {
        let label = case.case;
        let request = case.request();
        let created = create_id_mapping_workflow(&emulator, &request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from the first create, got {created:?}"
        );

        let before = get_id_mapping_workflow(&emulator, &case.workflow_name).await;
        let duplicate = create_id_mapping_workflow(&emulator, &request).await;
        assert_eq!(
            duplicate.status, 400,
            "case {label:?}: a duplicate workflowName must be refused, got {duplicate:?}"
        );
        assert_eq!(
            duplicate.error_code.as_deref(),
            Some("ConflictException"),
            "case {label:?}: expected ConflictException, got {:?}",
            duplicate.error_code
        );

        let after = get_id_mapping_workflow(&emulator, &case.workflow_name).await;
        assert_eq!(
            after.body, before.body,
            "case {label:?}: a refused create must leave the stored record untouched, got \
             {after:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_workflow_name_reads_as_not_found() {
    let emulator = Emulator::new();

    // One workflow exists, so a miss is a real lookup miss and not an empty store.
    let seed = &workflow_cases()[0];
    let created = create_id_mapping_workflow(&emulator, &seed.request()).await;
    assert_eq!(
        created.status, 200,
        "seeding the store failed: {created:?}"
    );

    // Names spanning what the modelled `EntityName` pattern `^[a-zA-Z_0-9-]*$` admits,
    // at both its length bounds — none of them created.
    let missing = [
        "z".to_string(),
        "MISSING".to_string(),
        "no_such-workflow-0".to_string(),
        "m".repeat(255),
        // The name of a MATCHING workflow is a different namespace, so creating one
        // under this name must not make the ID mapping reader find it.
        "shared-name".to_string(),
    ];
    emulator.seed_schema_mapping("customers").await;
    let matching = json!({
        "workflowName": "shared-name",
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

    for name in &missing {
        let reply = get_id_mapping_workflow(&emulator, name).await;
        assert_eq!(
            reply.status, 404,
            "case {name:?}: an uncreated workflow must read as not found, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_code
        );
    }
}

#[tokio::test]
async fn an_id_mapping_workflow_and_a_matching_workflow_may_share_a_name() {
    let emulator = Emulator::new();
    let name = "shared-name";

    let id_mapping = WorkflowCase {
        case: "a name used by both workflow kinds",
        workflow_name: name.to_string(),
        input_source_config: json!([{
            "inputSourceARN": expected_arn("idnamespace", "source"),
            "type": ID_NAMESPACE_TYPES[0],
        }]),
        id_mapping_techniques: rule_based(
            RULE_DEFINITION_TYPES[0],
            ATTRIBUTE_MATCHING_MODELS[0],
            RECORD_MATCHING_MODELS[0],
            None,
        ),
        description: Some("the ID mapping one".to_string()),
        output_source_config: None,
        incremental_run_config: None,
        role_arn: Some(role_arn()),
        tags: None,
    };
    let matching = json!({
        "workflowName": name,
        "description": "the matching one",
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

    emulator.seed_schema_mapping("customers").await;
    let created_matching = emulator.create_matching_workflow(&matching).await;
    assert_eq!(
        created_matching.status, 200,
        "expected HTTP 200 from CreateMatchingWorkflow, got {created_matching:?}"
    );
    let created_id_mapping = create_id_mapping_workflow(&emulator, &id_mapping.request()).await;    assert_eq!(
        created_id_mapping.status, 200,
        "the two kinds are separate namespaces, so this create must succeed, got \
         {created_id_mapping:?}"
    );

    let read_matching = emulator.get_matching_workflow(name).await;
    let read_id_mapping = get_id_mapping_workflow(&emulator, name).await;
    assert_eq!(
        string_member(&read_matching.body, "workflowArn"),
        expected_arn("matchingworkflow", name),
        "the matching workflow must keep its own ARN, got {:?}",
        read_matching.body
    );
    assert_eq!(
        string_member(&read_id_mapping.body, "workflowArn"),
        expected_arn("idmappingworkflow", name),
        "the ID mapping workflow must keep its own ARN, got {:?}",
        read_id_mapping.body
    );
    assert_configuration(
        id_mapping.case,
        "GetIdMappingWorkflow",
        &id_mapping,
        &read_id_mapping.body,
    );
    assert_eq!(
        read_matching.body.get("description").and_then(Value::as_str),
        Some("the matching one"),
        "the matching workflow's own record must be untouched, got {:?}",
        read_matching.body
    );
}
