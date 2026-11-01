// SPDX-License-Identifier: CC0-1.0

//! RULE: the audit record a successful state-changing call writes names the
//! SERVER-MINTED id of the record the call resolved, never the identifier string
//! the caller happened to send — so the same change to the same resource is
//! recorded identically whichever of the two forms the model's `Identifier`
//! shape accepts (`^arn:.+$|^[a-f0-9]{8}-…$`) the request used.
//!
//! The table below drives one whole create-and-delete chain per identifier form,
//! covering every operation of this service that resolves a target and records
//! the change: the four creates, the connect, the destination update and the four
//! deletes. Each row asserts the record's `resource_id` field EQUALS the bare
//! UUID the create answer reported, so a record echoing the request's ARN (or any
//! other caller string) fails.
//!
//! The harness (the router, `Answer`, `encode`, `uuid_of`, `audit_capture`) is the
//! one `app_bundle_ingestion_lifecycle` builds, and the app-authorization calls
//! are `app_authorization_deletion`'s; only the audit table is new here.

use serde_json::json;

use crate::app_authorization_deletion::{
    as_form, authorization_arn, connect_authorization, create_authorization, delete_authorization,
    Form,
};
use crate::app_bundle_ingestion_lifecycle::{
    arn_of, audit_capture, encode, uuid_of, Answer, Emulator,
};

/// One application AppFabric supports, as the model's own documentation of the
/// `app` member lists them (`smithy_query(query="doc",
/// shape="CreateAppAuthorizationRequest", member="app")`: "Valid values are:
/// SLACK ASANA JIRA M365 M365AUDITLOGS ZOOM ZENDESK OKTA GOOGLE DROPBOX
/// SMARTSHEET CISCO") — one the issue that prompted this suite did not cite.
const APP: &str = "ZENDESK";

/// The tenant every row of the table authorizes, which no assertion depends on.
const TENANT: &str = "tenant-audit";

/// `POST /appbundles/{b}/ingestions`.
async fn create_ingestion(emulator: &Emulator, bundle: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/ingestions", encode(bundle)),
            Some(json!({"app": APP, "tenantId": TENANT, "ingestionType": "auditLog"})),
        )
        .await
}

/// `POST /appbundles/{b}/ingestions/{i}/ingestiondestinations`.
async fn create_destination(emulator: &Emulator, bundle: &str, ingestion: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(bundle),
                encode(ingestion)
            ),
            Some(json!({
                "processingConfiguration": {"auditLog": {"schema": "raw", "format": "json"}},
                "destinationConfiguration": {"auditLog": {"destination":
                    {"s3Bucket": {"bucketName": "audit-first-bucket"}}}},
            })),
        )
        .await
}

/// `PATCH /appbundles/{b}/ingestions/{i}/ingestiondestinations/{d}`.
async fn update_destination(
    emulator: &Emulator,
    bundle: &str,
    ingestion: &str,
    destination: &str,
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
            Some(json!({"destinationConfiguration": {"auditLog": {"destination":
                {"s3Bucket": {"bucketName": "audit-second-bucket"}}}}})),
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

/// The value of one SPACE-FREE field of a captured audit record. The capture
/// renders an event as `name=value ` pairs; an identifier carries no space, so
/// the value is the whitespace-separated token with that prefix.
fn audit_field(rendered: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    rendered
        .split_whitespace()
        .find_map(|pair| pair.strip_prefix(&prefix).map(ToString::to_string))
}

/// Whether a captured record carries `name=value`, for a field whose value MAY
/// contain a space (an outcome such as `already absent` does).
fn has_field(rendered: &str, name: &str, value: &str) -> bool {
    rendered.contains(&format!("{name}={value} "))
}

/// The ARN of the resource a create answer reports, under `member`, failing with
/// the row's case when the create did not answer the 201 the model gives it.
fn created_arn(answer: &Answer, member: &str, case: &str) -> String {
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected HTTP 201 creating a {member}, got {} with body {}",
        answer.status, answer.body
    );
    arn_of(&answer.body, member)
}

