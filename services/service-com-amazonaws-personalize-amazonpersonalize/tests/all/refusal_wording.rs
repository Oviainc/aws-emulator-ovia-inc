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
//! resources (and of which types) hold the one being deleted, the KIND of resource an ARN
//! addresses and the operation that resolves it, and the shape of the ARN that resolves to
//! nothing — so the recorded case is one row among several.

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

/// What a lookup that resolves to nothing answers, in the wording Amazon Personalize uses
/// for the KIND of resource that was looked up: a schema lookup names the ARN alone, while
/// a dataset-group or a dataset lookup names the kind first and the ARN last. The ARN the
/// caller asked about is each sentence's one variable part.
///
/// Written out here rather than read from `crate::state` for the reason above: a test that
/// asserted the handler's own helper would pass whatever that helper said.
fn schema_not_found_message(arn: &str) -> String {
    format!("Resource Arn {arn} does not exist.")
}

/// The same, for an ARN that addresses no dataset group.
fn dataset_group_not_found_message(arn: &str) -> String {
    format!("The given dataset group does not exist: {arn}")
}

/// The same, for an ARN that addresses no dataset.
fn dataset_not_found_message(arn: &str) -> String {
    format!("The given dataset does not exist: {arn}")
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

/// One row of the not-found table: a well-formed ARN that addresses nothing, every
/// operation that has to resolve it, and the sentence its kind of resource is refused with.
struct AbsentRow {
    case: &'static str,
    arn: String,
    /// `(operation, the input member naming the ARN)` for each operation that resolves it.
    lookups: &'static [(&'static str, &'static str)],
    /// The wording this kind of resource's not-found refusal carries.
    not_found: fn(&str) -> String,
}

/// The operations that resolve a `schemaArn`, and so can answer the not-found class for
/// one. Source: this crate's `OPERATIONS.md` (`DescribeSchema`, `DeleteSchema`;
/// `CreateDataset` resolves one too and is exercised separately below, because it names
/// two ARNs at once).
const SCHEMA_LOOKUPS: &[(&str, &str)] = &[
    ("DescribeSchema", "schemaArn"),
    ("DeleteSchema", "schemaArn"),
];

/// The same for a `datasetGroupArn`.
const DATASET_GROUP_LOOKUPS: &[(&str, &str)] = &[
    ("DescribeDatasetGroup", "datasetGroupArn"),
    ("DeleteDatasetGroup", "datasetGroupArn"),
];

/// The same for a `datasetArn`.
const DATASET_LOOKUPS: &[(&str, &str)] = &[
    ("DescribeDataset", "datasetArn"),
    ("DeleteDataset", "datasetArn"),
];

#[tokio::test]
async fn a_lookup_that_resolves_to_nothing_names_the_kind_of_resource_it_looked_for() {
    let emulator = Emulator::new();

    // The rows vary what the rule generalises over: the KIND of resource the ARN addresses
    // (schema, dataset group, dataset), the operation that resolves it (a read and a
    // delete for each), and the shape of the ARN itself (a plain name, every permitted
    // character class, a long name, a dataset's two-segment resource part, another region
    // and account). Each kind's rows expect that kind's sentence, with their own ARN.
    let rows: Vec<AbsentRow> = vec![
        AbsentRow {
            case: "a schema under a plain name",
            arn: format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/absent-kind"),
            lookups: SCHEMA_LOOKUPS,
            not_found: schema_not_found_message,
        },
        AbsentRow {
            case: "a dataset group under a plain name",
            arn: format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/absent-kind"),
            lookups: DATASET_GROUP_LOOKUPS,
            not_found: dataset_group_not_found_message,
        },
        AbsentRow {
            case: "a dataset group with dashes, digits and underscores",
            arn: format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/Absent-9_dsg"),
            lookups: DATASET_GROUP_LOOKUPS,
            not_found: dataset_group_not_found_message,
        },
        AbsentRow {
            case: "a dataset group under a long name",
            arn: format!(
                "arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/{}",
                "g".repeat(63)
            ),
            lookups: DATASET_GROUP_LOOKUPS,
            not_found: dataset_group_not_found_message,
        },
        AbsentRow {
            case: "a dataset group in another region and account",
            arn: "arn:aws:personalize:eu-west-1:210987654321:dataset-group/elsewhere".to_string(),
            lookups: DATASET_GROUP_LOOKUPS,
            not_found: dataset_group_not_found_message,
        },
        AbsentRow {
            case: "a dataset addressed by group and type",
            arn: format!(
                "arn:aws:personalize:{REGION}:{ACCOUNT}:dataset/absent-kind/INTERACTIONS"
            ),
            lookups: DATASET_LOOKUPS,
            not_found: dataset_not_found_message,
        },
        AbsentRow {
            case: "a dataset of another type in a group that exists",
            arn: format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset/present-kind/ACTIONS"),
            lookups: DATASET_LOOKUPS,
            not_found: dataset_not_found_message,
        },
        AbsentRow {
            case: "a dataset in another region and account",
            arn: "arn:aws:personalize:eu-west-1:210987654321:dataset/elsewhere/USERS".to_string(),
            lookups: DATASET_LOOKUPS,
            not_found: dataset_not_found_message,
        },
    ];

    // A group and a schema that DO exist, so a row addressing a dataset inside a live
    // group is refused for the dataset and not for its container.
    let setup = "not-found wording setup";
    let present_group = create_active_group(&emulator, "present-kind", setup).await;
    let present_schema =
        create_schema(&emulator, "present-kind", &avro_schema("Interactions"), setup).await;

    for row in &rows {
        let expected = (row.not_found)(&row.arn);
        for (operation, member) in row.lookups {
            assert_refused(
                &emulator,
                operation,
                member,
                &row.arn,
                (400, "ResourceNotFoundException", &expected),
                row.case,
            )
            .await;
        }
    }

    // `CreateDataset` resolves two ARNs, and words whichever of them resolves to nothing
    // the same way a read of that resource would — so the two lookups are told apart.
    for (case, group_arn, schema_arn, expected) in [
        (
            "CreateDataset naming a group that does not exist",
            format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/absent-kind"),
            present_schema.clone(),
            dataset_group_not_found_message(&format!(
                "arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/absent-kind"
            )),
        ),
        (
            "CreateDataset naming a schema that does not exist",
            present_group.clone(),
            format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/absent-kind"),
            schema_not_found_message(&format!(
                "arn:aws:personalize:{REGION}:{ACCOUNT}:schema/absent-kind"
            )),
        ),
        (
            // Both ARNs unresolvable: the group is resolved first, so its own kind's
            // sentence is the one answered.
            "CreateDataset naming neither a group nor a schema that exists",
            format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/absent-kind"),
            format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/absent-kind"),
            dataset_group_not_found_message(&format!(
                "arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/absent-kind"
            )),
        ),
    ] {
        let (status, body) = emulator
            .call(
                "CreateDataset",
                serde_json::json!({
                    "name": "needs-both",
                    "schemaArn": schema_arn,
                    "datasetGroupArn": group_arn,
                    "datasetType": "Items",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateDataset to answer HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // A dataset and a group that EXIST are read, and once deleted their own ARNs answer
    // their kind's not-found sentence — so the wording belongs to the lookup that found
    // nothing rather than to every call.
    let case = "a group and a dataset that exist, then do not";
    let dataset_arn = create_dataset(
        &emulator,
        &present_group,
        &present_schema,
        "Interactions",
        case,
    )
    .await;
    for (operation, member, arn) in [
        ("DescribeDataset", "datasetArn", &dataset_arn),
        ("DescribeDatasetGroup", "datasetGroupArn", &present_group),
    ] {
        let (status, body) = emulator.call_with_arn(operation, member, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {body:?}"
        );
    }

    assert_deleted(&emulator, "DeleteDataset", "datasetArn", &dataset_arn, case).await;
    assert_deleted(
        &emulator,
        "DeleteDatasetGroup",
        "datasetGroupArn",
        &present_group,
        case,
    )
    .await;
    for (lookups, arn, not_found) in [
        (
            DATASET_LOOKUPS,
            &dataset_arn,
            dataset_not_found_message as fn(&str) -> String,
        ),
        (
            DATASET_GROUP_LOOKUPS,
            &present_group,
            dataset_group_not_found_message as fn(&str) -> String,
        ),
    ] {
        let expected = not_found(arn);
        for (operation, member) in lookups {
            assert_refused(
                &emulator,
                operation,
                member,
                arn,
                (400, "ResourceNotFoundException", &expected),
                case,
            )
            .await;
        }
    }
}
