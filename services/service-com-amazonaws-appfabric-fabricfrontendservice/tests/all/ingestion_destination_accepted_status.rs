// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ACCEPTED state of an ingestion destination, driven
//! over the wire (restJson1) through the same router the emulator boots.
//!
//! RULE: a destination whose configuration the service accepted reports the
//! modelled `Active` value of `IngestionDestinationStatus` and no
//! `statusReason` (the model gives a reason only for the `Failed` value), and
//! `CreateIngestionDestination` and `UpdateIngestionDestination` report the same
//! one — they go through the single writer of that pair,
//! `state::IngestionDestinationRecord::mark_accepted`, so neither can drift from
//! the other or from the model's own value list.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, `uuid_of`) is the one
//! `app_bundle_ingestion_lifecycle` builds, and the fixture chain (authorize →
//! connect → ingest) the one `app_authorization_and_ingestion_dependency`
//! builds.

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::ops::types::INGESTION_DESTINATION_STATUS_VALUES;
use service_com_amazonaws_appfabric_fabricfrontendservice::state::INGESTION_DESTINATION_STATUS_ACTIVE;

use crate::app_authorization_and_ingestion_dependency::bundle_with_ingestion;
use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, uuid_of, Answer, Emulator};

/// `POST /appbundles/{bundle}/ingestions/{ingestion}/ingestiondestinations`
/// with a caller-chosen body, so a row can name any modelled configuration.
async fn create_destination(
    emulator: &Emulator,
    bundle: &str,
    ingestion: &str,
    body: Value,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(bundle),
                encode(ingestion)
            ),
            Some(body),
        )
        .await
}

/// `PATCH .../ingestiondestinations/{destination}`.
async fn update_destination(
    emulator: &Emulator,
    bundle: &str,
    ingestion: &str,
    destination: &str,
    body: Value,
) -> Answer {
    emulator
        .call(
            "PATCH",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations/{}",
                encode(bundle),
                encode(ingestion),
                encode(destination)
            ),
            Some(body),
        )
        .await
}

/// `GET .../ingestiondestinations/{destination}`.
async fn get_destination(
    emulator: &Emulator,
    bundle: &str,
    ingestion: &str,
    destination: &str,
) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations/{}",
                encode(bundle),
                encode(ingestion),
                encode(destination)
            ),
            None,
        )
        .await
}

/// An `auditLog` processing configuration. `schema` and `format` come from the
/// model's own closed enums — `Schema = ocsf | raw` and `Format = json |
/// parquet` (`SCHEMA_VALUES` / `FORMAT_VALUES` in `types/shared_types.rs`).
fn processing(schema: &str, format: &str) -> Value {
    json!({"auditLog": {"schema": schema, "format": format}})
}

/// An `auditLog` destination configuration delivering to an S3 bucket.
fn s3_destination(bucket: &str, prefix: Option<&str>) -> Value {
    let mut s3 = json!({"bucketName": bucket});
    if let Some(prefix) = prefix {
        s3["prefix"] = json!(prefix);
    }
    json!({"auditLog": {"destination": {"s3Bucket": s3}}})
}

/// An `auditLog` destination configuration delivering to a Firehose stream.
fn firehose_destination(stream: &str) -> Value {
    json!({"auditLog": {"destination": {"firehoseStream": {"streamName": stream}}}})
}

/// The `ingestionDestination` of a success body.
fn destination_of(answer: &Answer) -> &Value {
    answer
        .body
        .get("ingestionDestination")
        .unwrap_or_else(|| panic!("expected an ingestionDestination in {}", answer.body))
}

/// Assert the accepted state the rule requires of one returned destination:
/// the modelled accepted status, and no status reason.
fn assert_accepted(case: &str, call: &str, answer: &Answer) {
    let destination = destination_of(answer);
    let status = destination.get("status").and_then(Value::as_str);
    assert_eq!(
        status,
        Some(INGESTION_DESTINATION_STATUS_ACTIVE),
        "case {case:?} after {call}: expected the accepted status \
         {INGESTION_DESTINATION_STATUS_ACTIVE:?}, got {status:?} in {}",
        answer.body
    );
    let reason = destination.get("statusReason");
    assert!(
        reason.is_none_or(Value::is_null),
        "case {case:?} after {call}: expected no statusReason on an accepted \
         destination, got {reason:?} in {}",
        answer.body
    );
}

