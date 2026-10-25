// SPDX-License-Identifier: CC0-1.0

//! The wording of the app-bundle not-found refusal, asserted over EVERY
//! implemented operation that takes an `appBundleIdentifier` and answers HTTP 404
//! `ResourceNotFoundException` for a bundle the store does not hold.
//!
//! RULE: AppFabric words that refusal by the KIND of path it refused — a
//! reporting operation states that the app bundle id was not found, while an
//! operation that writes into the bundle states that the supplied app bundle id
//! is not valid — with the same HTTP 404 `ResourceNotFoundException`, the
//! identifier reported in `resourceId` (never interpolated into the sentence) and
//! `resourceType` naming the app bundle.
//!
//! The table spans the whole class rather than one recorded call: every
//! implemented operation on both sides of the split, crossed with four
//! unknown-bundle identifiers (a uuid and an ARN no create ever minted, and a
//! bundle that was created and then deleted, named by each modelled `Identifier`
//! form).

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, encode, unknown_bundle_arn, unknown_uuid, uuid_of, Answer, Emulator,
};

/// The sentence a REPORTING operation answers, as `crate::state` holds it.
const READ_PATH_MESSAGE: &str = "The app bundle ID was not found.";

/// The sentence an operation that WRITES into the bundle answers.
const MUTATION_PATH_MESSAGE: &str = "The specified app bundle ID is not valid";

/// Which side of the split an operation sits on: what it DOES with the bundle it
/// names, not what it is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Path {
    /// The operation only reports on the bundle and what it holds.
    Reporting,
    /// The operation writes into the bundle (or into a child of it).
    Mutation,
}

impl Path {
    fn expected_message(self) -> &'static str {
        match self {
            Path::Reporting => READ_PATH_MESSAGE,
            Path::Mutation => MUTATION_PATH_MESSAGE,
        }
    }
}

