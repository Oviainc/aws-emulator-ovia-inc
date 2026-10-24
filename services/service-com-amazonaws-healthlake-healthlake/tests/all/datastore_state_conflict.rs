// SPDX-License-Identifier: CC0-1.0

//! What a control-plane MUTATION of a data store answers when the record it
//! targets is not in a state that can take it.
//!
//! One rule, asserted over a SET of inputs rather than one recorded request: a
//! mutation refused because of its target's LIFECYCLE STATE answers the
//! operation's modelled resource-state error — the `ConflictException` the model
//! documents as "The data store is in a transition state and the user requested
//! action cannot be performed." while the record is in a non-terminal transition
//! (`CREATING`, `DELETING`), and `ResourceNotFoundException` once it has reached
//! the terminal `DELETED` — and never the authorization-shaped
//! `AccessDeniedException`, which this service raises for an authorization
//! failure alone.
//!
//! The rows span the two axes the rule generalises over: the data store's status
//! (every state this slice reaches, including `ACTIVE` as the positive control)
//! and the mutation attempted on it (`UpdateFHIRDatastore` carrying each shape
//! of request it accepts, and `DeleteFHIRDatastore`). Every status is reached
//! through the real lifecycle rather than by poking the state, and every call
//! goes through [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the shared harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization and the error envelope are exercised too.

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    datastore_in_transition_message, DatastoreAction, DatastoreStatus, DATASTORE_NOT_FOUND_MESSAGE,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, HealthLake, PAST_TRANSITION};

/// The name every data store below is created with, so a refusal that wrote
/// something is visible as a changed name.
const CREATED_NAME: &str = "state-gated.store";

/// Every mutation of this service's data store control plane, as
/// `(operation, the request it carries besides the DatastoreId, the action its
/// state-conflict sentence names)`.
///
/// Source: `smithy_query(query="operation",
/// operation="UpdateFHIRDatastore,DeleteFHIRDatastore")` — the two operations
/// whose input is a `DatastoreId` and whose errors include `ConflictException`.
/// The `UpdateFHIRDatastore` rows differ along WHICH members the request
/// carries, so the gate cannot be keyed to one request's shape.
fn mutations() -> Vec<(&'static str, Value, DatastoreAction)> {
    vec![
        (
            "UpdateFHIRDatastore",
            json!({ "DatastoreName": "renamed-while-gated" }),
            DatastoreAction::Update,
        ),
        (
            "UpdateFHIRDatastore",
            json!({
                "AnalyticsConfiguration": { "Status": "ENABLED" },
                "NlpConfiguration": { "Status": "ENABLED" },
            }),
            DatastoreAction::Update,
        ),
        (
            "UpdateFHIRDatastore",
            json!({
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 7,
                    "BackupTagsEnabled": false,
                },
            }),
            DatastoreAction::Update,
        ),
        (
            "UpdateFHIRDatastore",
            json!({ "DatastoreName": "клиника записи 7" }),
            DatastoreAction::Update,
        ),
        (
            "UpdateFHIRDatastore",
            json!({}),
            DatastoreAction::Update,
        ),
        ("DeleteFHIRDatastore", json!({}), DatastoreAction::Delete),
    ]
}

/// Every state a data store reaches in this slice, with the status it settles to
/// once the transition the clock owes has fallen due.
///
/// Source: `smithy_query(query="shape", shape="DatastoreStatus")` for the value
/// set; the states the enum also declares (`CREATE_FAILED`, `UPDATING`,
/// `UPDATE_FAILED`) describe work or a failure nothing in this slice produces,
/// so no row can reach them through the lifecycle.
fn states() -> Vec<(DatastoreStatus, &'static str)> {
    vec![
        (DatastoreStatus::Creating, "ACTIVE"),
        (DatastoreStatus::Active, "ACTIVE"),
        (DatastoreStatus::Deleting, "DELETED"),
        (DatastoreStatus::Deleted, "DELETED"),
    ]
}

