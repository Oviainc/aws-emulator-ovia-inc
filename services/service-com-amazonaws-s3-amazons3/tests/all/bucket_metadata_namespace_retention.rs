// SPDX-License-Identifier: CC0-1.0

//! What the AWS-managed table bucket's namespace keeps, once a metadata configuration has
//! provisioned a table in it.
//!
//! The rule under test: **a metadata table that came into service OUTLIVES the configuration
//! that provisioned it. No S3 operation takes such a table out of the AWS-managed table
//! bucket's namespace — "delete the journal table, and then try again. To create a new S3
//! Metadata table, you must delete the S3 Metadata configuration for this bucket, and then
//! create a new S3 Metadata configuration" ([`METADATA_JOURNAL_TABLE_EXISTS_MESSAGE`], the
//! sentence the published V2 provisioning failure carries) makes deleting the configuration the
//! PRECONDITION for a new one and the leftover table a separate deletion the caller still owes
//! — so once a table in service holds a name, every later create whose table would be created
//! under it is ACCEPTED (HTTP 200, empty body: the collision is a provisioning outcome, not a
//! refusal) and reports that table `FAILED` with the published `*AlreadyExists` code and
//! sentence, with `JournalTableNotAvailable` for an inventory table whose own name is free but
//! whose journal table failed. Every row here therefore waits for the first configuration's
//! tables to reach `ACTIVE`; a configuration taken away before its tables ever came into
//! service leaves no table behind and frees their names, which is the sibling rule
//! [`super::bucket_metadata_unprovisioned_table_release`] spans.**
//!
//! That is what a caller sees whenever it recycles a metadata configuration: deleting the
//! configuration and creating it again does not get the tables back, and a cleanup path that
//! has to drop the journal table through the table API that owns it is what unblocks the new
//! configuration.
//!
//! The rows differ along what the rule generalises over: which tables the first configuration
//! provisioned (the journal table alone, with an inventory table, with an annotation table),
//! what happened to that configuration in between (deleted, deleted twice, or the general
//! purpose bucket itself deleted and created again), which bucket makes the second create
//! (the same bucket, or another whose name folds to the same table names), and which tables
//! that second create asks for (the same set, or one more than the first provisioned, whose
//! own name is therefore free).
//!
//! The ANNOTATION table is the one table a colliding create still provisions, and the rule
//! above is why: the published list of V2 provisioning failures names
//! `JournalTableAlreadyExists`, `InventoryTableAlreadyExists`, `JournalTableNotAvailable`,
//! `AccessDeniedCreatingResources`, `AccessDeniedWritingToTable`, `DestinationTableNotFound`,
//! `ServerInternalError` and `NoSuchBucket` and NO failure for an annotation table
//! (`smithy_query(query="doc",shape="com.amazonaws.s3#ErrorDetails",member="ErrorCode",
//! model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`),
//! so there is no code or sentence to report one with — which is what the annotation row below
//! asserts.

use service_com_amazonaws_s3_amazons3::state::{
    METADATA_INVENTORY_TABLE_EXISTS_CODE, METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
    METADATA_JOURNAL_TABLE_EXISTS_CODE, METADATA_JOURNAL_TABLE_EXISTS_MESSAGE,
    METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE, METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
    METADATA_TABLE_STATUS_ACTIVE, METADATA_TABLE_STATUS_FAILED,
};
use super::bucket_lifecycle::{create_bucket, router};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, wait_for_the_tables,
};
use super::bucket_metadata_configuration_deletion::{
    bucket_with_an_active_configuration, delete_metadata_configuration, payload, reported,
};
use super::object_and_tagging_roundtrip::text;
use super::object_storage::delete_bucket;

/// What happened to the first configuration before the second create is made.
#[derive(Debug, Clone, Copy)]
enum Between {
    /// `DeleteBucketMetadataConfiguration` once.
    ConfigurationDeleted,
    /// The same delete twice: the second one finds no configuration to delete and is
    /// refused, which must release no more than the first one did.
    ConfigurationDeletedTwice,
    /// The configuration deleted and then the general purpose bucket itself, which is created
    /// again under the same name — the bucket's own life does not reach into the table
    /// bucket's namespace either.
    BucketDeletedAndCreatedAgain,
}

