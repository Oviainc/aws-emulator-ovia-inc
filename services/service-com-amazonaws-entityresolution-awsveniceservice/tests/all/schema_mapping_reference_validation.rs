// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the three rules Entity Resolution applies to the NAMES a
//! schema-mapping or matching-workflow write carries, and to the refusal that names the
//! workflow holding a schema mapping.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request, through the harness the sibling `schema_mapping_workflow_lifecycle` suite owns —
//! so what is asserted is what an AWS SDK client would see.
//!
//! The three rules under test:
//!
//! 1. A schema mapping's `mappedInputFields` names each input column ONCE: a
//!    `SchemaInputAttribute` "corresponds to a column the source data table" and its
//!    `fieldName` is that column's @required name
//!    (`smithy_query(query="shape",shape="SchemaInputAttribute")`), so two entries carrying
//!    one name describe the same column twice. Both write paths that take the list
//!    (`CreateSchemaMapping` and `UpdateSchemaMapping`) refuse such a list with
//!    `ValidationException` (HTTP 400) and store nothing. The rows differ along what the
//!    rule generalises over: how many names repeat, how often each repeats, where the
//!    repeats sit, how long the list is over the modelled `length 2..60` bounds, and which
//!    character classes the names use — including two names that differ only in CASE, which
//!    are different columns and are admitted.
//! 2. Every `inputSourceConfig[].schemaName` of a matching-workflow write is that
//!    workflow's @required reference to a schema mapping ("The name of the schema to be
//!    retrieved", `smithy_query(query="shape",shape="InputSource")`), so a name no schema
//!    mapping carries is a reference to nothing: `CreateMatchingWorkflow` and
//!    `UpdateMatchingWorkflow` refuse it with `ValidationException` (HTTP 400) naming the
//!    missing mapping(s) and write nothing. The rows differ along how many sources the
//!    request carries over the modelled `length 1..20` bounds, how many of them are
//!    unresolvable, where they sit, and whether one unresolvable name is repeated.
//! 3. The refusal of a schema mapping a matching workflow still uses IDENTIFIES that
//!    workflow: `DeleteSchemaMapping` and `UpdateSchemaMapping` both answer
//!    `ConflictException` (HTTP 400) whose message carries the `workflowName` read out of
//!    the store, so a caller can find and remove the workflow that blocks it. The rows
//!    differ along the workflow's name (the character classes and both `length` bounds the
//!    modelled `EntityName` pattern admits) and along how many workflows reference the one
//!    schema mapping.

use serde_json::{json, Map, Value};

use crate::schema_mapping_workflow_lifecycle::{
    assert_error_shape, glue_table_arn, list_member, role_arn, string_member, Emulator, Reply,
};

/// `PUT /schemas/{schemaName}` — `UpdateSchemaMapping`'s modelled HTTP binding, with the
/// name as the `@httpLabel` and the new configuration as the body.
async fn update_schema_mapping(emulator: &Emulator, schema_name: &str, request: &Value) -> Reply {
    emulator
        .call("PUT", &format!("/schemas/{schema_name}"), Some(request))
        .await
}

/// `GET /matchingworkflows/{workflowName}`, the reader a refused create must answer 404 for.
async fn get_matching_workflow(emulator: &Emulator, workflow_name: &str) -> Reply {
    emulator
        .call("GET", &format!("/matchingworkflows/{workflow_name}"), None)
        .await
}

/// One `SchemaInputAttribute` of `attribute_type` naming the column `field_name`.
fn attribute(field_name: &str, attribute_type: &str) -> Value {
    json!({ "fieldName": field_name, "type": attribute_type })
}

/// Values `SchemaAttributeType` admits other than `UNIQUE_ID`, from
/// `smithy_query(query="shape",shape="SchemaAttributeType")`.
const OTHER_ATTRIBUTE_TYPES: &[&str] = &[
    "NAME",
    "EMAIL_ADDRESS",
    "PHONE_NUMBER",
    "ADDRESS_POSTALCODE",
    "IPV4",
    "DATE",
    "STRING",
];

