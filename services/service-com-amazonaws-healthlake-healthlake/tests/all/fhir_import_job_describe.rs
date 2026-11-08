// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `DescribeFHIRImportJob`: what the service reports
//! for one FHIR import job of one data store.
//!
//! The rule under test: `DescribeFHIRImportJob` reports the import job record
//! the data store its required `DatastoreId` names holds under the required
//! `JobId` — every member that record holds and nothing else, under the
//! modelled `ImportJobProperties` — after letting the record catch up with the
//! clock, so a status transition already due is reported and one not yet due is
//! not; a `DatastoreId` the service holds no data store for, and a `JobId` that
//! data store holds no import job for, are both refused with the modelled
//! `ResourceNotFoundException` in this service's fixed sentence for the
//! resource that was not found, never with an empty or synthesised job.
//!
//! `StartFHIRImportJob` — the operation that MINTS an import job record — needs
//! an input location in the emulator's Amazon S3 to read, so the rows below are
//! seeded directly into the SAME store this handler reads, through the shared
//! engine the harness exposes (exactly how `tests/all/fhir_export_job_listing.rs`
//! seeds export jobs and `tests/all/data_transformation_job.rs` seeds
//! transformation jobs); what a STARTED job records is pinned by
//! `tests/all/fhir_import_job_start.rs`.
//! Every `DescribeFHIRImportJob` call itself goes over the real awsJson1_0 wire
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
    InputDataConfig, JobProgressReport, OutputDataConfig, S3Configuration, JOB_STATUS_VALUES,
    VALIDATION_LEVEL_VALUES,
};
use service_com_amazonaws_healthlake_healthlake::state::{
    ComAmazonawsHealthlakeHealthlakeState, ExportJob, ImportJob, JobStatus,
    DATASTORE_NOT_FOUND_MESSAGE, IMPORT_JOB_NOT_FOUND_MESSAGE,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, HealthLake, PAST_TRANSITION};

/// The epoch second the seeded submit instants are measured from. A fixed
/// instant, so a row's reported `SubmitTime` is exactly the one it was seeded
/// with rather than something near it.
const BASE_SECS: i64 = 1_700_000_000;

/// The span a seeded clock-owed job transition falls due after it is armed.
///
/// Chosen to match `crate::state::EXPORT_JOB_RUN`, the span this service's other
/// pending job transitions use, so one wait ([`PAST_TRANSITION`]) outlasts
/// either.
const JOB_TRANSITION: Duration = Duration::from_millis(500);

/// The `DataAccessRoleArn` a seeded job carries, satisfying the modelled
/// `IamRoleArn` pattern (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeImport";

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

/// How much of the modelled `JobProgressReport` a seeded row carries: none at
/// all, a few members, or every member the shape declares.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Progress {
    Absent,
    Some,
    Every,
}

