// SPDX-License-Identifier: CC0-1.0

//! Which RESOURCE a reporting read of AppFabric's nested chain (app bundle ->
//! ingestion -> ingestion destination, and app bundle -> app authorization)
//! attributes an HTTP 404 `ResourceNotFoundException` to.
//!
//! RULE: the app bundle is the SCOPE every nested path is rooted at, so a bundle
//! identifier naming nothing stored is refused as the app bundle; inside a bundle
//! that does resolve, a read that cannot reach what it asks for is refused as the
//! resource the read TARGETS — the deepest resource its own request names — and
//! never as an intermediate ancestor it failed to resolve on the way. The refusal
//! carries that resource kind's `resourceType`, the identifier the caller supplied
//! for THAT resource in `resourceId`, and that kind's constant reporting sentence.
//!
//! The table spans what the rule generalises over: each read of the chain (so
//! each target kind, at two and at three levels of nesting), which part of the
//! chain below the bundle does not resolve (the ancestor, the target, or both at
//! once), whether it never existed or a delete removed it, and both modelled
//! `Identifier` forms (ARN and bare UUID) of the unresolved identifier.

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, encode, unknown_bundle_arn, unknown_ingestion_arn, unknown_uuid, uuid_of, Answer,
    Emulator,
};

/// The region every call in this suite signs for, and therefore the region of
/// every ARN built by hand below.
const REGION: &str = "us-east-1";

/// The account segment of every ARN a generated emulator emits.
const ACCOUNT: &str = "000000000000";

/// The `(resourceType, message)` pair of each resource kind's reporting-path
/// not-found refusal, as this crate's state module holds it: one CONSTANT
/// sentence per kind, which the caller's identifier is never interpolated into.
const BUNDLE_NOT_FOUND: (&str, &str) = ("AppBundle", "The app bundle ID was not found.");
const INGESTION_NOT_FOUND: (&str, &str) = ("Ingestion", "The specified ingestion does not exist");
const DESTINATION_NOT_FOUND: (&str, &str) = (
    "IngestionDestination",
    "The specified ingestion destination does not exist",
);
const AUTHORIZATION_NOT_FOUND: (&str, &str) = (
    "AppAuthorization",
    "The app authorization was not found.",
);

/// An `auditLog` processing configuration, over the model's own closed enums
/// (`Schema = ocsf | raw`, `Format = json | parquet`).
fn processing() -> Value {
    json!({"auditLog": {"schema": "ocsf", "format": "json"}})
}

/// An `auditLog` destination configuration delivering to an S3 bucket.
fn s3_destination(bucket: &str) -> Value {
    json!({"auditLog": {"destination": {"s3Bucket": {"bucketName": bucket}}}})
}

/// An ingestion-destination ARN in the form `CreateIngestionDestination` builds
/// (`appbundle/<uuid>/ingestion/<uuid>/ingestiondestination/<uuid>`).
fn destination_arn(bundle_uuid: &str, ingestion_uuid: &str, destination_uuid: &str) -> String {
    format!(
        "arn:aws:appfabric:{REGION}:{ACCOUNT}:appbundle/{bundle_uuid}\
         /ingestion/{ingestion_uuid}/ingestiondestination/{destination_uuid}"
    )
}

/// `GET /appbundles/{b}/ingestions/{i}/ingestiondestinations/{d}`.
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

/// `GET /appbundles/{b}/ingestions/{i}/ingestiondestinations`.
async fn list_destinations(emulator: &Emulator, bundle: &str, ingestion: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(bundle),
                encode(ingestion)
            ),
            None,
        )
        .await
}

/// `DELETE /appbundles/{b}/ingestions/{i}/ingestiondestinations/{d}`.
async fn delete_destination(
    emulator: &Emulator,
    bundle: &str,
    ingestion: &str,
    destination: &str,
) -> Answer {
    emulator
        .call(
            "DELETE",
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

/// `GET /appbundles/{b}/appauthorizations/{a}`.
async fn get_authorization(emulator: &Emulator, bundle: &str, authorization: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "/appbundles/{}/appauthorizations/{}",
                encode(bundle),
                encode(authorization)
            ),
            None,
        )
        .await
}

/// `POST /appbundles/{b}/ingestions/{i}/ingestiondestinations`.
async fn create_destination(
    emulator: &Emulator,
    bundle: &str,
    ingestion: &str,
    bucket: &str,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(bundle),
                encode(ingestion)
            ),
            Some(json!({
                "processingConfiguration": processing(),
                "destinationConfiguration": s3_destination(bucket),
            })),
        )
        .await
}