/// A `mappedInputFields` list of `length` attributes — one `UNIQUE_ID` first, so every row
/// satisfies the SEPARATE cardinality rule and only the names under test can refuse it —
/// whose column names come from `names` for as far as that list reaches and are distinct
/// afterwards.
fn fields(length: usize, names: &[&str]) -> Value {
    let list: Vec<Value> = (0..length)
        .map(|index| {
            let name = names
                .get(index)
                .map_or_else(|| format!("col_{index}"), |named| (*named).to_string());
            if index == 0 {
                attribute(&name, "UNIQUE_ID")
            } else {
                attribute(
                    &name,
                    OTHER_ATTRIBUTE_TYPES[index % OTHER_ATTRIBUTE_TYPES.len()],
                )
            }
        })
        .collect();
    Value::Array(list)
}

/// The whole `message` both write paths answer for a list that names one column twice,
/// written out here rather than read from the crate's own helper so this suite PINS the
/// wording a client sees. The repeated names follow, in the order the list states them.
fn duplicate_field_names(duplicates: &[&str]) -> String {
    format!(
        "SchemaMapping is not allowed to have repeated input fields in attributes: {}",
        duplicates.join(", ")
    )
}

/// The whole `message` a write whose `inputSourceConfig` names unresolvable schema mappings
/// answers, pinned here for the same reason.
fn references_missing(missing: &[&str]) -> String {
    format!(
        "References schema mapping(s) which does not exist: [{}]",
        missing.join(", ")
    )
}

/// The whole `message` the two in-use refusals answer, pinned here for the same reason.
fn in_use(workflow_name: &str) -> String {
    format!("SchemaMapping associated with Matching Workflow: {workflow_name}")
}

/// One `mappedInputFields` list, and the repeated names the refusal must name — empty for a
/// list the rule admits.
struct FieldNameCase {
    case: &'static str,
    mapped_input_fields: Value,
    duplicates: Vec<&'static str>,
}

/// The table for rule 1.
fn field_name_cases() -> Vec<FieldNameCase> {
    vec![
        FieldNameCase {
            case: "distinct names at the modelled list minimum of two",
            mapped_input_fields: fields(2, &["id", "name"]),
            duplicates: Vec::new(),
        },
        FieldNameCase {
            case: "two names differing only in case, which are different columns",
            mapped_input_fields: fields(3, &["id", "Email", "email"]),
            duplicates: Vec::new(),
        },
        FieldNameCase {
            case: "names using the space and hyphen the modelled pattern admits",
            mapped_input_fields: fields(3, &["id", "home address", "work-address"]),
            duplicates: Vec::new(),
        },
        FieldNameCase {
            case: "distinct names at the modelled list maximum of 60",
            mapped_input_fields: fields(60, &["id"]),
            duplicates: Vec::new(),
        },
        FieldNameCase {
            case: "the identifier column named twice, adjacent",
            mapped_input_fields: fields(2, &["id", "id"]),
            duplicates: vec!["id"],
        },
        FieldNameCase {
            case: "one name repeated at either end of a longer list",
            mapped_input_fields: fields(5, &["id", "email", "name", "phone", "email"]),
            duplicates: vec!["email"],
        },
        FieldNameCase {
            case: "one name repeated three times",
            mapped_input_fields: fields(5, &["id", "email", "email", "name", "email"]),
            duplicates: vec!["email"],
        },
        FieldNameCase {
            case: "two different names each repeated, reported in list order",
            mapped_input_fields: fields(6, &["id", "email", "name", "email", "zip", "name"]),
            duplicates: vec!["email", "name"],
        },
        FieldNameCase {
            case: "a repeated name carrying a space",
            mapped_input_fields: fields(4, &["id", "home address", "zip", "home address"]),
            duplicates: vec!["home address"],
        },
        FieldNameCase {
            case: "the last of a list at the modelled maximum of 60 repeating the first column",
            mapped_input_fields: {
                let mut names: Vec<String> = (0..60).map(|index| format!("col_{index}")).collect();
                names[0] = "id".to_string();
                names[59] = "col_1".to_string();
                let list: Vec<Value> = names
                    .iter()
                    .enumerate()
                    .map(|(index, name)| {
                        if index == 0 {
                            attribute(name, "UNIQUE_ID")
                        } else {
                            attribute(
                                name,
                                OTHER_ATTRIBUTE_TYPES[index % OTHER_ATTRIBUTE_TYPES.len()],
                            )
                        }
                    })
                    .collect();
                Value::Array(list)
            },
            duplicates: vec!["col_1"],
        },
    ]
}