/// The `message` member of an error body, or `""`.
fn message(answer: &Answer) -> &str {
    answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// One member of an error body, or `""`.
fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer
        .body
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// An `auditLog` processing configuration, over the model's own closed enums
/// (`Schema = ocsf | raw`, `Format = json | parquet`).
fn processing() -> Value {
    json!({"auditLog": {"schema": "ocsf", "format": "json"}})
}

/// An `auditLog` destination configuration delivering to an S3 bucket.
fn s3_destination() -> Value {
    json!({"auditLog": {"destination": {"s3Bucket": {"bucketName": "audit-logs-bucket"}}}})
}

/// Call every implemented operation that takes an `appBundleIdentifier` against
/// `bundle`, which names no stored app bundle, and return each answer beside the
/// kind of path the operation is.
///
/// Child identifiers (ingestion, app authorization, ingestion destination) are
/// uuids no create ever minted: the bundle is the outer parent of each of these
/// paths, so it is the level that refuses.
async fn refusals(emulator: &Emulator, bundle: &str) -> Vec<(&'static str, Path, Answer)> {
    let child = unknown_uuid();
    let b = encode(bundle);
    let c = encode(&child);

    let mut rows: Vec<(&'static str, Path, Answer)> = Vec::new();

    // The operations that WRITE into the named bundle or one of its children.
    rows.push((
        "DeleteIngestionDestination",
        Path::Mutation,
        emulator
            .call(
                "DELETE",
                &format!("/appbundles/{b}/ingestions/{c}/ingestiondestinations/{c}"),
                None,
            )
            .await,
    ));
    rows.push((
        "CreateIngestionDestination",
        Path::Mutation,
        emulator
            .call(
                "POST",
                &format!("/appbundles/{b}/ingestions/{c}/ingestiondestinations"),
                Some(json!({
                    "processingConfiguration": processing(),
                    "destinationConfiguration": s3_destination(),
                })),
            )
            .await,
    ));
    rows.push((
        "UpdateIngestionDestination",
        Path::Mutation,
        emulator
            .call(
                "PATCH",
                &format!("/appbundles/{b}/ingestions/{c}/ingestiondestinations/{c}"),
                Some(json!({"destinationConfiguration": s3_destination()})),
            )
            .await,
    ));
    rows.push((
        "DeleteIngestion",
        Path::Mutation,
        emulator
            .call("DELETE", &format!("/appbundles/{b}/ingestions/{c}"), None)
            .await,
    ));
    rows.push((
        "CreateIngestion",
        Path::Mutation,
        emulator
            .call(
                "POST",
                &format!("/appbundles/{b}/ingestions"),
                Some(json!({"app": "SLACK", "tenantId": "t-1", "ingestionType": "auditLog"})),
            )
            .await,
    ));
    rows.push((
        "StartIngestion",
        Path::Mutation,
        emulator
            .call(
                "POST",
                &format!("/appbundles/{b}/ingestions/{c}/start"),
                Some(json!({})),
            )
            .await,
    ));
    rows.push((
        "StopIngestion",
        Path::Mutation,
        emulator
            .call(
                "POST",
                &format!("/appbundles/{b}/ingestions/{c}/stop"),
                Some(json!({})),
            )
            .await,
    ));
    rows.push((
        "CreateAppAuthorization",
        Path::Mutation,
        emulator
            .call(
                "POST",
                &format!("/appbundles/{b}/appauthorizations"),
                Some(json!({
                    "app": "SLACK",
                    "authType": "apiKey",
                    "credential": {"apiKeyCredential": {"apiKey": "placeholder-api-key"}},
                    "tenant": {"tenantIdentifier": "t-1", "tenantDisplayName": "t-1"},
                })),
            )
            .await,
    ));
    rows.push((
        "ConnectAppAuthorization",
        Path::Mutation,
        emulator
            .call(
                "POST",
                &format!("/appbundles/{b}/appauthorizations/{c}/connect"),
                Some(json!({})),
            )
            .await,
    ));
    rows.push((
        "StartUserAccessTasks",
        Path::Mutation,
        emulator
            .call(
                "POST",
                "/useraccess/start",
                Some(json!({"appBundleIdentifier": bundle, "email": "user@example.com"})),
            )
            .await,
    ));

    // The operations that only REPORT on the named bundle.
    rows.push((
        "GetAppBundle",
        Path::Reporting,
        emulator.call("GET", &format!("/appbundles/{b}"), None).await,
    ));
    rows.push((
        "GetIngestion",
        Path::Reporting,
        emulator
            .call("GET", &format!("/appbundles/{b}/ingestions/{c}"), None)
            .await,
    ));
    rows.push((
        "ListIngestions",
        Path::Reporting,
        emulator
            .call("GET", &format!("/appbundles/{b}/ingestions"), None)
            .await,
    ));
    rows.push((
        "ListIngestionDestinations",
        Path::Reporting,
        emulator
            .call(
                "GET",
                &format!("/appbundles/{b}/ingestions/{c}/ingestiondestinations"),
                None,
            )
            .await,
    ));
    rows.push((
        "GetAppAuthorization",
        Path::Reporting,
        emulator
            .call(
                "GET",
                &format!("/appbundles/{b}/appauthorizations/{c}"),
                None,
            )
            .await,
    ));
    rows.push((
        "ListAppAuthorizations",
        Path::Reporting,
        emulator
            .call("GET", &format!("/appbundles/{b}/appauthorizations"), None)
            .await,
    ));
    rows.push((
        "BatchGetUserAccessTasks",
        Path::Reporting,
        emulator
            .call(
                "POST",
                "/useraccess/batchget",
                Some(json!({"appBundleIdentifier": bundle, "taskIdList": [child]})),
            )
            .await,
    ));

    rows
}

/// RULE: an operation naming an app bundle the store does not hold is refused
/// HTTP 404 `ResourceNotFoundException`, worded by the kind of path it refused —
/// the reporting sentence for an operation that only reports, the mutation
/// sentence for one that writes into the bundle — with the identifier carried in
/// `resourceId`, `resourceType` naming the app bundle, and no interpolation of
/// the identifier into the message.
#[tokio::test]
async fn an_unknown_app_bundle_is_worded_by_the_kind_of_path_that_refused() {
    let emulator = Emulator::new();

    // A bundle that really existed and is now gone, which is the other way an
    // identifier comes to name nothing.
    let deleted = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );
    let removed = emulator
        .call("DELETE", &format!("/appbundles/{}", encode(&deleted)), None)
        .await;
    assert!(
        removed.status < 300,
        "expected the bundle delete to succeed, got {} with body {}",
        removed.status,
        removed.body
    );

    let identifiers = [
        ("a uuid no create minted", unknown_uuid()),
        ("an arn no create minted", unknown_bundle_arn(&unknown_uuid())),
        ("a deleted bundle by arn", deleted.clone()),
        ("a deleted bundle by uuid", uuid_of(&deleted)),
    ];

    for (identifier_case, identifier) in &identifiers {
        for (operation, path, answer) in refusals(&emulator, identifier).await {
            let case = format!("{operation} with {identifier_case}");
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
                message(&answer),
                path.expected_message(),
                "case {case:?}: expected the {path:?} wording, got {:?}",
                message(&answer)
            );
            assert_eq!(
                member(&answer, "resourceId"),
                identifier.as_str(),
                "case {case:?}: expected the supplied identifier in resourceId, got {:?}",
                member(&answer, "resourceId")
            );
            assert_eq!(
                member(&answer, "resourceType"),
                "AppBundle",
                "case {case:?}: expected resourceType AppBundle, got {:?}",
                member(&answer, "resourceType")
            );
            assert!(
                !message(&answer).contains(identifier.as_str()),
                "case {case:?}: the message must not echo the identifier, got {:?}",
                message(&answer)
            );
        }
    }
}

