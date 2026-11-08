// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `DescribeFHIRExportJob`: what the service reports
//! for one FHIR export job of one data store.
//!
//! The rule under test: `DescribeFHIRExportJob` reports the export job record
//! the data store its required `DatastoreId` names holds under the required
//! `JobId` — every member that record holds and nothing else, under the
//! modelled `ExportJobProperties` — after letting the record catch up with the
//! clock, so a status transition already due is reported and one not yet due is
//! not; the record it reports is the SAME one `ListFHIRExportJobs` pages, and a
//! `DatastoreId` the service holds no data store for, or a `JobId` that data
//! store holds no export job for, are both refused with the modelled
//! `ResourceNotFoundException` in this service's fixed sentence for the
//! resource that was not found, never with an empty or synthesised job.
//!
//! What a job STARTED through `StartFHIRExportJob` reports is pinned by
//! `tests/all/fhir_export_job_start.rs`; a start leaves one status to the clock,
//! so the rows below are seeded into the SAME store this handler reads, through
//! the shared engine the harness exposes (exactly how
//! `tests/all/fhir_export_job_listing.rs` seeds export jobs and
//! `tests/all/fhir_import_job_describe.rs` seeds import jobs), to put a record
//! in each status and member combination the model admits without waiting for
//! one.
//! Every `DescribeFHIRExportJob` call itself goes over the real awsJson1_0 wire
//! through `MultiServiceRouter::dispatch` — the entry point `core/router`
//! serves — so routing, deserialization, the modelled constraints and the error
//! envelope are all exercised, not bypassed.
//!
//! Every modelled CLIENT error below is asserted at HTTP 400, whatever exact
//! code its shape's `@httpError` trait names: awsJson1_0 binds nothing to the
//! transport, so this service reports an error's FAULT. The rule itself is
//! pinned in `tests/all/error_envelopes.rs`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::ops::types::{
    InputDataConfig, OutputDataConfig, S3Configuration, JOB_STATUS_VALUES,
};
use service_com_amazonaws_healthlake_healthlake::state::{
    ComAmazonawsHealthlakeHealthlakeState, ExportJob, ImportJob, JobStatus,
    DATASTORE_NOT_FOUND_MESSAGE, EXPORT_JOB_NOT_FOUND_MESSAGE,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, HealthLake, PAST_TRANSITION};

/// The epoch second the seeded submit instants are measured from. A fixed
/// instant, so a row's reported `SubmitTime` is exactly the one it was seeded
/// with rather than something near it.
const BASE_SECS: i64 = 1_700_000_000;

/// The span a seeded clock-owed job transition falls due after it is armed.
///
/// Chosen to match `crate::state::EXPORT_JOB_RUN`, the span this service's own
/// export job transition uses, so one wait ([`PAST_TRANSITION`]) outlasts
/// either.
const JOB_TRANSITION: Duration = Duration::from_millis(500);

/// The `DataAccessRoleArn` a seeded job carries, satisfying the modelled
/// `IamRoleArn` pattern (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeExport";

/// Every status the modelled `JobStatus` enum declares, in the order the
/// generated `JOB_STATUS_VALUES` catalog lists them. Pinned against that
/// catalog by [`every_modelled_job_status_is_reported`], so a status the model
/// gains cannot stay untested here.
const EVERY_STATUS: [JobStatus; 10] = [
    JobStatus::CancelCompleted,
    JobStatus::CancelFailed,
    JobStatus::CancelInProgress,
    JobStatus::CancelSubmitted,
    JobStatus::Completed,
    JobStatus::CompletedWithErrors,
    JobStatus::Failed,
    JobStatus::InProgress,
    JobStatus::Queued,
    JobStatus::Submitted,
];

