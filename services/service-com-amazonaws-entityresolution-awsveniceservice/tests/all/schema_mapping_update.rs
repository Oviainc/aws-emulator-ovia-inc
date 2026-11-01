// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the two schema-mapping WRITE paths: the list-wide rule
//! both of them enforce, and the update that has to be readable afterwards.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request, through the harness the sibling `schema_mapping_workflow_lifecycle` suite
//! owns — so what is asserted is what an AWS SDK client would see.
//!
//! Two rules are under test.
//!
//! 1. A schema mapping declares EXACTLY ONE attribute of type `UNIQUE_ID`: the column
//!    that identifies one input record. The model constrains `SchemaInputAttributes` only
//!    by length (2..60) and each member's `type` only by the `SchemaAttributeType` enum,
//!    so it cannot express a condition over the whole list; both operations that take the
//!    list (`CreateSchemaMapping` and `UpdateSchemaMapping`, @required on each) refuse a
//!    list that declares any other number of them with `ValidationException` (HTTP 400)
//!    and store nothing. The table's rows differ along what the rule generalises over:
//!    how many `UNIQUE_ID` attributes the list carries (0, 1, 2, 3), where they sit in
//!    it, how long the list is (the modelled minimum of 2, a middle value, the maximum of
//!    60), and which other attribute types sit beside them.
//! 2. `UpdateSchemaMapping` REPLACES the stored `description` and `mappedInputFields`, so
//!    a `GetSchemaMapping` after it serves the values the update wrote and never the
//!    pre-update ones, while the resource's identity (`schemaArn`, `createdAt`) and its
//!    tags survive. The rows differ along how the list changes (an appended attribute, a
//!    removed one, a re-typed one, a reordered one, back to the modelled minimum) and
//!    along the `description` (changed, emptied, absent, at the modelled maximum of 255).
//!
//! The two refusals that are about the RESOURCE rather than the request body — an unknown
//! `schemaName`, and a schema some matching workflow still names in its
//! `inputSourceConfig`, which AWS makes immutable — are covered by their own table.

use serde_json::{json, Map, Value};

use crate::schema_mapping_workflow_lifecycle::{
    assert_error_shape, expected_arn, glue_table_arn, list_member, role_arn, string_member,
    timestamp_member, Emulator, Reply,
};

/// `PUT /schemas/{schemaName}` — `UpdateSchemaMapping`'s modelled HTTP binding
/// (`smithy_query(query="operation",operation="UpdateSchemaMapping")`), with the name as
/// the `@httpLabel` and the new configuration as the body.
async fn update_schema_mapping(emulator: &Emulator, schema_name: &str, request: &Value) -> Reply {
    emulator
        .call("PUT", &format!("/schemas/{schema_name}"), Some(request))
        .await
}

/// The whole `message` both write paths answer when `mappedInputFields` declares any
/// number of `UNIQUE_ID` attributes other than one — written out here rather than read
/// from the crate's own constant, so this suite PINS the wording a client sees.
const UNIQUE_ID_CARDINALITY: &str = "SchemaMapping must contain 1 attribute with type: UNIQUE_ID";

/// One `SchemaInputAttribute` of `attribute_type`, named after the column it maps.
fn attribute(field_name: &str, attribute_type: &str) -> Value {
    json!({ "fieldName": field_name, "type": attribute_type })
}

/// A `mappedInputFields` list of `length` attributes, with a `UNIQUE_ID` attribute at
/// each index in `unique_id_positions`.
///
/// The other attributes cycle [`OTHER_ATTRIBUTE_TYPES`], so a row is a real
/// `SchemaInputAttributes` rather than one repeated entry.
fn fields(length: usize, unique_id_positions: &[usize]) -> Value {
    let list: Vec<Value> = (0..length)
        .map(|index| {
            if unique_id_positions.contains(&index) {
                attribute(&format!("id_{index}"), "UNIQUE_ID")
            } else {
                attribute(
                    &format!("col_{index}"),
                    OTHER_ATTRIBUTE_TYPES[index % OTHER_ATTRIBUTE_TYPES.len()],
                )
            }
        })
        .collect();
    Value::Array(list)
}

