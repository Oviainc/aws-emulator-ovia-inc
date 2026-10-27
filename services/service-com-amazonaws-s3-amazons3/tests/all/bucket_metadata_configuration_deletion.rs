// SPDX-License-Identifier: CC0-1.0

//! Taking a bucket's S3 METADATA configuration away, and what a create finds afterwards.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate already owns), so what is pinned is the wire
//! behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rules under test:
//!
//! 1. `DeleteBucketMetadataConfiguration` "deletes an S3 Metadata configuration from a
//!    general purpose bucket" (that operation's own documentation): it answers the modelled
//!    204 with an empty body, a later `GetBucketMetadataConfiguration` reports
//!    `NoSuchConfiguration` (404), and the call is idempotent — a bucket carrying no
//!    configuration is already in the asked-for state. A bucket this emulator does not hold
//!    is `NoSuchBucket` (404) and an `x-amz-expected-bucket-owner` that is not the account's
//!    is `AccessDenied` (403).
//! 2. The delete takes the CONFIGURATION, not the tables it created: those are S3 Tables
//!    resources in the managed table bucket's namespace, which is why AWS tells a caller
//!    that creates the configuration again to "Delete the journal table, and then try again"
//!    (`ErrorDetails$ErrorCode`, `JournalTableAlreadyExists`). So an identical create after a
//!    delete is ACCEPTED — the bucket carries a configuration again — and its journal table
//!    reports `TableStatus` `FAILED` with that error code and the sentence it publishes,
//!    instead of ever reaching `ACTIVE`.
//! 3. An inventory table in the same create reports why it could not be created either: its
//!    own name taken (`InventoryTableAlreadyExists`) when the deleted configuration had one,
//!    and `JournalTableNotAvailable` when it did not — "an inventory table requires a
//!    journal table with an `ACTIVE` status" (same published list).

use service_com_amazonaws_s3_amazons3::state::{
    METADATA_INVENTORY_TABLE_EXISTS_CODE, METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
    METADATA_JOURNAL_TABLE_EXISTS_CODE, METADATA_JOURNAL_TABLE_EXISTS_MESSAGE,
    METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE, METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
    METADATA_TABLE_STATUS_ACTIVE, METADATA_TABLE_STATUS_FAILED,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, inside, wait_for_the_tables,
    ANNOTATION_ROLE, NOT_THE_OWNER, XMLNS,
};
use super::object_and_tagging_roundtrip::text;

/// `DELETE /<bucket>?metadataConfiguration` — `DeleteBucketMetadataConfiguration`.
async fn delete_metadata_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/{bucket}?metadataConfiguration"),
        headers,
        "",
    )
    .await
}

/// A `MetadataConfiguration` container: the required journal table, and the optional
/// inventory and annotation tables when the case asks for them.
fn payload(inventory: bool, annotation: bool) -> String {
    let mut body = format!("<MetadataConfiguration xmlns=\"{XMLNS}\">");
    body.push_str(
        "<JournalTableConfiguration><RecordExpiration>\
         <Expiration>ENABLED</Expiration><Days>7</Days>\
         </RecordExpiration></JournalTableConfiguration>",
    );
    if inventory {
        body.push_str(
            "<InventoryTableConfiguration>\
             <ConfigurationState>ENABLED</ConfigurationState>\
             </InventoryTableConfiguration>",
        );
    }
    if annotation {
        body.push_str(&format!(
            "<AnnotationTableConfiguration>\
             <ConfigurationState>ENABLED</ConfigurationState><Role>{ANNOTATION_ROLE}</Role>\
             </AnnotationTableConfiguration>"
        ));
    }
    body.push_str("</MetadataConfiguration>");
    body
}

/// The `TableStatus`, `ErrorCode` and `ErrorMessage` a read reports for one table container
/// of the configuration.
fn reported(body: &str, container: &str) -> (Option<String>, Option<String>, Option<String>) {
    let Some(fragment) = inside(body, container) else {
        return (None, None, None);
    };
    // The reader unescapes the text, so a sentence carrying an apostrophe is compared as a
    // client's parser hands it over rather than as the XML spells it.
    let element = |name: &str| inside(fragment, name).map(super::bucket_tagging::unescaped);
    (
        element("TableStatus"),
        element("ErrorCode"),
        element("ErrorMessage"),
    )
}

