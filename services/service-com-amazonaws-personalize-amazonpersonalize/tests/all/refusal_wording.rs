// SPDX-License-Identifier: CC0-1.0

//! What Personalize's resource refusals SAY, over the wire.
//!
//! A modelled error carries the service's own wording for its class: the fixed text of
//! that class's sentence, and only the parts the service itself varies (here, the ARN a
//! lookup was asked about). A refusal is not a place to compose a local diagnostic, so
//! neither the request nor the store may add a value to a sentence the service keeps
//! constant.
//!
//! Each suite below is a table whose rows vary what the rule generalises over — how many
//! resources (and of which types) hold the one being deleted, and the shape of the ARN
//! that resolves to nothing — so the recorded case is one row among several.

use crate::dataset_group_lifecycle::{
    assert_deleted, avro_schema, create_active_group, create_dataset, create_schema, error_code,
    error_message, Emulator, ACCOUNT, REGION,
};

/// What Amazon Personalize answers when a schema is still referenced by a dataset. The
/// whole sentence is the service's fixed text for the class: it names neither the schema
/// nor the datasets referencing it, however many there are.
///
/// The three expectations here are deliberately written out rather than read from
/// `crate::state`: a test that asserted the handler's own constant would pass whatever
/// that constant said. Keep them independent.
pub(crate) const SCHEMA_IN_USE_MESSAGE: &str =
    "The provided schema is being referenced by one or more datasets";

/// The same, for a dataset group that still holds datasets.
pub(crate) const DATASET_GROUP_IN_USE_MESSAGE: &str =
    "The provided dataset-group is being referenced by one or more datasets";

/// What a schema lookup that resolves to nothing answers. The ARN the caller asked about
/// is the sentence's one variable part.
fn schema_not_found_message(schema_arn: &str) -> String {
    format!("Resource Arn {schema_arn} does not exist.")
}

/// A row of the in-use table: the dataset types created against one schema in one group
/// before the deletes are attempted. Each type is one from the model's documented
/// `DatasetType` set (`DATASET_TYPE_VALUES` in `src/state.rs`), and the rows differ in how
/// many datasets hold the schema and the group — one, two, three — which is exactly what
/// the old per-request message interpolated and the rule says it may not.
struct InUseRow {
    schema: &'static str,
    referencing_types: &'static [&'static str],
}

const IN_USE_CATALOG: &[InUseRow] = &[
    InUseRow {
        schema: "wording-one-reference",
        referencing_types: &["Interactions"],
    },
    InUseRow {
        schema: "wording-two-references",
        referencing_types: &["Interactions", "Items"],
    },
    InUseRow {
        schema: "wording-three-references",
        referencing_types: &["Actions", "Items", "Users"],
    },
];

/// Assert `operation` on `arn` answers `status`, `code` and exactly `message`.
async fn assert_refused(
    emulator: &Emulator,
    operation: &str,
    member: &str,
    arn: &str,
    expected: (u16, &str, &str),
    case: &str,
) {
    let (expected_status, expected_code, expected_message) = expected;
    let (status, body) = emulator.call_with_arn(operation, member, arn).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected {operation} to answer HTTP {expected_status}, got {status} \
         {body:?}"
    );
    assert!(
        error_code(&body).contains(expected_code),
        "case {case:?}: expected {operation} to answer {expected_code}, got {body:?}"
    );
    let message = error_message(&body);
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected {operation} to answer {expected_message:?}, got {message:?}"
    );
}

