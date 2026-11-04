// SPDX-License-Identifier: CC0-1.0

//! The TEXT Entity Resolution puts in a response `message`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the sibling `schema_mapping_workflow_lifecycle` suite owns — so what is
//! asserted is the body an AWS SDK client would parse.
//!
//! The rule under test: each message this service returns — the @required `message`
//! member of a `Delete*` output, and the `message` of a `ResourceNotFoundException`
//! or `ConflictException` — is that OPERATION'S OWN text, naming the resource TYPE as
//! the API spells it (`SchemaMapping`, `MatchingWorkflow`, `IdNamespace`) and
//! substituting a name only where that operation's template includes one: the requested
//! `@httpLabel` for the `Delete*` not-found texts, and the REFERENCING workflow read out of
//! the store for the in-use conflict, which is what tells a caller which workflow blocks
//! the delete. For a path whose text carries no name it is INVARIANT over the name, the
//! size and the shape of the resource; and the wording is per-operation, so
//! `GetMatchingWorkflow`'s not-found text and `DeleteMatchingWorkflow`'s are different
//! strings, and two `Delete*` success texts need not even share their capitalization.
//!
//! Each table row differs along what the rule generalises over: the resource name
//! (every character class the modelled `EntityName` pattern `^[a-zA-Z_0-9-]*$`
//! admits, at the modelled `length` minimum of 1 and maximum of 255) and the size of
//! the record the call touches (the modelled minimum of two mapped input fields
//! through a wide one, one input source through several). The recorded request is one
//! row of these.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::schema_mapping_workflow_lifecycle::{
    assert_error_shape, glue_table_arn, role_arn, string_member, Emulator, Reply,
};

/// The message AWS returns for each path, as a literal here so this suite pins the
/// text independently of the constants the service crate declares.
const SCHEMA_MAPPING_DELETED: &str = "SchemaMapping Deleted.";
const SCHEMA_MAPPING_NOT_FOUND: &str = "SchemaMapping does not exist";
const SCHEMA_MAPPING_ALREADY_EXISTS: &str = "SchemaMapping already exists.";
const MATCHING_WORKFLOW_DELETED: &str = "MatchingWorkflow Deleted.";
const MATCHING_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";
const MATCHING_WORKFLOW_ALREADY_EXISTS: &str = "MatchingWorkflow already exists.";
/// The not-found text the ID-namespace READERS answer, as the recorded corpus shows
/// it: a CONSTANT that names no namespace, in the same `<ResourceType> Not Found` form
/// as [`MATCHING_WORKFLOW_NOT_FOUND`]. The DELETE path is worded differently (see
/// [`delete_id_namespace_not_found`]).
const ID_NAMESPACE_NOT_FOUND: &str = "IdNamespace Not Found";
/// `DeleteIdNamespace`'s success text, as AWS words it — `deleted` lowercase, which is
/// not the casing of [`SCHEMA_MAPPING_DELETED`].
const ID_NAMESPACE_DELETED: &str = "IdNamespace deleted.";

/// A DELETE path's text is a template over the request: the `@httpLabel` the caller
/// asked for is substituted into it.
fn delete_matching_workflow_not_found(workflow_name: &str) -> String {
    format!("MatchingWorkflow {workflow_name} does not exist")
}

/// The same template for the ID-namespace delete.
fn delete_id_namespace_not_found(id_namespace_name: &str) -> String {
    format!("IdNamespace {id_namespace_name} does not exist")
}

/// The in-use conflict's text is a template over the STORE: the matching workflow whose
/// `inputSourceConfig` still names the schema mapping is substituted into it.
fn schema_mapping_in_use(workflow_name: &str) -> String {
    format!("SchemaMapping associated with Matching Workflow: {workflow_name}")
}

/// One resource name plus the size of the record it is used with: the two things the
/// rule says the message text does not depend on.
struct NameCase {
    case: &'static str,
    name: String,
    /// How many `mappedInputFields` the schema mapping carries (the modelled
    /// `SchemaInputAttributes` `length` is 2..60), and how many `inputSourceConfig`
    /// entries the workflow carries (modelled `length` 1..20).
    fields: usize,
    sources: usize,
}

