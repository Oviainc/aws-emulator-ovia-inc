// SPDX-License-Identifier: CC0-1.0

//! WHICH records the store-wide FHIR-byte ceiling is charged against.
//!
//! The rule under test: a ceiling on what every data store retains TOGETHER
//! bounds what this service still HOLDS, so a record that has reached its
//! terminal status — whose retained bytes come back exactly by dropping the
//! record — is reaped before a call is refused for the bytes it holds, and the
//! total is taken again. Only a service whose LIVE data stores fill the ceiling
//! refuses.
//!
//! The rows below are every call that charges that total, each run twice: once
//! with the bytes held by data stores the caller DELETED (inside the retention
//! window, so the records are still held and still describe), and once with
//! them held by ACTIVE data stores. A deleted holder must not refuse the call; a
//! live one must.
//!
//! A holder's retained bytes are seeded onto the record's own counter — the
//! value every one of these ceilings is charged against, and the one a FHIR
//! write advances — rather than written as tens of megabytes of FHIR resources,
//! the way the sibling suites seed a record no reachable operation mints
//! (`tests/all/datastore_reaping.rs`). Everything else goes over the real wires
//! through [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter):
//! the awsJson1_0 control plane (create, delete, describe, restore) and a data
//! store's FHIR REST endpoint.
//!
//! Every modelled CLIENT error is asserted at HTTP 400 whatever status its
//! shape's `@httpError` names, for the reason `tests/all/error_envelopes.rs`
//! pins: awsJson1_0 binds nothing to the transport, so this service reports an
//! error's FAULT. A FHIR refusal is an `OperationOutcome` on its own REST wire,
//! which does carry its status.

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    ComAmazonawsHealthlakeHealthlakeState, MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE,
    MAX_FHIR_RESOURCE_BYTES_TOTAL,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, string_at, PAST_TRANSITION};
use crate::fhir_rest_and_validate_source::{created, endpoint_path, outcome_code, HealthLake};

/// How many data stores hold the retained bytes in a row below.
///
/// Each is seeded at [`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`], so together they
/// retain exactly [`MAX_FHIR_RESOURCE_BYTES_TOTAL`]: the store-wide ceiling is
/// reached without any one data store holding more than its own budget allows.
const HOLDERS: usize = MAX_FHIR_RESOURCE_BYTES_TOTAL / MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE;

/// What each holder retains in the row where the call FITS: a quarter of one data
/// store's budget each, so the holders together stay far below the store-wide
/// ceiling and nothing has to be reaped for the call to be served.
const FITTING_BYTES_EACH: usize = MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE / 4;

/// Which call charges the bytes every data store retains together.
#[derive(Clone, Copy, Debug)]
enum Charge {
    /// `CreateFHIRDatastore` carrying `PreloadDataConfig`: the preloaded data set
    /// is retained by the data store the call mints.
    PreloadedCreate,
    /// `RestoreFHIRDatastore`: the restored data store retains a copy of the
    /// source's FHIR resources.
    Restore,
    /// A FHIR `create` against a data store's REST endpoint.
    FhirWrite,
}

/// Who holds the bytes the ceiling is charged against.
#[derive(Clone, Copy, Debug)]
enum Holder {
    /// Data stores the caller DELETED, still inside their retention window: held
    /// records whose bytes the service gives back by dropping them.
    Deleted,
    /// Data stores that are ACTIVE — a genuinely full service.
    Live,
}

/// The data stores a row is run against.
struct Fixture {
    /// A live, backup-enabled data store holding one FHIR resource: the source a
    /// restore copies, and the data store a FHIR write is sent to.
    live: String,
    /// The FHIR endpoint path the service advertised for `live`.
    path: String,
    /// The data stores holding the retained bytes.
    holders: Vec<String>,
}

/// A create request for a data store whose continuous backup is ENABLED — the
/// only kind `RestoreFHIRDatastore` has a restore window for.
///
/// Member names and value sets: `smithy_query(query="shape",
/// shape="BackupConfiguration")` (`Status` one of ENABLED/DISABLED,
/// `RetentionPeriodInDays` in 1..30, `BackupTagsEnabled` a boolean).
fn backup_enabled_create(name: &str) -> Value {
    json!({
        "DatastoreTypeVersion": "R4",
        "DatastoreName": name,
        "BackupConfiguration": {
            "Status": "ENABLED",
            "BackupType": "CONTINUOUS",
            "RetentionPeriodInDays": 7,
            "BackupTagsEnabled": false,
        },
    })
}

/// Dispatch one control-plane call that must succeed, returning its body.
async fn ok(service: &HealthLake, case: &str, operation: &str, body: &Value) -> Value {
    let (status, payload) = service.rpc(operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer 200, got {status}: {payload}"
    );
    payload
}