/// RULE, at one level down: the two sentences are a property of the PATH, not of
/// the identifier, so the same unknown bundle under a stored one's sibling levels
/// gets the mutation sentence from the destination delete and the reporting
/// sentence from the bundle read — side by side in one store that does hold a
/// real destination chain.
#[tokio::test]
async fn deleting_a_destination_under_an_unknown_bundle_answers_the_mutation_wording() {
    let emulator = Emulator::new();
    let bundle = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );
    emulator.authorize(&bundle, "SLACK", "t-1").await;
    let ingestion = arn_of(
        &emulator
            .create_ingestion(
                &bundle,
                json!({"app": "SLACK", "tenantId": "t-1", "ingestionType": "auditLog"}),
            )
            .await
            .body,
        "ingestion",
    );
    let destination = arn_of(
        &emulator
            .call(
                "POST",
                &format!(
                    "/appbundles/{}/ingestions/{}/ingestiondestinations",
                    encode(&bundle),
                    encode(&ingestion)
                ),
                Some(json!({
                    "processingConfiguration": processing(),
                    "destinationConfiguration": s3_destination(),
                })),
            )
            .await
            .body,
        "ingestionDestination",
    );

    for (case, unknown) in [
        ("uuid", unknown_uuid()),
        ("arn", unknown_bundle_arn(&unknown_uuid())),
    ] {
        let refused = emulator
            .call(
                "DELETE",
                &format!(
                    "/appbundles/{}/ingestions/{}/ingestiondestinations/{}",
                    encode(&unknown),
                    encode(&ingestion),
                    encode(&destination)
                ),
                None,
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            message(&refused),
            MUTATION_PATH_MESSAGE,
            "case {case:?}: expected the mutation wording, got {:?}",
            message(&refused)
        );
    }

    // The refused deletes wrote nothing: the destination is still readable.
    let read = emulator
        .call(
            "GET",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations/{}",
                encode(&bundle),
                encode(&ingestion),
                encode(&destination)
            ),
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "expected the stored destination to survive a refused delete, got {} with body {}",
        read.status, read.body
    );

    // And the same unknown bundle on the reporting path keeps the read wording.
    let reported = emulator
        .call(
            "GET",
            &format!("/appbundles/{}", encode(&unknown_uuid())),
            None,
        )
        .await;
    assert_eq!(
        message(&reported),
        READ_PATH_MESSAGE,
        "expected the read wording from GetAppBundle, got {:?}",
        message(&reported)
    );
}