/// One export job to seed: every member the modelled `ExportJobProperties` can
/// carry, so a row can differ from the others along any one of them.
#[derive(Clone)]
struct Seed {
    id: &'static str,
    status: JobStatus,
    /// The submit instant, as seconds after [`BASE_SECS`].
    submitted_after_base: i64,
    /// `OutputDataConfig$S3Configuration$S3Uri` and `$KmsKeyId`, both required
    /// members of the modelled `S3Configuration` — and the whole union is a
    /// required member of `ExportJobProperties`, so every row carries one.
    output: (&'static str, &'static str),
    name: Option<&'static str>,
    /// `DataAccessRoleArn`, absent for a record holding none.
    role: bool,
    message: Option<&'static str>,
    /// `EndTime`, as seconds after [`BASE_SECS`], for a job that has ended.
    ended_after_base: Option<i64>,
    /// The transition the clock owes the seeded job, if any, and the message
    /// that status comes with.
    pending: Option<(JobStatus, Option<&'static str>)>,
}

impl Seed {
    /// A job carrying only what the modelled shape REQUIRES: an identifier, a
    /// status, a submit instant, the data store it exports from and the output
    /// configuration.
    fn new(id: &'static str, status: JobStatus) -> Self {
        Self {
            id,
            status,
            submitted_after_base: 0,
            output: (
                "s3://healthlake-export-bucket/outgoing",
                "1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
            name: None,
            role: false,
            message: None,
            ended_after_base: None,
            pending: None,
        }
    }

    fn submitted(mut self, submitted_after_base: i64) -> Self {
        self.submitted_after_base = submitted_after_base;
        self
    }

    fn named(mut self, name: &'static str) -> Self {
        self.name = Some(name);
        self
    }

    fn output(mut self, s3_uri: &'static str, kms_key_id: &'static str) -> Self {
        self.output = (s3_uri, kms_key_id);
        self
    }

    fn with_role(mut self) -> Self {
        self.role = true;
        self
    }

    fn message(mut self, message: &'static str) -> Self {
        self.message = Some(message);
        self
    }

    fn ended(mut self, ended_after_base: i64) -> Self {
        self.ended_after_base = Some(ended_after_base);
        self
    }

    fn pending(mut self, next: JobStatus, message: Option<&'static str>) -> Self {
        self.pending = Some((next, message));
        self
    }

    /// This row as the stored record the handler reads.
    fn record(&self, datastore_id: &str, armed_at: Instant) -> ExportJob {
        ExportJob {
            id: self.id.to_string(),
            datastore_id: datastore_id.to_string(),
            name: self.name.map(str::to_string),
            status: self.status,
            pending: self
                .pending
                .map(|(next, message)| (armed_at + JOB_TRANSITION, next, message.map(str::to_string))),
            submitted_at: ::aws_smithy_types::DateTime::from_secs(
                BASE_SECS + self.submitted_after_base,
            ),
            ended_at: self
                .ended_after_base
                .map(|offset| ::aws_smithy_types::DateTime::from_secs(BASE_SECS + offset)),
            output_data_config: OutputDataConfig::S3Configuration(S3Configuration {
                s3_uri: self.output.0.to_string(),
                kms_key_id: self.output.1.to_string(),
            }),
            data_access_role_arn: self.role.then(|| DATA_ACCESS_ROLE_ARN.to_string()),
            message: self.message.map(str::to_string),
            // Seeded rather than started, so no `ClientToken` named this job
            // and no request fingerprinted to it.
            client_token: None,
            request_fingerprint: String::new(),
        }
    }

    /// The `ExportJobProperties` this row must be reported as, WITHOUT its two
    /// timestamp members: a timestamp is carried as epoch seconds on this
    /// protocol, so `SubmitTime` and `EndTime` are read as numbers rather than
    /// compared to a JSON literal.
    fn expected_properties(&self, datastore_id: &str) -> Value {
        let mut expected = json!({
            "JobId": self.id,
            "JobStatus": self.status.as_wire(),
            "DatastoreId": datastore_id,
            "OutputDataConfig": {
                "S3Configuration": { "S3Uri": self.output.0, "KmsKeyId": self.output.1 }
            },
        });
        if let Some(name) = self.name {
            expected["JobName"] = json!(name);
        }
        if self.role {
            expected["DataAccessRoleArn"] = json!(DATA_ACCESS_ROLE_ARN);
        }
        if let Some(message) = self.message {
            expected["Message"] = json!(message);
        }
        expected
    }
}

/// Create a data store and return its `DatastoreId`.
async fn datastore(service: &HealthLake, name: &str) -> String {
    let (id, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": name }))
        .await;
    id
}

/// Seed `seeds` as export jobs of `datastore_id` in the engine the service
/// reads.
fn seed_jobs(service: &HealthLake, datastore_id: &str, seeds: &[Seed]) {
    let armed_at = Instant::now();
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let mut store = store.lock();
    for seed in seeds {
        store
            .export_jobs
            .insert(seed.id.to_string(), seed.record(datastore_id, armed_at));
    }
}

/// The `ExportJobProperties` `DescribeFHIRExportJob` reports for `job_id` on
/// `datastore_id`.
async fn describe(service: &HealthLake, datastore_id: &str, job_id: &str) -> Value {
    let payload = service
        .ok(
            "DescribeFHIRExportJob",
            &json!({ "DatastoreId": datastore_id, "JobId": job_id }),
        )
        .await;
    payload["ExportJobProperties"].clone()
}

/// Every export job `ListFHIRExportJobs` reports for `datastore_id`.
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

/// `properties` without its timestamp members, beside the epoch seconds each
/// carried (`None` when the member was not reported at all).
fn split_timestamps(properties: &Value) -> (Value, Option<f64>, Option<f64>) {
    let mut rest = properties.clone();
    let object = rest
        .as_object_mut()
        .unwrap_or_else(|| panic!("ExportJobProperties must be an object, got {properties}"));
    let submit_time = object.remove("SubmitTime").and_then(|at| at.as_f64());
    let end_time = object.remove("EndTime").and_then(|at| at.as_f64());
    (rest, submit_time, end_time)
}

/// `BASE_SECS + offset` as the epoch seconds a reported timestamp carries.
#[allow(clippy::cast_precision_loss)]
fn epoch_secs(offset: i64) -> f64 {
    (BASE_SECS + offset) as f64
}

/// The rows the round-trip table spans: the rule is that every member a job
/// record holds is reported under the modelled shape, so the rows differ along
/// which optional members are present, the form of each value (an ASCII and a
/// unicode `JobName`, a name at the modelled 64-character maximum, a bucket
/// root, a prefixed and a nested `S3Uri`, a KMS key as a bare id, as an alias
/// and as an ARN) and the status.
fn round_trip_rows() -> Vec<(&'static str, Seed)> {
    vec![
        (
            "only the required members",
            Seed::new("job-minimal", JobStatus::Submitted),
        ),
        (
            "named, with an access role and a bucket-root S3Uri",
            Seed::new("job-named", JobStatus::Queued)
                .submitted(10)
                .named("nightly-export")
                .output(
                    "s3://healthlake-export-bucket",
                    "1234abcd-12ab-34cd-56ef-1234567890ab",
                )
                .with_role(),
        ),
        (
            "running, a nested S3Uri and a KMS key alias",
            Seed::new("job-running", JobStatus::InProgress)
                .submitted(20)
                .named("hourly export")
                .output(
                    "s3://healthlake-export-bucket/tenant/42/out",
                    "alias/healthlake/export",
                )
                .with_role(),
        ),
        (
            "unicode JobName and a KMS key ARN",
            Seed::new("job-unicode", JobStatus::InProgress)
                .submitted(30)
                .named("выгрузка 42")
                .output(
                    "s3://healthlake-export-bucket.audit/r4",
                    "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                ),
        ),
        (
            "completed, JobName at the modelled 64-character maximum",
            Seed::new("job-completed", JobStatus::Completed)
                .submitted(40)
                .named("n.n:n/n=n+n-n%n@n_n n.n:n/n=n+n-n%n@n_n n.n:n/n=n+n-n%n@n_n nnnn")
                .with_role()
                .ended(45),
        ),
        (
            "completed with errors, a message beside the status",
            Seed::new("job-partial", JobStatus::CompletedWithErrors)
                .submitted(50)
                .message("3 of 10 resources could not be written")
                .ended(55),
        ),
        (
            "failed, a message and nothing exported",
            Seed::new("job-failed", JobStatus::Failed)
                .submitted(60)
                .message("the data access role could not write the output location")
                .ended(61),
        ),
        (
            "a cancel that completed",
            Seed::new("job-cancelled", JobStatus::CancelCompleted)
                .submitted(70)
                .named("cancelled run")
                .ended(72),
        ),
    ]
}

#[tokio::test]
async fn every_member_of_a_held_job_is_reported_under_the_modelled_shape() {
    let service = HealthLake::new();
    let exporting = datastore(&service, "exporting store").await;
    let rows = round_trip_rows();
    seed_jobs(
        &service,
        &exporting,
        &rows.iter().map(|(_, seed)| seed.clone()).collect::<Vec<_>>(),
    );

    for (case, seed) in &rows {
        let properties = describe(&service, &exporting, seed.id).await;
        let (reported, submit_time, end_time) = split_timestamps(&properties);
        assert_eq!(
            reported,
            seed.expected_properties(&exporting),
            "case {case:?}: must report exactly the members the record holds, got {properties}"
        );
        assert_eq!(
            submit_time,
            Some(epoch_secs(seed.submitted_after_base)),
            "case {case:?}: must report the stored SubmitTime, got {properties}"
        );
        assert_eq!(
            end_time,
            seed.ended_after_base.map(epoch_secs),
            "case {case:?}: must report the stored EndTime, got {properties}"
        );

        // A describe is a REPORT: a second one reports the same record.
        let again = describe(&service, &exporting, seed.id).await;
        assert_eq!(
            again, properties,
            "case {case:?}: two describes of one job must report the same properties, got {again}"
        );
    }

    // ONE record, two readers: what the listing reports for each job is exactly
    // what the describe reported, and the describes above added nothing.
    let listed = listed_jobs(&service, &exporting).await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "the listing must report exactly the seeded jobs, got {listed:?}"
    );
    for (case, seed) in &rows {
        let from_listing = listed
            .iter()
            .find(|job| job["JobId"] == json!(seed.id))
            .unwrap_or_else(|| panic!("case {case:?}: the listing must carry {:?}", seed.id));
        let described = describe(&service, &exporting, seed.id).await;
        assert_eq!(
            from_listing, &described,
            "case {case:?}: the listing and the describe must report the SAME record, got {from_listing} and {described}"
        );
    }
}

#[tokio::test]
async fn every_modelled_job_status_is_reported() {
    // The catalog is the generated `JOB_STATUS_VALUES`, which is the modelled
    // `JobStatus` enum's own value set: the stored statuses must spell exactly
    // it, so no record can report a status the model would refuse and none it
    // admits is untested.
    let wire: Vec<&str> = EVERY_STATUS.iter().map(|status| status.as_wire()).collect();
    assert_eq!(
        wire, JOB_STATUS_VALUES,
        "the statuses under test must be exactly the modelled JobStatus value set, got {wire:?}"
    );

    let service = HealthLake::new();
    let exporting = datastore(&service, "every status store").await;
    // One job per status, each under an identifier naming the status it holds
    // so a failure says which row it was.
    let ids: Vec<String> = EVERY_STATUS
        .iter()
        .map(|status| format!("job-{}", status.as_wire().to_lowercase()))
        .collect();
    {
        let armed_at = Instant::now();
        let store = service
            .engine
            .state::<ComAmazonawsHealthlakeHealthlakeState>();
        let mut store = store.lock();
        for (index, status) in EVERY_STATUS.iter().enumerate() {
            let mut record = Seed::new("", *status).record(&exporting, armed_at);
            record.id = ids[index].clone();
            // A job that has reached a terminal status carries the instant it
            // did; one still moving carries none.
            record.ended_at = status
                .is_terminal()
                .then(|| ::aws_smithy_types::DateTime::from_secs(BASE_SECS + 5));
            store.export_jobs.insert(ids[index].clone(), record);
        }
    }

    for (index, status) in EVERY_STATUS.iter().enumerate() {
        let case = &ids[index];
        let properties = describe(&service, &exporting, case).await;
        assert_eq!(
            properties["JobStatus"],
            json!(status.as_wire()),
            "case {case:?}: must report the stored status, got {properties}"
        );
        assert_eq!(
            properties.get("EndTime").is_some(),
            status.is_terminal(),
            "case {case:?}: EndTime must be reported for a terminal status only, got {properties}"
        );
    }
}

#[tokio::test]
async fn a_job_the_named_data_store_does_not_hold_is_refused_with_the_fixed_sentence() {
    // The sentences themselves, spelled out here rather than only compared to
    // the constants the handler and the rows below share, so a refusal whose
    // WORDING changed cannot still pass: both state the resource that was not
    // found and quote nothing from the request.
    assert_eq!(
        DATASTORE_NOT_FOUND_MESSAGE, "Could not find specified datastore.",
        "the data store not-found sentence must be the one this service states"
    );
    assert_eq!(
        EXPORT_JOB_NOT_FOUND_MESSAGE, "Could not find specified export job.",
        "the export job not-found sentence must be the one this service states"
    );

    let service = HealthLake::new();
    let exporting = datastore(&service, "exporting store").await;
    let other = datastore(&service, "other store").await;
    seed_jobs(
        &service,
        &exporting,
        &[Seed::new("job-held", JobStatus::Queued).with_role()],
    );
    // The same kind of record, held as an export job of ANOTHER data store.
    seed_jobs(
        &service,
        &other,
        &[Seed::new("job-elsewhere", JobStatus::Completed)],
    );
    // A FHIR import job's record lives in the same state under its own id; it
    // is not an export job, so its id must not resolve as one.
    {
        let store = service
            .engine
            .state::<ComAmazonawsHealthlakeHealthlakeState>();
        let mut store = store.lock();
        store.import_jobs.insert(
            "job-import".to_string(),
            ImportJob {
                id: "job-import".to_string(),
                datastore_id: exporting.clone(),
                name: None,
                status: JobStatus::Completed,
                pending: None,
                submitted_at: ::aws_smithy_types::DateTime::from_secs(BASE_SECS),
                ended_at: None,
                input_data_config: InputDataConfig::S3Uri(
                    "s3://healthlake-import-input/incoming".to_string(),
                ),
                output_data_config: None,
                data_access_role_arn: Some(DATA_ACCESS_ROLE_ARN.to_string()),
                message: None,
                progress_report: None,
                validation_level: None,
                // A record this suite seeds was not started through the API, so
                // it carries no `ClientToken` and there is no request to
                // fingerprint.
                client_token: None,
                request_fingerprint: String::new(),
            },
        );
    }

    // The rows span the forms a `JobId` the modelled constraints admit can take
    // on a data store the service DOES hold: one never minted, one of another
    // shape entirely, the identifier of a different resource kind held in the
    // same state, an export job of another data store, and a case variant of a
    // held id (the identifier is case-sensitive).
    for (case, job_id) in [
        ("a well-formed id nothing was minted under", "job-nothing"),
        ("an id of another shape entirely", "0123456789"),
        ("the id of a FHIR import job", "job-import"),
        ("an export job of another data store", "job-elsewhere"),
        ("a held id in another case", "JOB-HELD"),
    ] {
        let (status, payload) = service
            .call(
                "DescribeFHIRExportJob",
                &json!({ "DatastoreId": &exporting, "JobId": job_id }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unknown job must be refused, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: must answer ResourceNotFoundException, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            EXPORT_JOB_NOT_FOUND_MESSAGE,
            "case {case:?}: must state this service's fixed not-found sentence, got {payload}"
        );
        assert!(
            !payload.to_string().contains(job_id),
            "case {case:?}: a refusal must quote nothing from the request, got {payload}"
        );
    }

    // A `DatastoreId` the service holds no data store for is the data store
    // that was not found, whatever job it names: the job is addressed WITHIN a
    // data store.
    for (case, datastore_id) in [
        (
            "a well-formed id no data store was minted under",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        ("an id of another shape entirely", "store-9".to_string()),
    ] {
        let (status, payload) = service
            .call(
                "DescribeFHIRExportJob",
                &json!({ "DatastoreId": &datastore_id, "JobId": "job-held" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unknown data store must be refused, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: must answer ResourceNotFoundException, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            DATASTORE_NOT_FOUND_MESSAGE,
            "case {case:?}: must state this service's fixed not-found sentence, got {payload}"
        );
        assert!(
            !payload.to_string().contains(&datastore_id),
            "case {case:?}: a refusal must quote nothing from the request, got {payload}"
        );
    }

    // The refusals above left the held record readable, and minted nothing.
    let properties = describe(&service, &exporting, "job-held").await;
    assert_eq!(
        properties["JobId"],
        json!("job-held"),
        "the refusals must leave the held record untouched, got {properties}"
    );
    let listed = listed_jobs(&service, &exporting).await;
    assert_eq!(
        listed.len(),
        1,
        "a refused describe must mint no job, got {listed:?}"
    );
}

#[tokio::test]
async fn an_identifier_outside_the_modelled_constraints_is_refused() {
    let service = HealthLake::new();
    let exporting = datastore(&service, "exporting store").await;
    seed_jobs(
        &service,
        &exporting,
        &[Seed::new("job-held", JobStatus::Queued)],
    );

    // `JobId` and `DatastoreId` are both len 1..32 with the pattern
    // `^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$`: the rows are each published bound and
    // one past it, plus a character the alphabet excludes.
    for (case, job_id, reaches_handler) in [
        ("empty, one below the minimum length", String::new(), false),
        ("at the minimum length", "a".to_string(), true),
        ("at the maximum length", "a".repeat(32), true),
        ("one past the maximum length", "a".repeat(33), false),
        (
            "an exclamation mark the pattern excludes",
            "job!held".to_string(),
            false,
        ),
    ] {
        let (status, payload) = service
            .call(
                "DescribeFHIRExportJob",
                &json!({ "DatastoreId": &exporting, "JobId": job_id }),
            )
            .await;
        if reaches_handler {
            // In range and well formed: the request reaches the handler, which
            // answers the not-found the model documents rather than a refusal.
            assert_eq!(
                error_code(&payload),
                "ResourceNotFoundException",
                "case {case:?}: a well-formed id must reach the handler, got {payload}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: a JobId outside the modelled constraints must be refused, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: must answer ValidationException, got {payload}"
        );
    }

    // Both identifiers are required: a request missing either is refused before
    // the handler runs.
    for (case, request) in [
        ("no JobId", json!({ "DatastoreId": &exporting })),
        ("no DatastoreId", json!({ "JobId": "job-held" })),
        ("neither identifier", json!({})),
    ] {
        let (status, payload) = service.call("DescribeFHIRExportJob", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request missing a required member must be refused, got {status}: {payload}"
        );
    }
}

#[tokio::test]
async fn a_status_transition_the_clock_owes_is_reported_once_it_falls_due() {
    let service = HealthLake::new();
    let exporting = datastore(&service, "exporting store").await;
    seed_jobs(
        &service,
        &exporting,
        &[
            Seed::new("job-moving", JobStatus::Submitted)
                .submitted(10)
                .pending(JobStatus::InProgress, None),
            Seed::new("job-ending", JobStatus::InProgress)
                .submitted(20)
                .pending(JobStatus::Completed, Some("every resource was exported")),
            Seed::new("job-settled", JobStatus::Queued).submitted(30),
        ],
    );

    // Before the span is up every job still reports the status it was seeded
    // with, so a caller describing a job straight after starting it sees the
    // transient status rather than the settled one — twice over, because a
    // describe must not advance it.
    for attempt in ["first read", "second read"] {
        for (case, expected) in [
            ("job-moving", "SUBMITTED"),
            ("job-ending", "IN_PROGRESS"),
            ("job-settled", "QUEUED"),
        ] {
            let properties = describe(&service, &exporting, case).await;
            assert_eq!(
                properties["JobStatus"],
                json!(expected),
                "case {case:?} ({attempt}): before the transition falls due the job must report {expected:?}, got {properties}"
            );
            assert!(
                properties.get("EndTime").is_none(),
                "case {case:?} ({attempt}): a job that has not ended must report no EndTime, got {properties}"
            );
            assert!(
                properties.get("Message").is_none(),
                "case {case:?} ({attempt}): the message of an owed status must not be reported before it, got {properties}"
            );
        }
    }

    tokio::time::sleep(PAST_TRANSITION).await;

    // A job that moved to a NON-terminal status has still not ended; the one
    // that reached a terminal status reports the instant it did, and the
    // message that status came with.
    for (case, expected, ends, message) in [
        ("job-moving", "IN_PROGRESS", false, None),
        (
            "job-ending",
            "COMPLETED",
            true,
            Some("every resource was exported"),
        ),
        ("job-settled", "QUEUED", false, None),
    ] {
        let properties = describe(&service, &exporting, case).await;
        assert_eq!(
            properties["JobStatus"],
            json!(expected),
            "case {case:?}: once due the job must report {expected:?}, got {properties}"
        );
        assert_eq!(
            properties.get("EndTime").is_some(),
            ends,
            "case {case:?}: EndTime must be reported for a terminal status only, got {properties}"
        );
        assert_eq!(
            properties.get("Message").and_then(Value::as_str),
            message,
            "case {case:?}: the message the owed status came with must be reported, got {properties}"
        );
    }

    // `EndTime` is stored when the job ends, not re-derived per read, so a
    // second describe reports the same instant — and the listing, which settles
    // the same records, agrees with the describe.
    let first = describe(&service, &exporting, "job-ending").await;
    let again = describe(&service, &exporting, "job-ending").await;
    assert_eq!(
        again["EndTime"], first["EndTime"],
        "two describes of one ended job must report the same EndTime, got {again} after {first}"
    );
    let listed = listed_jobs(&service, &exporting).await;
    let from_listing = listed
        .iter()
        .find(|job| job["JobId"] == json!("job-ending"))
        .unwrap_or_else(|| panic!("the listing must carry the ended job, got {listed:?}"));
    assert_eq!(
        from_listing, &first,
        "the listing and the describe must report the SAME settled record, got {from_listing}"
    );
}