/// One row: the configuration that provisions the names, what happens to it, and the create
/// that then collides with them.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The general purpose bucket whose configuration provisions the names first.
    first_bucket: &'static str,
    /// Whether that first configuration asks for an inventory table, and an annotation table.
    first: (bool, bool),
    /// What happens to the first configuration before the second create.
    between: Between,
    /// The bucket the second create names: the same one, or another whose name folds to the
    /// same table names (a dot where the first has a hyphen).
    second_bucket: &'static str,
    /// Whether the second create asks for an inventory table, and an annotation table.
    second: (bool, bool),
    /// The code and sentence the second create's inventory table must report, when it asks
    /// for one.
    expected_inventory: Option<(&'static str, &'static str)>,
}

/// The rows, spanning the forms the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the journal table alone, re-created identically",
            first_bucket: "namespace-journal",
            first: (false, false),
            between: Between::ConfigurationDeleted,
            second_bucket: "namespace-journal",
            second: (false, false),
            expected_inventory: None,
        },
        Row {
            case: "a journal and an inventory table, re-created identically",
            first_bucket: "namespace-inventory",
            first: (true, false),
            between: Between::ConfigurationDeleted,
            second_bucket: "namespace-inventory",
            second: (true, false),
            expected_inventory: Some((
                METADATA_INVENTORY_TABLE_EXISTS_CODE,
                METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
            )),
        },
        Row {
            case: "a journal table alone, re-created asking for an inventory table too",
            first_bucket: "namespace-added-inventory",
            first: (false, false),
            between: Between::ConfigurationDeleted,
            second_bucket: "namespace-added-inventory",
            second: (true, false),
            // The inventory table's own name is free — the first configuration never
            // provisioned one — so what it reports is the journal table it relies on.
            expected_inventory: Some((
                METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE,
                METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
            )),
        },
        Row {
            case: "a journal and an annotation table, after a refused second delete",
            first_bucket: "namespace-annotation",
            first: (false, true),
            between: Between::ConfigurationDeletedTwice,
            second_bucket: "namespace-annotation",
            second: (false, true),
            expected_inventory: None,
        },
        Row {
            case: "the journal table alone, after the bucket itself was deleted",
            first_bucket: "namespace-bucket-cycle",
            first: (false, false),
            between: Between::BucketDeletedAndCreatedAgain,
            second_bucket: "namespace-bucket-cycle",
            second: (false, false),
            expected_inventory: None,
        },
        Row {
            case: "another bucket whose name folds to the same table names",
            first_bucket: "namespace-folded-name",
            first: (false, false),
            between: Between::ConfigurationDeleted,
            second_bucket: "namespace.folded.name",
            second: (false, false),
            expected_inventory: None,
        },
    ]
}

