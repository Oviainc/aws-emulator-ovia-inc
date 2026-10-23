// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteIngestion`, driven over the wire (restJson1)
//! through the same router the emulator boots.
//!
//! Each test states a RULE over a table of inputs rather than one recorded call:
//! a delete removes exactly the ingestion its identifier names from the bundle
//! that owns it (in either identifier form), a delete of a target the bundle does
//! not hold is an idempotent no-op success, an app bundle that does not exist is
//! the modelled not-found, and the two preconditions the operation documents are
//! refused with the modelled `ValidationException`.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, the app-bundle and
//! ingestion calls) is the one `app_bundle_ingestion_lifecycle` already builds;
//! only the destination set-up and the delete tables are new here.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::{
    service_state, INGESTION_STATE_DISABLED, INGESTION_STATE_ENABLED,
};

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, encode, unknown_bundle_arn, unknown_ingestion_arn, unknown_uuid, uuid_of, Emulator,
};

/// The applications the model's `app` member documents ("Valid values are: SLACK
/// ASANA JIRA M365 M365AUDITLOGS ZOOM ZENDESK OKTA GOOGLE DROPBOX SMARTSHEET
/// CISCO" — `smithy_query(query="doc", shape="CreateIngestionRequest",
/// member="app")`). The tables below walk this list instead of repeating one
/// value, so no row depends on which application an ingestion carries.
const APPS: &[&str] = &[
    "ZENDESK",
    "OKTA",
    "DROPBOX",
    "CISCO",
    "SMARTSHEET",
    "M365AUDITLOGS",
];

/// Which of the two forms the model's `Identifier` shape accepts a table row
/// names a resource by (`^arn:.+$|^[a-f0-9]{8}-…$`).
#[derive(Debug, Clone, Copy)]
enum Form {
    Arn,
    Uuid,
}

/// An ARN rendered in the form a row asks for.
fn as_form(arn: &str, form: Form) -> String {
    match form {
        Form::Arn => arn.to_string(),
        Form::Uuid => uuid_of(arn),
    }
}

/// A second UUID-shaped identifier no create call ever minted, so a row can name
/// two distinct unknown resources.
fn other_unknown_uuid() -> String {
    "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee".to_string()
}

/// `POST /appbundles/{b}/ingestions/{i}/ingestiondestinations` — the real
/// `CreateIngestionDestination`, which is what gives an ingestion the
/// destinations the delete precondition is about. `schema`/`format` come from the
/// model's closed enums (`Schema = ocsf | raw`, `Format = json | parquet`), and
/// the destination is an S3 bucket (`Destination` = `s3Bucket` | `firehoseStream`).
async fn create_destination(emulator: &Emulator, bundle: &str, ingestion: &str, bucket: &str) {
    let created = emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(bundle),
                encode(ingestion)
            ),
            Some(json!({
                "processingConfiguration": {"auditLog": {"schema": "ocsf", "format": "json"}},
                "destinationConfiguration":
                    {"auditLog": {"destination": {"s3Bucket": {"bucketName": bucket}}}},
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected HTTP 201 from CreateIngestionDestination, got {} with body {}",
        created.status, created.body
    );
}

/// The ARNs `ListIngestions` reports for one bundle, in the order it reports them.
async fn ingestion_arns(emulator: &Emulator, bundle: &str, case: &str) -> Vec<String> {
    let answer = emulator.list_ingestions(bundle, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from ListIngestions, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("ingestions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no ingestions in {}", answer.body))
        .iter()
        .map(|summary| {
            summary
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a summary without an arn in {}", answer.body))
                .to_string()
        })
        .collect()
}

/// Create one app bundle and `total` ingestions under it, and answer
/// `(bundle arn, ingestion arns in creation order)`.
async fn bundle_with_ingestions(emulator: &Emulator, total: usize, case: &str) -> (String, Vec<String>) {
    let bundle = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );
    let mut ingestions = Vec::with_capacity(total);
    for index in 0..total {
        let created = emulator
            .create_ingestion(
                &bundle,
                json!({
                    "app": APPS[index % APPS.len()],
                    "tenantId": format!("tenant-{index}"),
                    "ingestionType": "auditLog",
                }),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateIngestion, got {} with body {}",
            created.status, created.body
        );
        ingestions.push(arn_of(&created.body, "ingestion"));
    }
    (bundle, ingestions)
}