#[tokio::test]
async fn an_audit_record_names_the_resolved_record_not_the_callers_identifier() {
    let captured = audit_capture();

    for form in [Form::Arn, Form::Uuid] {
        let case = format!("identifiers in {form:?} form");
        // A fresh store per row: AppFabric holds ONE app bundle per account per
        // Region, and every row builds a whole chain under its own bundle.
        let emulator = Emulator::new();

        let created_bundle = emulator.create_app_bundle(json!({})).await;
        let bundle = created_arn(&created_bundle, "appBundle", &case);
        let bundle_id = uuid_of(&bundle);
        let named_bundle = as_form(&bundle, form);

        let created_authorization =
            create_authorization(&emulator, &named_bundle, APP, "apiKey", TENANT).await;
        let authorization = authorization_arn(&created_authorization, &case);
        let authorization_id = uuid_of(&authorization);
        let named_authorization = as_form(&authorization, form);

        let connected =
            connect_authorization(&emulator, &named_bundle, &named_authorization, "apiKey").await;
        assert_eq!(
            connected.status, 200,
            "case {case:?}: expected HTTP 200 from ConnectAppAuthorization, got {} with body {}",
            connected.status, connected.body
        );

        let created_ingestion = create_ingestion(&emulator, &named_bundle).await;
        let ingestion = created_arn(&created_ingestion, "ingestion", &case);
        let ingestion_id = uuid_of(&ingestion);
        let named_ingestion = as_form(&ingestion, form);

        let created_destination =
            create_destination(&emulator, &named_bundle, &named_ingestion).await;
        let destination = created_arn(&created_destination, "ingestionDestination", &case);
        let destination_id = uuid_of(&destination);
        let named_destination = as_form(&destination, form);

        let updated = update_destination(
            &emulator,
            &named_bundle,
            &named_ingestion,
            &named_destination,
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateIngestionDestination, got {} with body {}",
            updated.status, updated.body
        );

        // The deletes, innermost first: that is the order AppFabric's documented
        // preconditions require.
        for (operation, answer) in [
            (
                "DeleteIngestionDestination",
                delete_destination(
                    &emulator,
                    &named_bundle,
                    &named_ingestion,
                    &named_destination,
                )
                .await,
            ),
            (
                "DeleteIngestion",
                emulator
                    .delete_ingestion(&named_bundle, &named_ingestion)
                    .await,
            ),
            (
                "DeleteAppAuthorization",
                delete_authorization(&emulator, &named_bundle, &named_authorization).await,
            ),
            (
                "DeleteAppBundle",
                emulator.delete_app_bundle(&named_bundle).await,
            ),
        ] {
            assert_eq!(
                answer.status, 204,
                "case {case:?}: expected HTTP 204 from {operation}, got {} with body {}",
                answer.status, answer.body
            );
        }

        // (the operation, the resource type, the server-minted id it owes, the outcome)
        let expected: Vec<(&str, &str, &str, &str)> = vec![
            ("CreateAppBundle", "AppBundle", &bundle_id, "created"),
            (
                "CreateAppAuthorization",
                "AppAuthorization",
                &authorization_id,
                "created",
            ),
            (
                "ConnectAppAuthorization",
                "AppAuthorization",
                &authorization_id,
                "connected",
            ),
            ("CreateIngestion", "Ingestion", &ingestion_id, "created"),
            (
                "CreateIngestionDestination",
                "IngestionDestination",
                &destination_id,
                "created",
            ),
            (
                "UpdateIngestionDestination",
                "IngestionDestination",
                &destination_id,
                "updated",
            ),
            (
                "DeleteIngestionDestination",
                "IngestionDestination",
                &destination_id,
                "deleted",
            ),
            ("DeleteIngestion", "Ingestion", &ingestion_id, "deleted"),
            (
                "DeleteAppAuthorization",
                "AppAuthorization",
                &authorization_id,
                "deleted",
            ),
            ("DeleteAppBundle", "AppBundle", &bundle_id, "deleted"),
        ];

        // Scoped to the resources THIS row minted — by the id each record claims
        // to be about, so a record naming the ARN of one of them is in scope too
        // — because the capture is process-wide and other tests write into it.
        let ours = [&bundle_id, &authorization_id, &ingestion_id, &destination_id];
        let records: Vec<String> = captured
            .audit_records()
            .into_iter()
            .filter(|rendered| {
                audit_field(rendered, "resource_id")
                    .is_some_and(|logged| ours.iter().any(|id| logged.contains(id.as_str())))
            })
            .collect();

        for (operation, resource_type, resource_id, outcome) in &expected {
            let matching: Vec<&String> = records
                .iter()
                .filter(|rendered| {
                    has_field(rendered, "operation", operation)
                        && has_field(rendered, "outcome", outcome)
                        && has_field(rendered, "resource_type", resource_type)
                })
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "case {case:?}: expected exactly one audit record for {operation}/{outcome}, \
                 got {matching:?} out of {records:?}"
            );
            let logged = audit_field(matching[0], "resource_id").unwrap_or_default();
            assert_eq!(
                logged, *resource_id,
                "case {case:?}: expected the resolved {resource_type} id {resource_id:?} in the \
                 {operation} record, got {logged:?}"
            );
        }

        assert_eq!(
            records.len(),
            expected.len(),
            "case {case:?}: expected one audit record per state-changing call, got {records:?}"
        );
    }
}