/// Values `SchemaAttributeType` accepts other than `UNIQUE_ID`, from
/// `smithy_query(query="shape",shape="SchemaAttributeType")` — including `IPV6`,
/// `PHONE_COUNTRYCODE` and `PROVIDER_ID`, which the issue never cited.
const OTHER_ATTRIBUTE_TYPES: &[&str] = &[
    "NAME",
    "EMAIL_ADDRESS",
    "PHONE_NUMBER",
    "ADDRESS_POSTALCODE",
    "IPV6",
    "DATE",
    "MAID",
    "PHONE_COUNTRYCODE",
    "PROVIDER_ID",
    "STRING",
];

/// One `mappedInputFields` list, and whether the rule admits it.
struct CardinalityCase {
    case: &'static str,
    mapped_input_fields: Value,
    admitted: bool,
}

/// The table, spanning the count of `UNIQUE_ID` attributes, their position, the list
/// length over the modelled `length 2..60` bounds, and the types beside them.
fn cardinality_cases() -> Vec<CardinalityCase> {
    vec![
        CardinalityCase {
            case: "one UNIQUE_ID first, at the modelled list minimum of two",
            mapped_input_fields: fields(2, &[0]),
            admitted: true,
        },
        CardinalityCase {
            case: "one UNIQUE_ID last, at the modelled list minimum of two",
            mapped_input_fields: fields(2, &[1]),
            admitted: true,
        },
        CardinalityCase {
            case: "one UNIQUE_ID in the middle of a longer list",
            mapped_input_fields: fields(7, &[3]),
            admitted: true,
        },
        CardinalityCase {
            case: "one UNIQUE_ID at the modelled list maximum of 60",
            mapped_input_fields: fields(60, &[59]),
            admitted: true,
        },
        CardinalityCase {
            case: "no UNIQUE_ID at all, at the modelled list minimum",
            mapped_input_fields: fields(2, &[]),
            admitted: false,
        },
        CardinalityCase {
            case: "no UNIQUE_ID at all, in a long list",
            mapped_input_fields: fields(31, &[]),
            admitted: false,
        },
        CardinalityCase {
            case: "no UNIQUE_ID at all, at the modelled list maximum of 60",
            mapped_input_fields: fields(60, &[]),
            admitted: false,
        },
        CardinalityCase {
            case: "two UNIQUE_ID attributes, adjacent",
            mapped_input_fields: fields(4, &[0, 1]),
            admitted: false,
        },
        CardinalityCase {
            case: "two UNIQUE_ID attributes, at either end of the list",
            mapped_input_fields: fields(9, &[0, 8]),
            admitted: false,
        },
        CardinalityCase {
            case: "the whole list of two being UNIQUE_ID attributes",
            mapped_input_fields: fields(2, &[0, 1]),
            admitted: false,
        },
        CardinalityCase {
            case: "three UNIQUE_ID attributes",
            mapped_input_fields: fields(12, &[2, 5, 11]),
            admitted: false,
        },
        CardinalityCase {
            case: "60 UNIQUE_ID attributes, the modelled list maximum",
            mapped_input_fields: fields(60, &(0..60).collect::<Vec<usize>>()),
            admitted: false,
        },
    ]
}

/// A schema mapping that satisfies the rule, for the rows that need a STORED record to
/// update.
fn baseline_fields() -> Value {
    json!([
        attribute("id", "UNIQUE_ID"),
        attribute("name", "NAME"),
        attribute("email", "EMAIL_ADDRESS"),
    ])
}

/// A `CreateSchemaMapping` body for `name` carrying `mapped_input_fields`, plus the
/// optional members a round trip has to preserve.
fn create_request(name: &str, mapped_input_fields: &Value) -> Value {
    json!({
        "schemaName": name,
        "description": "probe schema mapping",
        "mappedInputFields": mapped_input_fields.clone(),
        "tags": { "team": "identity" },
    })
}

