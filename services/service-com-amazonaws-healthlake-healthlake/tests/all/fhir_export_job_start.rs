// SPDX-License-Identifier: CC0-1.0

//! What `StartFHIRExportJob` mints, what `DescribeFHIRExportJob` reads back, and
//! what either refuses.
//!
//! The rules under test, each over a SET of inputs rather than one case:
//!
//! 1. a start MINTS one export job record on the data store it names, persisting
//!    every member the request carried, answers the minted `JobId` with the
//!    `SUBMITTED` status the record holds, and that ONE record is what
//!    `DescribeFHIRExportJob` and `ListFHIRExportJobs` both report;
//! 2. the clock then settles the started job to the TERMINAL status the work
//!    this emulator could actually do warrants: `COMPLETED` for a data store
//!    with nothing to transmit, and the modelled `FAILED` with the service's own
//!    `Message` saying why for one that holds FHIR resources — with `EndTime`
//!    stamped once, when the job ends;
//! 3. only a settled, usable (`ACTIVE`) data store can be exported from: an
//!    unknown or `DELETED` one is `ResourceNotFoundException`, one in a
//!    lifecycle transition is `ValidationException`, and a refused start mints
//!    nothing;
//! 4. one `ClientToken` starts one export job: a repeat of the same request
//!    answers out of the job it started — whatever status that job has reached
//!    by then — and the same token with a different request is refused;
//! 5. a job is addressed WITHIN its data store: `DescribeFHIRExportJob` refuses
//!    a `JobId` the named data store holds no job for, and refuses one held for
//!    ANOTHER data store.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the harness in `fhir_rest_and_validate_source`, so routing,
//! deserialization, the modelled constraints and the error envelope are all
//! exercised on BOTH wires: the awsJson1_0 control plane and the FHIR REST
//! endpoint the exported data store serves.
//!
//! Every modelled CLIENT error is asserted at HTTP 400 whatever status its
//! shape's `@httpError` names, for the reason `tests/all/error_envelopes.rs`
//! pins: awsJson1_0 binds nothing to the transport, so this service reports an
//! error's FAULT.

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    datastore_in_transition_message, export_not_transmitted_message, DatastoreAction,
    DatastoreStatus, DATASTORE_NOT_FOUND_MESSAGE, EXPORT_JOB_NOT_FOUND_MESSAGE,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, PAST_TRANSITION};
use crate::fhir_rest_and_validate_source::{created, HealthLake};

/// The `DataAccessRoleArn` every start below carries, satisfying the modelled
/// `IamRoleArn` pattern (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeExport";

/// A well-formed `DatastoreId` no data store is held for: 32 lowercase hex
/// characters, the form this service mints, so the refusal below is about the
/// data store not existing rather than about the identifier's shape.
const UNHELD_DATASTORE_ID: &str = "ffffffffffffffffffffffffffffffff";

/// Dispatch one control-plane call that must succeed, returning its body.
async fn ok(service: &HealthLake, operation: &str, body: &Value) -> Value {
    let (status, payload) = service.rpc(operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status}: {payload}"
    );
    payload
}

/// A required string member of a response body.
fn string_at(body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("the response must carry a string {member}, got {body}"))
        .to_string()
}

/// The `ExportJobProperties` `DescribeFHIRExportJob` reports for one job.
async fn describe_job(service: &HealthLake, datastore_id: &str, job_id: &str) -> Value {
    ok(
        service,
        "DescribeFHIRExportJob",
        &json!({ "DatastoreId": datastore_id, "JobId": job_id }),
    )
    .await["ExportJobProperties"]
        .clone()
}