/// A `CreateSchemaMapping` body for `name` carrying `mapped_input_fields`.
fn create_schema_request(name: &str, mapped_input_fields: &Value) -> Value {
    json!({
        "schemaName": name,
        "description": "probe schema mapping",
        "mappedInputFields": mapped_input_fields.clone(),
    })
}

/// An `UpdateSchemaMapping` body carrying `mapped_input_fields`.
fn update_schema_request(mapped_input_fields: &Value) -> Value {
    let mut request = Map::new();
    request.insert("description".to_string(), json!("updated probe"));
    request.insert(
        "mappedInputFields".to_string(),
        mapped_input_fields.clone(),
    );
    Value::Object(request)
}

/// A list of distinct attributes a seeded schema mapping is created with.
fn baseline_fields() -> Value {
    fields(3, &["id", "name", "email"])
}

/// RULE 1: a `mappedInputFields` that names one input column twice is refused by BOTH write
/// paths with `ValidationException` (HTTP 400) naming the repeated columns, and nothing is
/// stored: a refused create leaves no schema mapping behind, and a refused update leaves the
/// stored list exactly as it was.
#[tokio::test]
async fn a_schema_mapping_names_each_input_column_once() {
    let emulator = Emulator::new();
    let baseline = baseline_fields();

    for (index, case) in field_name_cases().iter().enumerate() {
        let label = case.case;
        let admitted = case.duplicates.is_empty();
        let expected = duplicate_field_names(&case.duplicates);

        // The CREATE path.
        let created_name = format!("fieldnames-create-{index}");
        let created = emulator
            .create_schema_mapping(&create_schema_request(
                &created_name,
                &case.mapped_input_fields,
            ))
            .await;
        if admitted {
            assert_eq!(
                created.status, 200,
                "case {label:?}: expected CreateSchemaMapping to be admitted, got {created:?}"
            );
        } else {
            assert_error_shape(label, &created, 400, "ValidationException");
            assert_eq!(
                string_member(&created.body, "message"),
                expected,
                "case {label:?}: expected the message {expected:?}, got {:?}",
                created.body
            );
            let read = emulator.get_schema_mapping(&created_name).await;
            assert_error_shape(label, &read, 404, "ResourceNotFoundException");
        }

        // The UPDATE path, over a schema mapping whose stored list is distinct.
        let updated_name = format!("fieldnames-update-{index}");
        let seeded = emulator
            .create_schema_mapping(&create_schema_request(&updated_name, &baseline))
            .await;
        assert_eq!(
            seeded.status, 200,
            "case {label:?}: seeding {updated_name:?} failed: {seeded:?}"
        );
        let update = update_schema_mapping(
            &emulator,
            &updated_name,
            &update_schema_request(&case.mapped_input_fields),
        )
        .await;
        let read = emulator.get_schema_mapping(&updated_name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the seeded schema mapping must stay readable, got {read:?}"
        );
        if admitted {
            assert_eq!(
                update.status, 200,
                "case {label:?}: expected UpdateSchemaMapping to be admitted, got {update:?}"
            );
            assert_eq!(
                list_member(&read.body, "mappedInputFields"),
                case.mapped_input_fields
                    .as_array()
                    .expect("the row's mappedInputFields is a list"),
                "case {label:?}: expected the update to be readable, got {:?}",
                read.body
            );
        } else {
            assert_error_shape(label, &update, 400, "ValidationException");
            assert_eq!(
                string_member(&update.body, "message"),
                expected,
                "case {label:?}: expected the message {expected:?}, got {:?}",
                update.body
            );
            assert_eq!(
                list_member(&read.body, "mappedInputFields"),
                baseline.as_array().expect("the baseline list is a list"),
                "case {label:?}: a refused update must leave the stored list alone, got {:?}",
                read.body
            );
        }
    }
}

/// One matching-workflow configuration, as the schema mappings its `inputSourceConfig`
/// names and which of those are never created.
struct ReferenceCase {
    case: &'static str,
    /// Every `schemaName` the `inputSourceConfig` names, in request order.
    referenced: Vec<String>,
    /// The ones no schema mapping is created for, in the order the refusal must name them.
    missing: Vec<String>,
}