/// RULE: deleting an ingestion removes exactly the record its identifier names
/// from the bundle that owns it — the reader no longer reports it, every sibling
/// survives in creation order — and, the operation being `@idempotent`, repeating
/// the call answers HTTP 204 again without changing anything further.
///
/// The table spans how many ingestions the bundle holds, which one is deleted
/// (first, middle, last) and the identifier form each of the two labels is named
/// by, because none of those may change the outcome.
#[tokio::test]
async fn deleting_an_ingestion_removes_exactly_that_ingestion() {
    // (case, ingestions created, index deleted, bundle form, ingestion form)
    let cases: Vec<(&str, usize, usize, Form, Form)> = vec![
        ("one ingestion, both by arn", 1, 0, Form::Arn, Form::Arn),
        ("one ingestion, both by uuid", 1, 0, Form::Uuid, Form::Uuid),
        ("two, delete the last by uuid", 2, 1, Form::Arn, Form::Uuid),
        ("three, delete the first", 3, 0, Form::Uuid, Form::Arn),
        ("three, delete the middle", 3, 1, Form::Arn, Form::Arn),
        ("four, delete the last", 4, 3, Form::Uuid, Form::Uuid),
        ("five, delete the third", 5, 2, Form::Arn, Form::Uuid),
    ];

    for (case, total, index, bundle_form, ingestion_form) in cases {
        let emulator = Emulator::new();
        let (bundle, ingestions) = bundle_with_ingestions(&emulator, total, case).await;
        let doomed = ingestions[index].clone();
        let survivors: Vec<String> = ingestions
            .iter()
            .filter(|arn| **arn != doomed)
            .cloned()
            .collect();

        let deleted = emulator
            .delete_ingestion(
                &as_form(&bundle, bundle_form),
                &as_form(&doomed, ingestion_form),
            )
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204, got {} with body {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: a 204 carries no response members, got {}",
            deleted.body
        );

        // Gone through the reader, in either identifier form.
        for form in [doomed.clone(), uuid_of(&doomed)] {
            let read = emulator.get_ingestion(&bundle, &form).await;
            assert_eq!(
                read.status, 404,
                "case {case:?} read by {form:?}: expected HTTP 404 after the delete, got {} with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.error_code(),
                "ResourceNotFoundException",
                "case {case:?} read by {form:?}: expected ResourceNotFoundException, got {}",
                read.body
            );
        }

        // And every sibling is untouched.
        let listed = ingestion_arns(&emulator, &bundle, case).await;
        assert_eq!(
            listed, survivors,
            "case {case:?}: expected the surviving ingestions {survivors:?}, got {listed:?}"
        );
        for survivor in &survivors {
            let read = emulator.get_ingestion(&bundle, survivor).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: survivor {survivor:?} must still be readable, got {} with body {}",
                read.status, read.body
            );
        }

        let again = emulator
            .delete_ingestion(
                &as_form(&bundle, bundle_form),
                &as_form(&doomed, ingestion_form),
            )
            .await;
        assert_eq!(
            again.status, 204,
            "case {case:?}: a repeated delete is idempotent, got {} with body {}",
            again.status, again.body
        );
        let listed = ingestion_arns(&emulator, &bundle, case).await;
        assert_eq!(
            listed, survivors,
            "case {case:?}: the repeated delete must change nothing, expected {survivors:?}, got {listed:?}"
        );
    }
}