#[tokio::test]
async fn a_table_outlives_the_configuration_that_provisioned_it() {
    let router = router();

    for row in rows() {
        let Row {
            case,
            first_bucket,
            first,
            between,
            second_bucket,
            second,
            expected_inventory,
        } = row;

        // A configuration whose tables are in service, so the names really are the
        // namespace's before anything is taken away.
        bucket_with_an_active_configuration(&router, first_bucket, first.0, first.1).await;

        let deleted = delete_metadata_configuration(&router, first_bucket, &[]).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: the configuration delete must succeed, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );
        match between {
            Between::ConfigurationDeleted => {}
            Between::ConfigurationDeletedTwice => {
                let again = delete_metadata_configuration(&router, first_bucket, &[]).await;
                assert_eq!(
                    again.status, 404,
                    "case {case:?}: a second delete finds no configuration to delete, got {} \
                     with body {:?}",
                    again.status,
                    text(&again)
                );
            }
            Between::BucketDeletedAndCreatedAgain => {
                let dropped = delete_bucket(&router, first_bucket).await;
                assert_eq!(
                    dropped.status, 204,
                    "case {case:?}: the bucket delete must succeed, got {} with body {:?}",
                    dropped.status,
                    text(&dropped)
                );
                let remade = create_bucket(&router, first_bucket, None, None).await;
                assert_eq!(
                    remade.status, 200,
                    "case {case:?}: the bucket must be created again, got {} with body {:?}",
                    remade.status,
                    text(&remade)
                );
            }
        }

        // A row that collides from ANOTHER bucket needs that bucket to exist first.
        if second_bucket != first_bucket {
            let created = create_bucket(&router, second_bucket, None, None).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the colliding bucket must be created, got {} with body {:?}",
                created.status,
                text(&created)
            );
        }

        // The second create is ACCEPTED: the bucket carries a configuration, whatever its
        // tables could do.
        let recreated =
            create_metadata_configuration(&router, second_bucket, &[], &payload(second.0, second.1))
                .await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the create must be accepted, got {} with body {:?}",
            recreated.status,
            text(&recreated)
        );
        assert!(
            recreated.body.is_empty(),
            "case {case:?}: the create answers an empty body, got {:?}",
            text(&recreated)
        );

        // Its journal table never reaches ACTIVE: the name is still the namespace's, so
        // provisioning failed with the published code and sentence — before and after the span
        // a successful creation would take, since nothing is going to create it.
        for when in ["straight after the create", "after the activation span"] {
            if when == "after the activation span" {
                wait_for_the_tables();
            }
            let body = text(&get_metadata_configuration(&router, second_bucket, &[]).await);
            let (status, code, message) = reported(&body, "JournalTableConfigurationResult");
            assert_eq!(
                status.as_deref(),
                Some(METADATA_TABLE_STATUS_FAILED),
                "case {case:?} ({when}): the journal table must report \
                 {METADATA_TABLE_STATUS_FAILED}, got {body:?}"
            );
            assert_eq!(
                code.as_deref(),
                Some(METADATA_JOURNAL_TABLE_EXISTS_CODE),
                "case {case:?} ({when}): expected the {METADATA_JOURNAL_TABLE_EXISTS_CODE} \
                 error code, got {code:?}"
            );
            assert_eq!(
                message.as_deref(),
                Some(METADATA_JOURNAL_TABLE_EXISTS_MESSAGE),
                "case {case:?} ({when}): expected the published sentence, got {message:?}"
            );
        }

        let body = text(&get_metadata_configuration(&router, second_bucket, &[]).await);
        // What the inventory table reports, when the create asked for one: its own name being
        // taken, or the journal table it relies on having failed.
        let (status, code, message) = reported(&body, "InventoryTableConfigurationResult");
        match expected_inventory {
            None => assert_eq!(
                status, None,
                "case {case:?}: a create naming no inventory table reports none, got {body:?}"
            ),
            Some((expected_code, expected_message)) => {
                assert_eq!(
                    status.as_deref(),
                    Some(METADATA_TABLE_STATUS_FAILED),
                    "case {case:?}: the inventory table must report \
                     {METADATA_TABLE_STATUS_FAILED}, got {body:?}"
                );
                assert_eq!(
                    code.as_deref(),
                    Some(expected_code),
                    "case {case:?}: expected the {expected_code} error code, got {code:?}"
                );
                assert_eq!(
                    message.as_deref(),
                    Some(expected_message),
                    "case {case:?}: expected the published sentence, got {message:?}"
                );
            }
        }

        // The ANNOTATION table is unaffected by the journal table's failure: the published V2
        // list carries no provisioning failure for an annotation table at all (see this
        // suite's header for the member it is read from), so there is no code or sentence to
        // report one with and the table is created and reaches service.
        let (status, code, _) = reported(&body, "AnnotationTableConfigurationResult");
        match second.1 {
            false => assert_eq!(
                status, None,
                "case {case:?}: a create naming no annotation table reports none, got {body:?}"
            ),
            true => {
                assert_eq!(
                    status.as_deref(),
                    Some(METADATA_TABLE_STATUS_ACTIVE),
                    "case {case:?}: the annotation table must report \
                     {METADATA_TABLE_STATUS_ACTIVE}, got {body:?}"
                );
                assert_eq!(
                    code, None,
                    "case {case:?}: a table that was created reports no error code, got \
                     {code:?}"
                );
            }
        }
    }
}