#[tokio::test]
async fn an_in_use_refusal_reads_the_same_however_many_resources_hold_it() {
    let emulator = Emulator::new();

    for row in IN_USE_CATALOG {
        let group_arn = create_active_group(
            &emulator,
            &format!("{}-dsg", row.schema),
            "in-use wording setup",
        )
        .await;
        let schema_arn = create_schema(
            &emulator,
            row.schema,
            &avro_schema("Interactions"),
            "in-use wording setup",
        )
        .await;

        let mut outstanding: Vec<String> = Vec::new();
        for dataset_type in row.referencing_types {
            outstanding.push(
                create_dataset(
                    &emulator,
                    &group_arn,
                    &schema_arn,
                    dataset_type,
                    "in-use wording setup",
                )
                .await,
            );
        }

        // Every count from the row's down to one: the sentence does not move with it, and
        // it never carries an ARN, a count or a dataset name.
        while let Some(dataset_arn) = outstanding.pop() {
            let case = format!(
                "schema {} held by {} dataset(s)",
                row.schema,
                outstanding.len() + 1
            );
            assert_refused(
                &emulator,
                "DeleteSchema",
                "schemaArn",
                &schema_arn,
                (400, "ResourceInUseException", SCHEMA_IN_USE_MESSAGE),
                &case,
            )
            .await;
            assert_refused(
                &emulator,
                "DeleteDatasetGroup",
                "datasetGroupArn",
                &group_arn,
                (400, "ResourceInUseException", DATASET_GROUP_IN_USE_MESSAGE),
                &case,
            )
            .await;

            assert_deleted(
                &emulator,
                "DeleteDataset",
                "datasetArn",
                &dataset_arn,
                &case,
            )
            .await;
        }

        // With nothing holding them, both deletes go through, so the wording above was the
        // refusal's and not this emulator's answer to every delete.
        let case = format!("schema {} held by nothing", row.schema);
        assert_deleted(&emulator, "DeleteSchema", "schemaArn", &schema_arn, &case).await;
        assert_deleted(
            &emulator,
            "DeleteDatasetGroup",
            "datasetGroupArn",
            &group_arn,
            &case,
        )
        .await;
    }
}

#[tokio::test]
async fn a_schema_lookup_that_resolves_to_nothing_names_the_arn_it_was_asked_about() {
    let emulator = Emulator::new();

    // The rows vary the shape of the ARN the caller asks about: a plain name, a name of
    // every permitted character class, a long one, and one scoped to another region and
    // account — the ARN is the sentence's only variable part, so each has to come back in
    // its own reply.
    let absent: &[(&str, String)] = &[
        (
            "a plain name",
            format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/absent"),
        ),
        (
            "dashes, digits and underscores",
            format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/Absent-9_schema"),
        ),
        (
            "a long name",
            format!(
                "arn:aws:personalize:{REGION}:{ACCOUNT}:schema/{}",
                "n".repeat(63)
            ),
        ),
        (
            "another region and account",
            "arn:aws:personalize:eu-west-1:210987654321:schema/elsewhere".to_string(),
        ),
    ];

    for (index, (case, arn)) in absent.iter().enumerate() {
        let expected = schema_not_found_message(arn);
        for operation in ["DescribeSchema", "DeleteSchema"] {
            assert_refused(
                &emulator,
                operation,
                "schemaArn",
                arn,
                (400, "ResourceNotFoundException", &expected),
                case,
            )
            .await;
        }

        // A create that has to resolve the schema answers the same class the same way.
        let group_arn =
            create_active_group(&emulator, &format!("absent-schema-{index}-dsg"), case).await;
        let (status, body) = emulator
            .call(
                "CreateDataset",
                serde_json::json!({
                    "name": "needs-a-schema",
                    "schemaArn": arn,
                    "datasetGroupArn": group_arn,
                    "datasetType": "Interactions",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateDataset against an absent schema to answer HTTP 400, \
             got {status} {body:?}"
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected CreateDataset to answer {expected:?}, got {body:?}"
        );
        assert_deleted(
            &emulator,
            "DeleteDatasetGroup",
            "datasetGroupArn",
            &group_arn,
            case,
        )
        .await;
    }

    // A schema that EXISTS is described, so the wording above belongs to the lookup that
    // found nothing rather than to every schema read.
    let case = "a schema that exists";
    let schema_arn = create_schema(&emulator, "present", &avro_schema("Items"), case).await;
    let (status, body) = emulator
        .call_with_arn("DescribeSchema", "schemaArn", &schema_arn)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeSchema to answer 200, got {status} {body:?}"
    );
    assert_deleted(&emulator, "DeleteSchema", "schemaArn", &schema_arn, case).await;
    assert_refused(
        &emulator,
        "DescribeSchema",
        "schemaArn",
        &schema_arn,
        (
            400,
            "ResourceNotFoundException",
            &schema_not_found_message(&schema_arn),
        ),
        case,
    )
    .await;
}
