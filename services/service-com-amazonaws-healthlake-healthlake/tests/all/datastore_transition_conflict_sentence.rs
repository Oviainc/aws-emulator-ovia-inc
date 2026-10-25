// SPDX-License-Identifier: CC0-1.0

//! The ONE sentence this service refuses a data store in a lifecycle transition
//! with, asserted through every operation that states it.
//!
//! The rule: an operation refused because its target data store is in a
//! non-terminal lifecycle transition (`CREATING`, `UPDATING`, `DELETING`) states
//! the crate's single transition-conflict sentence — the status the record is
//! actually in, the past participle of the refused action, and nothing quoted
//! back from the request — whichever modelled shape that operation carries it
//! on. The sentence is read from the crate's own
//! [`datastore_in_transition_message`] rather than retyped here, so the test and
//! the service cannot drift, and every operation below goes through the SAME
//! function: `DeleteFHIRDatastore` and `StartFHIRExportJob`, which state it on
//! `ConflictException` and `ValidationException` respectively, and
//! `RestoreFHIRDatastore`, which used to rebuild the sentence itself.
//!
//! The rows span both axes the rule generalises over: the refused operation
//! (with the error shape each one models) and the transition the target sits in.
//! Every status is reached through the real lifecycle rather than by poking the
//! state, and every call goes through
//! [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the shared harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization and the error envelope are exercised too.
//!
//! Every modelled CLIENT error is asserted at HTTP 400 whatever status its
//! shape's `@httpError` names, for the reason `tests/all/error_envelopes.rs`
//! pins.

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    datastore_in_transition_message, DatastoreAction, DatastoreStatus,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, HealthLake, PAST_TRANSITION};

/// The name every data store below is created with, so a refusal that wrote
/// something is visible as a changed name.
const SOURCE_NAME: &str = "transition.sentence.store";

/// An IAM role ARN for the export rows, in the form the modelled
/// `IamRoleArn` pattern admits
/// (`smithy_query(query="shape", shape="IamRoleArn")`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeExport";

/// An `S3Configuration` the modelled union admits, as
/// `smithy_query(query="shape", shape="S3Configuration")` bounds it: an `S3Uri`
/// matching `^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$` and a `KmsKeyId`
/// matching `EncryptionKeyID` (a bare key id here).
const EXPORT_S3_URI: &str = "s3://transition-sentence-bucket/out/";
const EXPORT_KMS_KEY_ID: &str = "1234abcd-12ab-34cd-56ef-1234567890ab";

/// Every state a data store is refused in by this rule, i.e. the non-terminal
/// lifecycle transitions.
///
/// Source: `smithy_query(query="shape", shape="DatastoreStatus")` for the value
/// set, and the `ConflictException` the model documents as "The data store is in
/// a transition state and the user requested action cannot be performed." for
/// which of those values is a transition. The terminal `DELETED` is a different
/// classification (a not-found record) and the failure states (`CREATE_FAILED`,
/// `UPDATE_FAILED`) are no transition either; `tests/all/datastore_state_conflict.rs`
/// spans the whole status set for the delete path.
const TRANSITIONS: [DatastoreStatus; 3] = [
    DatastoreStatus::Creating,
    DatastoreStatus::Updating,
    DatastoreStatus::Deleting,
];

/// Every operation that states the shared sentence, with the action it names
/// itself by and the modelled error shape it carries the sentence on.
///
/// Source: `smithy_query(query="operation",
/// operation="DeleteFHIRDatastore,StartFHIRExportJob,RestoreFHIRDatastore")` —
/// the first and the last model a `ConflictException`, the middle one does not
/// and carries the refusal on the `ValidationException` it does model.
fn refusals() -> Vec<(&'static str, DatastoreAction, &'static str)> {
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

/// Mint a data store whose continuous backup is ENABLED — so a restore has a
/// window to work from and nothing but the status is left to refuse any row over
/// — and drive it, through the real lifecycle, to `status`.
///
/// `BackupConfiguration` member names and value sets:
/// `smithy_query(query="shape", shape="BackupConfiguration")`.
async fn backup_enabled_datastore(service: &HealthLake, status: DatastoreStatus) -> String {
    let (id, _) = service
        .create(&json!({
            "DatastoreTypeVersion": "R4",
            "DatastoreName": SOURCE_NAME,
            "BackupConfiguration": {
                "Status": "ENABLED",
                "BackupType": "CONTINUOUS",
                "RetentionPeriodInDays": 7,
                "BackupTagsEnabled": false,
            },
        }))
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
            panic!("this suite drives no row to the terminal DELETED status")
        }
    }

    // The row really put the record in the state it claims, as the reporting
    // operation sees it.
    let described = service.describe(&id).await;
    assert_eq!(
        described["DatastoreStatus"],
        status.as_wire(),
        "the row must put the data store in status {}, got {described}",
        status.as_wire()
    );
    id
}