/// Every export job `ListFHIRExportJobs` reports for a data store.
async fn listed_jobs(service: &HealthLake, datastore_id: &str) -> Vec<Value> {
    ok(
        service,
        "ListFHIRExportJobs",
        &json!({ "DatastoreId": datastore_id }),
    )
    .await["ExportJobPropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One `OutputDataConfig` the modelled union admits, built from the member
/// bounds `smithy_query(query="shape", shape="S3Configuration")` states:
/// `S3Uri` matches `^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$` (len
/// ..1024) and `KmsKeyId` matches the `EncryptionKeyID` pattern (a bare key id,
/// an `alias/...`, or a full KMS ARN; len 1..400).
fn output_data_config(s3_uri: &str, kms_key_id: &str) -> Value {
    json!({ "S3Configuration": { "S3Uri": s3_uri, "KmsKeyId": kms_key_id } })
}

/// A `StartFHIRExportJob` request body for `datastore_id`.
fn start_request(datastore_id: &str, job_name: Option<&str>, output: &Value) -> Value {
    let mut request = json!({
        "DatastoreId": datastore_id,
        "DataAccessRoleArn": DATA_ACCESS_ROLE_ARN,
        "OutputDataConfig": output,
    });
    if let Some(name) = job_name {
        request["JobName"] = json!(name);
    }
    request
}

/// `JobId` values are 32 lowercase hex characters, which satisfies the modelled
/// `JobId` length bound (1..32) and its alphabet.
fn assert_well_formed_job_id(case: &str, job_id: &str) {
    assert_eq!(
        job_id.len(),
        32,
        "case {case:?}: JobId must be 32 characters, got {job_id:?}"
    );
    assert!(
        job_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "case {case:?}: JobId must be lowercase hex, got {job_id:?}"
    );
}

/// The start requests the round-trip table spans: the rule is that every member
/// a start carries is persisted and reported back, so the rows differ along
/// whether the optional `JobName` is set and along the FORM of the values the
/// modelled shapes admit — a bucket-root `S3Uri` against a prefixed and a nested
/// one, and each of the three `KmsKeyId` forms the modelled pattern spells
/// (bare key id, `alias/...`, full ARN).
fn round_trip_rows() -> Vec<(&'static str, Option<String>, Value)> {
    vec![
        (
            "no JobName, bucket-root S3Uri, bare key id",
            None,
            output_data_config(
                "s3://healthlake-export-bucket",
                "1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
        ),
        (
            "ASCII JobName, prefixed S3Uri, key ARN",
            Some("nightly-export".to_string()),
            output_data_config(
                "s3://healthlake-export-bucket/nightly",
                "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
        ),
        (
            "unicode JobName with spaces, nested S3Uri, key alias",
            Some("выгрузка 42".to_string()),
            output_data_config(
                "s3://healthlake-export-bucket/tenant/42/out",
                "alias/healthlake/export",
            ),
        ),
        (
            "JobName at the modelled 64-character maximum",
            Some("n".repeat(64)),
            output_data_config("s3://healthlake-export-bucket/max", "export-key_1"),
        ),
    ]
}

#[tokio::test]
async fn a_start_mints_one_job_and_describe_reports_every_member_back() {
    let service = HealthLake::new();
    let (datastore_id, _path) = service.active_datastore("exporting store").await;
    let mut minted: Vec<String> = Vec::new();

    for (case, job_name, output) in round_trip_rows() {
        let request = start_request(&datastore_id, job_name.as_deref(), &output);
        let started = ok(&service, "StartFHIRExportJob", &request).await;

        let job_id = string_at(&started, "JobId");
        assert_well_formed_job_id(case, &job_id);
        assert_eq!(
            started["JobStatus"],
            json!("SUBMITTED"),
            "case {case:?}: a started job must be answered SUBMITTED, got {started}"
        );
        assert_eq!(
            started["DatastoreId"],
            json!(datastore_id),
            "case {case:?}: the start must report the data store it exports from, got {started}"
        );
        assert!(
            !minted.contains(&job_id),
            "case {case:?}: every start must mint its own JobId, {job_id:?} was minted before"
        );
        minted.push(job_id.clone());

        // The record is read back through the operation that reports ONE job.
        let described = describe_job(&service, &datastore_id, &job_id).await;
        for (member, expected) in [
            ("JobId", json!(job_id)),
            ("JobStatus", json!("SUBMITTED")),
            ("DatastoreId", json!(datastore_id)),
            ("DataAccessRoleArn", json!(DATA_ACCESS_ROLE_ARN)),
            ("OutputDataConfig", output.clone()),
        ] {
            assert_eq!(
                described[member], expected,
                "case {case:?}: the described job must report {member} as {expected}, got {described}"
            );
        }
        assert_eq!(
            described.get("JobName").cloned(),
            job_name.clone().map(Value::from),
            "case {case:?}: the described job must report the JobName the start carried, got {described}"
        );
        assert!(
            described["SubmitTime"].is_number(),
            "case {case:?}: the described job must report a SubmitTime, got {described}"
        );
        assert!(
            described.get("EndTime").is_none(),
            "case {case:?}: a job that has not ended must report no EndTime, got {described}"
        );
        assert!(
            described.get("Message").is_none(),
            "case {case:?}: a job that has not ended must report no Message, got {described}"
        );

        // ONE record: what the listing reports for this job is exactly what the
        // describe reported.
        let jobs = listed_jobs(&service, &datastore_id).await;
        let listed = jobs
            .iter()
            .find(|job| job["JobId"] == json!(job_id))
            .unwrap_or_else(|| panic!("case {case:?}: the listing must carry {job_id:?}, got {jobs:?}"));
        assert_eq!(
            listed, &described,
            "case {case:?}: the listing and the describe must report the SAME record"
        );
        assert_eq!(
            jobs.len(),
            minted.len(),
            "case {case:?}: each start must add exactly one job, got {jobs:?}"
        );
    }
}

#[tokio::test]
async fn the_clock_settles_a_started_job_to_the_status_its_work_warrants() {
    let service = HealthLake::new();

    // The rows differ along how many FHIR resources the exported data store
    // holds: none (nothing to transmit) against one and several (a transmission
    // this emulator cannot perform).
    for resources in [0usize, 1, 3] {
        let case = format!("{resources} resource(s) held");
        let (datastore_id, path) =
            service
                .active_datastore(&format!("store with {resources} resources"))
                .await;
        for index in 0..resources {
            created(
                &service,
                &path,
                "Patient",
                &json!({ "resourceType": "Patient", "active": index % 2 == 0 }),
            )
            .await;
        }

        let started = ok(
            &service,
            "StartFHIRExportJob",
            &start_request(
                &datastore_id,
                Some("settling"),
                &output_data_config("s3://healthlake-export-bucket/settling", "export-key_1"),
            ),
        )
        .await;
        let job_id = string_at(&started, "JobId");

        // Before the span is up the job still reports the status it was started
        // with, so a caller describing it straight away sees the transient one.
        let pending = describe_job(&service, &datastore_id, &job_id).await;
        assert_eq!(
            pending["JobStatus"],
            json!("SUBMITTED"),
            "case {case:?}: before the transition falls due the job must report SUBMITTED, got {pending}"
        );
        assert!(
            pending.get("EndTime").is_none() && pending.get("Message").is_none(),
            "case {case:?}: a running job must report neither EndTime nor Message, got {pending}"
        );

        tokio::time::sleep(PAST_TRANSITION).await;

        let settled = describe_job(&service, &datastore_id, &job_id).await;
        let (expected_status, expected_message) = if resources == 0 {
            ("COMPLETED", None)
        } else {
            ("FAILED", Some(export_not_transmitted_message(resources)))
        };
        assert_eq!(
            settled["JobStatus"],
            json!(expected_status),
            "case {case:?}: once due the job must report {expected_status:?}, got {settled}"
        );
        assert_eq!(
            settled.get("Message").and_then(Value::as_str),
            expected_message.as_deref(),
            "case {case:?}: the ended job must report {expected_message:?}, got {settled}"
        );
        assert!(
            settled["EndTime"].is_number(),
            "case {case:?}: a job that reached a terminal status must report an EndTime, got {settled}"
        );

        // `EndTime` is stored when the job ends, not re-derived per read, so a
        // second read reports the same instant — and the listing agrees.
        let again = describe_job(&service, &datastore_id, &job_id).await;
        assert_eq!(
            again, settled,
            "case {case:?}: two reads of one ended job must report the same record"
        );
        let jobs = listed_jobs(&service, &datastore_id).await;
        assert_eq!(
            jobs,
            vec![settled],
            "case {case:?}: the listing must report the settled record, got {jobs:?}"
        );
    }
}

/// Assert one `StartFHIRExportJob` refusal: the HTTP fault, the modelled shape
/// and the sentence it carries.
///
/// A helper rather than a table row built up front, because two of the rows
/// below are about a TRANSIENT data store status: the request has to go out
/// while the record still holds it, so each case is prepared and then refused
/// immediately.
async fn assert_start_refused(
    service: &HealthLake,
    case: &str,
    datastore_id: &str,
    expected_code: &str,
    expected_message: &str,
) {
    let (status, payload) = service
        .rpc(
            "StartFHIRExportJob",
            &start_request(
                datastore_id,
                None,
                &output_data_config("s3://healthlake-export-bucket/refused", "export-key_1"),
            ),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the start must be refused, got {status}: {payload}"
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
}

#[tokio::test]
async fn an_export_is_refused_for_a_data_store_that_cannot_serve_it() {
    let service = HealthLake::new();
    let (live, _path) = service.active_datastore("live store").await;

    // A DatastoreId no record is held for at all.
    assert_start_refused(
        &service,
        "a DatastoreId no data store is held for",
        UNHELD_DATASTORE_ID,
        "ResourceNotFoundException",
        DATASTORE_NOT_FOUND_MESSAGE,
    )
    .await;

    // A data store that has only just been created is still CREATING, so the
    // start goes out before its transition falls due.
    let creating = string_at(
        &ok(
            &service,
            "CreateFHIRDatastore",
            &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "creating store" }),
        )
        .await,
        "DatastoreId",
    );
    assert_start_refused(
        &service,
        "a data store still CREATING",
        &creating,
        "ValidationException",
        &datastore_in_transition_message(DatastoreStatus::Creating, DatastoreAction::ExportFrom),
    )
    .await;

    // A data store that has only just been deleted is still DELETING, and once
    // that transition falls due the record is terminally DELETED: in transition
    // first, then gone.
    let (deleting, _path) = service.active_datastore("deleting store").await;
    ok(
        &service,
        "DeleteFHIRDatastore",
        &json!({ "DatastoreId": deleting }),
    )
    .await;
    assert_start_refused(
        &service,
        "a data store still DELETING",
        &deleting,
        "ValidationException",
        &datastore_in_transition_message(DatastoreStatus::Deleting, DatastoreAction::ExportFrom),
    )
    .await;

    tokio::time::sleep(PAST_TRANSITION).await;
    assert_start_refused(
        &service,
        "a DELETED data store",
        &deleting,
        "ResourceNotFoundException",
        DATASTORE_NOT_FOUND_MESSAGE,
    )
    .await;

    // Every refusal above minted nothing: the one live data store still holds no
    // export job.
    let jobs = listed_jobs(&service, &live).await;
    assert!(
        jobs.is_empty(),
        "a refused start must mint no job, got {jobs:?}"
    );
}

#[tokio::test]
async fn one_client_token_starts_one_export_job() {
    let service = HealthLake::new();
    let (datastore_id, _path) = service.active_datastore("idempotent store").await;
    let output = output_data_config("s3://healthlake-export-bucket/idempotent", "export-key_1");
    let mut request = start_request(&datastore_id, Some("hourly"), &output);
    request["ClientToken"] = json!("token-one");

    let first = ok(&service, "StartFHIRExportJob", &request).await;
    let job_id = string_at(&first, "JobId");

    // The SAME request with the SAME token resolves to the job it started.
    let repeat = ok(&service, "StartFHIRExportJob", &request).await;
    assert_eq!(
        repeat, first,
        "a repeat of one request under one ClientToken must answer the job it started"
    );
    let jobs = listed_jobs(&service, &datastore_id).await;
    assert_eq!(
        jobs.len(),
        1,
        "one ClientToken must start exactly one job, got {jobs:?}"
    );

    // The same token with a DIFFERENT request is a different request, and is
    // refused rather than answered with the earlier job. The rows differ along
    // which member differs.
    for (case, member, value) in [
        ("a different JobName", "JobName", json!("nightly")),
        (
            "a different DataAccessRoleArn",
            "DataAccessRoleArn",
            json!("arn:aws:iam::111122223333:role/OtherExport"),
        ),
        (
            "a different OutputDataConfig",
            "OutputDataConfig",
            output_data_config("s3://healthlake-export-bucket/elsewhere", "export-key_1"),
        ),
    ] {
        let mut different = request.clone();
        different[member] = value;
        let (status, payload) = service.rpc("StartFHIRExportJob", &different).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a reused ClientToken must be refused, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: a reused ClientToken must answer ValidationException, got {payload}"
        );
    }

    // A DIFFERENT token starts a second job of the same request.
    let mut second_token = request.clone();
    second_token["ClientToken"] = json!("token-two");
    let second = ok(&service, "StartFHIRExportJob", &second_token).await;
    assert_ne!(
        string_at(&second, "JobId"),
        job_id,
        "a distinct ClientToken must start its own job, got {second}"
    );

    // Once the clock has settled the first job, its token still resolves to it —
    // answered out of the STORED record, so the status is the one the job has
    // reached rather than the one it started in.
    tokio::time::sleep(PAST_TRANSITION).await;
    let settled_repeat = ok(&service, "StartFHIRExportJob", &request).await;
    assert_eq!(
        settled_repeat["JobId"],
        json!(job_id),
        "the token must still resolve to the job it started, got {settled_repeat}"
    );
    assert_eq!(
        settled_repeat["JobStatus"],
        json!("COMPLETED"),
        "a repeat must report the status the stored job has reached, got {settled_repeat}"
    );
    let jobs = listed_jobs(&service, &datastore_id).await;
    assert_eq!(
        jobs.len(),
        2,
        "the repeats above must have started no further job, got {jobs:?}"
    );
}

#[tokio::test]
async fn a_job_is_described_only_within_the_data_store_that_holds_it() {
    let service = HealthLake::new();
    let (exporting, _path) = service.active_datastore("described store").await;
    let (other, _path) = service.active_datastore("other store").await;
    let output = output_data_config("s3://healthlake-export-bucket/described", "export-key_1");
    let started = ok(
        &service,
        "StartFHIRExportJob",
        &start_request(&exporting, Some("hourly"), &output),
    )
    .await;
    let job_id = string_at(&started, "JobId");

    // The rows differ along which half of the address is wrong: the data store,
    // the job, or neither (the job belongs to another data store).
    let rows: Vec<(&str, String, String, &str, String)> = vec![
        (
            "a DatastoreId no data store is held for",
            UNHELD_DATASTORE_ID.to_string(),
            job_id.clone(),
            "ResourceNotFoundException",
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a JobId the data store holds no job for",
            exporting.clone(),
            UNHELD_DATASTORE_ID.to_string(),
            "ResourceNotFoundException",
            EXPORT_JOB_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a job held for ANOTHER data store",
            other.clone(),
            job_id.clone(),
            "ResourceNotFoundException",
            EXPORT_JOB_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];

    for (case, datastore_id, asked_job_id, expected_code, expected_message) in rows {
        let (status, payload) = service
            .rpc(
                "DescribeFHIRExportJob",
                &json!({ "DatastoreId": datastore_id, "JobId": asked_job_id }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the describe must be refused, got {status}: {payload}"
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
    }

    // The job IS described under the data store that holds it.
    let described = describe_job(&service, &exporting, &job_id).await;
    assert_eq!(
        described["JobId"],
        json!(job_id),
        "the job must be described under its own data store, got {described}"
    );
}