/// The `DatastoreProperties` `DescribeFHIRDatastore` reports for `id`.
async fn describe(service: &HealthLake, case: &str, id: &str) -> Value {
    ok(
        service,
        case,
        "DescribeFHIRDatastore",
        &json!({ "DatastoreId": id }),
    )
    .await["DatastoreProperties"]
        .clone()
}

/// Create a data store from `request` and report the `DatastoreId` it minted.
async fn create(service: &HealthLake, case: &str, request: &Value) -> String {
    let created = ok(service, case, "CreateFHIRDatastore", request).await;
    string_at(&created, "DatastoreId")
}

/// Stand up the data stores a row runs against: one live backup-enabled data
/// store holding a FHIR resource, and [`HOLDERS`] data stores each retaining
/// `held_each` bytes, put at the status this row's [`Holder`] names.
async fn setup(service: &HealthLake, case: &str, holder: Holder, held_each: usize) -> Fixture {
    let live = create(service, case, &backup_enabled_create("ceiling.live")).await;
    let mut holders = Vec::with_capacity(HOLDERS);
    for index in 0..HOLDERS {
        holders.push(
            create(
                service,
                case,
                &json!({
                    "DatastoreTypeVersion": "R4",
                    "DatastoreName": format!("ceiling.holder-{index}"),
                }),
            )
            .await,
        );
    }
    // One wait for every data store's CREATING -> ACTIVE transition, which the
    // clock owes them all from their own create.
    tokio::time::sleep(PAST_TRANSITION).await;
    let properties = describe(service, case, &live).await;
    assert_eq!(
        properties["DatastoreStatus"],
        json!("ACTIVE"),
        "case {case:?}: the live data store must be ACTIVE before it is written to, got \
         {properties}"
    );
    let path = endpoint_path(&string_at(&properties, "DatastoreEndpoint"));
    // A resource on the live data store, so a restore of it copies bytes rather
    // than nothing at all.
    created(
        service,
        &path,
        "Patient",
        &json!({
            "resourceType": "Patient",
            "name": [{ "family": "Lovelace", "given": ["Ada"] }],
        }),
    )
    .await;

    {
        let store = service
            .engine
            .state::<ComAmazonawsHealthlakeHealthlakeState>();
        let mut store = store.lock();
        for id in &holders {
            let record = store
                .datastores
                .get_mut(id)
                .unwrap_or_else(|| panic!("case {case:?}: a created data store is held"));
            record.fhir_resource_bytes = held_each;
        }
    }

    if matches!(holder, Holder::Deleted) {
        for id in &holders {
            ok(
                service,
                case,
                "DeleteFHIRDatastore",
                &json!({ "DatastoreId": id }),
            )
            .await;
        }
        // One wait for every holder's DELETING -> DELETED transition; their
        // retention runs on from there, so the records are still held.
        tokio::time::sleep(PAST_TRANSITION).await;
        for id in &holders {
            let properties = describe(service, case, id).await;
            assert_eq!(
                properties["DatastoreStatus"],
                json!("DELETED"),
                "case {case:?}: a deleted holder must report DELETED and still describe, got \
                 {properties}"
            );
        }
    }

    Fixture {
        live,
        path,
        holders,
    }
}

/// Make this row's charging call, and report its status and body.
async fn charge(service: &HealthLake, charge: Charge, fixture: &Fixture) -> (u16, Value) {
    match charge {
        Charge::PreloadedCreate => {
            service
                .rpc(
                    "CreateFHIRDatastore",
                    &json!({
                        "DatastoreTypeVersion": "R4",
                        "DatastoreName": "ceiling.preloaded",
                        "PreloadDataConfig": { "PreloadDataType": "SYNTHEA" },
                    }),
                )
                .await
        }
        Charge::Restore => {
            service
                .rpc(
                    "RestoreFHIRDatastore",
                    &json!({
                        "SourceDatastoreId": &fixture.live,
                        "RestoreConfiguration": { "ContinuousBackupRestoreConfiguration": {} },
                    }),
                )
                .await
        }
        Charge::FhirWrite => {
            let (status, body, _headers) = service
                .fhir(
                    "POST",
                    &format!("{}Patient", fixture.path),
                    &json!({
                        "resourceType": "Patient",
                        "name": [{ "family": "Hopper", "given": ["Grace"] }],
                    }),
                )
                .await;
            (status, body)
        }
    }
}

/// The status a served call answers on its own wire: 200 for a control-plane
/// mint, 201 for a FHIR `create` (FHIR R4 §3.1.0.1).
fn served_status(charge: Charge) -> u16 {
    match charge {
        Charge::PreloadedCreate | Charge::Restore => 200,
        Charge::FhirWrite => 201,
    }
}