#[tokio::test]
async fn every_operation_states_a_transition_conflict_with_the_one_shared_sentence() {
    for (operation, action, expected_code) in refusals() {
        for status in TRANSITIONS {
            let case = format!("{operation} on a {} data store", status.as_wire());
            // A service per row, so neither the lifecycle waits nor an accepted
            // call of one row can be observed by another.
            let service = HealthLake::new();
            let id = backup_enabled_datastore(&service, status).await;

            let (http_status, payload) = service.call(operation, &request_for(operation, &id)).await;
            assert_eq!(
                http_status, 400,
                "case {case:?}: a refused call must answer 400, got {http_status}: {payload}"
            );

            let code = error_code(&payload);
            assert_eq!(
                code, expected_code,
                "case {case:?}: expected __type {expected_code}, got {payload}"
            );

            // Read from the crate rather than retyped, so the one sentence the
            // service states and the one this test expects cannot drift.
            let expected_message = datastore_in_transition_message(status, action);
            let message = error_message(&payload);
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected {expected_message:?}, got {message:?}"
            );
            assert!(
                message.contains(status.as_wire()),
                "case {case:?}: the conflict sentence must state the status the record is in, \
                 got {message:?}"
            );
            assert!(
                message.contains(action.as_participle()),
                "case {case:?}: the conflict sentence must name the refused action, got {message:?}"
            );
            assert!(
                !message.contains(&id),
                "case {case:?}: a refusal must quote no requested identifier, got {message:?}"
            );

            // The refusal wrote nothing: the name is still the one the create
            // set, and the lifecycle the record was already on is undisturbed.
            tokio::time::sleep(PAST_TRANSITION).await;
            let settled = if status == DatastoreStatus::Deleting {
                "DELETED"
            } else {
                "ACTIVE"
            };
            let after = service.describe(&id).await;
            assert_eq!(
                after["DatastoreName"], SOURCE_NAME,
                "case {case:?}: a refused call must not rename the data store, got {after}"
            );
            assert_eq!(
                after["DatastoreStatus"], settled,
                "case {case:?}: a refused call must leave the lifecycle alone, expected \
                 {settled}, got {after}"
            );
        }

        // The positive control: an ACTIVE data store takes every one of these
        // calls, so the refusals above are about the STATE and not about the
        // request each row sent.
        let case = format!("{operation} on an ACTIVE data store");
        let service = HealthLake::new();
        let id = backup_enabled_datastore(&service, DatastoreStatus::Active).await;
        let (http_status, payload) = service.call(operation, &request_for(operation, &id)).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: an ACTIVE data store must accept the call, \
             got {http_status}: {payload}"
        );
    }
}

#[tokio::test]
async fn the_shared_sentence_differs_between_operations_only_in_the_action() {
    // One status, every action: the sentences agree word for word except for the
    // participle that names the refused operation — which is what holding the
    // sentence in one place buys.
    for status in TRANSITIONS {
        for (operation, action, _) in refusals() {
            let case = format!("{operation} on a {} data store", status.as_wire());
            let message = datastore_in_transition_message(status, action);
            let neutralised = message.replace(action.as_participle(), "<action>");
            let reference = datastore_in_transition_message(status, DatastoreAction::Delete)
                .replace(DatastoreAction::Delete.as_participle(), "<action>");
            assert_eq!(
                neutralised, reference,
                "case {case:?}: expected the shared sentence {reference:?}, got {neutralised:?}"
            );
        }
    }
}
