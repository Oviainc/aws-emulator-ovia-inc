// SPDX-License-Identifier: CC0-1.0

//! An idempotency token's replay is resolved BEFORE the gates on the resource
//! the call names.
//!
//! The rule under test: a repeat of an identical request under a `ClientToken`
//! already seen is answered out of the record that token minted, so a change
//! made to the resource the first call named AFTER that call cannot turn the
//! retry into a refusal — while a FIRST call under an unseen token still meets
//! every gate the resource's current state imposes.
//!
//! The table spans what happened to the named resource between the first call
//! and the repeat — nothing, a change in flight, a settled change, a deletion in
//! flight, a settled deletion — and both operations in this crate that mint a
//! job against another resource, so the rule is asserted over the resource
//! states it generalises across rather than over one recorded case.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization, the modelled constraints and the error envelope are all
//! exercised, not bypassed.
//!
//! Every modelled CLIENT error is asserted at HTTP 400 whatever status its
//! shape's `@httpError` names, for the reason `tests/all/error_envelopes.rs`
//! pins: awsJson1_0 binds nothing to the transport, so this service reports an
//! error's FAULT.

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    datastore_in_transition_message, profile_not_found_message, DatastoreAction, DatastoreStatus,
    DATASTORE_NOT_FOUND_MESSAGE,
};

use crate::fhir_datastore_lifecycle::{
    error_code, error_message, string_at, HealthLake, PAST_TRANSITION,
};
use crate::fhir_export_job_start::{output_data_config, start_request};
use crate::start_data_transformation_job::profile;

/// The `DataAccessRoleArn` the data transformation start below carries,
/// satisfying the modelled `DataTransformationIamRoleArn` pattern
/// (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const TRANSFORM_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeTransform";

/// What happens to the data store between the first `StartFHIRExportJob` and the
/// identical repeat under the same `ClientToken`.
#[derive(Clone, Copy, Debug)]
enum Intervening {
    /// Nothing at all: the baseline, where the data store is still usable.
    Nothing,
    /// A configuration change is being applied when the repeat goes out.
    UpdateInFlight,
    /// That change has settled, so the data store is usable again.
    UpdateSettled,
    /// A deletion is being applied when the repeat goes out.
    DeleteInFlight,
    /// The deletion has settled: the data store is terminally gone, while its
    /// retention still holds the job records it owned.
    DeleteSettled,
}

/// One row: what intervenes, and the status that leaves the data store holding
/// when the repeat goes out.
struct Row {
    case: &'static str,
    client_token: &'static str,
    intervening: Intervening,
    status_at_repeat: DatastoreStatus,
}

/// The rows the table spans: every status a data store that HAS served a start
/// can hold by the time the retry arrives. `CREATING` is absent because a data
/// store cannot return to it — a start naming one is refused before any job
/// exists to replay, which `tests/all/fhir_export_job_start.rs` pins.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "nothing intervenes",
            client_token: "token-untouched",
            intervening: Intervening::Nothing,
            status_at_repeat: DatastoreStatus::Active,
        },
        Row {
            case: "an update is still being applied",
            client_token: "token-updating",
            intervening: Intervening::UpdateInFlight,
            status_at_repeat: DatastoreStatus::Updating,
        },
        Row {
            case: "an update has settled",
            client_token: "token-updated",
            intervening: Intervening::UpdateSettled,
            status_at_repeat: DatastoreStatus::Active,
        },
        Row {
            case: "a delete is still being applied",
            client_token: "token-deleting",
            intervening: Intervening::DeleteInFlight,
            status_at_repeat: DatastoreStatus::Deleting,
        },
        Row {
            case: "a delete has settled",
            client_token: "token-deleted",
            intervening: Intervening::DeleteSettled,
            status_at_repeat: DatastoreStatus::Deleted,
        },
    ]
}

