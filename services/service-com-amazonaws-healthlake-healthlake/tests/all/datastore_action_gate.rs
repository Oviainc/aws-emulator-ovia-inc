// SPDX-License-Identifier: CC0-1.0

//! The ONE status gate every data-store action in this service comes through,
//! asserted across both axes it spans.
//!
//! The rule: whether an action on a data store may proceed is decided from that
//! record's `DatastoreStatus` alone, identically for every action — a settled,
//! usable (`ACTIVE`) record takes it; a record in a lifecycle transition refuses
//! it as a transition conflict stating
//! [`datastore_in_transition_message`](service_com_amazonaws_healthlake_healthlake::state::datastore_in_transition_message)
//! for that action; a record that has reached the terminal `DELETED` status
//! refuses it as gone, stating
//! [`DATASTORE_NOT_FOUND_MESSAGE`](service_com_amazonaws_healthlake_healthlake::state::DATASTORE_NOT_FOUND_MESSAGE).
//! What an operation decides for itself is only the modelled error SHAPE it
//! carries that answer on, which the model does not give all of them alike.
//!
//! The rows are the whole cross product of the two axes the rule generalises
//! over: the ACTION (`DeleteFHIRDatastore`, `StartFHIRExportJob` and
//! `RestoreFHIRDatastore` — the three operations that gate on a data store's
//! status, each with the error shape its own model declares) and the target's
//! STATUS (every state this slice reaches, `ACTIVE` included as the positive
//! control). The status table is the shared one in
//! `tests/all/datastore_state_conflict.rs` rather than a copy, and the statuses
//! are reached through its shared lifecycle driver, so this suite cannot drift
//! from its siblings about which states exist.
//!
//! The sibling suites assert parts of the same rule — the delete path over every
//! status (`tests/all/datastore_state_conflict.rs`) and the transition SENTENCE
//! over every action (`tests/all/datastore_transition_conflict_sentence.rs`) —
//! and neither is changed by this one: what this suite adds is the full product,
//! so the terminal arm of the gate is pinned for every action and not only for
//! the delete.
//!
//! `UpdateFHIRDatastore` is deliberately NOT a row: it draws a different line,
//! pinned by `tests/all/datastore_update_not_active.rs`.
//!
//! Every modelled CLIENT error is asserted at HTTP 400 whatever status its
//! shape's `@httpError` names, for the reason `tests/all/error_envelopes.rs`
//! pins.

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    datastore_in_transition_message, DatastoreAction, DatastoreStatus,
    DATASTORE_NOT_FOUND_MESSAGE,
};

use crate::datastore_state_conflict::{datastore_in, states};
use crate::fhir_datastore_lifecycle::{error_code, error_message, HealthLake, PAST_TRANSITION};

/// The name every data store below is created with, so an action that wrote
/// something after being refused is visible as a changed name.
const CREATED_NAME: &str = "action.gated.store";

/// An IAM role ARN in the form the modelled `IamRoleArn` pattern admits
/// (`smithy_query(query="operation", operation="StartFHIRExportJob")`:
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeExport";

/// An `S3Configuration` the modelled union admits
/// (`smithy_query(query="shape", shape="S3Configuration")`): an `S3Uri` matching
/// `^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$` and a bare `KmsKeyId`.
const EXPORT_S3_URI: &str = "s3://action-gate-bucket/out/";
const EXPORT_KMS_KEY_ID: &str = "1234abcd-12ab-34cd-56ef-1234567890ab";

/// Every action that comes through the shared gate, with the action its
/// transition sentence names itself by and the modelled error shape it carries a
/// transition conflict on.
///
/// Source: `smithy_query(query="operation",
/// operation="DeleteFHIRDatastore,StartFHIRExportJob,RestoreFHIRDatastore")` —
/// the delete and the restore declare a `ConflictException`, the export start
/// declares none and carries the refusal on the `ValidationException` it does
/// declare. Every one of them declares `ResourceNotFoundException`, which is the
/// gate's terminal arm for all three.
fn gated_actions() -> Vec<(&'static str, DatastoreAction, &'static str)> {
    vec![
        (
            "DeleteFHIRDatastore",
            DatastoreAction::Delete,
            "ConflictException",
        ),
        (
            "StartFHIRExportJob",
            DatastoreAction::ExportFrom,
            "ValidationException",
        ),
        (
            "RestoreFHIRDatastore",
            DatastoreAction::Restore,
            "ConflictException",
        ),
    ]
}

/// A request for `operation` that targets the data store `id` and is otherwise
/// complete, so the only thing left to refuse it over is the target's state.
fn request_for(operation: &str, id: &str) -> Value {
    match operation {
        "DeleteFHIRDatastore" => json!({ "DatastoreId": id }),
        "StartFHIRExportJob" => json!({
            "DatastoreId": id,
            "DataAccessRoleArn": DATA_ACCESS_ROLE_ARN,
            "OutputDataConfig": {
                "S3Configuration": {
                    "S3Uri": EXPORT_S3_URI,
                    "KmsKeyId": EXPORT_KMS_KEY_ID,
                },
            },
        }),
        "RestoreFHIRDatastore" => json!({
            "SourceDatastoreId": id,
            "RestoreConfiguration": { "ContinuousBackupRestoreConfiguration": {} },
        }),
        other => panic!("no request is defined for {other}"),
    }
}