/// The table for rule 2.
fn reference_cases() -> Vec<ReferenceCase> {
    let named = |names: &[&str]| names.iter().map(|name| (*name).to_string()).collect();
    vec![
        ReferenceCase {
            case: "the modelled minimum of one input source, which resolves",
            referenced: named(&["present_a"]),
            missing: Vec::new(),
        },
        ReferenceCase {
            case: "several input sources, all of which resolve",
            referenced: named(&["present_a", "present_b", "present_c"]),
            missing: Vec::new(),
        },
        ReferenceCase {
            case: "the modelled minimum of one input source, unresolvable",
            referenced: named(&["absent_one"]),
            missing: named(&["absent_one"]),
        },
        ReferenceCase {
            case: "the first of three input sources unresolvable",
            referenced: named(&["absent_first", "present_a", "present_b"]),
            missing: named(&["absent_first"]),
        },
        ReferenceCase {
            case: "the last of three input sources unresolvable",
            referenced: named(&["present_a", "present_b", "absent_last"]),
            missing: named(&["absent_last"]),
        },
        ReferenceCase {
            case: "two unresolvable names, reported in request order",
            referenced: named(&["absent_second", "present_a", "absent-1"]),
            missing: named(&["absent_second", "absent-1"]),
        },
        ReferenceCase {
            case: "one unresolvable name repeated, reported once",
            referenced: named(&["absent_twice", "present_a", "absent_twice"]),
            missing: named(&["absent_twice"]),
        },
        ReferenceCase {
            case: "a name at the modelled EntityName maximum of 255, unresolvable",
            referenced: vec!["z".repeat(255)],
            missing: vec!["z".repeat(255)],
        },
        ReferenceCase {
            case: "the modelled maximum of 20 input sources with the last unresolvable",
            referenced: {
                let mut names: Vec<String> =
                    (0..19).map(|index| format!("wide_{index}")).collect();
                names.push("absent_wide".to_string());
                names
            },
            missing: named(&["absent_wide"]),
        },
    ]
}

/// A `CreateMatchingWorkflow` body for `name` whose `inputSourceConfig` names `schemas`.
fn create_workflow_request(name: &str, schemas: &[String]) -> Value {
    let mut request = update_workflow_request(schemas);
    request["workflowName"] = json!(name);
    request
}

/// An `UpdateMatchingWorkflow` body whose `inputSourceConfig` names `schemas`
/// (`workflowName` is the `@httpLabel`, so it is not a body member).
fn update_workflow_request(schemas: &[String]) -> Value {
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
        "inputSourceConfig": Value::Array(sources),
        "outputSourceConfig": [{
            "outputS3Path": "s3://er-output/references",
            "output": [{ "name": "id" }],
        }],
        "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
        "roleArn": role_arn(),
    })
}

/// Create every schema mapping of `referenced` that is not in `missing`, so the row's
/// unresolvable names are the ONLY thing that can refuse the write.
async fn seed_present_schemas(emulator: &Emulator, case: &ReferenceCase) {
    for schema in &case.referenced {
        if case.missing.contains(schema) {
            continue;
        }
        let reply = emulator
            .create_schema_mapping(&create_schema_request(schema, &baseline_fields()))
            .await;
        assert!(
            reply.status == 200 || reply.error_code.as_deref() == Some("ConflictException"),
            "case {:?}: seeding the schema mapping {schema:?} failed: {reply:?}",
            case.case
        );
    }
}