/// An `UpdateSchemaMapping` body carrying `mapped_input_fields` and, when `description`
/// is `Some`, that description — so a row can send the member ABSENT, which says the
/// schema now carries none.
fn update_request(description: Option<&str>, mapped_input_fields: &Value) -> Value {
    let mut request = Map::new();
    if let Some(description) = description {
        request.insert("description".to_string(), json!(description));
    }
    request.insert(
        "mappedInputFields".to_string(),
        mapped_input_fields.clone(),
    );
    Value::Object(request)
}

/// Both write paths admit a `mappedInputFields` that declares exactly one `UNIQUE_ID`
/// attribute and refuse every other count with `ValidationException` (HTTP 400), storing
/// nothing: a refused create leaves no schema mapping behind, and a refused update leaves
/// the stored one exactly as it was.
#[tokio::test]
async fn a_schema_mapping_is_written_only_with_exactly_one_unique_id_attribute() {
    let emulator = Emulator::new();
    let baseline = baseline_fields();

    for (index, case) in cardinality_cases().iter().enumerate() {
        let label = case.case;

        // The CREATE path.
        let created_name = format!("cardinality-create-{index}");
        let created = emulator
            .create_schema_mapping(&create_request(&created_name, &case.mapped_input_fields))
            .await;
        if case.admitted {
            assert_eq!(
                created.status, 200,
                "case {label:?}: expected CreateSchemaMapping to be admitted, got {created:?}"
            );
            assert_eq!(
                list_member(&created.body, "mappedInputFields"),
                case.mapped_input_fields
                    .as_array()
                    .expect("the row's mappedInputFields is a list"),
                "case {label:?}: expected the stored list to be served back, got {:?}",
                created.body
            );
        } else {
            assert_error_shape(label, &created, 400, "ValidationException");
            assert_eq!(
                string_member(&created.body, "message"),
                UNIQUE_ID_CARDINALITY,
                "case {label:?}: expected the message {UNIQUE_ID_CARDINALITY:?}, got {:?}",
                created.body
            );
            let read = emulator.get_schema_mapping(&created_name).await;
            assert_error_shape(label, &read, 404, "ResourceNotFoundException");
        }

        // The UPDATE path, over a schema mapping that already satisfies the rule.
        let updated_name = format!("cardinality-update-{index}");
        let seeded = emulator
            .create_schema_mapping(&create_request(&updated_name, &baseline))
            .await;
        assert_eq!(
            seeded.status, 200,
            "case {label:?}: seeding {updated_name:?} failed: {seeded:?}"
        );
        let update = update_schema_mapping(
            &emulator,
            &updated_name,
            &update_request(Some("updated probe schema mapping"), &case.mapped_input_fields),
        )
        .await;
        let read = emulator.get_schema_mapping(&updated_name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the seeded schema mapping must stay readable, got {read:?}"
        );
        if case.admitted {
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
                UNIQUE_ID_CARDINALITY,
                "case {label:?}: expected the message {UNIQUE_ID_CARDINALITY:?}, got {:?}",
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

/// One update, as the members it carries and what the schema mapping must read as
/// afterwards.
struct UpdateCase {
    case: &'static str,
    /// What the request sends for `description`: `None` is the member ABSENT, which the
    /// `PUT` says the schema now carries none of.
    description: Option<String>,
    mapped_input_fields: Value,
}

/// The table: the `mappedInputFields` list changes in each of the ways the rule
/// generalises over (an appended attribute, a removed one, a re-typed one, a reordered
/// one, back to the modelled minimum, out to the modelled maximum), paired with a
/// `description` over changed, emptied, absent and the modelled `length` maximum of 255.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "an appended attribute and a changed description",
            description: Some("updated probe schema mapping".to_string()),
            mapped_input_fields: json!([
                attribute("id", "UNIQUE_ID"),
                attribute("name", "NAME"),
                attribute("email", "EMAIL_ADDRESS"),
                attribute("phone", "PHONE_NUMBER"),
            ]),
        },
        UpdateCase {
            case: "a removed attribute, back to the modelled list minimum of two",
            description: Some("trimmed".to_string()),
            mapped_input_fields: json!([
                attribute("id", "UNIQUE_ID"),
                attribute("name", "NAME"),
            ]),
        },
        UpdateCase {
            case: "the same columns with one re-typed, and an emptied description",
            description: Some(String::new()),
            mapped_input_fields: json!([
                attribute("id", "UNIQUE_ID"),
                attribute("name", "NAME_FIRST"),
                attribute("email", "EMAIL_ADDRESS"),
            ]),
        },
        UpdateCase {
            case: "the same attributes reordered, with no description member at all",
            description: None,
            mapped_input_fields: json!([
                attribute("email", "EMAIL_ADDRESS"),
                attribute("name", "NAME"),
                attribute("id", "UNIQUE_ID"),
            ]),
        },
        UpdateCase {
            case: "out to the modelled list maximum of 60, description at its own maximum of 255",
            description: Some("d".repeat(255)),
            mapped_input_fields: fields(60, &[0]),
        },
        UpdateCase {
            case: "a re-named identifier column, which is the only change",
            description: Some("probe schema mapping".to_string()),
            mapped_input_fields: json!([
                attribute("record_id", "UNIQUE_ID"),
                attribute("name", "NAME"),
                attribute("email", "EMAIL_ADDRESS"),
            ]),
        },
    ]
}