/// RULE: a delete whose target the named bundle does not hold is a SUCCESS that
/// changes nothing (`@idempotent`: the store is already in the state the request
/// asks for), whichever form the absent identifier takes and whether it is
/// unknown altogether or names a resource living under a different parent.
#[tokio::test]
async fn deleting_an_ingestion_the_bundle_does_not_hold_is_a_noop_success() {
    let emulator = Emulator::new();
    let (bundle, ingestions) = bundle_with_ingestions(&emulator, 2, "set-up").await;
    let (other_bundle, other_ingestions) =
        bundle_with_ingestions(&emulator, 1, "set-up sibling").await;
    let foreign = other_ingestions[0].clone();

    // (case, bundle identifier, ingestion identifier the bundle does not hold)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an unknown uuid",
            bundle.clone(),
            unknown_uuid(),
        ),
        (
            "another unknown uuid, bundle by uuid",
            uuid_of(&bundle),
            other_unknown_uuid(),
        ),
        (
            "an ingestion arn this service never minted",
            bundle.clone(),
            unknown_ingestion_arn(&uuid_of(&bundle), &unknown_uuid()),
        ),
        (
            "another bundle's ingestion, by arn",
            bundle.clone(),
            foreign.clone(),
        ),
        (
            "another bundle's ingestion, by uuid",
            uuid_of(&bundle),
            uuid_of(&foreign),
        ),
    ];

    for (case, bundle_identifier, ingestion_identifier) in cases {
        let answer = emulator
            .delete_ingestion(&bundle_identifier, &ingestion_identifier)
            .await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: expected HTTP 204, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: a 204 carries no response members, got {}",
            answer.body
        );

        let listed = ingestion_arns(&emulator, &bundle, case).await;
        assert_eq!(
            listed, ingestions,
            "case {case:?}: the no-op must change nothing, expected {ingestions:?}, got {listed:?}"
        );
        let read = emulator.get_ingestion(&other_bundle, &foreign).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the other bundle's ingestion must survive, got {} with body {}",
            read.status, read.body
        );
    }
}

/// RULE: the app bundle in the path is a resource of its own, so an identifier
/// naming no stored bundle is the modelled `ResourceNotFoundException` (HTTP 404)
/// reporting the `AppBundle` — never a silent success — whatever the ingestion
/// identifier beside it names.
#[tokio::test]
async fn deleting_under_an_unknown_app_bundle_is_not_found() {
    let emulator = Emulator::new();
    let (bundle, ingestions) = bundle_with_ingestions(&emulator, 1, "set-up").await;
    let real = ingestions[0].clone();

    // (case, unknown bundle identifier, ingestion identifier)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "unknown bundle uuid, unknown ingestion uuid",
            unknown_uuid(),
            other_unknown_uuid(),
        ),
        (
            "unknown bundle arn, unknown ingestion arn",
            unknown_bundle_arn(&unknown_uuid()),
            unknown_ingestion_arn(&unknown_uuid(), &other_unknown_uuid()),
        ),
        (
            "unknown bundle uuid, an ingestion that does exist elsewhere",
            other_unknown_uuid(),
            real.clone(),
        ),
        (
            "unknown bundle arn, that ingestion by uuid",
            unknown_bundle_arn(&other_unknown_uuid()),
            uuid_of(&real),
        ),
    ];

    for (case, bundle_identifier, ingestion_identifier) in cases {
        let answer = emulator
            .delete_ingestion(&bundle_identifier, &ingestion_identifier)
            .await;
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
            Some("AppBundle"),
            "case {case:?}: the missing resource is the app bundle, got {}",
            answer.body
        );
    }

    // And the refusals left the real ingestion alone.
    let read = emulator.get_ingestion(&bundle, &real).await;
    assert_eq!(
        read.status, 200,
        "the stored ingestion must survive a delete under an unknown bundle, got {} with body {}",
        read.status, read.body
    );
}