/// Assert the charging call was SERVED, and that what it answers names what it
/// stored.
fn assert_served(case: &str, charge: Charge, status: u16, payload: &Value) {
    assert_eq!(
        status,
        served_status(charge),
        "case {case:?}: the call must be served when no LIVE data store fills the ceiling, got \
         {status}: {payload}"
    );
    match charge {
        Charge::PreloadedCreate | Charge::Restore => {
            assert_eq!(
                payload["DatastoreStatus"],
                json!("CREATING"),
                "case {case:?}: a minted data store is reported in CREATING, got {payload}"
            );
            assert!(
                !string_at(payload, "DatastoreId").is_empty(),
                "case {case:?}: a minted data store carries an id, got {payload}"
            );
        }
        Charge::FhirWrite => {
            assert_eq!(
                payload["resourceType"],
                json!("Patient"),
                "case {case:?}: a served FHIR create answers the stored resource, got {payload}"
            );
        }
    }
}

/// Assert the charging call was REFUSED for the store-wide byte ceiling: the
/// error its wire models for a service that cannot retain more, naming the
/// ceiling it holds.
fn assert_refused_for_the_ceiling(case: &str, charge: Charge, status: u16, payload: &Value) {
    let diagnostics = match charge {
        Charge::PreloadedCreate | Charge::Restore => {
            assert_eq!(
                status, 400,
                "case {case:?}: a modelled client refusal is reported at 400, got {status}: \
                 {payload}"
            );
            assert_eq!(
                error_code(payload),
                "ThrottlingException",
                "case {case:?}: a full service refuses with the modelled ThrottlingException, got \
                 {payload}"
            );
            error_message(payload)
        }
        Charge::FhirWrite => {
            assert_eq!(
                status, 507,
                "case {case:?}: a FHIR write a full service cannot retain answers 507, got \
                 {status}: {payload}"
            );
            assert_eq!(
                outcome_code(payload),
                "too-costly",
                "case {case:?}: a FHIR write past a retention budget is too-costly, got {payload}"
            );
            payload["issue"][0]["diagnostics"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        }
    };
    assert!(
        diagnostics.contains(&MAX_FHIR_RESOURCE_BYTES_TOTAL.to_string()),
        "case {case:?}: the refusal must name the store-wide byte ceiling \
         {MAX_FHIR_RESOURCE_BYTES_TOTAL}, got {diagnostics:?}"
    );
}

#[tokio::test]
async fn a_store_wide_byte_ceiling_is_charged_only_against_records_this_service_still_holds() {
    for charged in [Charge::PreloadedCreate, Charge::Restore, Charge::FhirWrite] {
        for holder in [Holder::Deleted, Holder::Live] {
            let case = format!("{charged:?} with the bytes held by {holder:?} data stores");
            let service = HealthLake::new();
            let fixture = setup(
                &service,
                &case,
                holder,
                MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE,
            )
            .await;

            let (status, payload) = charge(&service, charged, &fixture).await;

            match holder {
                Holder::Deleted => {
                    assert_served(&case, charged, status, &payload);
                    // The bytes came back by DROPPING the terminal records, so
                    // each one is gone rather than merely uncharged.
                    for id in &fixture.holders {
                        let (status, payload) = service
                            .rpc("DescribeFHIRDatastore", &json!({ "DatastoreId": id }))
                            .await;
                        assert_eq!(
                            status, 400,
                            "case {case:?}: a holder whose bytes were given back must be gone, \
                             got {status}: {payload}"
                        );
                        assert_eq!(
                            error_code(&payload),
                            "ResourceNotFoundException",
                            "case {case:?}: a reaped holder answers ResourceNotFoundException, \
                             got {payload}"
                        );
                    }
                }
                Holder::Live => {
                    assert_refused_for_the_ceiling(&case, charged, status, &payload);
                }
            }
        }
    }
}

#[tokio::test]
async fn a_call_that_fits_the_ceiling_leaves_a_terminal_record_readable() {
    // The sweep is the answer to a FULL service, not something every call does:
    // a call the ceiling admits as things stand leaves the deleted records
    // readable for the rest of their retention.
    for charged in [Charge::PreloadedCreate, Charge::Restore, Charge::FhirWrite] {
        let case = format!("{charged:?} well inside the ceiling");
        let service = HealthLake::new();
        let fixture = setup(&service, &case, Holder::Deleted, FITTING_BYTES_EACH).await;

        let (status, payload) = charge(&service, charged, &fixture).await;
        assert_served(&case, charged, status, &payload);

        for id in &fixture.holders {
            let properties = describe(&service, &case, id).await;
            assert_eq!(
                properties["DatastoreStatus"],
                json!("DELETED"),
                "case {case:?}: a call that fits must leave a deleted data store readable for its \
                 retention, got {properties}"
            );
        }
    }
}
