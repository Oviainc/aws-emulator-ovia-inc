// SPDX-License-Identifier: CC0-1.0

//! What `CreateSchema` requires of the Avro document it is handed, per dataset type.
//!
//! A Personalize schema "is associated with a dataset type and has a set of required field
//! and keywords" (`smithy_query(query="doc", shape="CreateSchema")`), and the dataset type
//! a document is written for is the Avro record's own `name`. So a document that does not
//! declare every field its type requires is refused at the call that submitted it, with
//! `InvalidInputException` naming exactly the fields it left out — never accepted, stored
//! and handed back an ARN.
//!
//! The suites are tables, not single calls: the rows walk every documented dataset type
//! and, for each, EVERY subset of its required fields, so the recorded case (an
//! interactions record declaring the user field and neither the item nor the time) is one
//! row among many.

use serde_json::{json, Value};

use crate::dataset_group_lifecycle::{error_code, error_message, Emulator, ACCOUNT, REGION};

/// The fields a dataset schema of each dataset type must declare, in the order a refusal
/// lists the ones a document left out.
///
/// The dataset types are the model's own documented set for
/// `CreateDatasetRequest$datasetType` ("Interactions, Items, Users, Actions,
/// Action_Interactions"); the model does not enumerate each type's required fields, so the
/// sets below are Amazon Personalize's own, spelled as a refusal words them. They are
/// written out here rather than read from `crate::state`: a test that asserted the
/// handler's own catalog would pass whatever that catalog said.
pub(crate) const REQUIRED_FIELDS: &[(&str, &[&str])] = &[
    ("Interactions", &["user_id", "item_id", "timestamp"]),
    ("Items", &["item_id"]),
    ("Users", &["user_id"]),
    ("Actions", &["action_id"]),
    (
        "Action_Interactions",
        &["user_id", "action_id", "timestamp", "event_type"],
    ),
];

/// The fields `dataset_type` requires, or nothing at all when it names no dataset type.
pub(crate) fn required_fields(dataset_type: &str) -> &'static [&'static str] {
    REQUIRED_FIELDS
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(dataset_type))
        .map(|(_, fields)| *fields)
        .unwrap_or(&[])
}

/// What a document that left `missing` out answers: the class's fixed sentence followed by
/// the bracketed, comma-separated names.
fn missing_fields_message(missing: &[&str]) -> String {
    format!("Schema is missing required fields: [{}]", missing.join(", "))
}

/// An Avro record named `record_name` declaring `declared`, each as a string field.
fn record(record_name: &str, declared: &[String]) -> String {
    json!({
        "type": "record",
        "name": record_name,
        "namespace": "com.amazonaws.personalize.schema",
        "fields": declared
            .iter()
            .map(|field| json!({"name": field, "type": "string"}))
            .collect::<Vec<Value>>(),
        "version": "1.0",
    })
    .to_string()
}

/// The ARN this service mints for the schema `name`, so a create's answer can be predicted
/// rather than echoed.
fn expected_schema_arn(name: &str) -> String {
    format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/{name}")
}

/// Assert `CreateSchema` refuses `document` with HTTP 400 `InvalidInputException` and
/// exactly `expected_message`, and that it stored nothing under `name`.
async fn assert_refused(
    emulator: &Emulator,
    name: &str,
    document: &str,
    expected_message: &str,
    case: &str,
) {
    let (status, body) = emulator
        .call("CreateSchema", json!({"name": name, "schema": document}))
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected CreateSchema to answer HTTP 400, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case {case:?}: expected InvalidInputException, got {body:?}"
    );
    let message = error_message(&body);
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected {expected_message:?}, got {message:?}"
    );

    // A refused document is not a created schema: the name it asked for resolves to
    // nothing afterwards.
    let arn = expected_schema_arn(name);
    let (status, described) = emulator
        .call_with_arn("DescribeSchema", "schemaArn", &arn)
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected DescribeSchema to answer HTTP 400 after the refusal, got \
         {status} {described:?}"
    );
    assert!(
        error_code(&described).contains("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException after the refusal, got {described:?}"
    );
}

/// Assert `CreateSchema` accepts `document` under `name` and that `DescribeSchema` reports
/// back the very document that was stored.
async fn assert_created(emulator: &Emulator, name: &str, document: &str, case: &str) {
    let (status, body) = emulator
        .call("CreateSchema", json!({"name": name, "schema": document}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateSchema to answer HTTP 200, got {status} {body:?}"
    );
    let arn = body["schemaArn"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        arn,
        expected_schema_arn(name),
        "case {case:?}: expected the ARN this service mints for the name, got {body:?}"
    );

    let (status, described) = emulator
        .call_with_arn("DescribeSchema", "schemaArn", &arn)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeSchema to answer HTTP 200, got {status} {described:?}"
    );
    let stored = described["schema"]["schema"].as_str().unwrap_or_default();
    assert_eq!(
        stored, document,
        "case {case:?}: expected the stored document back, got {stored:?}"
    );
}