/// `UpdateSchemaMapping` writes `description` and `mappedInputFields` into the stored
/// record, so its own response AND a later `GetSchemaMapping` serve the updated values
/// while the resource's identity and its tags survive.
#[tokio::test]
async fn update_schema_mapping_is_readable_through_get_schema_mapping() {
    let emulator = Emulator::new();

    for (index, case) in update_cases().iter().enumerate() {
        let label = case.case;
        let name = format!("updated-{index}");

        let created = emulator
            .create_schema_mapping(&create_request(&name, &baseline_fields()))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: seeding {name:?} failed: {created:?}"
        );
        let before = emulator.get_schema_mapping(&name).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: the seeded schema mapping must be readable, got {before:?}"
        );
        let created_at = timestamp_member(&before.body, "createdAt");

        let request = update_request(case.description.as_deref(), &case.mapped_input_fields);
        let update = update_schema_mapping(&emulator, &name, &request).await;
        assert_eq!(
            update.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateSchemaMapping, got {update:?}"
        );

        // The update's OWN response is served from the record it just wrote.
        assert_eq!(
            string_member(&update.body, "schemaArn"),
            expected_arn("schemamapping", &name),
            "case {label:?}: the update must keep the ARN minted at create time, got {:?}",
            update.body
        );
        assert_eq!(
            update.body.get("description").and_then(Value::as_str),
            case.description.as_deref(),
            "case {label:?}: expected the updated description, got {:?}",
            update.body
        );
        assert_eq!(
            list_member(&update.body, "mappedInputFields"),
            case.mapped_input_fields
                .as_array()
                .expect("the row's mappedInputFields is a list"),
            "case {label:?}: expected the updated list, got {:?}",
            update.body
        );

        // And a READ after the write serves exactly the same values: this is the
        // read-after-write the rule is about.
        let after = emulator.get_schema_mapping(&name).await;
        assert_eq!(
            after.status, 200,
            "case {label:?}: expected HTTP 200 from GetSchemaMapping, got {after:?}"
        );
        assert_eq!(
            after.body.get("description").and_then(Value::as_str),
            case.description.as_deref(),
            "case {label:?}: GetSchemaMapping must serve the updated description, got {:?}",
            after.body
        );
        assert_eq!(
            list_member(&after.body, "mappedInputFields"),
            case.mapped_input_fields
                .as_array()
                .expect("the row's mappedInputFields is a list"),
            "case {label:?}: GetSchemaMapping must serve the updated list, got {:?}",
            after.body
        );
        assert_eq!(
            after.body.get("mappedInputFields"),
            update.body.get("mappedInputFields"),
            "case {label:?}: the update response and the read are one record, got {:?} vs {:?}",
            update.body,
            after.body
        );

        // The identity members and the tags are not members of this request, so they
        // survive it; `updatedAt` is re-stamped, so it never precedes `createdAt`.
        assert_eq!(
            string_member(&after.body, "schemaName"),
            name,
            "case {label:?}: the name is the key and cannot change, got {:?}",
            after.body
        );
        assert_eq!(
            timestamp_member(&after.body, "createdAt"),
            created_at,
            "case {label:?}: createdAt is not a member of this request, got {:?}",
            after.body
        );
        assert!(
            timestamp_member(&after.body, "updatedAt") >= created_at,
            "case {label:?}: expected updatedAt at or after createdAt {created_at}, got {:?}",
            after.body
        );
        assert_eq!(
            after.body.get("tags"),
            before.body.get("tags"),
            "case {label:?}: the tags are not members of this request, got {:?}",
            after.body
        );
    }
}

