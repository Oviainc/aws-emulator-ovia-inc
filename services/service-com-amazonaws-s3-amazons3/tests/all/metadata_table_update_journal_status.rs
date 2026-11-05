// SPDX-License-Identifier: CC0-1.0

//! WHEN a table of an S3 Metadata configuration may be reconfigured: what the three update
//! operations answer while the configuration's JOURNAL table is not yet in service.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and metadata-configuration suites in this crate own), so what is pinned
//! is the wire behaviour — status, error code, `<Message>` — and what the configuration reads
//! back as afterwards.
//!
//! The rule under test: a metadata configuration is built around its journal table —
//! `MetadataConfiguration$JournalTableConfiguration` is required of every configuration, and
//! an inventory table "requires a journal table with an `ACTIVE` status" (the `ErrorDetails`
//! the published V2 list carries for a journal table that has not got one) — so none of the
//! configuration's tables may be reconfigured until that table is in service. Each update is
//! refused with HTTP 400 `InvalidRequest` and records nothing: the one that updates the
//! journal table's own settings reports that the journal table has not got an `ACTIVE`
//! status, and the two that update a table DEPENDING on it report that the journal table is
//! being created.
//!
//! The rows differ along what the rule generalises over: WHICH of the three updates the
//! request is (the journal table's own settings, the inventory table, the annotation table),
//! and the journal table's status when it arrives — being created, and `ACTIVE` once the
//! table has fallen due, which is served.

use service_com_amazonaws_s3_amazons3::service::metadata_tables::{
    JOURNAL_TABLE_BEING_CREATED_MESSAGE, JOURNAL_TABLE_NOT_ACTIVE_MESSAGE,
};
use service_com_amazonaws_s3_amazons3::state::{INVALID_REQUEST_CODE, INVALID_REQUEST_STATUS};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, wait_for_the_tables,
    ANNOTATION_ROLE, XMLNS,
};
use super::object_and_tagging_roundtrip::text;

/// The `MetadataConfiguration` every row creates: a journal table with its record expiration
/// off, and both optional tables enabled, so each of the three updates has a table of the
/// configuration to be about.
fn starting_configuration() -> String {
    format!(
        "<MetadataConfiguration xmlns=\"{XMLNS}\">\
         <JournalTableConfiguration><RecordExpiration>\
         <Expiration>DISABLED</Expiration>\
         </RecordExpiration></JournalTableConfiguration>\
         <InventoryTableConfiguration>\
         <ConfigurationState>ENABLED</ConfigurationState>\
         </InventoryTableConfiguration>\
         <AnnotationTableConfiguration>\
         <ConfigurationState>ENABLED</ConfigurationState>\
         <Role>{ANNOTATION_ROLE}</Role>\
         </AnnotationTableConfiguration>\
         </MetadataConfiguration>"
    )
}

/// One row: the case, the subresource the update addresses, the payload it carries, and the
/// sentence S3 refuses it with while the journal table is being created.
struct Row {
    case: &'static str,
    subresource: &'static str,
    payload: String,
    refused_with: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the journal table's own record expiration",
            subresource: "metadataJournalTable",
            payload: format!(
                "<JournalTableConfiguration xmlns=\"{XMLNS}\"><RecordExpiration>\
                 <Expiration>ENABLED</Expiration><Days>7</Days>\
                 </RecordExpiration></JournalTableConfiguration>"
            ),
            refused_with: JOURNAL_TABLE_NOT_ACTIVE_MESSAGE,
        },
        Row {
            case: "the inventory table, which reads from the journal table",
            subresource: "metadataInventoryTable",
            payload: format!(
                "<InventoryTableConfiguration xmlns=\"{XMLNS}\">\
                 <ConfigurationState>DISABLED</ConfigurationState>\
                 </InventoryTableConfiguration>"
            ),
            refused_with: JOURNAL_TABLE_BEING_CREATED_MESSAGE,
        },
        Row {
            case: "the annotation table of the same configuration",
            subresource: "metadataAnnotationTable",
            payload: format!(
                "<AnnotationTableConfiguration xmlns=\"{XMLNS}\">\
                 <ConfigurationState>DISABLED</ConfigurationState>\
                 </AnnotationTableConfiguration>"
            ),
            refused_with: JOURNAL_TABLE_BEING_CREATED_MESSAGE,
        },
    ]
}

/// `PUT /<bucket>?<subresource>` — one of the three metadata-table updates, with `payload` as
/// its container.
async fn update(
    router: &MultiServiceRouter,
    bucket: &str,
    subresource: &str,
    payload: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?{subresource}"),
        &[],
        payload,
    )
    .await
}

#[tokio::test]
async fn an_update_before_the_journal_table_is_in_service_is_refused_and_records_nothing() {
    let router = router();

    for (index, row) in rows().iter().enumerate() {
        let Row {
            case,
            subresource,
            payload,
            refused_with,
        } = row;
        // A bucket per row, so each row meets a configuration whose journal table has only
        // just been asked for and is therefore still being created.
        let bucket = format!("journal-status-{index}");
        create_bucket(&router, &bucket, None, None).await;
        let created =
            create_metadata_configuration(&router, &bucket, &[], &starting_configuration()).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: the configuration must be created first, got {} with body {:?}",
            created.status,
            text(&created)
        );

        let before = text(&get_metadata_configuration(&router, &bucket, &[]).await);
        let refused = update(&router, &bucket, subresource, payload).await;
        assert_eq!(
            refused.status,
            INVALID_REQUEST_STATUS,
            "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS} while the journal table is \
             being created, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            INVALID_REQUEST_CODE,
            "case {case:?}: expected the published code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            *refused_with,
            "case {case:?}: expected the published sentence, got {:?}",
            refused.error_message()
        );

        // Nothing was written: the configuration reads back exactly as it did before the
        // refused call, down to the table states and names.
        let after = text(&get_metadata_configuration(&router, &bucket, &[]).await);
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the configuration exactly as it was"
        );

        // Once the journal table has fallen due, the SAME request is served — so the refusal
        // is about the table's status and not about the payload.
        wait_for_the_tables();
        let served = update(&router, &bucket, subresource, payload).await;
        assert_eq!(
            served.status,
            200,
            "case {case:?}: the same update must be served once the journal table is ACTIVE, \
             got {} with body {:?}",
            served.status,
            text(&served)
        );
        let reading = get_metadata_configuration(&router, &bucket, &[]).await;
        assert_eq!(
            reading.status, 200,
            "case {case:?}: the configuration must read back after the served update, got {}",
            reading.status
        );
        assert_ne!(
            text(&reading),
            before,
            "case {case:?}: the served update must have changed what the read reports"
        );
    }
}