#[tokio::test]
async fn a_schema_must_declare_every_field_its_dataset_type_requires() {
    let emulator = Emulator::new();

    for (dataset_type, required) in REQUIRED_FIELDS {
        // Every subset of this type's required set — the complete document, each single
        // omission, each pair, down to the document declaring none of them — so what is
        // asserted is the rule and not one recorded omission. A metadata field no type
        // requires rides along in every row, so an extra field neither satisfies a
        // requirement nor provokes a refusal.
        for mask in 0..(1u32 << required.len()) {
            let declared: Vec<String> = required
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, field)| field.to_ascii_uppercase())
                .collect();
            let missing: Vec<&str> = required
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) == 0)
                .map(|(_, field)| *field)
                .collect();

            let mut fields = declared.clone();
            fields.push("SOME_METADATA".to_string());
            let document = record(dataset_type, &fields);
            let name = format!("{}-{mask}", dataset_type.to_ascii_lowercase());
            let case = format!("{dataset_type} record declaring {declared:?}");

            if missing.is_empty() {
                assert_created(&emulator, &name, &document, &case).await;
            } else {
                let expected = missing_fields_message(&missing);
                assert_refused(&emulator, &name, &document, &expected, &case).await;
            }
        }
    }
}

#[tokio::test]
async fn a_declared_field_counts_whatever_case_it_is_written_in() {
    let emulator = Emulator::new();

    // A dataset type is matched case-insensitively, so the field names of the document
    // written for it are too: each spelling below declares the one field a Users record
    // requires.
    for (index, spelling) in ["USER_ID", "user_id", "User_Id", "uSeR_iD"]
        .into_iter()
        .enumerate()
    {
        let document = record("Users", &[spelling.to_string()]);
        let case = format!("a Users record declaring {spelling:?}");
        assert_created(&emulator, &format!("case-{index}"), &document, &case).await;
    }

    // And a name that is not the required field, in any case, is still not that field.
    for (index, spelling) in ["USERID", "user-id", "USER_IDS"].into_iter().enumerate() {
        let document = record("Users", &[spelling.to_string()]);
        let case = format!("a Users record declaring only {spelling:?}");
        assert_refused(
            &emulator,
            &format!("near-miss-{index}"),
            &document,
            &missing_fields_message(&["user_id"]),
            &case,
        )
        .await;
    }
}

/// One document that is not checked against any per-type field set: the record names no
/// dataset type, so there is no set of required fields to check it against — the type a
/// dataset uses it as is settled by `CreateDataset`.
struct UncheckedRow {
    case: &'static str,
    name: &'static str,
    record_name: &'static str,
}

const UNCHECKED_CATALOG: &[UncheckedRow] = &[
    UncheckedRow {
        case: "a record named for nothing this service knows",
        name: "unchecked-plain",
        record_name: "Catalog",
    },
    UncheckedRow {
        case: "a record named for a type that does not exist",
        name: "unchecked-near-miss",
        record_name: "Interaction",
    },
    UncheckedRow {
        case: "a record with an empty name",
        name: "unchecked-empty",
        record_name: "",
    },
];

#[tokio::test]
async fn a_record_that_names_no_dataset_type_is_checked_against_no_field_set() {
    let emulator = Emulator::new();

    for row in UNCHECKED_CATALOG {
        let document = record(row.record_name, &["ANYTHING".to_string()]);
        assert_created(&emulator, row.name, &document, row.case).await;
    }
}

#[tokio::test]
async fn a_field_entry_that_declares_no_name_declares_nothing() {
    let emulator = Emulator::new();

    // An entry carrying no `name`, a name that is not a string, and a field list with no
    // entries at all: none of them declares the field an Items record requires.
    let documents: &[(&str, &str, String)] = &[
        (
            "an entry with no name",
            "nameless-entry",
            json!({
                "type": "record",
                "name": "Items",
                "fields": [{"type": "string"}],
            })
            .to_string(),
        ),
        (
            "a name that is not a string",
            "non-string-name",
            json!({
                "type": "record",
                "name": "Items",
                "fields": [{"name": ["ITEM_ID"], "type": "string"}],
            })
            .to_string(),
        ),
        (
            "an empty field list",
            "no-entries",
            json!({"type": "record", "name": "Items", "fields": []}).to_string(),
        ),
    ];

    for (case, name, document) in documents {
        assert_refused(
            &emulator,
            name,
            document,
            &missing_fields_message(&["item_id"]),
            case,
        )
        .await;
    }
}