/// RULE: the status this emulator writes for an accepted destination is one of
/// the values the model's `IngestionDestinationStatus` enum carries — the list
/// the generated types expose — so the named constant both write paths use can
/// never name a value the wire does not accept.
#[test]
fn the_accepted_status_is_one_the_model_carries() {
    assert!(
        INGESTION_DESTINATION_STATUS_VALUES.contains(&INGESTION_DESTINATION_STATUS_ACTIVE),
        "expected the accepted status {INGESTION_DESTINATION_STATUS_ACTIVE:?} to be one of the \
         modelled values, got {INGESTION_DESTINATION_STATUS_VALUES:?}"
    );
}

/// RULE: a destination whose configuration was accepted carries the modelled
/// accepted status and no status reason, whichever call accepted it — the create
/// call that minted it or the update call that last rewrote it — and that is
/// what every later read reports.
///
/// The table spans what the rule generalises over: the destination union member
/// (`s3Bucket` / `firehoseStream`) before and after the update, the optional
/// `prefix` appearing, changing and disappearing, both `Schema` values and both
/// `Format` values, and the `Identifier` form the update names the destination
/// by (ARN or bare UUID).
#[tokio::test]
async fn an_accepted_destination_carries_the_modelled_accepted_status() {
    // (case, app, tenant, processingConfiguration, created destination,
    //  updated destination, update by ARN)
    let cases: Vec<(&str, &str, &str, Value, Value, Value, bool)> = vec![
        (
            "s3 without prefix -> s3 with prefix, ocsf/json",
            "SLACK",
            "tenant-slack",
            processing("ocsf", "json"),
            s3_destination("audit-logs-bucket", None),
            s3_destination("audit-logs-bucket", Some("slack/")),
            true,
        ),
        (
            "s3 prefix disappears, raw/parquet",
            "ZENDESK",
            "tenant-zendesk",
            processing("raw", "parquet"),
            s3_destination("audit-logs-bucket", Some("zendesk/")),
            s3_destination("other-audit-bucket", None),
            false,
        ),
        (
            "s3 -> firehose, raw/json",
            "DROPBOX",
            "tenant-dropbox",
            processing("raw", "json"),
            s3_destination("audit-logs-bucket", Some("dropbox/")),
            firehose_destination("audit-log-stream"),
            true,
        ),
        (
            "firehose -> s3, ocsf/parquet",
            "SMARTSHEET",
            "tenant-smartsheet",
            processing("ocsf", "parquet"),
            firehose_destination("audit-log-stream"),
            s3_destination("audit-logs-bucket", None),
            false,
        ),
        (
            "firehose -> firehose, ocsf/json",
            "CISCO",
            "tenant-cisco",
            processing("ocsf", "json"),
            firehose_destination("audit-log-stream"),
            firehose_destination("other-audit-stream"),
            true,
        ),
    ];

    for (case, app, tenant, processing_configuration, created_in, updated_in, by_arn) in cases {
        // A fresh store per row: one Region holds only one app bundle.
        let emulator = Emulator::new();
        let (bundle, ingestion) = bundle_with_ingestion(&emulator, app, tenant, case).await;

        let created = create_destination(
            &emulator,
            &bundle,
            &ingestion,
            json!({
                "processingConfiguration": processing_configuration,
                "destinationConfiguration": created_in,
            }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateIngestionDestination, got {} with body {}",
            created.status, created.body
        );
        assert_accepted(case, "CreateIngestionDestination", &created);

        let arn = arn_of(&created.body, "ingestionDestination");
        let read = get_destination(&emulator, &bundle, &ingestion, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetIngestionDestination, got {} with body {}",
            read.status, read.body
        );
        assert_accepted(case, "a read of the created destination", &read);

        let identifier = if by_arn { arn.clone() } else { uuid_of(&arn) };
        let updated = update_destination(
            &emulator,
            &bundle,
            &ingestion,
            &identifier,
            json!({"destinationConfiguration": updated_in}),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateIngestionDestination, got {} with body {}",
            updated.status, updated.body
        );
        assert_accepted(case, "UpdateIngestionDestination", &updated);

        let reread = get_destination(&emulator, &bundle, &ingestion, &identifier).await;
        assert_eq!(
            reread.status, 200,
            "case {case:?}: expected HTTP 200 re-reading the updated destination, got {} with body {}",
            reread.status, reread.body
        );
        assert_accepted(case, "a read of the updated destination", &reread);
    }
}