/// RULE (the same one, from the other side): a delete of a resource that is
/// ALREADY GONE has no record to read an id off, so it records the identifier the
/// request named — and that is the only outcome whose `resource_id` the caller
/// chooses. The table spans both identifier forms and both idempotent deletes
/// that model no not-found.
#[tokio::test]
async fn a_delete_of_an_absent_resource_records_the_requested_identifier() {
    let captured = audit_capture();

    for form in [Form::Arn, Form::Uuid] {
        let case = format!("identifiers in {form:?} form");
        let emulator = Emulator::new();

        let created_bundle = emulator.create_app_bundle(json!({})).await;
        let bundle = created_arn(&created_bundle, "appBundle", &case);
        let bundle_id = uuid_of(&bundle);
        let named_bundle = as_form(&bundle, form);

        let created_authorization =
            create_authorization(&emulator, &named_bundle, APP, "apiKey", TENANT).await;
        let authorization = authorization_arn(&created_authorization, &case);
        connect_authorization(
            &emulator,
            &named_bundle,
            &as_form(&authorization, form),
            "apiKey",
        )
        .await;

        let created_ingestion = create_ingestion(&emulator, &named_bundle).await;
        let ingestion = created_arn(&created_ingestion, "ingestion", &case);
        let ingestion_id = uuid_of(&ingestion);
        let named_ingestion = as_form(&ingestion, form);

        // Delete each target twice: the second call has no record to resolve, so
        // it is the "already absent" branch.
        for _ in 0..2 {
            let deleted = emulator
                .delete_ingestion(&named_bundle, &named_ingestion)
                .await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: expected HTTP 204 from DeleteIngestion, got {} with body {}",
                deleted.status, deleted.body
            );
        }
        emulator.delete_app_authorizations(&bundle).await;
        for _ in 0..2 {
            let deleted = emulator.delete_app_bundle(&named_bundle).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: expected HTTP 204 from DeleteAppBundle, got {} with body {}",
                deleted.status, deleted.body
            );
        }

        let records: Vec<String> = captured
            .audit_records()
            .into_iter()
            .filter(|rendered| {
                audit_field(rendered, "resource_id").is_some_and(|logged| {
                    logged.contains(&bundle_id) || logged.contains(&ingestion_id)
                })
            })
            .collect();
        for operation in ["DeleteIngestion", "DeleteAppBundle"] {
            let matching: Vec<&String> = records
                .iter()
                .filter(|rendered| {
                    has_field(rendered, "operation", operation)
                        && has_field(rendered, "outcome", "already absent")
                })
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "case {case:?}: expected exactly one {operation} record for an absent target, \
                 got {matching:?} out of {records:?}"
            );
            let logged = audit_field(matching[0], "resource_id").unwrap_or_default();
            let requested = if operation == "DeleteAppBundle" {
                named_bundle.clone()
            } else {
                named_ingestion.clone()
            };
            assert_eq!(
                logged, requested,
                "case {case:?}: expected the requested identifier {requested:?} in the \
                 {operation} record for an absent target, got {logged:?}"
            );
        }
    }
}