/// The table: `EntityName` over letters, digits, `_` and `-`, at the modelled length
/// minimum of 1 and maximum of 255, each paired with a different record size.
fn name_cases() -> Vec<NameCase> {
    vec![
        NameCase {
            case: "a single-letter name at the modelled length minimum",
            name: "a".to_string(),
            fields: 2,
            sources: 1,
        },
        NameCase {
            case: "mixed case letters",
            name: "Customers".to_string(),
            fields: 5,
            sources: 2,
        },
        NameCase {
            case: "underscore, hyphen and digits together",
            name: "crm_people-1".to_string(),
            fields: 3,
            sources: 3,
        },
        NameCase {
            case: "a digits-only name",
            name: "0".to_string(),
            fields: 2,
            sources: 1,
        },
        NameCase {
            case: "every character class the modelled pattern admits",
            name: "UPPER_lower-123".to_string(),
            fields: 12,
            sources: 4,
        },
        NameCase {
            case: "a name at the modelled length maximum of 255",
            name: "n".repeat(255),
            fields: 60,
            sources: 1,
        },
    ]
}

/// A `CreateSchemaMapping` request for `name` with `fields` mapped input fields.
fn schema_request(name: &str, fields: usize) -> Value {
    let mapped: Vec<Value> = (0..fields)
        .map(|index| {
            if index == 0 {
                json!({ "fieldName": "id", "type": "UNIQUE_ID" })
            } else {
                json!({ "fieldName": format!("col_{index}"), "type": "STRING" })
            }
        })
        .collect();
    json!({ "schemaName": name, "mappedInputFields": Value::Array(mapped) })
}

/// A `CreateMatchingWorkflow` request for `name` whose `inputSourceConfig` references
/// `schemas` (`InputSource$schemaName` is a workflow's reference to a schema mapping).
fn workflow_request(name: &str, schemas: &[String]) -> Value {
    let sources: Vec<Value> = schemas
        .iter()
        .map(|schema| {
            json!({
                "inputSourceARN": glue_table_arn("customers"),
                "schemaName": schema,
            })
        })
        .collect();
    json!({
        "workflowName": name,
        "inputSourceConfig": Value::Array(sources),
        "outputSourceConfig": [{
            "outputS3Path": "s3://er-output/messages",
            "output": [{ "name": "id" }],
        }],
        "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
        "roleArn": role_arn(),
    })
}

/// The schema mappings a workflow of `case` references, created first so the
/// workflow's `inputSourceConfig` points at records that exist.
async fn create_referenced_schemas(emulator: &Emulator, case: &NameCase) -> Vec<String> {
    let mut created = Vec::new();
    for index in 0..case.sources {
        let schema = format!("{}-src{index}", &case.name[..case.name.len().min(200)]);
        let reply = emulator
            .create_schema_mapping(&schema_request(&schema, case.fields))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: the referenced schema mapping {schema:?} must be created, got {reply:?}",
            case.case
        );
        created.push(schema);
    }
    created
}

/// Assert `reply` is the modelled client failure `code` with `status` and carries
/// exactly `message`. The status/code half is the harness's shared assertion.
fn assert_error(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_error_shape(case, reply, status, code);
    assert_eq!(
        string_member(&reply.body, "message"),
        message,
        "case {case:?}: expected the message {message:?}, got {:?}",
        reply.body
    );
}

#[tokio::test]
async fn delete_schema_mapping_answers_the_services_constant_success_message() {
    let emulator = Emulator::new();
    let mut seen = BTreeSet::new();

    for case in name_cases() {
        let label = case.case;
        let created = emulator
            .create_schema_mapping(&schema_request(&case.name, case.fields))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateSchemaMapping, got {created:?}"
        );

        let deleted = emulator.delete_schema_mapping(&case.name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected HTTP 200 from DeleteSchemaMapping, got {deleted:?}"
        );
        let message = string_member(&deleted.body, "message");
        assert_eq!(
            message, SCHEMA_MAPPING_DELETED,
            "case {label:?}: expected the success message {SCHEMA_MAPPING_DELETED:?}, got {:?}",
            deleted.body
        );
        seen.insert(message);

        // The round trip: the record really is gone, and the reader answers with its
        // OWN not-found text.
        let read = emulator.get_schema_mapping(&case.name).await;
        assert_error(
            label,
            &read,
            404,
            "ResourceNotFoundException",
            SCHEMA_MAPPING_NOT_FOUND,
        );
    }

    assert_eq!(
        seen.len(),
        1,
        "the success message must not depend on the record that was deleted, saw {seen:?}"
    );
}