/// An ingestion under `bundle` for `(app, tenant)`, as its ARN.
async fn create_ingestion(emulator: &Emulator, bundle: &str, tenant: &str) -> String {
    let created = emulator
        .create_ingestion(
            bundle,
            json!({"app": "SLACK", "tenantId": tenant, "ingestionType": "auditLog"}),
        )
        .await;
    assert!(
        created.status < 300,
        "fixture ingestion for tenant {tenant:?}: expected success, got {} with body {}",
        created.status,
        created.body
    );
    arn_of(&created.body, "ingestion")
}

/// One row: the case name, the answer, and the `(resourceType, resourceId,
/// message)` the refusal must carry.
type Row = (String, Answer, &'static str, String, &'static str);

fn row(
    case: &str,
    answer: Answer,
    attribution: (&'static str, &'static str),
    resource_id: &str,
) -> Row {
    (
        case.to_string(),
        answer,
        attribution.0,
        resource_id.to_string(),
        attribution.1,
    )
}

/// RULE (see the module docs): a reporting read of the chain attributes its
/// not-found to the resource it TARGETS once the app bundle scope resolves, and
/// to the app bundle when that scope itself does not resolve.
#[tokio::test]
async fn a_chain_read_attributes_not_found_to_the_resource_it_targets() {
    let emulator = Emulator::new();
    let bundle = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );

    // The chain this suite reads: one ingestion holding one destination.
    let ingestion = create_ingestion(&emulator, &bundle, "t-1").await;
    let created = create_destination(&emulator, &bundle, &ingestion, "audit-logs-bucket").await;
    let destination = arn_of(&created.body, "ingestionDestination");

    // A second ingestion with its own destination, both then deleted: the pair
    // of identifiers below is one that really existed, and whose ancestor is
    // gone as well, rather than one no create ever minted.
    let orphaned_ingestion = create_ingestion(&emulator, &bundle, "t-2").await;
    let orphaned = arn_of(
        &create_destination(&emulator, &bundle, &orphaned_ingestion, "other-audit-bucket")
            .await
            .body,
        "ingestionDestination",
    );
    for removed in [
        delete_destination(&emulator, &bundle, &orphaned_ingestion, &orphaned).await,
        emulator
            .delete_ingestion(&bundle, &orphaned_ingestion)
            .await,
    ] {
        assert!(
            removed.status < 300,
            "expected the fixture deletes to succeed, got {} with body {}",
            removed.status,
            removed.body
        );
    }

    let unknown = unknown_uuid();
    let unknown_bundle = unknown_bundle_arn(&unknown);
    let unknown_ingestion = unknown_ingestion_arn(&uuid_of(&bundle), &unknown);
    let unknown_destination = destination_arn(&uuid_of(&bundle), &uuid_of(&ingestion), &unknown);

    let rows: Vec<Row> = vec![
        // GetIngestionDestination — target: the ingestion destination. Every
        // row below has a RESOLVABLE bundle, so the refusal is the target's
        // whichever part of the chain under it does not resolve.
        row(
            "GetIngestionDestination with an unknown destination uuid",
            get_destination(&emulator, &bundle, &ingestion, &unknown).await,
            DESTINATION_NOT_FOUND,
            &unknown,
        ),
        row(
            "GetIngestionDestination with an unknown destination arn",
            get_destination(&emulator, &bundle, &ingestion, &unknown_destination).await,
            DESTINATION_NOT_FOUND,
            &unknown_destination,
        ),
        row(
            "GetIngestionDestination with an unknown ingestion uuid and a stored destination",
            get_destination(&emulator, &bundle, &unknown, &destination).await,
            DESTINATION_NOT_FOUND,
            &destination,
        ),
        row(
            "GetIngestionDestination with an unknown ingestion arn and a stored destination",
            get_destination(&emulator, &bundle, &unknown_ingestion, &destination).await,
            DESTINATION_NOT_FOUND,
            &destination,
        ),
        row(
            "GetIngestionDestination with the ingestion and the destination both unknown",
            get_destination(&emulator, &bundle, &unknown, &unknown_destination).await,
            DESTINATION_NOT_FOUND,
            &unknown_destination,
        ),
        row(
            "GetIngestionDestination with a destination and ingestion a delete removed",
            get_destination(&emulator, &bundle, &orphaned_ingestion, &orphaned).await,
            DESTINATION_NOT_FOUND,
            &orphaned,
        ),
        row(
            "GetIngestionDestination with a destination and ingestion a delete removed, by uuid",
            get_destination(
                &emulator,
                &bundle,
                &uuid_of(&orphaned_ingestion),
                &uuid_of(&orphaned),
            )
            .await,
            DESTINATION_NOT_FOUND,
            &uuid_of(&orphaned),
        ),
        // GetIngestion — target: the ingestion, the deepest resource it names.
        row(
            "GetIngestion with an unknown ingestion uuid",
            emulator.get_ingestion(&bundle, &unknown).await,
            INGESTION_NOT_FOUND,
            &unknown,
        ),
        row(
            "GetIngestion with an unknown ingestion arn",
            emulator.get_ingestion(&bundle, &unknown_ingestion).await,
            INGESTION_NOT_FOUND,
            &unknown_ingestion,
        ),
        row(
            "GetIngestion with an ingestion a delete removed",
            emulator
                .get_ingestion(&bundle, &orphaned_ingestion)
                .await,
            INGESTION_NOT_FOUND,
            &orphaned_ingestion,
        ),
        // ListIngestionDestinations — target: the ingestion whose destinations
        // the page comes from, which is the deepest resource it names.
        row(
            "ListIngestionDestinations under an unknown ingestion uuid",
            list_destinations(&emulator, &bundle, &unknown).await,
            INGESTION_NOT_FOUND,
            &unknown,
        ),
        row(
            "ListIngestionDestinations under an ingestion a delete removed",
            list_destinations(&emulator, &bundle, &orphaned_ingestion).await,
            INGESTION_NOT_FOUND,
            &orphaned_ingestion,
        ),
        // GetAppAuthorization — the other two-level read of this service.
        row(
            "GetAppAuthorization with an unknown authorization uuid",
            get_authorization(&emulator, &bundle, &unknown).await,
            AUTHORIZATION_NOT_FOUND,
            &unknown,
        ),
        // The app bundle is the SCOPE: when IT does not resolve, every one of
        // these reads is refused as the app bundle instead.
        row(
            "GetIngestionDestination under an unknown bundle arn",
            get_destination(&emulator, &unknown_bundle, &ingestion, &destination).await,
            BUNDLE_NOT_FOUND,
            &unknown_bundle,
        ),
        row(
            "GetIngestionDestination under an unknown bundle uuid",
            get_destination(&emulator, &unknown, &unknown, &unknown).await,
            BUNDLE_NOT_FOUND,
            &unknown,
        ),
        row(
            "GetIngestion under an unknown bundle uuid",
            emulator.get_ingestion(&unknown, &ingestion).await,
            BUNDLE_NOT_FOUND,
            &unknown,
        ),
        row(
            "ListIngestionDestinations under an unknown bundle uuid",
            list_destinations(&emulator, &unknown, &ingestion).await,
            BUNDLE_NOT_FOUND,
            &unknown,
        ),
        row(
            "GetAppAuthorization under an unknown bundle uuid",
            get_authorization(&emulator, &unknown, &unknown).await,
            BUNDLE_NOT_FOUND,
            &unknown,
        ),
    ];

    for (case, answer, resource_type, resource_id, message) in rows {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        assert_eq!(
            answer.body.get("resourceType").and_then(Value::as_str),
            Some(resource_type),
            "case {case:?}: expected the refusal to be about {resource_type}, got {}",
            answer.body
        );
        assert_eq!(
            answer.body.get("resourceId").and_then(Value::as_str),
            Some(resource_id.as_str()),
            "case {case:?}: expected {resource_id:?} in resourceId, got {}",
            answer.body
        );
        assert_eq!(
            answer.body.get("message").and_then(Value::as_str),
            Some(message),
            "case {case:?}: expected the constant message {message:?}, got {}",
            answer.body
        );
    }

    // None of those refused reads changed what the store holds: the destination
    // the fixture created still reports exactly what the create wrote.
    let read = get_destination(&emulator, &bundle, &ingestion, &destination).await;
    assert_eq!(
        read.body.get("ingestionDestination"),
        created.body.get("ingestionDestination"),
        "a refused read must leave the stored record alone, expected {}, got {}",
        created.body,
        read.body
    );
}