/// Mint a data store and drive it, through the real lifecycle, to `status`.
async fn datastore_in(service: &HealthLake, status: DatastoreStatus) -> String {
    let (id, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": CREATED_NAME }))
        .await;
    match status {
        // The create's own transition is still pending.
        DatastoreStatus::Creating => {}
        DatastoreStatus::Active => tokio::time::sleep(PAST_TRANSITION).await,
        DatastoreStatus::Deleting => {
            tokio::time::sleep(PAST_TRANSITION).await;
            service
                .ok("DeleteFHIRDatastore", &json!({ "DatastoreId": &id }))
                .await;
        }
        DatastoreStatus::Deleted => {
            tokio::time::sleep(PAST_TRANSITION).await;
            service
                .ok("DeleteFHIRDatastore", &json!({ "DatastoreId": &id }))
                .await;
            tokio::time::sleep(PAST_TRANSITION).await;
        }
    }

    // The row really put the record in the state it claims, as the reporting
    // operation sees it.
    let described = service
        .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &id }))
        .await;
    assert_eq!(
        described["DatastoreProperties"]["DatastoreStatus"],
        status.as_wire(),
        "the row must put the data store in status {}, got {described}",
        status.as_wire()
    );
    id
}

#[tokio::test]
async fn a_mutation_on_a_data_store_in_transition_answers_conflict_not_access_denied() {
    for (status, settled) in states() {
        for (operation, members, action) in mutations() {
            let case = format!("{operation} on a {} data store with {members}", status.as_wire());
            // A service per row, so neither the lifecycle waits nor an accepted
            // mutation of one row can be observed by another.
            let service = HealthLake::new();
            let id = datastore_in(&service, status).await;

            let mut request = members.clone();
            request["DatastoreId"] = json!(&id);
            let (http_status, payload) = service.call(operation, &request).await;

            if status == DatastoreStatus::Active {
                // The positive control: an ACTIVE record takes every mutation,
                // so the refusals below are about the STATE and not about the
                // request.
                assert_eq!(
                    http_status, 200,
                    "case {case:?}: an ACTIVE data store must accept the mutation, \
                     got {http_status}: {payload}"
                );
                continue;
            }

            // Every modelled client error of this service goes on the wire as
            // 400, whatever 4xx its shape declares: see
            // `tests/all/error_envelopes.rs` for that rule.
            assert_eq!(
                http_status, 400,
                "case {case:?}: a refused mutation must answer 400, got {http_status}: {payload}"
            );

            let code = error_code(&payload);
            let message = error_message(&payload);
            let expected_code = if status == DatastoreStatus::Deleted {
                "ResourceNotFoundException"
            } else {
                "ConflictException"
            };
            assert_eq!(
                code, expected_code,
                "case {case:?}: expected __type {expected_code}, got {payload}"
            );
            assert_ne!(
                code, "AccessDeniedException",
                "case {case:?}: a resource-state refusal must not be reported as an \
                 authorization failure, got {payload}"
            );

            let expected_message = if status == DatastoreStatus::Deleted {
                DATASTORE_NOT_FOUND_MESSAGE.to_string()
            } else {
                // Read from the crate rather than retyped, so the test and the
                // service cannot drift.
                datastore_in_transition_message(status, action)
            };
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected {expected_message:?}, got {message:?}"
            );
            assert!(
                !message.contains(&id),
                "case {case:?}: a refusal must quote no requested identifier, got {message:?}"
            );
            if status != DatastoreStatus::Deleted {
                assert!(
                    message.contains(status.as_wire()),
                    "case {case:?}: the conflict sentence must state the status the record is \
                     in, got {message:?}"
                );
                assert!(
                    message.contains(action.as_participle()),
                    "case {case:?}: the conflict sentence must name the refused action, \
                     got {message:?}"
                );
            }

            // The refusal wrote nothing: the name is still the one the create
            // set, and the lifecycle the record was already on is undisturbed —
            // the transition the clock owed still lands, and a terminal record
            // stays terminal.
            tokio::time::sleep(PAST_TRANSITION).await;
            let after = service
                .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &id }))
                .await["DatastoreProperties"]
                .clone();
            assert_eq!(
                after["DatastoreName"], CREATED_NAME,
                "case {case:?}: a refused mutation must not rename the data store, got {after}"
            );
            assert_eq!(
                after["DatastoreStatus"], settled,
                "case {case:?}: a refused mutation must leave the lifecycle alone, expected \
                 {settled}, got {after}"
            );
        }
    }
}