/// One import job to seed: every member the modelled `ImportJobProperties` can
/// carry, so a row can differ from the others along any one of them.
#[derive(Clone)]
struct Seed {
    id: &'static str,
    status: JobStatus,
    /// The submit instant, as seconds after [`BASE_SECS`].
    submitted_after_base: i64,
    /// `InputDataConfig$S3Uri`, satisfying the modelled `S3Uri` pattern
    /// (`^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$`).
    input_s3_uri: &'static str,
    /// `JobOutputDataConfig$S3Configuration$S3Uri` and `$KmsKeyId`, both
    /// required members of the modelled `S3Configuration`, or `None` for a
    /// record holding no output configuration at all.
    output: Option<(&'static str, &'static str)>,
    name: Option<&'static str>,
    /// `DataAccessRoleArn`, absent for a record holding none.
    role: bool,
    message: Option<&'static str>,
    progress: Progress,
    /// `ValidationLevel`, from the modelled `ValidationLevel` value set.
    validation_level: Option<&'static str>,
    /// `EndTime`, as seconds after [`BASE_SECS`], for a job that has ended.
    ended_after_base: Option<i64>,
    /// The transition the clock owes the seeded job, if any, and the message
    /// that status comes with.
    pending: Option<(JobStatus, Option<&'static str>)>,
}

impl Seed {
    /// A job carrying only what the modelled shape REQUIRES: an identifier, a
    /// status, a submit instant, the data store it imports into and the input
    /// configuration.
    fn new(id: &'static str, status: JobStatus) -> Self {
        Self {
            id,
            status,
            submitted_after_base: 0,
            input_s3_uri: "s3://healthlake-import-input/incoming",
            output: None,
            name: None,
            role: false,
            message: None,
            progress: Progress::Absent,
            validation_level: None,
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

    fn input(mut self, s3_uri: &'static str) -> Self {
        self.input_s3_uri = s3_uri;
        self
    }

    fn output(mut self, s3_uri: &'static str, kms_key_id: &'static str) -> Self {
        self.output = Some((s3_uri, kms_key_id));
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

    fn progress(mut self, progress: Progress) -> Self {
        self.progress = progress;
        self
    }

    fn validation_level(mut self, level: &'static str) -> Self {
        self.validation_level = Some(level);
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
    fn record(&self, datastore_id: &str, armed_at: Instant) -> ImportJob {
        ImportJob {
            id: self.id.to_string(),
            datastore_id: datastore_id.to_string(),
            name: self.name.map(str::to_string),
            status: self.status,
            pending: self.pending.map(|(next, message)| {
                (
                    armed_at + JOB_TRANSITION,
                    next,
                    message.map(str::to_string),
                )
            }),
            submitted_at: ::aws_smithy_types::DateTime::from_secs(
                BASE_SECS + self.submitted_after_base,
            ),
            ended_at: self
                .ended_after_base
                .map(|offset| ::aws_smithy_types::DateTime::from_secs(BASE_SECS + offset)),
            input_data_config: InputDataConfig::S3Uri(self.input_s3_uri.to_string()),
            output_data_config: self.output.map(|(s3_uri, kms_key_id)| {
                OutputDataConfig::S3Configuration(S3Configuration {
                    s3_uri: s3_uri.to_string(),
                    kms_key_id: kms_key_id.to_string(),
                })
            }),
            data_access_role_arn: self.role.then(|| DATA_ACCESS_ROLE_ARN.to_string()),
            message: self.message.map(str::to_string),
            progress_report: progress_report(self.progress),
            validation_level: self.validation_level.map(str::to_string),
            // A record this suite seeds was not started through the API, so it
            // carries no `ClientToken` and there is no request to fingerprint.
            client_token: None,
            request_fingerprint: String::new(),
        }
    }

    /// The `ImportJobProperties` this row must be reported as, WITHOUT its two
    /// timestamp members: a timestamp is carried as epoch seconds on this
    /// protocol, so `SubmitTime` and `EndTime` are read as numbers rather than
    /// compared to a JSON literal.
    fn expected_properties(&self, datastore_id: &str) -> Value {
        let mut expected = json!({
            "JobId": self.id,
            "JobStatus": self.status.as_wire(),
            "DatastoreId": datastore_id,
            "InputDataConfig": { "S3Uri": self.input_s3_uri },
        });
        if let Some(name) = self.name {
            expected["JobName"] = json!(name);
        }
        if let Some((s3_uri, kms_key_id)) = self.output {
            expected["JobOutputDataConfig"] = json!({
                "S3Configuration": { "S3Uri": s3_uri, "KmsKeyId": kms_key_id }
            });
        }
        if self.role {
            expected["DataAccessRoleArn"] = json!(DATA_ACCESS_ROLE_ARN);
        }
        if let Some(message) = self.message {
            expected["Message"] = json!(message);
        }
        if let Some(report) = expected_progress_report(self.progress) {
            expected["JobProgressReport"] = report;
        }
        if let Some(level) = self.validation_level {
            expected["ValidationLevel"] = json!(level);
        }
        expected
    }
}

/// The stored `JobProgressReport` for one [`Progress`] row.
///
/// The `Every` row names every member the modelled `JobProgressReport` declares
/// — 15 counts, two sizes in MB and a throughput, per
/// `smithy_query(query="shape", shape="JobProgressReport")` — so a member the
/// shape carries cannot go unreported without this suite noticing. The doubles
/// are non-integral, so a row asserts the value the wire carries rather than an
/// integer that happens to round-trip.
fn progress_report(progress: Progress) -> Option<JobProgressReport> {
    match progress {
        Progress::Absent => None,
        // A run that has scanned and imported, and nothing else to report yet.
        Progress::Some => Some(JobProgressReport {
            total_number_of_scanned_files: Some(3),
            total_number_of_resources_imported: Some(120),
            throughput: Some(12.5),
            ..JobProgressReport::default()
        }),
        Progress::Every => Some(JobProgressReport {
            total_number_of_scanned_files: Some(11),
            total_size_of_scanned_files_in_mb: Some(10.5),
            total_number_of_imported_files: Some(9),
            total_number_of_resources_scanned: Some(480),
            total_number_of_resources_imported: Some(475),
            total_number_of_resources_with_customer_error: Some(5),
            total_number_of_files_read_with_customer_error: Some(2),
            total_number_of_scanned_non_fhir_files: Some(4),
            total_size_of_scanned_non_fhir_files_in_mb: Some(2.25),
            total_number_of_imported_non_fhir_files: Some(3),
            total_number_of_non_fhir_resources_scanned: Some(60),
            total_number_of_non_fhir_resources_imported: Some(58),
            total_number_of_non_fhir_resources_with_customer_error: Some(2),
            total_number_of_non_fhir_files_read_with_customer_error: Some(1),
            throughput: Some(7.5),
            total_files_converted: Some(4),
            total_resources_generated: Some(58),
        }),
    }
}

/// The `JobProgressReport` a [`Progress`] row must be reported as, member
/// names and all.
fn expected_progress_report(progress: Progress) -> Option<Value> {
    match progress {
        Progress::Absent => None,
        Progress::Some => Some(json!({
            "TotalNumberOfScannedFiles": 3,
            "TotalNumberOfResourcesImported": 120,
            "Throughput": 12.5,
        })),
        Progress::Every => Some(json!({
            "TotalNumberOfScannedFiles": 11,
            "TotalSizeOfScannedFilesInMB": 10.5,
            "TotalNumberOfImportedFiles": 9,
            "TotalNumberOfResourcesScanned": 480,
            "TotalNumberOfResourcesImported": 475,
            "TotalNumberOfResourcesWithCustomerError": 5,
            "TotalNumberOfFilesReadWithCustomerError": 2,
            "TotalNumberOfScannedNonFhirFiles": 4,
            "TotalSizeOfScannedNonFhirFilesInMB": 2.25,
            "TotalNumberOfImportedNonFhirFiles": 3,
            "TotalNumberOfNonFhirResourcesScanned": 60,
            "TotalNumberOfNonFhirResourcesImported": 58,
            "TotalNumberOfNonFhirResourcesWithCustomerError": 2,
            "TotalNumberOfNonFhirFilesReadWithCustomerError": 1,
            "Throughput": 7.5,
            "TotalFilesConverted": 4,
            "TotalResourcesGenerated": 58,
        })),
    }
}

/// Create a data store and return its `DatastoreId`.
async fn datastore(service: &HealthLake, name: &str) -> String {
    let (id, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": name }))
        .await;
    id
}

/// Seed `seeds` as import jobs of `datastore_id` in the engine the service
/// reads.
fn seed_jobs(service: &HealthLake, datastore_id: &str, seeds: &[Seed]) {
    let armed_at = Instant::now();
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let mut store = store.lock();
    for seed in seeds {
        store
            .import_jobs
            .insert(seed.id.to_string(), seed.record(datastore_id, armed_at));
    }
}

/// The `ImportJobProperties` `DescribeFHIRImportJob` reports for `job_id` on
/// `datastore_id`.
async fn describe(service: &HealthLake, datastore_id: &str, job_id: &str) -> Value {
    let payload = service
        .ok(
            "DescribeFHIRImportJob",
            &json!({ "DatastoreId": datastore_id, "JobId": job_id }),
        )
        .await;
    payload["ImportJobProperties"].clone()
}

/// `properties` without its timestamp members, beside the epoch seconds each
/// carried (`None` when the member was not reported at all).
fn split_timestamps(properties: &Value) -> (Value, Option<f64>, Option<f64>) {
    let mut rest = properties.clone();
    let object = rest
        .as_object_mut()
        .unwrap_or_else(|| panic!("ImportJobProperties must be an object, got {properties}"));
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
/// path and a bare bucket `S3Uri`, a KMS key as a bare id and as an ARN), the
/// status, each modelled `ValidationLevel`, and progress of none, of a few
/// members and of every member the report declares.
fn round_trip_rows() -> Vec<(&'static str, Seed)> {
    vec![
        (
            "only the required members",
            Seed::new("job-minimal", JobStatus::Submitted),
        ),
        (
            "named, with an access role and the minimal validation level",
            Seed::new("job-named", JobStatus::Queued)
                .submitted(10)
                .named("nightly-bulk")
                .with_role()
                .validation_level("minimal"),
        ),
        (
            "bare bucket input, an output configuration and a few progress counts",
            Seed::new("job-running", JobStatus::InProgress)
                .submitted(20)
                .named("hourly import")
                .input("s3://healthlake-import-bulk")
                .output(
                    "s3://healthlake-import-output/r4",
                    "1234abcd-12ab-34cd-56ef-1234567890ab",
                )
                .with_role()
                .progress(Progress::Some)
                .validation_level("strict"),
        ),
        (
            "unicode JobName, a KMS key ARN and every progress member",
            Seed::new("job-unicode", JobStatus::InProgress)
                .submitted(30)
                .named("клиника записи 42")
                .output(
                    "s3://healthlake-import-output.audit/r4",
                    "arn:aws:kms:us-east-1:111122223333:key/abcd1234",
                )
                .progress(Progress::Every)
                .validation_level("structure-only"),
        ),
        (
            "completed, JobName at the modelled 64-character maximum",
            Seed::new("job-completed", JobStatus::Completed)
                .submitted(40)
                .named("n.n:n/n=n+n-n%n@n_n n.n:n/n=n+n-n%n@n_n n.n:n/n=n+n-n%n@n_n nnnn")
                .with_role()
                .progress(Progress::Every)
                .ended(45),
        ),
        (
            "completed with errors, a message beside the counts",
            Seed::new("job-partial", JobStatus::CompletedWithErrors)
                .submitted(50)
                .message("3 of 10 source files could not be read")
                .progress(Progress::Some)
                .ended(55),
        ),
        (
            "failed, a message and nothing imported",
            Seed::new("job-failed", JobStatus::Failed)
                .submitted(60)
                .message("the data access role could not read the input location")
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
    let importing = datastore(&service, "importing store").await;
    let rows = round_trip_rows();
    seed_jobs(
        &service,
        &importing,
        &rows.iter().map(|(_, seed)| seed.clone()).collect::<Vec<_>>(),
    );

    for (case, seed) in &rows {
        let properties = describe(&service, &importing, seed.id).await;
        let (reported, submit_time, end_time) = split_timestamps(&properties);
        assert_eq!(
            reported,
            seed.expected_properties(&importing),
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
        let again = describe(&service, &importing, seed.id).await;
        assert_eq!(
            again, properties,
            "case {case:?}: two describes of one job must report the same properties, got {again}"
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
    let importing = datastore(&service, "every status store").await;
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
            let seed = Seed::new("", *status);
            let mut record = seed.record(&importing, armed_at);
            record.id = ids[index].clone();
            // A job that has reached a terminal status carries the instant it
            // did; one still moving carries none.
            record.ended_at = status
                .is_terminal()
                .then(|| ::aws_smithy_types::DateTime::from_secs(BASE_SECS + 5));
            store.import_jobs.insert(ids[index].clone(), record);
        }
    }

    for (index, status) in EVERY_STATUS.iter().enumerate() {
        let case = &ids[index];
        let properties = describe(&service, &importing, case).await;
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
async fn every_modelled_validation_level_is_reported() {
    let service = HealthLake::new();
    let importing = datastore(&service, "validation level store").await;

    // The catalog is the generated `VALIDATION_LEVEL_VALUES`, the modelled
    // `ValidationLevel` enum's own value set, so a level the model gains cannot
    // stay untested here.
    for level in VALIDATION_LEVEL_VALUES {
        let job_id = format!("job-{level}");
        {
            let armed_at = Instant::now();
            let store = service
                .engine
                .state::<ComAmazonawsHealthlakeHealthlakeState>();
            let mut store = store.lock();
            let mut record = Seed::new("", JobStatus::Submitted).record(&importing, armed_at);
            record.id = job_id.clone();
            record.validation_level = Some((*level).to_string());
            store.import_jobs.insert(job_id.clone(), record);
        }
        let properties = describe(&service, &importing, &job_id).await;
        assert_eq!(
            properties["ValidationLevel"],
            json!(level),
            "case {level:?}: must report the stored validation level, got {properties}"
        );
    }
}

#[tokio::test]
async fn a_job_the_named_data_store_does_not_hold_is_refused_with_the_fixed_sentence() {
    let service = HealthLake::new();
    let importing = datastore(&service, "importing store").await;
    let other = datastore(&service, "other store").await;
    seed_jobs(
        &service,
        &importing,
        &[Seed::new("job-held", JobStatus::Queued).with_role()],
    );
    // The same identifier, held as an import job of ANOTHER data store.
    seed_jobs(
        &service,
        &other,
        &[Seed::new("job-elsewhere", JobStatus::Completed)],
    );
    // A FHIR export job's record lives in the same state under its own id; it
    // is not an import job, so its id must not resolve as one.
    {
        let store = service
            .engine
            .state::<ComAmazonawsHealthlakeHealthlakeState>();
        let mut store = store.lock();
        store.export_jobs.insert(
            "job-export".to_string(),
            ExportJob {
                id: "job-export".to_string(),
                datastore_id: importing.clone(),
                name: None,
                status: JobStatus::Completed,
                pending: None,
                submitted_at: ::aws_smithy_types::DateTime::from_secs(BASE_SECS),
                ended_at: None,
                output_data_config: OutputDataConfig::S3Configuration(S3Configuration {
                    s3_uri: "s3://healthlake-export-bucket/job-export".to_string(),
                    kms_key_id: "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(),
                }),
                data_access_role_arn: Some(DATA_ACCESS_ROLE_ARN.to_string()),
                message: None,
                // Seeded rather than started, so no `ClientToken` named this
                // job and no request fingerprinted to it.
                client_token: None,
                request_fingerprint: String::new(),
            },
        );
    }

    // The rows span the forms a `JobId` the modelled constraints admit can take
    // on a data store the service DOES hold: one never minted, one of another
    // shape entirely, the identifier of a different resource kind held in the
    // same state, an import job of another data store, and a case variant of a
    // held id (the identifier is case-sensitive).
    for (case, job_id) in [
        ("a well-formed id nothing was minted under", "job-nothing"),
        ("an id of another shape entirely", "0123456789"),
        ("the id of a FHIR export job", "job-export"),
        ("an import job of another data store", "job-elsewhere"),
        ("a held id in another case", "JOB-HELD"),
    ] {
        let (status, payload) = service
            .call(
                "DescribeFHIRImportJob",
                &json!({ "DatastoreId": &importing, "JobId": job_id }),
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
            IMPORT_JOB_NOT_FOUND_MESSAGE,
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
                "DescribeFHIRImportJob",
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

    // The refusals above left the held record readable.
    let properties = describe(&service, &importing, "job-held").await;
    assert_eq!(
        properties["JobId"],
        json!("job-held"),
        "the refusals must leave the held record untouched, got {properties}"
    );
}

#[tokio::test]
async fn an_identifier_outside_the_modelled_constraints_is_refused() {
    let service = HealthLake::new();
    let importing = datastore(&service, "importing store").await;
    seed_jobs(&service, &importing, &[Seed::new("job-held", JobStatus::Queued)]);

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
                "DescribeFHIRImportJob",
                &json!({ "DatastoreId": &importing, "JobId": job_id }),
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
        ("no JobId", json!({ "DatastoreId": &importing })),
        ("no DatastoreId", json!({ "JobId": "job-held" })),
        ("neither identifier", json!({})),
    ] {
        let (status, payload) = service.call("DescribeFHIRImportJob", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request missing a required member must be refused, got {status}: {payload}"
        );
    }
}

#[tokio::test]
async fn a_status_transition_the_clock_owes_is_reported_once_it_falls_due() {
    let service = HealthLake::new();
    let importing = datastore(&service, "importing store").await;
    seed_jobs(
        &service,
        &importing,
        &[
            Seed::new("job-moving", JobStatus::Submitted)
                .submitted(10)
                .pending(JobStatus::InProgress, None),
            Seed::new("job-ending", JobStatus::InProgress)
                .submitted(20)
                .pending(JobStatus::Completed, Some("every resource was imported")),
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
            let properties = describe(&service, &importing, case).await;
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
            Some("every resource was imported"),
        ),
        ("job-settled", "QUEUED", false, None),
    ] {
        let properties = describe(&service, &importing, case).await;
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
    // second describe reports the same instant.
    let first = describe(&service, &importing, "job-ending").await;
    let again = describe(&service, &importing, "job-ending").await;
    assert_eq!(
        again["EndTime"], first["EndTime"],
        "two describes of one ended job must report the same EndTime, got {again} after {first}"
    );
}