/// A `CreateMatchingWorkflow` body whose one input source names `schema`.
fn workflow_request(name: &str, schema: &str) -> Value {
    json!({
        "workflowName": name,
        "inputSourceConfig": [
            { "inputSourceARN": glue_table_arn("customers"), "schemaName": schema },
        ],
        "outputSourceConfig": [{
            "outputS3Path": "s3://er-output/updates",
            "output": [{ "name": "id" }],
        }],
        "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
        "roleArn": role_arn(),
    })
}

/// An update of a schema mapping nothing was created under is the resource MISS — for any
/// name — and a schema mapping a matching workflow references is immutable until that
/// workflow is gone. Neither refusal changes the stored record.
#[tokio::test]
async fn update_schema_mapping_refuses_an_absent_schema_and_one_a_workflow_uses() {
    let emulator = Emulator::new();
    let request = update_request(Some("updated"), &baseline_fields());

    // (case, the `schemaName` label) — every character class the modelled `EntityName`
    // pattern `^[a-zA-Z_0-9-]*$` admits, at both its `length 1..255` bounds.
    let absent: Vec<(&str, String)> = vec![
        ("a single-letter name at the modelled minimum", "a".to_string()),
        ("mixed case letters", "Customers".to_string()),
        ("underscore, hyphen and digits together", "crm_people-1".to_string()),
        ("a digits-only name", "0".to_string()),
        ("a name at the modelled maximum of 255", "n".repeat(255)),
    ];
    for (case, name) in &absent {
        let reply = update_schema_mapping(&emulator, name, &request).await;
        assert_error_shape(case, &reply, 404, "ResourceNotFoundException");
        assert_eq!(
            string_member(&reply.body, "message"),
            "SchemaMapping does not exist",
            "case {case:?}: expected the service's own not-found text, got {:?}",
            reply.body
        );
    }

    // A schema mapping a workflow's `inputSourceConfig` names is immutable: "a schema is
    // immutable if it is being used by a workflow".
    for (index, (case, _)) in absent.iter().enumerate() {
        let schema = format!("referenced-{index}");
        let workflow = format!("referencing-{index}");
        let created = emulator
            .create_schema_mapping(&create_request(&schema, &baseline_fields()))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: seeding {schema:?} failed: {created:?}"
        );
        let seeded_workflow = emulator
            .create_matching_workflow(&workflow_request(&workflow, &schema))
            .await;
        assert_eq!(
            seeded_workflow.status, 200,
            "case {case:?}: seeding {workflow:?} failed: {seeded_workflow:?}"
        );

        let refused = update_schema_mapping(&emulator, &schema, &request).await;
        assert_error_shape(case, &refused, 400, "ConflictException");
        let read = emulator.get_schema_mapping(&schema).await;
        assert_eq!(
            list_member(&read.body, "mappedInputFields"),
            baseline_fields()
                .as_array()
                .expect("the baseline list is a list"),
            "case {case:?}: a refused update must leave the stored list alone, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("description").and_then(Value::as_str),
            Some("probe schema mapping"),
            "case {case:?}: a refused update must leave the stored description alone, got {:?}",
            read.body
        );

        // With the workflow gone the same update is admitted, which is what makes the
        // refusal above about the REFERENCE and not about the request.
        let deleted = emulator.delete_matching_workflow(&workflow).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: deleting {workflow:?} failed: {deleted:?}"
        );
        let admitted = update_schema_mapping(&emulator, &schema, &request).await;
        assert_eq!(
            admitted.status, 200,
            "case {case:?}: with the workflow gone the update must be admitted, got {admitted:?}"
        );
    }
}