/// A bucket carrying a metadata configuration whose tables are in service, so a row acts on
/// the state the API asks a caller to wait for.
async fn bucket_with_an_active_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    inventory: bool,
    annotation: bool,
) {
    let created = create_bucket(router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {bucket:?} must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let configured =
        create_metadata_configuration(router, bucket, &[], &payload(inventory, annotation)).await;
    assert_eq!(
        configured.status, 200,
        "the configuration must be created first, got {} with body {:?}",
        configured.status,
        text(&configured)
    );
    wait_for_the_tables();
    let read = get_metadata_configuration(router, bucket, &[]).await;
    let body = text(&read);
    assert_eq!(
        reported(&body, "JournalTableConfigurationResult").0.as_deref(),
        Some(METADATA_TABLE_STATUS_ACTIVE),
        "the journal table must reach {METADATA_TABLE_STATUS_ACTIVE} before a row acts on it, \
         got {body:?}"
    );
}

#[tokio::test]
async fn a_deleted_configuration_is_gone_and_the_delete_is_idempotent() {
    let router = router();
    bucket_with_an_active_configuration(&router, "metadata-delete", false, false).await;

    // While the configuration is there, a second create is refused rather than replacing it.
    let duplicate =
        create_metadata_configuration(&router, "metadata-delete", &[], &payload(false, false))
            .await;
    assert_eq!(
        duplicate.status, 409,
        "a second create while a configuration is active must be refused, got {} with body {:?}",
        duplicate.status,
        text(&duplicate)
    );
    assert_eq!(
        duplicate.error_code(),
        "MetadataConfigurationAlreadyExists",
        "expected the MetadataConfigurationAlreadyExists code, got {:?}",
        duplicate.error_code()
    );

    // The delete answers the modelled 204 with an empty body, and the configuration is gone.
    let deleted = delete_metadata_configuration(&router, "metadata-delete", &[]).await;
    assert_eq!(
        deleted.status, 204,
        "the delete must answer the modelled 204, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
    assert!(
        deleted.body.is_empty(),
        "a 204 carries no body, got {:?}",
        text(&deleted)
    );
    let absent = get_metadata_configuration(&router, "metadata-delete", &[]).await;
    assert_eq!(
        absent.status, 404,
        "a read of the deleted configuration must answer 404, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchConfiguration",
        "expected the NoSuchConfiguration code, got {:?}",
        absent.error_code()
    );

    // Idempotent: the bucket is already in the asked-for state, so a second delete succeeds
    // and still leaves no configuration.
    let again = delete_metadata_configuration(&router, "metadata-delete", &[]).await;
    assert_eq!(
        again.status, 204,
        "a second delete must still answer 204, got {} with body {:?}",
        again.status,
        text(&again)
    );
    assert_eq!(
        get_metadata_configuration(&router, "metadata-delete", &[])
            .await
            .status,
        404,
        "the bucket must still carry no configuration"
    );

    // The addresses the delete refuses: a bucket this emulator does not hold, and an
    // expected owner that is not the account's.
    for bucket in ["metadata-absent", "metadata", "metadata-delete-2"] {
        let refused = delete_metadata_configuration(&router, bucket, &[]).await;
        assert_eq!(
            refused.status, 404,
            "case {bucket:?}: a bucket this emulator does not hold must answer 404, got {} with \
             body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "NoSuchBucket",
            "case {bucket:?}: expected the NoSuchBucket code, got {:?}",
            refused.error_code()
        );
    }
    let denied = delete_metadata_configuration(
        &router,
        "metadata-delete",
        &[("x-amz-expected-bucket-owner", NOT_THE_OWNER)],
    )
    .await;
    assert_eq!(
        denied.status, 403,
        "an owner that is not the account's must answer 403, got {} with body {:?}",
        denied.status,
        text(&denied)
    );
    assert_eq!(
        denied.error_code(),
        "AccessDenied",
        "expected the AccessDenied code, got {:?}",
        denied.error_code()
    );
}

/// One row of the recreate table: the case name, the bucket it acts on, whether the DELETED
/// configuration had an inventory table, whether the new create asks for one, and the error
/// code and sentence that inventory table must then report (`None` for a create that names
/// no inventory table).
type Recreate = (
    &'static str,
    &'static str,
    bool,
    bool,
    Option<(&'static str, &'static str)>,
);

#[tokio::test]
async fn a_create_after_a_delete_reports_the_table_it_could_not_create() {
    let router = router();

    // Three buckets, so the rows differ along which tables the DELETED configuration had
    // created — which is what decides the error code each table of the new configuration
    // reports:
    //
    // * the journal table alone, and the new create names only a journal table: the journal
    //   name is taken, so it alone fails;
    // * the journal table alone, and the new create also enables an inventory table: the
    //   inventory table's own name is free, so it reports the journal table it relies on;
    // * the journal AND inventory tables, and the new create names both: each reports its
    //   own name being taken.
    let rows: [Recreate; 3] = [
        (
            "the journal table alone",
            "metadata-recreate-journal",
            false,
            false,
            None,
        ),
        (
            "an inventory table whose journal table failed",
            "metadata-recreate-dependent",
            false,
            true,
            Some((
                METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE,
                METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
            )),
        ),
        (
            "an inventory table whose own name is taken",
            "metadata-recreate-inventory",
            true,
            true,
            Some((
                METADATA_INVENTORY_TABLE_EXISTS_CODE,
                METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
            )),
        ),
    ];

    for (case, bucket, first_inventory, second_inventory, expected_inventory) in rows {
        bucket_with_an_active_configuration(&router, bucket, first_inventory, false).await;
        let deleted = delete_metadata_configuration(&router, bucket, &[]).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: the delete must succeed, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );

        // The identical create is ACCEPTED: the bucket carries a configuration again.
        let recreated =
            create_metadata_configuration(&router, bucket, &[], &payload(second_inventory, false))
                .await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: a create after a delete must be accepted, got {} with body {:?}",
            recreated.status,
            text(&recreated)
        );

        // Its journal table never reaches ACTIVE: the table the deleted configuration
        // created is still in the namespace, so provisioning failed with the published code
        // and sentence — before and after the span a successful creation would take.
        for when in ["straight after the create", "after the activation span"] {
            if when == "after the activation span" {
                wait_for_the_tables();
            }
            let read = get_metadata_configuration(&router, bucket, &[]).await;
            let body = text(&read);
            let (status, code, message) = reported(&body, "JournalTableConfigurationResult");
            assert_eq!(
                status.as_deref(),
                Some(METADATA_TABLE_STATUS_FAILED),
                "case {case:?} ({when}): the journal table must report {METADATA_TABLE_STATUS_FAILED}, got {body:?}"
            );
            assert_eq!(
                code.as_deref(),
                Some(METADATA_JOURNAL_TABLE_EXISTS_CODE),
                "case {case:?} ({when}): expected the {METADATA_JOURNAL_TABLE_EXISTS_CODE} error code, got {code:?}"
            );
            assert_eq!(
                message.as_deref(),
                Some(METADATA_JOURNAL_TABLE_EXISTS_MESSAGE),
                "case {case:?} ({when}): expected the published sentence, got {message:?}"
            );
        }

        // What the inventory table reports, when the create asked for one.
        let read = get_metadata_configuration(&router, bucket, &[]).await;
        let body = text(&read);
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
                    "case {case:?}: the inventory table must report {METADATA_TABLE_STATUS_FAILED}, got {body:?}"
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

        // And the configuration that failed to provision is still a configuration the bucket
        // carries, so a second create is refused for that rather than retried.
        let duplicate =
            create_metadata_configuration(&router, bucket, &[], &payload(second_inventory, false))
                .await;
        assert_eq!(
            duplicate.error_code(),
            "MetadataConfigurationAlreadyExists",
            "case {case:?}: expected the MetadataConfigurationAlreadyExists code, got {:?}",
            duplicate.error_code()
        );
    }
}