/// What a FIRST start — one under a token no held job carries — must answer for
/// a data store in `status`, as the operation's own gate states it: only a
/// settled, usable data store can be exported from, a transition is the modelled
/// `ValidationException` in this resource's one transition sentence, and a
/// deleted or unheld one is `ResourceNotFoundException`. `None` means the start
/// is served.
fn expected_first_start(status: DatastoreStatus) -> Option<(&'static str, String)> {
    match status {
        DatastoreStatus::Active => None,
        DatastoreStatus::Creating | DatastoreStatus::Updating | DatastoreStatus::Deleting => Some((
            "ValidationException",
            datastore_in_transition_message(status, DatastoreAction::ExportFrom),
        )),
        DatastoreStatus::Deleted => Some((
            "ResourceNotFoundException",
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        )),
    }
}

/// Create a data store and wait out its lifecycle transition, so a start can be
/// served against it.
async fn active_datastore(service: &HealthLake, name: &str) -> String {
    let (id, _arn) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": name }))
        .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    id
}

/// Apply one row's intervening change to the data store.
async fn intervene(service: &HealthLake, datastore_id: &str, intervening: Intervening) {
    match intervening {
        Intervening::Nothing => {}
        Intervening::UpdateInFlight | Intervening::UpdateSettled => {
            service
                .ok(
                    "UpdateFHIRDatastore",
                    &json!({ "DatastoreId": datastore_id, "DatastoreName": "renamed.midway" }),
                )
                .await;
        }
        Intervening::DeleteInFlight | Intervening::DeleteSettled => {
            service
                .ok(
                    "DeleteFHIRDatastore",
                    &json!({ "DatastoreId": datastore_id }),
                )
                .await;
        }
    }
    if matches!(
        intervening,
        Intervening::UpdateSettled | Intervening::DeleteSettled
    ) {
        tokio::time::sleep(PAST_TRANSITION).await;
    }
}

