// SPDX-License-Identifier: CC0-1.0

//! What `DeleteFHIRDatastore` answers when the record it targets is not in a
//! state that can take it.
//!
//! One rule, asserted over a SET of inputs rather than one recorded request: a
//! delete refused because of its target's LIFECYCLE STATE answers the
//! operation's modelled resource-state error — the `ConflictException` the model
//! documents as "The data store is in a transition state and the user requested
//! action cannot be performed." while the record is in a non-terminal transition
//! (`CREATING`, `UPDATING`, `DELETING`), and `ResourceNotFoundException` once it
//! has reached the terminal `DELETED`.
//!
//! What `UpdateFHIRDatastore` answers for a target that is not `ACTIVE` is a
//! DIFFERENT classification, pinned over every status and every shape of update
//! in `tests/all/datastore_update_not_active.rs`: the two operations are not
//! required to agree, and neither suite may be changed to match the other.
//!
//! The rows span the axis the rule generalises over: the data store's status
//! (every state this slice reaches, including `ACTIVE` as the positive control).
//! Every status is reached through the real lifecycle rather than by poking the
//! state, and every call goes through
//! [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the shared harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization and the error envelope are exercised too.

use serde_json::json;
use service_com_amazonaws_healthlake_healthlake::state::{
    datastore_in_transition_message, DatastoreAction, DatastoreStatus, DATASTORE_NOT_FOUND_MESSAGE,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, HealthLake, PAST_TRANSITION};

/// The name every data store below is created with, so a refusal that wrote
/// something is visible as a changed name.
const CREATED_NAME: &str = "state-gated.store";

/// The mutation this suite's rule covers, and the action its state-conflict
/// sentence names.
///
/// Source: `smithy_query(query="operation", operation="DeleteFHIRDatastore")` —
/// the operation's whole input is the `DatastoreId`, so the one axis left to
/// span is the target's status.
const MUTATION: (&str, DatastoreAction) = ("DeleteFHIRDatastore", DatastoreAction::Delete);

/// Every state a data store reaches in this slice, with the status it settles to
/// once the transition the clock owes has fallen due.
///
/// Source: `smithy_query(query="shape", shape="DatastoreStatus")` for the value
/// set; the states the enum also declares (`CREATE_FAILED`, `UPDATE_FAILED`)
/// describe a FAILURE of the asynchronous work nothing in this slice fails at,
/// so no row can reach them through the lifecycle.
///
/// `pub(crate)` beside [`datastore_in`], which drives a data store to one of
/// them: the sibling suites in `tests/all/` span the statuses from this one
/// table rather than each copying it, so they cannot drift about which states
/// the slice can reach.
pub(crate) fn states() -> Vec<(DatastoreStatus, &'static str)> {
    vec![
        (DatastoreStatus::Creating, "ACTIVE"),
        (DatastoreStatus::Active, "ACTIVE"),
        (DatastoreStatus::Updating, "ACTIVE"),
        (DatastoreStatus::Deleting, "DELETED"),
        (DatastoreStatus::Deleted, "DELETED"),
    ]
}

/// Mint a data store named `name` and drive it, through the real lifecycle, to
/// `status`.
///
/// `pub(crate)` so the sibling suites in `tests/all/` reach a given status
/// through this one driver rather than each growing its own copy of the
/// lifecycle waits.
pub(crate) async fn datastore_in(
    service: &HealthLake,
    status: DatastoreStatus,
    name: &str,
) -> String {
    let (id, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": name }))
        .await;
    match status {
        // The create's own transition is still pending.
        DatastoreStatus::Creating => {}
        DatastoreStatus::Active => tokio::time::sleep(PAST_TRANSITION).await,
        // An update of its own, carrying no member, so the record is in
        // `UPDATING` without the name this suite watches having changed.
        DatastoreStatus::Updating => {
            tokio::time::sleep(PAST_TRANSITION).await;
            service
                .ok("UpdateFHIRDatastore", &json!({ "DatastoreId": &id }))
                .await;
        }
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
async fn a_delete_of_a_data_store_in_transition_answers_its_resource_state_error() {
    let (operation, action) = MUTATION;
    for (status, settled) in states() {
        let case = format!("{operation} on a {} data store", status.as_wire());
        // A service per row, so neither the lifecycle waits nor an accepted
        // mutation of one row can be observed by another.
        let service = HealthLake::new();
        let id = datastore_in(&service, status, CREATED_NAME).await;

        let request = json!({ "DatastoreId": &id });
        let (http_status, payload) = service.call(operation, &request).await;

        if status == DatastoreStatus::Active {
            // The positive control: an ACTIVE record takes the mutation, so the
            // refusals below are about the STATE and not about the request.
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