#[tokio::test]
async fn delete_matching_workflow_answers_the_services_constant_success_message() {
    let emulator = Emulator::new();
    let mut seen = BTreeSet::new();

    for case in name_cases() {
        let label = case.case;
        let schemas = create_referenced_schemas(&emulator, &case).await;
        let created = emulator
            .create_matching_workflow(&workflow_request(&case.name, &schemas))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateMatchingWorkflow, got {created:?}"
        );

        let deleted = emulator.delete_matching_workflow(&case.name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected HTTP 200 from DeleteMatchingWorkflow, got {deleted:?}"
        );
        let message = string_member(&deleted.body, "message");
        assert_eq!(
            message, MATCHING_WORKFLOW_DELETED,
            "case {label:?}: expected the success message {MATCHING_WORKFLOW_DELETED:?}, \
             got {:?}",
            deleted.body
        );
        seen.insert(message);

        let read = emulator.get_matching_workflow(&case.name).await;
        assert_error(
            label,
            &read,
            404,
            "ResourceNotFoundException",
            MATCHING_WORKFLOW_NOT_FOUND,
        );
    }

    assert_eq!(
        seen.len(),
        1,
        "the success message must not depend on the workflow that was deleted, saw {seen:?}"
    );
}

#[tokio::test]
async fn a_reader_answers_its_own_constant_not_found_message() {
    let emulator = Emulator::new();
    let mut schema_texts = BTreeSet::new();
    let mut workflow_texts = BTreeSet::new();

    for case in name_cases() {
        let label = case.case;

        let schema = emulator.get_schema_mapping(&case.name).await;
        assert_error(
            label,
            &schema,
            404,
            "ResourceNotFoundException",
            SCHEMA_MAPPING_NOT_FOUND,
        );
        schema_texts.insert(string_member(&schema.body, "message"));

        let workflow = emulator.get_matching_workflow(&case.name).await;
        assert_error(
            label,
            &workflow,
            404,
            "ResourceNotFoundException",
            MATCHING_WORKFLOW_NOT_FOUND,
        );
        workflow_texts.insert(string_member(&workflow.body, "message"));
    }

    for (operation, texts) in [
        ("GetSchemaMapping", &schema_texts),
        ("GetMatchingWorkflow", &workflow_texts),
    ] {
        assert_eq!(
            texts.len(),
            1,
            "{operation}: the not-found message must not name the resource that was \
             asked for, saw {texts:?}"
        );
    }
}

#[tokio::test]
async fn delete_matching_workflow_not_found_substitutes_the_requested_name() {
    let emulator = Emulator::new();

    for case in name_cases() {
        let label = case.case;
        let expected = delete_matching_workflow_not_found(&case.name);

        let refused = emulator.delete_matching_workflow(&case.name).await;
        assert_error(
            label,
            &refused,
            404,
            "ResourceNotFoundException",
            &expected,
        );

        // The two workflow not-found paths are worded differently, so one shared
        // string cannot serve both.
        let read = emulator.get_matching_workflow(&case.name).await;
        assert_ne!(
            string_member(&read.body, "message"),
            expected,
            "case {label:?}: GetMatchingWorkflow and DeleteMatchingWorkflow must not \
             answer the same not-found text"
        );
    }
}

#[tokio::test]
async fn every_id_namespace_reader_answers_the_same_constant_not_found_text() {
    let emulator = Emulator::new();
    let mut texts = BTreeSet::new();
    // The whole configuration `UpdateIdNamespace` models is optional, so an empty body
    // is a legitimate update and the 404 below is about the namespace, not the body.
    let update = json!({});

    for case in name_cases() {
        let label = case.case;

        // The two READ paths that address an ID namespace by its name: both are
        // lookups of the SAME resource by the SAME key, and the recorded text names no
        // namespace, so neither composes its message from the request. The DELETE path
        // does name it, which is asserted below.
        for (operation, reply) in [
            ("GetIdNamespace", emulator.get_id_namespace(&case.name).await),
            (
                "UpdateIdNamespace",
                emulator.update_id_namespace(&case.name, &update).await,
            ),
        ] {
            assert_error(
                label,
                &reply,
                404,
                "ResourceNotFoundException",
                ID_NAMESPACE_NOT_FOUND,
            );
            texts.insert((operation, string_member(&reply.body, "message")));
        }

        // The delete of the same absent name names it, so it is NOT in the invariant
        // set above.
        let deleted = emulator.delete_id_namespace(&case.name).await;
        assert_error(
            label,
            &deleted,
            404,
            "ResourceNotFoundException",
            &delete_id_namespace_not_found(&case.name),
        );
    }

    assert_eq!(
        texts.len(),
        2,
        "each ID-namespace reader must answer ONE text, invariant over the name it was \
         asked for, saw {texts:?}"
    );
}