/// RULE 2: a matching-workflow write whose `inputSourceConfig` names a schema mapping this
/// account does not hold is refused with `ValidationException` (HTTP 400) naming the missing
/// mapping(s), and writes nothing — a refused create leaves `GetMatchingWorkflow` a 404, and
/// a refused update leaves the stored configuration as it was. Creating the missing mapping
/// makes the identical call succeed, which is what makes the refusal about the REFERENCE.
#[tokio::test]
async fn a_matching_workflow_write_resolves_every_schema_it_references() {
    let emulator = Emulator::new();

    for (index, case) in reference_cases().iter().enumerate() {
        let label = case.case;
        let admitted = case.missing.is_empty();
        let missing: Vec<&str> = case.missing.iter().map(String::as_str).collect();
        let expected = references_missing(&missing);
        seed_present_schemas(&emulator, case).await;

        // The CREATE path.
        let created_name = format!("references-create-{index}");
        let created = emulator
            .create_matching_workflow(&create_workflow_request(&created_name, &case.referenced))
            .await;
        if admitted {
            assert_eq!(
                created.status, 200,
                "case {label:?}: expected CreateMatchingWorkflow to be admitted, got {created:?}"
            );
        } else {
            assert_error_shape(label, &created, 400, "ValidationException");
            assert_eq!(
                string_member(&created.body, "message"),
                expected,
                "case {label:?}: expected the message {expected:?}, got {:?}",
                created.body
            );
            let read = get_matching_workflow(&emulator, &created_name).await;
            assert_error_shape(label, &read, 404, "ResourceNotFoundException");
        }

        // The UPDATE path, over a workflow whose stored configuration resolves.
        let updated_name = format!("references-update-{index}");
        let resolvable = vec!["present_a".to_string()];
        let seeded_schema = emulator
            .create_schema_mapping(&create_schema_request("present_a", &baseline_fields()))
            .await;
        assert!(
            seeded_schema.status == 200
                || seeded_schema.error_code.as_deref() == Some("ConflictException"),
            "case {label:?}: seeding the stored workflow's schema mapping failed: \
             {seeded_schema:?}"
        );
        let seeded = emulator
            .create_matching_workflow(&create_workflow_request(&updated_name, &resolvable))
            .await;
        assert_eq!(
            seeded.status, 200,
            "case {label:?}: seeding {updated_name:?} failed: {seeded:?}"
        );
        let update = emulator
            .update_matching_workflow(&updated_name, &update_workflow_request(&case.referenced))
            .await;
        let read = get_matching_workflow(&emulator, &updated_name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the seeded workflow must stay readable, got {read:?}"
        );
        if admitted {
            assert_eq!(
                update.status, 200,
                "case {label:?}: expected UpdateMatchingWorkflow to be admitted, got {update:?}"
            );
        } else {
            assert_error_shape(label, &update, 400, "ValidationException");
            assert_eq!(
                string_member(&update.body, "message"),
                expected,
                "case {label:?}: expected the message {expected:?}, got {:?}",
                update.body
            );
            assert_eq!(
                list_member(&read.body, "inputSourceConfig").len(),
                resolvable.len(),
                "case {label:?}: a refused update must leave the stored configuration alone, \
                 got {:?}",
                read.body
            );

            // With the missing mappings created, the SAME create is admitted.
            for schema in &case.missing {
                let reply = emulator
                    .create_schema_mapping(&create_schema_request(schema, &baseline_fields()))
                    .await;
                assert_eq!(
                    reply.status, 200,
                    "case {label:?}: creating the missing schema mapping {schema:?} failed: \
                     {reply:?}"
                );
            }
            let retried = emulator
                .create_matching_workflow(&create_workflow_request(&created_name, &case.referenced))
                .await;
            assert_eq!(
                retried.status, 200,
                "case {label:?}: with every referenced schema mapping created the same create \
                 must be admitted, got {retried:?}"
            );
            // Leave the store as the next row expects it: the names this row treated as
            // missing are dropped again with the workflow that referenced them.
            let removed = emulator.delete_matching_workflow(&created_name).await;
            assert_eq!(
                removed.status, 200,
                "case {label:?}: removing {created_name:?} failed: {removed:?}"
            );
            for schema in &case.missing {
                let dropped = emulator.delete_schema_mapping(schema).await;
                assert_eq!(
                    dropped.status, 200,
                    "case {label:?}: removing the schema mapping {schema:?} failed: {dropped:?}"
                );
            }
        }
    }
}