/// Put the stored ingestion into the `IngestionState` a precondition row needs.
///
/// `StartIngestion` and `StopIngestion` are still scaffolded (HTTP 501), so the
/// store is the only way to reach the `enabled` state — and it is the SAME store
/// the handler reads (`state::service_state`, the engine this emulator was
/// mounted on), so the set-up is the real record, not a double of one. The
/// destinations a row needs are created through the real
/// `CreateIngestionDestination` instead (see [`create_destination`]).
fn set_ingestion_state(
    engine: &engine::InMemoryEngine,
    bundle_identifier: &str,
    ingestion_identifier: &str,
    state: &str,
    case: &str,
) {
    let store =
        service_state(engine).unwrap_or_else(|| panic!("case {case:?}: no appfabric store"));
    let mut store = store.lock();
    let ingestion = store
        .resolve_bundle_mut(bundle_identifier)
        .unwrap_or_else(|| panic!("case {case:?}: bundle {bundle_identifier} is not stored"))
        .resolve_ingestion_mut(ingestion_identifier)
        .unwrap_or_else(|| {
            panic!("case {case:?}: ingestion {ingestion_identifier} is not stored")
        });
    ingestion.state = state.to_string();
}

/// RULE: the operation's two documented preconditions — "You must stop (disable)
/// the ingestion and you must delete all associated ingestion destinations before
/// you can delete an app ingestion" — are enforced: an ingestion that is not
/// `disabled`, or that still holds any ingestion destination, is refused with the
/// modelled `ValidationException` (HTTP 400) and stays exactly as it was, while a
/// disabled ingestion holding none is deleted.
///
/// The table spans both values of `IngestionState` and a destination count of
/// none, one and several, which is every combination the rule ranges over.
#[tokio::test]
async fn an_ingestion_is_deletable_only_once_stopped_and_emptied() {
    // (case, ingestion state, destinations held, expected status)
    let cases: Vec<(&str, &str, usize, u16)> = vec![
        (
            "disabled, no destinations",
            INGESTION_STATE_DISABLED,
            0,
            204,
        ),
        ("enabled, no destinations", INGESTION_STATE_ENABLED, 0, 400),
        ("enabled, one destination", INGESTION_STATE_ENABLED, 1, 400),
        ("disabled, one destination", INGESTION_STATE_DISABLED, 1, 400),
        (
            "disabled, three destinations",
            INGESTION_STATE_DISABLED,
            3,
            400,
        ),
        ("enabled, two destinations", INGESTION_STATE_ENABLED, 2, 400),
    ];

    for (case, state, destinations, expected) in cases {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let emulator = Emulator::with_engine(&engine);
        let (bundle, ingestions) = bundle_with_ingestions(&emulator, 1, case).await;
        let ingestion = ingestions[0].clone();
        for index in 0..destinations {
            create_destination(&emulator, &bundle, &ingestion, &format!("audit-logs-{index}"))
                .await;
        }
        // Created disabled, so only the `enabled` rows need the store at all.
        if state != INGESTION_STATE_DISABLED {
            set_ingestion_state(&engine, &bundle, &ingestion, state, case);
        }

        let answer = emulator.delete_ingestion(&bundle, &ingestion).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );

        let read = emulator.get_ingestion(&bundle, &ingestion).await;
        if expected == 204 {
            assert_eq!(
                read.status, 404,
                "case {case:?}: the deleted ingestion must be gone, got {} with body {}",
                read.status, read.body
            );
        } else {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                answer.body
            );
            assert_eq!(
                read.status, 200,
                "case {case:?}: a refused delete must leave the ingestion in place, got {} with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.body
                    .get("ingestion")
                    .and_then(|ingestion| ingestion.get("state"))
                    .and_then(Value::as_str),
                Some(state),
                "case {case:?}: a refused delete must not change the state, got {}",
                read.body
            );
        }
    }
}