#[tokio::test]
async fn delete_id_namespace_answers_the_operations_constant_success_message() {
    let emulator = Emulator::new();
    let mut seen = BTreeSet::new();

    for case in name_cases() {
        let label = case.case;
        // The record's SIZE is what the rule says the text does not depend on, so each
        // row carries as many `inputSourceConfig` entries as its workflow would.
        let sources: Vec<Value> = (0..case.sources)
            .map(|index| json!({ "inputSourceARN": glue_table_arn(&format!("t{index}")) }))
            .collect();
        let created = emulator
            .create_id_namespace(&json!({
                "idNamespaceName": case.name,
                "type": "SOURCE",
                "inputSourceConfig": Value::Array(sources),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateIdNamespace, got {created:?}"
        );

        let deleted = emulator.delete_id_namespace(&case.name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected HTTP 200 from DeleteIdNamespace, got {deleted:?}"
        );
        let message = string_member(&deleted.body, "message");
        assert_eq!(
            message, ID_NAMESPACE_DELETED,
            "case {label:?}: expected the success message {ID_NAMESPACE_DELETED:?}, got {:?}",
            deleted.body
        );
        seen.insert(message);
    }

    assert_eq!(
        seen.len(),
        1,
        "the success message must not depend on the namespace that was deleted, saw {seen:?}"
    );
}

#[tokio::test]
async fn a_duplicate_create_answers_the_operations_constant_conflict_message() {
    let emulator = Emulator::new();
    let mut schema_texts = BTreeSet::new();
    let mut workflow_texts = BTreeSet::new();

    for case in name_cases() {
        let label = case.case;
        let request = schema_request(&case.name, case.fields);
        let created = emulator.create_schema_mapping(&request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateSchemaMapping, got {created:?}"
        );
        let duplicate = emulator.create_schema_mapping(&request).await;
        assert_error(
            label,
            &duplicate,
            400,
            "ConflictException",
            SCHEMA_MAPPING_ALREADY_EXISTS,
        );
        schema_texts.insert(string_member(&duplicate.body, "message"));

        let schemas = create_referenced_schemas(&emulator, &case).await;
        let workflow = workflow_request(&case.name, &schemas);
        let created = emulator.create_matching_workflow(&workflow).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateMatchingWorkflow, got {created:?}"
        );
        let duplicate = emulator.create_matching_workflow(&workflow).await;
        assert_error(
            label,
            &duplicate,
            400,
            "ConflictException",
            MATCHING_WORKFLOW_ALREADY_EXISTS,
        );
        workflow_texts.insert(string_member(&duplicate.body, "message"));
    }

    for (operation, texts) in [
        ("CreateSchemaMapping", &schema_texts),
        ("CreateMatchingWorkflow", &workflow_texts),
    ] {
        assert_eq!(
            texts.len(),
            1,
            "{operation}: the already-exists message must not name the resource, \
             saw {texts:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_referenced_schema_mapping_names_the_referencing_workflow() {
    let emulator = Emulator::new();

    for case in name_cases() {
        let label = case.case;
        let schemas = create_referenced_schemas(&emulator, &case).await;
        let created = emulator
            .create_matching_workflow(&workflow_request(&case.name, &schemas))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateMatchingWorkflow, got {created:?}"
        );

        for schema in &schemas {
            let refused = emulator.delete_schema_mapping(schema).await;
            assert_eq!(
                refused.status, 400,
                "case {label:?}: deleting the referenced schema mapping {schema:?} must be \
                 refused, got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("ConflictException"),
                "case {label:?}: expected ConflictException, got {:?}",
                refused.error_code
            );
            let message = string_member(&refused.body, "message");
            assert_eq!(
                message,
                schema_mapping_in_use(&case.name),
                "case {label:?}: the conflict message must name the workflow that references \
                 {schema:?}, got {message:?}"
            );
            assert!(
                !message.contains(schema.as_str()),
                "case {label:?}: the conflict message must not be composed from the \
                 requested name, got {message:?}"
            );
        }
    }
}