/// Every export job `ListFHIRExportJobs` reports for a data store.
async fn listed_jobs(service: &HealthLake, datastore_id: &str) -> Vec<Value> {
    service
        .ok(
            "ListFHIRExportJobs",
            &json!({ "DatastoreId": datastore_id }),
        )
        .await["ExportJobPropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `ExportJobProperties` `DescribeFHIRExportJob` reports for one job.
async fn describe_export_job(service: &HealthLake, datastore_id: &str, job_id: &str) -> Value {
    service
        .ok(
            "DescribeFHIRExportJob",
            &json!({ "DatastoreId": datastore_id, "JobId": job_id }),
        )
        .await["ExportJobProperties"]
        .clone()
}

#[tokio::test]
async fn an_export_start_replays_its_token_whatever_became_of_the_data_store() {
    let service = HealthLake::new();

    for row in rows() {
        let case = row.case;
        let datastore_id = active_datastore(&service, &format!("store: {case}")).await;
        let output = output_data_config("s3://healthlake-export-bucket/replayed", "export-key_1");
        let mut request = start_request(&datastore_id, Some("hourly"), &output);
        request["ClientToken"] = json!(row.client_token);

        let first = service.ok("StartFHIRExportJob", &request).await;
        let job_id = string_at(&first, "JobId");

        intervene(&service, &datastore_id, row.intervening).await;

        // The row really did leave the data store in the status it claims, so
        // what follows is asserted against a gate that would otherwise refuse.
        let reported = service.describe(&datastore_id).await;
        assert_eq!(
            string_at(&reported, "DatastoreStatus"),
            row.status_at_repeat.as_wire(),
            "case {case:?}: the data store must hold {} when the repeat goes out, got {reported}",
            row.status_at_repeat.as_wire()
        );

        // The IDENTICAL request under the SAME token is answered out of the job
        // that token minted, and out of the STORED record: the status is the
        // one the job has reached, which the reporting operation agrees with.
        let (status, repeat) = service.call("StartFHIRExportJob", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeat under one ClientToken must be served, got {status}: {repeat}"
        );
        assert_eq!(
            repeat["JobId"],
            json!(job_id),
            "case {case:?}: the repeat must answer the job the token started ({job_id:?}), \
             got {repeat}"
        );
        assert_eq!(
            repeat["DatastoreId"],
            json!(datastore_id),
            "case {case:?}: the repeat must report the stored job's data store, got {repeat}"
        );
        let described = describe_export_job(&service, &datastore_id, &job_id).await;
        assert_eq!(
            repeat["JobStatus"], described["JobStatus"],
            "case {case:?}: the repeat must report the stored job's status, got {repeat} \
             against {described}"
        );

        // And it started nothing: the one job the token minted is all there is.
        let jobs = listed_jobs(&service, &datastore_id).await;
        assert_eq!(
            jobs.len(),
            1,
            "case {case:?}: a replayed token must start no further job, got {jobs:?}"
        );

        // A FIRST start under an unseen token has no record to be answered out
        // of, so it meets the gate the data store's current status imposes. The
        // token is per row, because one service holds every row's jobs.
        let mut fresh = request.clone();
        fresh["ClientToken"] = json!(format!("{}-fresh", row.client_token));
        let (status, payload) = service.call("StartFHIRExportJob", &fresh).await;
        match expected_first_start(row.status_at_repeat) {
            Some((expected_code, expected_message)) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a first start must still be refused, got {status}: {payload}"
                );
                assert_eq!(
                    error_code(&payload),
                    expected_code,
                    "case {case:?}: the refusal must be {expected_code}, got {payload}"
                );
                assert_eq!(
                    error_message(&payload),
                    expected_message,
                    "case {case:?}: the refusal must state {expected_message:?}, got {payload}"
                );
                let jobs = listed_jobs(&service, &datastore_id).await;
                assert_eq!(
                    jobs.len(),
                    1,
                    "case {case:?}: a refused start must mint no job, got {jobs:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a usable data store must still serve a first start, \
                     got {status}: {payload}"
                );
                assert_ne!(
                    payload["JobId"],
                    json!(job_id),
                    "case {case:?}: an unseen token must start its own job, got {payload}"
                );
                let jobs = listed_jobs(&service, &datastore_id).await;
                assert_eq!(
                    jobs.len(),
                    2,
                    "case {case:?}: the served first start must add one job, got {jobs:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_transformation_start_replays_its_token_once_its_profile_is_gone() {
    let service = HealthLake::new();
    let (profile_id, _name) = profile(&service, "replayed-profile").await;
    let request = json!({
        "ClientToken": "token-transform",
        "ProfileId": profile_id,
        "DataAccessRoleArn": TRANSFORM_ROLE_ARN,
        "InputDataConfig": { "S3Uri": "s3://transform-in-bucket/incoming" },
        "OutputDataConfig": { "S3Configuration": {
            "S3Uri": "s3://transform-out-bucket/r4",
            "KmsKeyId": "transform-key_1",
        } },
    });

    let first = service.ok("StartDataTransformationJob", &request).await;
    let job_id = string_at(&first, "JobId");

    service
        .ok(
            "DeleteDataTransformationProfile",
            &json!({ "ProfileId": profile_id }),
        )
        .await;

    // The same rule as the export start above: the retry is answered out of the
    // record the token minted, even though the resource the request names is
    // gone.
    let (status, repeat) = service.call("StartDataTransformationJob", &request).await;
    assert_eq!(
        status, 200,
        "the repeat under one ClientToken must be served, got {status}: {repeat}"
    );
    assert_eq!(
        repeat["JobId"],
        json!(job_id),
        "the repeat must answer the job the token started ({job_id:?}), got {repeat}"
    );
    let described = service
        .ok(
            "DescribeDataTransformationJob",
            &json!({ "JobId": job_id }),
        )
        .await["TransformationJobProperties"]
        .clone();
    assert_eq!(
        repeat["JobStatus"], described["JobStatus"],
        "the repeat must report the stored job's status, got {repeat} against {described}"
    );

    // A FIRST start under an unseen token still meets the gate on the profile.
    let mut fresh = request.clone();
    fresh["ClientToken"] = json!("token-transform-fresh");
    let (status, payload) = service.call("StartDataTransformationJob", &fresh).await;
    assert_eq!(
        status, 400,
        "a first start naming a deleted profile must be refused, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ResourceNotFoundException",
        "the refusal must be ResourceNotFoundException, got {payload}"
    );
    let expected = profile_not_found_message(&profile_id);
    assert_eq!(
        error_message(&payload),
        expected,
        "the refusal must state {expected:?}, got {payload}"
    );
}