/// RULE 3: both refusals of a schema mapping a matching workflow still uses name THAT
/// workflow, read out of the store, so the message changes with the workflow and not with
/// the request. A schema mapping several workflows reference names the first of them in
/// stored order, and once that workflow is gone the next one is named — which is what shows
/// the name is the store's and not a constant.
#[tokio::test]
async fn the_in_use_refusals_name_the_workflow_that_holds_the_schema_mapping() {
    let emulator = Emulator::new();

    // (case, the `workflowName`) — the character classes the modelled `EntityName` pattern
    // `^[a-zA-Z_0-9-]*$` admits, at both its `length 1..255` bounds.
    let workflow_names: Vec<(&str, String)> = vec![
        ("a single-letter name at the modelled minimum", "w".to_string()),
        ("mixed case letters", "Customers".to_string()),
        ("underscore, hyphen and digits together", "crm_people-1".to_string()),
        ("a digits-only name", "7".to_string()),
        ("a name at the modelled maximum of 255", "n".repeat(255)),
    ];

    for (index, (case, workflow_name)) in workflow_names.iter().enumerate() {
        let schema = format!("in-use-{index}");
        let created = emulator
            .create_schema_mapping(&create_schema_request(&schema, &baseline_fields()))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: seeding {schema:?} failed: {created:?}"
        );
        let seeded = emulator
            .create_matching_workflow(&create_workflow_request(
                workflow_name,
                std::slice::from_ref(&schema),
            ))
            .await;
        assert_eq!(
            seeded.status, 200,
            "case {case:?}: seeding {workflow_name:?} failed: {seeded:?}"
        );

        let expected = in_use(workflow_name);
        let refused_delete = emulator.delete_schema_mapping(&schema).await;
        assert_error_shape(case, &refused_delete, 400, "ConflictException");
        assert_eq!(
            string_member(&refused_delete.body, "message"),
            expected,
            "case {case:?}: DeleteSchemaMapping must name the referencing workflow, got {:?}",
            refused_delete.body
        );

        let refused_update = update_schema_mapping(
            &emulator,
            &schema,
            &update_schema_request(&baseline_fields()),
        )
        .await;
        assert_error_shape(case, &refused_update, 400, "ConflictException");
        assert_eq!(
            string_member(&refused_update.body, "message"),
            expected,
            "case {case:?}: UpdateSchemaMapping must name the referencing workflow, got {:?}",
            refused_update.body
        );

        // With the workflow gone the same delete is admitted, which is what makes the
        // refusal about the REFERENCE rather than about the request.
        let removed = emulator.delete_matching_workflow(workflow_name).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: deleting {workflow_name:?} failed: {removed:?}"
        );
        let admitted = emulator.delete_schema_mapping(&schema).await;
        assert_eq!(
            admitted.status, 200,
            "case {case:?}: with the workflow gone the delete must be admitted, got {admitted:?}"
        );
    }

    // Several workflows referencing ONE schema mapping: the refusal names the first in
    // stored order, and the next one once that is gone.
    let schema = "shared-schema".to_string();
    let created = emulator
        .create_schema_mapping(&create_schema_request(&schema, &baseline_fields()))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"a shared schema mapping\": seeding {schema:?} failed: {created:?}"
    );
    // Created in the reverse of the order the store holds them in (`workflowName` order),
    // so the name in the message cannot be "whichever was created first".
    for workflow_name in ["zulu-workflow", "alpha-workflow"] {
        let seeded = emulator
            .create_matching_workflow(&create_workflow_request(
                workflow_name,
                std::slice::from_ref(&schema),
            ))
            .await;
        assert_eq!(
            seeded.status, 200,
            "case \"a shared schema mapping\": seeding {workflow_name:?} failed: {seeded:?}"
        );
    }
    for workflow_name in ["alpha-workflow", "zulu-workflow"] {
        let refused = emulator.delete_schema_mapping(&schema).await;
        assert_error_shape("a shared schema mapping", &refused, 400, "ConflictException");
        assert_eq!(
            string_member(&refused.body, "message"),
            in_use(workflow_name),
            "case \"a shared schema mapping\": expected the message to name {workflow_name:?}, \
             got {:?}",
            refused.body
        );
        let removed = emulator.delete_matching_workflow(workflow_name).await;
        assert_eq!(
            removed.status, 200,
            "case \"a shared schema mapping\": deleting {workflow_name:?} failed: {removed:?}"
        );
    }
    let admitted = emulator.delete_schema_mapping(&schema).await;
    assert_eq!(
        admitted.status, 200,
        "case \"a shared schema mapping\": with every workflow gone the delete must be \
         admitted, got {admitted:?}"
    );
}