/// Enable continuous backup on an ACTIVE data store and wait for the update to
/// settle, so a restore of it has a window to work from and the ACTIVE row of
/// every action is accepted on its own terms.
///
/// Only the ACTIVE rows need it: the gate is reached before anything else about
/// the request or the record is considered, which is itself what the refused
/// rows below show.
///
/// `BackupConfiguration` member names and value sets:
/// `smithy_query(query="shape", shape="BackupConfiguration")`.
async fn enable_backup(service: &HealthLake, id: &str) {
    service
        .ok(
            "UpdateFHIRDatastore",
            &json!({
                "DatastoreId": id,
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 7,
                    "BackupTagsEnabled": false,
                },
            }),
        )
        .await;
    tokio::time::sleep(PAST_TRANSITION).await;
}

#[tokio::test]
async fn every_gated_action_partitions_the_statuses_the_same_way() {
    for (operation, action, transition_code) in gated_actions() {
        for (status, settled) in states() {
            let case = format!("{operation} on a {} data store", status.as_wire());
            // A service per row, so neither the lifecycle waits nor an accepted
            // call of one row can be observed by another.
            let service = HealthLake::new();
            let id = datastore_in(&service, status, CREATED_NAME).await;
            if status == DatastoreStatus::Active {
                enable_backup(&service, &id).await;
            }

            let (http_status, payload) = service.call(operation, &request_for(operation, &id)).await;

            if status == DatastoreStatus::Active {
                // The positive control: a settled, usable record takes every one
                // of these actions, so the refusals below are about the STATE
                // and not about the request each row sent.
                assert_eq!(
                    http_status, 200,
                    "case {case:?}: an ACTIVE data store must accept the action, \
                     got {http_status}: {payload}"
                );
                continue;
            }

            assert_eq!(
                http_status, 400,
                "case {case:?}: a refused action must answer 400, got {http_status}: {payload}"
            );

            // The two arms of the one partition: a terminal record is GONE for
            // every action, whatever shape that action carries a transition
            // conflict on.
            let (expected_code, expected_message) = if status == DatastoreStatus::Deleted {
                (
                    "ResourceNotFoundException",
                    DATASTORE_NOT_FOUND_MESSAGE.to_string(),
                )
            } else {
                // Read from the crate rather than retyped, so the one sentence
                // the service states and the one expected here cannot drift.
                (transition_code, datastore_in_transition_message(status, action))
            };
            let code = error_code(&payload);
            assert_eq!(
                code, expected_code,
                "case {case:?}: expected __type {expected_code}, got {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected {expected_message:?}, got {message:?}"
            );
            assert!(
                !message.contains(&id),
                "case {case:?}: a refusal must quote no requested identifier, got {message:?}"
            );

            // The refusal wrote nothing: the name is still the one the create
            // set, and the lifecycle the record was already on is undisturbed —
            // the transition the clock owed still lands, and a terminal record
            // stays terminal.
            tokio::time::sleep(PAST_TRANSITION).await;
            let after = service.describe(&id).await;
            assert_eq!(
                after["DatastoreName"], CREATED_NAME,
                "case {case:?}: a refused action must not rename the data store, got {after}"
            );
            assert_eq!(
                after["DatastoreStatus"], settled,
                "case {case:?}: a refused action must leave the lifecycle alone, expected \
                 {settled}, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn one_status_earns_every_action_the_same_classification() {
    // The same record state, every action: which KIND of refusal the status
    // earns (a transition conflict, or a gone record) does not depend on the
    // action — only the shape it is carried on and the participle in the
    // sentence do. Asserted per status over the actions, so a status classified
    // one way for one action and another way for another is a failure whichever
    // action the drift was introduced in.
    for (status, _) in states() {
        if status == DatastoreStatus::Active {
            continue;
        }
        let mut seen: Vec<(&'static str, bool, String)> = Vec::new();
        for (operation, action, transition_code) in gated_actions() {
            let case = format!("{operation} on a {} data store", status.as_wire());
            let service = HealthLake::new();
            let id = datastore_in(&service, status, CREATED_NAME).await;
            let (_, payload) = service.call(operation, &request_for(operation, &id)).await;
            let code = error_code(&payload);
            let gone = code == "ResourceNotFoundException";
            assert!(
                gone || code == transition_code,
                "case {case:?}: a refusal must be this action's transition shape \
                 {transition_code:?} or the gone shape, got {code:?}"
            );
            // The sentence with this action's own participle taken out, so the
            // texts of different actions are comparable word for word.
            let neutralised = error_message(&payload).replace(action.as_participle(), "<action>");
            seen.push((operation, gone, neutralised));
        }
        let (first_operation, first_gone, first_message) = seen[0].clone();
        for (operation, gone, message) in &seen[1..] {
            let case = format!("{operation} vs {first_operation} on a {} data store", status.as_wire());
            assert_eq!(
                *gone, first_gone,
                "case {case:?}: both actions must classify the status alike, got gone={gone} \
                 and gone={first_gone}"
            );
            assert_eq!(
                *message, first_message,
                "case {case:?}: expected the shared sentence {first_message:?}, got {message:?}"
            );
        }
    }
}
